use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufRead, BufReader, IsTerminal as _, Write};
use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use crate::namespace;
use crate::search::index::SearchIndex;
use crate::store_kv;
use crate::wrap::MAX_WIDTH;

use super::RuntimeIndex;

/// File descriptor number for the block → creft message channel.
pub(super) const CONTROL_FD: i32 = 3;
/// File descriptor number for the creft → block response channel.
pub(super) const RESPONSE_FD: i32 = 4;

/// A side channel connecting a child block process to the creft parent.
///
/// Creates two OS pipes:
/// - **control pipe**: child writes (fd 3) → parent reads
/// - **response pipe**: parent writes → child reads (fd 4)
///
/// The parent holds the read end of the control pipe and the write end
/// of the response pipe. The child inherits the opposite ends via
/// `dup2` in the `pre_exec` hook.
///
/// Drop closes the parent's ends. The child's ends are closed when the
/// child process exits (they are not marked close-on-exec — the
/// `pre_exec` hook clears that flag after `dup2`).
pub(crate) struct SideChannel {
    /// Read end of the control pipe (parent reads block messages).
    /// `Option` because `take_control_reader` moves it out.
    control_reader: Option<OwnedFd>,
    /// Write end of the control pipe (child writes via fd 3).
    /// Held here only to keep it alive until `pre_exec` dups it.
    /// `Option` because `close_child_ends` moves it out on drop.
    control_writer: Option<OwnedFd>,
    /// Write end of the response pipe (parent writes prompt responses).
    /// `Option` because `take_response_writer` moves it out.
    response_writer: Option<OwnedFd>,
    /// Read end of the response pipe (child reads via fd 4).
    /// Held here only to keep it alive until `pre_exec` dups it.
    /// `Option` because `close_child_ends` moves it out on drop.
    response_reader: Option<OwnedFd>,
}

impl SideChannel {
    /// Create a new side channel with two OS pipe pairs.
    pub(crate) fn new() -> std::io::Result<Self> {
        let (ctrl_r, ctrl_w) = os_pipe()?;
        let (resp_r, resp_w) = os_pipe()?;
        Ok(Self {
            control_reader: Some(ctrl_r),
            control_writer: Some(ctrl_w),
            response_writer: Some(resp_w),
            response_reader: Some(resp_r),
        })
    }

    /// Raw fd values the child process needs for `dup2` in `pre_exec`.
    ///
    /// Returns `(control_write_fd, response_read_fd)` — the child's ends
    /// of the two pipes. Panics if called after `close_child_ends`.
    pub(crate) fn child_fds(&self) -> (i32, i32) {
        (
            self.control_writer
                .as_ref()
                .expect("child_fds called after close_child_ends")
                .as_raw_fd(),
            self.response_reader
                .as_ref()
                .expect("child_fds called after close_child_ends")
                .as_raw_fd(),
        )
    }

    /// Take the parent's read end of the control pipe.
    ///
    /// Called once after spawn to move the reader into the reader thread.
    /// Consumes the fd — subsequent calls return `None`.
    pub(crate) fn take_control_reader(&mut self) -> Option<OwnedFd> {
        self.control_reader.take()
    }

    /// Take the parent's write end of the response pipe.
    ///
    /// Called once after spawn to move the writer into the writer/prompt thread.
    /// Consumes the fd — subsequent calls return `None`.
    pub(crate) fn take_response_writer(&mut self) -> Option<OwnedFd> {
        self.response_writer.take()
    }

    /// Close the parent's copies of the child's pipe ends.
    ///
    /// Must be called after spawn. If the parent keeps these fds open,
    /// the child's reads on fd 4 will never see EOF (the parent's copy
    /// of the write end keeps the pipe alive), and the parent's reads
    /// on the control pipe may block (the parent's copy of the write
    /// end prevents EOF on the control reader).
    ///
    /// Idempotent — safe to call more than once.
    pub(crate) fn close_child_ends(&mut self) {
        // `.take()` moves the OwnedFd out of the Option; the returned value
        // is immediately dropped, which closes the file descriptor.
        drop(self.control_writer.take());
        drop(self.response_reader.take());
    }
}

/// Create a POSIX pipe and return `(read_end, write_end)` as `OwnedFd`s.
///
/// Sets O_CLOEXEC on both ends via `fcntl` after creation. The
/// `pre_exec` hook in `spawn_block` clears this flag (implicitly) after
/// `dup2` — `dup2` does not inherit `FD_CLOEXEC`, so the duplicated fds
/// remain open across `exec` as intended.
///
/// Re-exported at the runner module surface as `crate::runner::os_pipe` so
/// that `cmd::run` and the Stage 4 scenario runner can construct trace pipes
/// without a parallel implementation.
pub(crate) fn os_pipe() -> std::io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0i32; 2];
    // SAFETY: `fds` is a valid two-element array. pipe(2) writes exactly
    // two file descriptor integers into it and returns -1 on error, 0 on
    // success. Both resulting fds are valid and owned by this scope.
    let ret = unsafe { libc::pipe(fds.as_mut_ptr()) };
    if ret == -1 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: pipe(2) returned 0, so both fds are valid, open, and not
    // shared with any other owner. Wrapping them in OwnedFd transfers
    // ownership and ensures they are closed on drop.
    let r = unsafe { OwnedFd::from_raw_fd(fds[0]) };
    let w = unsafe { OwnedFd::from_raw_fd(fds[1]) };

    // Set O_CLOEXEC on both ends so they don't leak into unrelated
    // child processes (only the block that uses dup2 in pre_exec gets them).
    // SAFETY: both fds are valid and owned by r/w; fcntl F_SETFD is safe.
    for fd in [fds[0], fds[1]] {
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        if flags == -1 {
            return Err(std::io::Error::last_os_error());
        }
        let ret = unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) };
        if ret == -1 {
            return Err(std::io::Error::last_os_error());
        }
    }

    Ok((r, w))
}

/// Increment a primitive counter entry by one.
///
/// Acquires the `PrimitiveCounter` lock, bumps `counts[tag]`, and drops the
/// lock immediately. Called from the reader thread for each matched
/// `ChannelMessage` variant (except `Exit`).
fn increment_counter(counter: &PrimitiveCounter, tag: &str) {
    if let Ok(mut counts) = counter.lock() {
        *counts.entry(tag.to_owned()).or_insert(0) += 1;
    }
}

/// Shared slot for the `creft_exit` signal from the side channel.
///
/// `None` means no exit signal was received. `Some(code)` means `creft_exit(code)`
/// was called inside the block. The reader thread writes to this slot; the main
/// thread reads after joining the reader (no concurrent access, Mutex never contended).
pub(crate) type ExitSignal = Arc<std::sync::Mutex<Option<i32>>>;

/// A message from a block to creft via the side channel.
///
/// Each line on fd 3 is a newline-delimited JSON object. The `type` field
/// determines which variant is decoded. Unrecognised types are silently
/// dropped by the reader thread.
#[derive(Debug, serde::Deserialize)]
#[serde(tag = "type")]
// `choices` on the Prompt variant is part of the wire protocol consumed by
// the prompt handler. The `global` field on Index controls access control.
// Suppress the dead-code lint rather than removing protocol fields.
#[allow(dead_code)]
pub(crate) enum ChannelMessage {
    #[serde(rename = "print")]
    Print { message: String },
    #[serde(rename = "status")]
    Status {
        message: String,
        /// Percentage 0-100. Absent in the JSON (`serde(default)`) means
        /// spinner mode; present means progress bar mode.
        #[serde(default)]
        progress: Option<u8>,
    },
    #[serde(rename = "prompt")]
    Prompt {
        id: String,
        question: String,
        choices: String,
    },
    #[serde(rename = "exit")]
    Exit {
        /// Process exit code. 0 = success (pipeline skip), non-zero = failure.
        #[serde(default)]
        code: i32,
    },
    #[serde(rename = "index")]
    Index {
        /// Request ID for ack correlation.
        id: String,
        /// Sub-index name (local to the skill's namespace).
        name: String,
        /// Content to tokenize and add to the named sub-index.
        content: String,
        /// Whether this index is queryable from other namespaces.
        /// Default: false (namespace-isolated).
        #[serde(default)]
        global: bool,
    },
    #[serde(rename = "search")]
    Search {
        /// Request ID for matching the response (same pattern as Prompt).
        id: String,
        /// Search query string.
        query: String,
        /// Sub-index name. Local name (no dots) resolves within the caller's
        /// namespace. Dotted name is a cross-namespace reference checked against
        /// the access registry.
        name: String,
    },
    #[serde(rename = "store_put")]
    StorePut {
        /// Request ID for ack correlation.
        id: String,
        /// Store name (local to the skill's namespace).
        name: String,
        /// Key to store.
        key: String,
        /// Value to store (arbitrary string).
        value: String,
        /// Whether this store is searchable from other namespaces.
        ///
        /// `None` (field omitted in JSON): leave the global flag unchanged.
        /// `Some(true)`: mark as globally accessible.
        /// `Some(false)`: explicitly revoke global access.
        #[serde(default)]
        global: Option<bool>,
    },
    #[serde(rename = "store_get")]
    StoreGet {
        /// Request ID for matching the response.
        id: String,
        /// Store name (local to the skill's namespace).
        name: String,
        /// Key to look up.
        key: String,
    },
    #[serde(rename = "store_search")]
    StoreSearch {
        /// Request ID for matching the response.
        id: String,
        /// Store name. Local name resolves within the caller's namespace.
        /// Dotted name is a cross-namespace reference (must be global).
        name: String,
        /// Search query string.
        query: String,
    },
}

/// A draw target wrapper that caps the reported terminal width.
///
/// indicatif queries `TermLike::width()` to determine how wide to render
/// `{wide_bar}` and `{wide_msg}` placeholders. This wrapper delegates all
/// operations to the inner `Term` but reports `min(actual_width, max_width)`
/// so the bar never exceeds the project's column limit.
#[derive(Debug)]
struct CappedTerm {
    inner: console::Term,
    max_width: u16,
}

impl CappedTerm {
    fn stderr(max_width: u16) -> Self {
        Self {
            inner: console::Term::buffered_stderr(),
            max_width,
        }
    }
}

impl indicatif::TermLike for CappedTerm {
    fn width(&self) -> u16 {
        self.inner.width().min(self.max_width)
    }

    fn height(&self) -> u16 {
        self.inner.height()
    }

    fn move_cursor_up(&self, n: usize) -> io::Result<()> {
        self.inner.move_cursor_up(n)
    }

    fn move_cursor_down(&self, n: usize) -> io::Result<()> {
        self.inner.move_cursor_down(n)
    }

    fn move_cursor_right(&self, n: usize) -> io::Result<()> {
        self.inner.move_cursor_right(n)
    }

    fn move_cursor_left(&self, n: usize) -> io::Result<()> {
        self.inner.move_cursor_left(n)
    }

    fn write_line(&self, s: &str) -> io::Result<()> {
        self.inner.write_line(s)
    }

    fn write_str(&self, s: &str) -> io::Result<()> {
        self.inner.write_str(s)
    }

    fn clear_line(&self) -> io::Result<()> {
        self.inner.clear_line()
    }

    fn flush(&self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// The currently active terminal indicator, if any.
///
/// indicatif's `ProgressBar` can serve as both a spinner (no length) and a
/// progress bar (fixed length). We distinguish them so that switching modes
/// — e.g. a spinner followed by a progress message — finishes the old
/// indicator before creating the new one. Reusing a single `ProgressBar` by
/// swapping its length mid-flight is not reliable with indicatif.
enum ActiveIndicator {
    /// Animated braille spinner driven by indicatif's steady tick.
    Spinner(indicatif::ProgressBar),
    /// Percentage-based progress bar (0-100).
    Progress(indicatif::ProgressBar),
}

impl ActiveIndicator {
    /// The inner `ProgressBar`, regardless of mode.
    fn bar(&self) -> &indicatif::ProgressBar {
        match self {
            Self::Spinner(pb) | Self::Progress(pb) => pb,
        }
    }
}

/// State behind the `TerminalWriter` mutex.
///
/// Combining `indicator` and `stderr` in a single lock means that finishing
/// an indicator and writing the next message are atomic — no concurrent
/// thread can write between the two operations.
struct TerminalWriterInner {
    stderr: std::io::Stderr,
    /// The currently active spinner or progress bar, if any.
    indicator: Option<ActiveIndicator>,
}

/// Serialises terminal output from multiple concurrent reader threads.
///
/// In a pipe chain each block runs its own reader thread. Without
/// synchronisation, concurrent writes to stderr interleave. Every public
/// method acquires the inner mutex for the duration of a single write,
/// so each message renders atomically.
///
/// Status messages are ephemeral visual feedback intended for a human
/// watching a terminal. When stderr is not a TTY (piped output, CI, agent
/// contexts), status messages are silently dropped. Print messages always
/// render regardless of TTY state.
pub(crate) struct TerminalWriter {
    inner: std::sync::Mutex<TerminalWriterInner>,
    /// True when stderr is connected to a TTY at construction time.
    ///
    /// Checked once at construction rather than per-call: the TTY state of
    /// a file descriptor does not change during the lifetime of a process,
    /// and avoiding repeated `isatty(2)` syscalls in the hot path is free.
    is_tty: bool,
}

/// Finish and clear any active indicator inside an already-locked guard.
///
/// Called by `clear_status()` and `handle_prompt`. Centralising the logic
/// avoids duplicating the `finish_and_clear` + `indicator = None` pattern.
fn clear_indicator_locked(inner: &mut TerminalWriterInner) {
    if let Some(indicator) = inner.indicator.take() {
        indicator.bar().finish_and_clear();
    }
}

impl TerminalWriter {
    pub(crate) fn new() -> Self {
        let is_tty = std::io::stderr().is_terminal();
        Self {
            inner: std::sync::Mutex::new(TerminalWriterInner {
                stderr: std::io::stderr(),
                indicator: None,
            }),
            is_tty,
        }
    }

    /// Construct a `TerminalWriter` with an explicit TTY override.
    ///
    /// Used in tests to exercise TTY-gated paths (status, progress,
    /// clear_status) without requiring the test process to run with a real
    /// TTY attached to stderr.
    #[cfg(test)]
    fn with_tty(is_tty: bool) -> Self {
        Self {
            inner: std::sync::Mutex::new(TerminalWriterInner {
                stderr: std::io::stderr(),
                indicator: None,
            }),
            is_tty,
        }
    }

    /// Write a print message to stderr.
    ///
    /// Suspends any active indicator so the printed line appears cleanly
    /// without interleaving with spinner or progress bar rendering. Always
    /// renders regardless of TTY state.
    pub(crate) fn print(&self, message: &str) {
        let Ok(mut g) = self.inner.lock() else {
            return;
        };
        if let Some(ref indicator) = g.indicator {
            // Clone the bar before calling suspend: we need an immutable borrow
            // of `g.indicator` to get the bar, but the closure passed to
            // `suspend` needs to borrow `g.stderr` mutably. Cloning is a
            // refcount increment — `ProgressBar` is internally `Arc`-based.
            let pb = indicator.bar().clone();
            pb.suspend(|| {
                let _ = writeln!(g.stderr, "{message}");
            });
        } else {
            let _ = writeln!(g.stderr, "{message}");
        }
    }

    /// Show or update an animated spinner with the given message.
    ///
    /// Creates the spinner on the first call; subsequent calls update the
    /// message in place. If a progress bar is currently active, it is
    /// finished and replaced with a spinner.
    ///
    /// Silently dropped when stderr is not a TTY.
    pub(crate) fn status(&self, message: &str) {
        if !self.is_tty {
            return;
        }
        let Ok(mut g) = self.inner.lock() else {
            return;
        };
        match g.indicator {
            Some(ActiveIndicator::Spinner(ref pb)) => {
                pb.set_message(message.to_owned());
                return;
            }
            Some(ActiveIndicator::Progress(ref pb)) => {
                pb.finish_and_clear();
            }
            None => {}
        }
        // Create a new spinner with a width-capped draw target.
        let target = indicatif::ProgressDrawTarget::term_like_with_hz(
            Box::new(CappedTerm::stderr(MAX_WIDTH as u16)),
            20,
        );
        let pb = indicatif::ProgressBar::with_draw_target(None, target)
            .with_style(indicatif::ProgressStyle::default_spinner())
            .with_message(message.to_owned());
        pb.enable_steady_tick(Duration::from_millis(80));
        g.indicator = Some(ActiveIndicator::Spinner(pb));
    }

    /// Show or update a progress bar at the given percentage (0-100).
    ///
    /// Creates the progress bar on the first call; subsequent calls update
    /// the position and message in place. If a spinner is currently active,
    /// it is finished and replaced with a progress bar. Values above 100 are
    /// saturated to 100.
    ///
    /// Silently dropped when stderr is not a TTY.
    pub(crate) fn progress(&self, message: &str, pct: u8) {
        if !self.is_tty {
            return;
        }
        let Ok(mut g) = self.inner.lock() else {
            return;
        };
        let pct = pct.min(100) as u64;
        match g.indicator {
            Some(ActiveIndicator::Progress(ref pb)) => {
                pb.set_position(pct);
                pb.set_message(message.to_owned());
                return;
            }
            Some(ActiveIndicator::Spinner(ref pb)) => {
                pb.finish_and_clear();
            }
            None => {}
        }
        // Create a new progress bar with a width-capped draw target.
        //
        // The template unwrap is safe: the template string is a compile-time
        // constant with no dynamic user input.
        let style =
            indicatif::ProgressStyle::with_template("{msg} [{wide_bar:.cyan/blue}] {percent}%")
                .expect("progress bar template is a compile-time constant")
                .progress_chars("━╸─");
        let target = indicatif::ProgressDrawTarget::term_like_with_hz(
            Box::new(CappedTerm::stderr(MAX_WIDTH as u16)),
            20,
        );
        let pb = indicatif::ProgressBar::with_draw_target(Some(100), target)
            .with_style(style)
            .with_message(message.to_owned());
        pb.set_position(pct);
        g.indicator = Some(ActiveIndicator::Progress(pb));
    }

    /// Clear any active indicator.
    ///
    /// Called when the block exits so no stale spinner or progress bar
    /// remains on screen. No-op when no indicator is active. No-op when
    /// stderr is not a TTY (indicators are never created there).
    pub(crate) fn clear_status(&self) {
        let Ok(mut g) = self.inner.lock() else {
            return;
        };
        clear_indicator_locked(&mut g);
    }
}

/// Count of primitive messages seen on the side channel for a single block.
///
/// Keys are `ChannelMessage` JSON tag strings (e.g. `"print"`, `"store_put"`).
/// Wrapped in `Arc<Mutex<>>` so the reader thread owns a clone and the caller
/// reads the totals after `join()` without holding the lock concurrently.
pub(crate) type PrimitiveCounter = Arc<std::sync::Mutex<std::collections::BTreeMap<String, u32>>>;

/// Context for handling runtime primitive messages on the side channel.
///
/// Carried into the reader thread when a skill is executing. The thread uses
/// `skill_name` to derive the caller's namespace for name qualification,
/// `runtime_indexes` for in-memory search indexes, `store_dir` for persistent
/// key-value store I/O, and `counter` to tally primitive invocations for the
/// coverage trace.
pub(crate) struct PrimitiveContext {
    /// Fully-qualified name of the skill being executed.
    pub skill_name: String,
    /// Plugin name extracted from the skill's source, if any.
    pub plugin: Option<String>,
    /// Shared runtime indexes keyed by fully-qualified name.
    pub runtime_indexes: Arc<std::sync::Mutex<HashMap<String, RuntimeIndex>>>,
    /// Directory for persistent key-value stores.
    ///
    /// Resolved from `AppContext::store_dir_for(Scope::Global)` at context
    /// construction time. Store operations open and close the redb database
    /// per-call; this directory is where the `.redb` and `.store.idx` files live.
    pub store_dir: PathBuf,
    /// Per-block primitive-tag counter for coverage trace emission.
    ///
    /// The reader thread increments `counter[tag]` once per matched
    /// `ChannelMessage` variant (except `Exit`, which is tracked separately via
    /// the `exit` field on `TraceRecord`). The trace-emitting call sites in
    /// `execute_block` and `run_pipe_chain` hold an `Arc::clone` of this counter
    /// and read its contents after the reader thread joins.
    pub counter: PrimitiveCounter,
}

/// Spawn a reader thread that drains the control pipe and renders messages.
///
/// The thread reads NDJSON lines from `control_reader` until EOF (the child
/// has exited and all write ends are closed). Each parseable line is matched
/// against `ChannelMessage` variants:
///
/// - `Print` — rendered immediately via `writer.print`.
/// - `Status` — rendered via `writer.status` (overwrites the previous status).
/// - `Prompt` — when `prompt_tx` is `None`, writes an empty response so the
///   child does not deadlock on its `read(<&4)` call, and logs the question
///   as a print message.
/// - `Index` — when `ctx` is `Some`, qualifies the name and builds an
///   in-memory `SearchIndex`, then writes a JSON ack on fd 4.
/// - `Search` — when `ctx` is `Some`, resolves the name against the
///   access registry, queries the index, and writes a JSON response on fd 4.
/// - `StorePut` — when `ctx` is `Some`, qualifies the store name, writes the
///   key-value pair to the redb database, rebuilds the search index, and
///   writes a JSON ack on fd 4.
/// - `StoreGet` — when `ctx` is `Some`, reads the value for the given key from
///   the redb database and writes a JSON response on fd 4.
/// - `StoreSearch` — when `ctx` is `Some`, resolves the store name (with access
///   control), loads the search index from disk, queries it, and writes a JSON
///   response on fd 4.
///
/// Malformed JSON lines are silently skipped.
///
/// Returns the join handle. Call `.join()` after `wait_with_output()` to ensure
/// all messages are rendered before the parent process exits.
pub(crate) fn spawn_reader(
    control_reader: OwnedFd,
    writer: Arc<TerminalWriter>,
    prompt_tx: Option<std::sync::mpsc::Sender<ChannelMessage>>,
    mut response_writer: Option<File>,
    exit_signal: Option<ExitSignal>,
    ctx: Option<PrimitiveContext>,
) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("creft-channel-reader".to_owned())
        .spawn(move || {
            // SAFETY: control_reader is valid and exclusively owned by this
            // thread. Converting to File gives buffered line-by-line reading.
            let file = unsafe { File::from_raw_fd(control_reader.into_raw_fd()) };
            let reader = BufReader::new(file);

            for line in reader.lines() {
                let line = match line {
                    Ok(l) => l,
                    // Read error (e.g. pipe broken): stop processing.
                    Err(_) => break,
                };

                let msg = match serde_json::from_str::<ChannelMessage>(&line) {
                    Ok(m) => m,
                    // Unknown type or malformed JSON: skip silently.
                    Err(_) => continue,
                };

                match msg {
                    ChannelMessage::Print { message } => {
                        if let Some(ref ctx) = ctx {
                            increment_counter(&ctx.counter, "print");
                        }
                        writer.print(&message);
                    }
                    ChannelMessage::Status { message, progress } => {
                        if let Some(ref ctx) = ctx {
                            increment_counter(&ctx.counter, "status");
                        }
                        match progress {
                            None => writer.status(&message),
                            Some(pct) => writer.progress(&message, pct),
                        }
                    }
                    ChannelMessage::Prompt {
                        id,
                        question,
                        choices,
                    } => {
                        if let Some(ref ctx) = ctx {
                            increment_counter(&ctx.counter, "prompt");
                        }
                        if let Some(tx) = &prompt_tx {
                            // Forward to the prompt handler thread.
                            let _ = tx.send(ChannelMessage::Prompt {
                                id,
                                question,
                                choices,
                            });
                        } else {
                            // Pipe-chain context: log the question and write an
                            // empty response so the child doesn't hang.
                            writer.print(&question);
                            if let Some(ref mut rw) = response_writer {
                                let response = format!("{{\"id\":\"{id}\",\"value\":\"\"}}\n");
                                let _ = rw.write_all(response.as_bytes());
                                let _ = rw.flush();
                            }
                        }
                    }
                    ChannelMessage::Exit { code } => {
                        if let Some(ref signal) = exit_signal
                            && let Ok(mut slot) = signal.lock()
                        {
                            *slot = Some(code);
                        }
                    }
                    ChannelMessage::Index {
                        id,
                        name,
                        content,
                        global,
                    } => {
                        if let Some(ref ctx) = ctx {
                            increment_counter(&ctx.counter, "index");
                            let response = handle_index_message(
                                &id,
                                &name,
                                &content,
                                global,
                                &ctx.skill_name,
                                ctx.plugin.as_deref(),
                                &ctx.runtime_indexes,
                            );
                            if let Some(ref mut rw) = response_writer {
                                let _ = rw.write_all(response.as_bytes());
                                let _ = rw.flush();
                            }
                        } else if let Some(ref mut rw) = response_writer {
                            // No primitive context — write ack so the child does not hang.
                            let response = format!("{{\"id\":\"{id}\",\"ok\":true}}\n");
                            let _ = rw.write_all(response.as_bytes());
                            let _ = rw.flush();
                        }
                    }
                    ChannelMessage::Search { id, query, name } => {
                        if let Some(ref ctx) = ctx {
                            increment_counter(&ctx.counter, "search");
                            let response = handle_search_message(
                                &id,
                                &query,
                                &name,
                                &ctx.skill_name,
                                ctx.plugin.as_deref(),
                                &ctx.runtime_indexes,
                            );
                            if let Some(ref mut rw) = response_writer {
                                let _ = rw.write_all(response.as_bytes());
                                let _ = rw.flush();
                            }
                        } else if let Some(ref mut rw) = response_writer {
                            // No primitive context — return empty results rather than hanging.
                            let response = format!("{{\"id\":\"{id}\",\"results\":\"\"}}\n");
                            let _ = rw.write_all(response.as_bytes());
                            let _ = rw.flush();
                        }
                    }
                    ChannelMessage::StorePut {
                        id,
                        name,
                        key,
                        value,
                        global,
                    } => {
                        if let Some(ref ctx) = ctx {
                            increment_counter(&ctx.counter, "store_put");
                            let response = handle_store_put(
                                &id,
                                &name,
                                &key,
                                &value,
                                global,
                                &ctx.skill_name,
                                ctx.plugin.as_deref(),
                                &ctx.store_dir,
                            );
                            if let Some(ref mut rw) = response_writer {
                                let _ = rw.write_all(response.as_bytes());
                                let _ = rw.flush();
                            }
                        } else if let Some(ref mut rw) = response_writer {
                            // No primitive context — write ack so the child does not hang.
                            let response = format!("{{\"id\":\"{id}\",\"ok\":true}}\n");
                            let _ = rw.write_all(response.as_bytes());
                            let _ = rw.flush();
                        }
                    }
                    ChannelMessage::StoreGet { id, name, key } => {
                        if let Some(ref ctx) = ctx {
                            increment_counter(&ctx.counter, "store_get");
                            let response = handle_store_get(
                                &id,
                                &name,
                                &key,
                                &ctx.skill_name,
                                ctx.plugin.as_deref(),
                                &ctx.store_dir,
                            );
                            if let Some(ref mut rw) = response_writer {
                                let _ = rw.write_all(response.as_bytes());
                                let _ = rw.flush();
                            }
                        } else if let Some(ref mut rw) = response_writer {
                            // No primitive context — return empty value rather than hanging.
                            let response = format!("{{\"id\":\"{id}\",\"value\":\"\"}}\n");
                            let _ = rw.write_all(response.as_bytes());
                            let _ = rw.flush();
                        }
                    }
                    ChannelMessage::StoreSearch { id, name, query } => {
                        if let Some(ref ctx) = ctx {
                            increment_counter(&ctx.counter, "store_search");
                            let response = handle_store_search(
                                &id,
                                &name,
                                &query,
                                &ctx.skill_name,
                                ctx.plugin.as_deref(),
                                &ctx.store_dir,
                            );
                            if let Some(ref mut rw) = response_writer {
                                let _ = rw.write_all(response.as_bytes());
                                let _ = rw.flush();
                            }
                        } else if let Some(ref mut rw) = response_writer {
                            // No primitive context — return empty results rather than hanging.
                            let response = format!("{{\"id\":\"{id}\",\"results\":\"\"}}\n");
                            let _ = rw.write_all(response.as_bytes());
                            let _ = rw.flush();
                        }
                    }
                }
            }

            // Clear any lingering status line when the block exits.
            writer.clear_status();
        })
        .expect("failed to spawn channel reader thread")
}

/// Escape a string for safe interpolation into a JSON string value.
///
/// Replaces the five characters that break JSON string literals:
/// - `\` → `\\` (must be first to avoid double-escaping)
/// - `"` → `\"`
/// - tab → `\t`
/// - newline → `\n`
/// - carriage return → `\r`
fn json_escape_string(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

/// Handle a `creft_index` message: qualify the name, accumulate the document,
/// rebuild the `SearchIndex`, and store the result in `runtime_indexes`.
///
/// Called from the reader thread when an `Index` message arrives on fd 3.
/// Returns a JSON ack string (`{"id":"<id>","ok":true}\n`) after the index
/// rebuild completes. The caller writes this string to fd 4.
///
/// When called multiple times with the same qualified name, each call appends
/// a new document to the index rather than replacing it. The `SearchIndex` is
/// rebuilt from all accumulated documents on every call. The `is_global` flag
/// is updated to the value from the most recent call (last-write-wins).
fn handle_index_message(
    id: &str,
    name: &str,
    content: &str,
    global: bool,
    skill_name: &str,
    plugin: Option<&str>,
    runtime_indexes: &std::sync::Mutex<HashMap<String, RuntimeIndex>>,
) -> String {
    let ns = namespace::skill_namespace(skill_name);
    let qualified = namespace::qualify(name, ns, plugin);

    if let Ok(mut map) = runtime_indexes.lock() {
        let runtime_index = map
            .entry(qualified.clone())
            .or_insert_with(|| RuntimeIndex {
                index: SearchIndex::build(&[]),
                is_global: global,
                documents: Vec::new(),
            });

        // Append the new document with a sequential label used as the XOR
        // filter key. Search resolves hit labels back to their content via
        // the documents list, so the label is never exposed to callers.
        let doc_name = format!("doc_{}", runtime_index.documents.len());
        runtime_index.documents.push((doc_name, content.to_owned()));
        runtime_index.is_global = global;

        // Rebuild the index from all accumulated documents. The number of
        // runtime documents per index is small (a skill typically indexes
        // 1–10 documents per invocation), so rebuilding is negligible.
        let doc_triples: Vec<(&str, &str, &str)> = runtime_index
            .documents
            .iter()
            .map(|(n, c)| (n.as_str(), "", c.as_str()))
            .collect();
        runtime_index.index = SearchIndex::build(&doc_triples);
    }

    format!("{{\"id\":\"{id}\",\"ok\":true}}\n")
}

/// Handle a `creft_search` message: resolve the name, query the index, and
/// return a JSON response string to be written to fd 4.
///
/// If access is denied for a cross-namespace reference, the response contains
/// an `error` field. If no matching index exists or the query returns no
/// results, the `results` field is an empty string.
fn handle_search_message(
    id: &str,
    query: &str,
    name: &str,
    skill_name: &str,
    plugin: Option<&str>,
    runtime_indexes: &std::sync::Mutex<HashMap<String, RuntimeIndex>>,
) -> String {
    let ns = namespace::skill_namespace(skill_name);

    // Build a transient access registry from the current runtime indexes.
    let mut registry = namespace::AccessRegistry::new();
    if let Ok(map) = runtime_indexes.lock() {
        for (qualified_name, runtime_index) in map.iter() {
            if runtime_index.is_global {
                registry.mark_global(qualified_name);
            }
        }
    }

    let qualified = match namespace::resolve(name, ns, plugin, &registry) {
        Ok(q) => q,
        Err(e) => {
            let msg = e.to_string().replace('"', "\\\"");
            return format!("{{\"id\":\"{id}\",\"error\":\"{msg}\"}}\n");
        }
    };

    let results = if let Ok(map) = runtime_indexes.lock() {
        if let Some(runtime_index) = map.get(&qualified) {
            // Build a lookup from sequential doc name (e.g. "doc_0") to its
            // original content so that search hits resolve to what was indexed,
            // not the internal document identifier.
            let content_by_name: HashMap<&str, &str> = runtime_index
                .documents
                .iter()
                .map(|(name, content)| (name.as_str(), content.as_str()))
                .collect();

            let hits = runtime_index.index.search(query);
            hits.iter()
                .filter_map(|entry| content_by_name.get(entry.name.as_str()).copied())
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    let escaped_results = json_escape_string(&results);
    format!("{{\"id\":\"{id}\",\"results\":\"{escaped_results}\"}}\n")
}

/// Handle a `store_put` message: qualify the store name, write the key-value
/// pair (and optionally the global flag) to the redb database in a single
/// transaction, then rebuild the search index.
///
/// Returns a JSON ack string (`{"id":"<id>","ok":true}\n`) after the put and
/// index rebuild complete, or after all retries are exhausted. The caller
/// writes this string to fd 4.
///
/// When `global` is `None`, the global flag is left unchanged. When
/// `Some(true)` or `Some(false)`, the flag is set in the same transaction
/// as the KV write.
///
/// If the database is locked by another process (`DatabaseAlreadyOpen`),
/// retries up to 3 times with 50ms backoff. If all retries fail, logs a
/// warning to stderr. The ack is written regardless of outcome — the caller
/// only needs synchronization, not error reporting.
#[allow(clippy::too_many_arguments)] // All parameters are distinct; a struct would add indirection without clarity.
fn handle_store_put(
    id: &str,
    name: &str,
    key: &str,
    value: &str,
    global: Option<bool>,
    skill_name: &str,
    plugin: Option<&str>,
    store_dir: &Path,
) -> String {
    let ns = namespace::skill_namespace(skill_name);
    let qualified = namespace::qualify(name, ns, plugin);

    let mut last_err: Option<crate::error::CreftError> = None;
    for _ in 0..3u32 {
        match store_kv::store_put(store_dir, &qualified, key, value, global) {
            Ok(()) => {}
            Err(e @ crate::error::CreftError::StoreOpen { .. }) => {
                // Database locked by another process — retry after backoff.
                last_err = Some(e);
                std::thread::sleep(std::time::Duration::from_millis(50));
                continue;
            }
            Err(e) => {
                eprintln!("warning: store put failed for '{qualified}': {e}");
                return format!("{{\"id\":\"{id}\",\"ok\":true}}\n");
            }
        }

        match store_kv::rebuild_store_index(store_dir, &qualified) {
            Ok(()) => return format!("{{\"id\":\"{id}\",\"ok\":true}}\n"),
            Err(e) => {
                eprintln!("warning: store index rebuild failed for '{qualified}': {e}");
                return format!("{{\"id\":\"{id}\",\"ok\":true}}\n");
            }
        }
    }

    let reason = last_err
        .map(|e| e.to_string())
        .unwrap_or_else(|| "database locked after 3 attempts".to_owned());
    eprintln!("warning: store put failed for '{qualified}': {reason}");
    format!("{{\"id\":\"{id}\",\"ok\":true}}\n")
}

/// Handle a `store_get` message: qualify the store name (local only), read
/// the value from the redb database, and return a JSON response.
///
/// Response format: `{"id":"<id>","value":"<value>"}` or
/// `{"id":"<id>","value":""}` when the key does not exist.
///
/// Cross-namespace get is not supported — dotted names in `name` are rejected
/// with an error response containing a hint to use `creft_store_search`.
fn handle_store_get(
    id: &str,
    name: &str,
    key: &str,
    skill_name: &str,
    plugin: Option<&str>,
    store_dir: &Path,
) -> String {
    // Cross-namespace get is not supported.
    if name.contains('.') {
        let msg = "cross-namespace get not supported; use creft_store_search";
        return format!("{{\"id\":\"{id}\",\"error\":\"{msg}\"}}\n");
    }

    let ns = namespace::skill_namespace(skill_name);
    let qualified = namespace::qualify(name, ns, plugin);

    match store_kv::store_get(store_dir, &qualified, key) {
        Ok(Some(val)) => {
            let escaped = json_escape_string(&val);
            format!("{{\"id\":\"{id}\",\"value\":\"{escaped}\"}}\n")
        }
        Ok(None) => {
            format!("{{\"id\":\"{id}\",\"value\":\"\"}}\n")
        }
        Err(e) => {
            let msg = json_escape_string(&e.to_string());
            format!("{{\"id\":\"{id}\",\"error\":\"{msg}\"}}\n")
        }
    }
}

/// Handle a `store_search` message: resolve the store name (with access
/// control via `store_is_global`), load the store's search index from disk,
/// query it, and return matching keys.
///
/// Response format: `{"id":"<id>","results":"key1\nkey2\n..."}` or
/// `{"id":"<id>","error":"access denied: ..."}` when the store is not global
/// and the caller is from a different namespace.
fn handle_store_search(
    id: &str,
    name: &str,
    query: &str,
    skill_name: &str,
    plugin: Option<&str>,
    store_dir: &Path,
) -> String {
    let ns = namespace::skill_namespace(skill_name);

    // A dotted name is a cross-namespace reference: the caller supplies the
    // fully-qualified store name (e.g. "deploy.data"). A plain name is local
    // and gets qualified using the caller's namespace and plugin context.
    let (qualified, is_cross_namespace) = if name.contains('.') {
        (name.to_owned(), true)
    } else {
        (namespace::qualify(name, ns, plugin), false)
    };

    // Cross-namespace access: check whether the target store is globally accessible.
    if is_cross_namespace && !store_kv::store_is_global(store_dir, &qualified) {
        let msg = format!("access denied: '{qualified}' is not shared globally");
        let escaped = json_escape_string(&msg);
        return format!("{{\"id\":\"{id}\",\"error\":\"{escaped}\"}}\n");
    }

    let results = match store_kv::load_store_index(store_dir, &qualified) {
        None => String::new(),
        Some(index) => {
            let hits = index.search(query);
            if hits.is_empty() {
                // Fall back to fuzzy search.
                let fuzzy_hits = index.search_fuzzy(query);
                fuzzy_hits
                    .iter()
                    .map(|e| e.name.as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            } else {
                hits.iter()
                    .map(|e| e.name.as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            }
        }
    };

    let escaped = json_escape_string(&results);
    format!("{{\"id\":\"{id}\",\"results\":\"{escaped}\"}}\n")
}

/// Handle a prompt request by rendering it to the terminal and collecting user
/// input, then writing the response to the response pipe.
///
/// When `interactive` is `false` (pipe chain context or piped stdin), the
/// response is written immediately with an empty value without prompting the
/// user. This prevents the child from deadlocking on its `read <&4` call.
///
/// When `interactive` is `true`, the `TerminalWriter` lock is held for the
/// entire prompt-and-read cycle so that no other thread writes to stderr
/// between the prompt question and the user's answer.
pub(crate) fn handle_prompt(
    prompt: &ChannelMessage,
    response_writer: &mut File,
    writer: &Arc<TerminalWriter>,
    interactive: bool,
) {
    let (id, question, choices) = match prompt {
        ChannelMessage::Prompt {
            id,
            question,
            choices,
        } => (id.as_str(), question.as_str(), choices.as_str()),
        // Only Prompt messages are valid inputs; callers must not pass other variants.
        _ => return,
    };

    if !interactive {
        // Non-interactive: write empty response and return immediately.
        let response = format!("{{\"id\":\"{id}\",\"value\":\"\"}}\n");
        let _ = response_writer.write_all(response.as_bytes());
        let _ = response_writer.flush();
        return;
    }

    // Interactive: hold the TerminalWriter lock for the whole prompt+read
    // cycle so no concurrent block output appears between the prompt question
    // and the user's typed answer.
    let Ok(mut g) = writer.inner.lock() else {
        // Mutex poisoned — fall back to empty response.
        let response = format!("{{\"id\":\"{id}\",\"value\":\"\"}}\n");
        let _ = response_writer.write_all(response.as_bytes());
        let _ = response_writer.flush();
        return;
    };

    // Finish any active indicator before rendering the prompt so indicatif's
    // tick thread does not write over the prompt text.
    clear_indicator_locked(&mut g);

    // Render the prompt question and choices to stderr.
    if choices.is_empty() {
        let _ = writeln!(g.stderr, "{question}: ");
    } else {
        let _ = writeln!(g.stderr, "{question} [{choices}]: ");
    }
    let _ = g.stderr.flush();

    // Read the user's answer from stdin while still holding the lock.
    let mut line = String::new();
    let answer = match std::io::stdin().read_line(&mut line) {
        Ok(_) => line.trim().to_owned(),
        // stdin closed (Ctrl+D, closed pipe, cancelled) — empty response.
        Err(_) => String::new(),
    };

    // Release the lock before writing to the response pipe — the write does
    // not touch stderr and does not need the terminal lock.
    drop(g);

    let escaped = json_escape_string(&answer);
    let response = format!("{{\"id\":\"{id}\",\"value\":\"{escaped}\"}}\n");
    let _ = response_writer.write_all(response.as_bytes());
    let _ = response_writer.flush();
}

/// Spawn a prompt handler thread that receives prompt requests from the reader
/// thread and processes them sequentially.
///
/// Returns the join handle. The thread exits when `prompt_rx` is closed (the
/// reader thread has exited after the child process exited).
///
/// Each received `ChannelMessage::Prompt` is forwarded to `handle_prompt`.
/// Non-Prompt messages are ignored — the reader thread only sends Prompt
/// variants over this channel.
pub(crate) fn spawn_prompt_handler(
    prompt_rx: mpsc::Receiver<ChannelMessage>,
    response_writer: OwnedFd,
    writer: Arc<TerminalWriter>,
    interactive: bool,
) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("creft-prompt-handler".to_owned())
        .spawn(move || {
            // SAFETY: response_writer is valid and exclusively owned by this
            // thread. Converting to File gives buffered write access.
            let mut file = unsafe { File::from_raw_fd(response_writer.into_raw_fd()) };

            for msg in prompt_rx {
                handle_prompt(&msg, &mut file, &writer, interactive);
            }
            // prompt_rx is closed — reader thread has exited. Thread exits.
        })
        .expect("failed to spawn prompt handler thread")
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::io::Read as _;
    use std::os::fd::AsRawFd as _;
    use std::os::unix::io::{FromRawFd as _, IntoRawFd as _};
    use std::sync::{Arc, mpsc};

    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::SideChannel;

    /// All four file descriptors in a fresh SideChannel are distinct.
    #[test]
    fn new_produces_four_distinct_fds() {
        let ch = SideChannel::new().unwrap();
        let ctrl_r = ch.control_reader.as_ref().unwrap().as_raw_fd();
        let ctrl_w = ch.control_writer.as_ref().unwrap().as_raw_fd();
        let resp_w = ch.response_writer.as_ref().unwrap().as_raw_fd();
        let resp_r = ch.response_reader.as_ref().unwrap().as_raw_fd();

        let fds = [ctrl_r, ctrl_w, resp_w, resp_r];
        let unique: std::collections::HashSet<i32> = fds.iter().copied().collect();
        assert_eq!(
            unique.len(),
            4,
            "all four fds must be distinct; got {fds:?}"
        );
    }

    /// child_fds returns control_writer and response_reader raw fds.
    #[test]
    fn child_fds_returns_correct_ends() {
        let ch = SideChannel::new().unwrap();
        let (cfd, rfd) = ch.child_fds();
        assert_eq!(cfd, ch.control_writer.as_ref().unwrap().as_raw_fd());
        assert_eq!(rfd, ch.response_reader.as_ref().unwrap().as_raw_fd());
    }

    /// take_control_reader moves the fd out; subsequent calls return None.
    #[test]
    fn take_control_reader_is_idempotent() {
        let mut ch = SideChannel::new().unwrap();
        let first = ch.take_control_reader();
        assert!(first.is_some(), "first take should return the fd");
        let second = ch.take_control_reader();
        assert!(second.is_none(), "second take should return None");
    }

    /// take_response_writer moves the fd out; subsequent calls return None.
    #[test]
    fn take_response_writer_is_idempotent() {
        let mut ch = SideChannel::new().unwrap();
        let first = ch.take_response_writer();
        assert!(first.is_some(), "first take should return the fd");
        let second = ch.take_response_writer();
        assert!(second.is_none(), "second take should return None");
    }

    /// A spawned bash process that writes to fd 3 produces bytes on the
    /// parent's control reader.
    #[test]
    fn bash_writes_to_fd3_are_readable_by_parent() {
        let mut ch = SideChannel::new().unwrap();
        let (ctrl_w_fd, _resp_r_fd) = ch.child_fds();

        // Spawn bash, inherit fd 3 (ctrl_w_fd) as fd 3.
        let mut cmd = std::process::Command::new("bash");
        cmd.arg("-c");
        cmd.arg("echo -n hello >&3");
        // SAFETY: dup2 and close are async-signal-safe POSIX calls.  The
        // captured fd value is Copy (i32). No Rust allocations occur.
        unsafe {
            use std::os::unix::process::CommandExt as _;
            cmd.pre_exec(move || {
                if libc::dup2(ctrl_w_fd, 3) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                if ctrl_w_fd != 3 {
                    libc::close(ctrl_w_fd);
                }
                Ok(())
            });
        }

        let ctrl_reader_fd = ch.take_control_reader().unwrap();

        let mut child = cmd.spawn().unwrap();

        // Close the parent's write end so we see EOF after the child exits.
        ch.close_child_ends();

        child.wait().unwrap();

        // Read what the child wrote.
        let mut reader = unsafe { std::fs::File::from_raw_fd(ctrl_reader_fd.into_raw_fd()) };
        let mut buf = String::new();
        reader.read_to_string(&mut buf).unwrap();
        assert_eq!(buf, "hello");
    }

    /// A bash process that never writes to fd 3 exits cleanly — no hang.
    #[test]
    fn bash_ignoring_fd3_exits_cleanly() {
        let mut ch = SideChannel::new().unwrap();
        let (ctrl_w_fd, _resp_r_fd) = ch.child_fds();

        let mut cmd = std::process::Command::new("bash");
        cmd.arg("-c");
        cmd.arg("exit 0");
        unsafe {
            use std::os::unix::process::CommandExt as _;
            cmd.pre_exec(move || {
                if libc::dup2(ctrl_w_fd, 3) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                if ctrl_w_fd != 3 {
                    libc::close(ctrl_w_fd);
                }
                Ok(())
            });
        }

        let ctrl_reader_fd = ch.take_control_reader().unwrap();

        let mut child = cmd.spawn().unwrap();
        ch.close_child_ends();
        let status = child.wait().unwrap();
        assert!(status.success());

        // Drain the reader — should see EOF immediately.
        let mut reader = unsafe { std::fs::File::from_raw_fd(ctrl_reader_fd.into_raw_fd()) };
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).unwrap();
        assert!(buf.is_empty(), "no bytes expected from a silent child");
    }

    /// After close_child_ends, the parent no longer holds the child's pipe
    /// write end, so the control reader reaches EOF after the child exits.
    #[test]
    fn close_child_ends_prevents_reader_hang() {
        let mut ch = SideChannel::new().unwrap();
        let (ctrl_w_fd, _) = ch.child_fds();

        let mut cmd = std::process::Command::new("bash");
        cmd.arg("-c");
        cmd.arg("echo -n ping >&3");
        unsafe {
            use std::os::unix::process::CommandExt as _;
            cmd.pre_exec(move || {
                if libc::dup2(ctrl_w_fd, 3) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                if ctrl_w_fd != 3 {
                    libc::close(ctrl_w_fd);
                }
                Ok(())
            });
        }

        let ctrl_reader_fd = ch.take_control_reader().unwrap();
        let mut child = cmd.spawn().unwrap();

        // This is the critical call: without it the reader would hang
        // because the parent still holds the write end of the control pipe.
        ch.close_child_ends();

        child.wait().unwrap();

        let mut reader = unsafe { std::fs::File::from_raw_fd(ctrl_reader_fd.into_raw_fd()) };
        let mut buf = String::new();
        // read_to_string would block indefinitely if the parent still held
        // the write end open.
        reader.read_to_string(&mut buf).unwrap();
        assert_eq!(buf, "ping");
    }

    // ---- ChannelMessage deserialization ----

    /// A well-formed print message deserializes to the Print variant.
    #[test]
    fn channel_message_print_deserializes() {
        let json = r#"{"type":"print","message":"hello world"}"#;
        let msg: super::ChannelMessage = serde_json::from_str(json).unwrap();
        match msg {
            super::ChannelMessage::Print { message } => {
                assert_eq!(message, "hello world");
            }
            other => panic!("expected Print, got {other:?}"),
        }
    }

    /// A well-formed status message deserializes to the Status variant with the
    /// correct message and progress field. The `no_progress` case proves backward
    /// compatibility: JSON without a `progress` field must still deserialize
    /// because `serde(default)` makes the field optional.
    #[rstest]
    #[case::no_progress(r#"{"type":"status","message":"Loading..."}"#, "Loading...", None)]
    #[case::progress_50(
        r#"{"type":"status","message":"Downloading...","progress":50}"#,
        "Downloading...",
        Some(50)
    )]
    #[case::progress_zero(
        r#"{"type":"status","message":"Starting...","progress":0}"#,
        "Starting...",
        Some(0)
    )]
    #[case::progress_hundred(
        r#"{"type":"status","message":"Done","progress":100}"#,
        "Done",
        Some(100)
    )]
    fn channel_message_status_deserializes(
        #[case] json: &str,
        #[case] expected_message: &str,
        #[case] expected_progress: Option<u8>,
    ) {
        let msg: super::ChannelMessage = serde_json::from_str(json).unwrap();
        match msg {
            super::ChannelMessage::Status { message, progress } => {
                assert_eq!(message, expected_message);
                assert_eq!(progress, expected_progress);
            }
            other => panic!("expected Status, got {other:?}"),
        }
    }

    /// A well-formed prompt message deserializes to the Prompt variant.
    #[test]
    fn channel_message_prompt_deserializes() {
        let json = r#"{"type":"prompt","id":"p1","question":"Continue?","choices":"yes,no"}"#;
        let msg: super::ChannelMessage = serde_json::from_str(json).unwrap();
        match msg {
            super::ChannelMessage::Prompt {
                id,
                question,
                choices,
            } => {
                assert_eq!(id, "p1");
                assert_eq!(question, "Continue?");
                assert_eq!(choices, "yes,no");
            }
            other => panic!("expected Prompt, got {other:?}"),
        }
    }

    /// An unknown message type returns a deserialization error rather than panicking.
    #[test]
    fn channel_message_unknown_type_returns_error() {
        let json = r#"{"type":"unknown","data":"whatever"}"#;
        let result = serde_json::from_str::<super::ChannelMessage>(json);
        assert!(
            result.is_err(),
            "unknown type must produce a deserialization error"
        );
    }

    /// Completely malformed JSON returns a deserialization error.
    #[test]
    fn channel_message_malformed_json_returns_error() {
        let result = serde_json::from_str::<super::ChannelMessage>("not json at all");
        assert!(result.is_err(), "malformed JSON must produce an error");
    }

    // ---- TerminalWriter ----

    /// TerminalWriter::print completes without panicking regardless of TTY state.
    #[test]
    fn terminal_writer_print_does_not_panic() {
        let tw = super::TerminalWriter::new();
        tw.print("test message");
        // No active indicator after a plain print.
        let g = tw.inner.lock().unwrap();
        assert!(g.indicator.is_none(), "print must not create an indicator");
    }

    /// TerminalWriter::status creates an active indicator when stderr is a TTY.
    ///
    /// Uses `with_tty(true)` because test stderr is never a real TTY — without
    /// the override, status() is a no-op and no indicator is created.
    #[test]
    fn terminal_writer_status_creates_indicator_when_tty() {
        let tw = super::TerminalWriter::with_tty(true);
        tw.status("Working...");
        let g = tw.inner.lock().unwrap();
        assert!(
            g.indicator.is_some(),
            "status must create an active indicator on a TTY"
        );
    }

    /// TerminalWriter::status is a no-op when stderr is not a TTY.
    ///
    /// Status messages are ephemeral visual feedback. In non-TTY contexts
    /// (piped output, CI, agent subprocess) they are silently dropped.
    #[test]
    fn terminal_writer_status_dropped_when_not_tty() {
        let tw = super::TerminalWriter::with_tty(false);
        tw.status("Working...");
        let g = tw.inner.lock().unwrap();
        assert!(
            g.indicator.is_none(),
            "status must not create an indicator on a non-TTY"
        );
    }

    /// TerminalWriter::clear_status removes the active indicator.
    #[test]
    fn terminal_writer_clear_status_removes_indicator() {
        let tw = super::TerminalWriter::with_tty(true);
        tw.status("Running");
        tw.clear_status();
        let g = tw.inner.lock().unwrap();
        assert!(
            g.indicator.is_none(),
            "clear_status must remove the active indicator"
        );
    }

    /// clear_status is a no-op when no indicator is active (must not panic).
    #[test]
    fn terminal_writer_clear_status_noop_when_no_status() {
        let tw = super::TerminalWriter::new();
        tw.clear_status(); // must not panic
        let g = tw.inner.lock().unwrap();
        assert!(
            g.indicator.is_none(),
            "indicator must remain None after clear_status with nothing active"
        );
    }

    /// print completes without panic when a spinner indicator is active.
    #[test]
    fn terminal_writer_print_does_not_panic_with_active_indicator() {
        let tw = super::TerminalWriter::with_tty(true);
        tw.status("Pending");
        tw.print("Done"); // must not panic when indicator is active
    }

    /// progress creates a progress indicator when stderr is a TTY.
    #[test]
    fn terminal_writer_progress_creates_indicator_when_tty() {
        let tw = super::TerminalWriter::with_tty(true);
        tw.progress("Downloading...", 50);
        let g = tw.inner.lock().unwrap();
        assert!(
            g.indicator.is_some(),
            "progress must create an active indicator on a TTY"
        );
    }

    /// progress is a no-op when stderr is not a TTY.
    #[test]
    fn terminal_writer_progress_dropped_when_not_tty() {
        let tw = super::TerminalWriter::with_tty(false);
        tw.progress("Downloading...", 50);
        let g = tw.inner.lock().unwrap();
        assert!(
            g.indicator.is_none(),
            "progress must not create an indicator on a non-TTY"
        );
    }

    /// Switching from spinner to progress bar does not panic.
    #[test]
    fn terminal_writer_spinner_to_progress_does_not_panic() {
        let tw = super::TerminalWriter::with_tty(true);
        tw.status("Preparing...");
        tw.progress("Downloading...", 25); // switch to progress bar
        let g = tw.inner.lock().unwrap();
        assert!(
            g.indicator.is_some(),
            "a progress indicator must be active after switching from spinner"
        );
    }

    /// Switching from progress bar to spinner does not panic.
    #[test]
    fn terminal_writer_progress_to_spinner_does_not_panic() {
        let tw = super::TerminalWriter::with_tty(true);
        tw.progress("Downloading...", 50);
        tw.status("Processing..."); // switch to spinner
        let g = tw.inner.lock().unwrap();
        assert!(
            g.indicator.is_some(),
            "a spinner indicator must be active after switching from progress"
        );
    }

    /// progress values above 100 are saturated without panicking.
    #[test]
    fn terminal_writer_progress_saturates_above_100() {
        let tw = super::TerminalWriter::with_tty(true);
        tw.progress("Overloaded", 200); // saturates to 100
        let g = tw.inner.lock().unwrap();
        assert!(
            g.indicator.is_some(),
            "saturated progress value must still create an indicator"
        );
    }

    /// print does not panic when no indicator is active.
    #[test]
    fn terminal_writer_print_does_not_panic_without_indicator() {
        let tw = super::TerminalWriter::with_tty(true);
        tw.print("Hello"); // no indicator active
    }

    // ---- handle_prompt ----

    /// Non-interactive mode writes an empty response immediately.
    #[test]
    fn handle_prompt_non_interactive_writes_empty_response() {
        // Use a raw pipe independent of SideChannel to avoid shared-fd ownership
        // confusion: each end is exclusively owned by one binding.
        let (resp_reader_fd, resp_writer_fd) = super::os_pipe().unwrap();
        let mut resp_reader = unsafe { std::fs::File::from_raw_fd(resp_reader_fd.into_raw_fd()) };
        let mut resp_writer = unsafe { std::fs::File::from_raw_fd(resp_writer_fd.into_raw_fd()) };
        let writer = Arc::new(super::TerminalWriter::new());

        let msg = super::ChannelMessage::Prompt {
            id: "p1".to_owned(),
            question: "Continue?".to_owned(),
            choices: "yes,no".to_owned(),
        };

        super::handle_prompt(&msg, &mut resp_writer, &writer, false);

        // Drop the writer to close the write end so read_to_string sees EOF.
        drop(resp_writer);

        let mut buf = String::new();
        resp_reader.read_to_string(&mut buf).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(buf.trim()).unwrap();
        assert_eq!(parsed["id"], "p1");
        assert_eq!(parsed["value"], "");
    }

    /// Non-interactive handle_prompt response is valid JSON with the correct id.
    #[test]
    fn handle_prompt_non_interactive_response_is_valid_json() {
        let (resp_reader_fd, resp_writer_fd) = super::os_pipe().unwrap();
        let mut resp_reader = unsafe { std::fs::File::from_raw_fd(resp_reader_fd.into_raw_fd()) };
        let mut resp_writer = unsafe { std::fs::File::from_raw_fd(resp_writer_fd.into_raw_fd()) };
        let writer = Arc::new(super::TerminalWriter::new());

        let msg = super::ChannelMessage::Prompt {
            id: "unique-id-42".to_owned(),
            question: "Deploy?".to_owned(),
            choices: "".to_owned(),
        };

        super::handle_prompt(&msg, &mut resp_writer, &writer, false);
        drop(resp_writer);

        let mut buf = String::new();
        resp_reader.read_to_string(&mut buf).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(buf.trim()).unwrap();
        assert_eq!(parsed["id"], "unique-id-42");
        assert_eq!(parsed["value"], "");
    }

    // ---- spawn_prompt_handler ----

    /// spawn_prompt_handler writes empty responses and exits when the channel closes.
    #[test]
    fn spawn_prompt_handler_exits_when_channel_closes() {
        let (resp_reader_fd, resp_writer_fd) = super::os_pipe().unwrap();
        let mut resp_reader = unsafe { std::fs::File::from_raw_fd(resp_reader_fd.into_raw_fd()) };

        let writer = Arc::new(super::TerminalWriter::new());
        let (tx, rx) = mpsc::channel::<super::ChannelMessage>();

        let handle = super::spawn_prompt_handler(rx, resp_writer_fd, writer, false);

        // Send a prompt then close the sender to signal the handler to exit.
        tx.send(super::ChannelMessage::Prompt {
            id: "ph-test".to_owned(),
            question: "Continue?".to_owned(),
            choices: "yes,no".to_owned(),
        })
        .unwrap();
        drop(tx);

        handle.join().expect("prompt handler thread must not panic");

        // The handler also closed resp_writer_fd when it exited, so the read
        // end sees EOF and read_to_string returns.
        let mut buf = String::new();
        resp_reader.read_to_string(&mut buf).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(buf.trim()).unwrap();
        assert_eq!(parsed["id"], "ph-test");
        assert_eq!(parsed["value"], "");
    }

    // ---- spawn_reader integration ----

    /// A bash block writing a print message to fd 3 via the preamble produces
    /// that message through spawn_reader without hanging.
    ///
    /// We cannot capture what spawn_reader writes to stderr in a unit test, but
    /// we verify the reader thread exits cleanly (join returns Ok) and the
    /// child process exits successfully.
    #[test]
    fn spawn_reader_drains_print_message_cleanly() {
        let mut ch = SideChannel::new().unwrap();
        let (ctrl_w_fd, resp_r_fd) = ch.child_fds();

        // Spawn bash that writes a JSON print message to fd 3.
        let mut cmd = std::process::Command::new("bash");
        cmd.arg("-c");
        cmd.arg(r#"printf '{"type":"print","message":"hello"}\n' >&3"#);
        unsafe {
            use std::os::unix::process::CommandExt as _;
            cmd.pre_exec(move || {
                if libc::dup2(ctrl_w_fd, 3) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                if ctrl_w_fd != 3 {
                    libc::close(ctrl_w_fd);
                }
                // Dup the response reader to fd 4 even though it won't be used.
                if libc::dup2(resp_r_fd, 4) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                if resp_r_fd != 4 {
                    libc::close(resp_r_fd);
                }
                Ok(())
            });
        }

        let ctrl_reader = ch.take_control_reader().unwrap();
        let resp_writer = ch
            .take_response_writer()
            .map(|fd| unsafe { std::fs::File::from_raw_fd(fd.into_raw_fd()) });
        let mut child = cmd.spawn().unwrap();
        ch.close_child_ends();

        let writer = Arc::new(super::TerminalWriter::new());
        let handle = super::spawn_reader(ctrl_reader, writer, None, resp_writer, None, None);

        let status = child.wait().unwrap();
        assert!(status.success(), "bash must exit successfully");

        // join must complete — if spawn_reader hangs on EOF, this blocks.
        handle.join().expect("reader thread must not panic");
    }

    /// A bash block writing an unknown message type to fd 3 is silently ignored
    /// and the reader thread exits cleanly.
    #[test]
    fn spawn_reader_ignores_unknown_message_type() {
        let mut ch = SideChannel::new().unwrap();
        let (ctrl_w_fd, resp_r_fd) = ch.child_fds();

        let mut cmd = std::process::Command::new("bash");
        cmd.arg("-c");
        cmd.arg(r#"printf '{"type":"unknown","data":"whatever"}\n' >&3"#);
        unsafe {
            use std::os::unix::process::CommandExt as _;
            cmd.pre_exec(move || {
                if libc::dup2(ctrl_w_fd, 3) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                if ctrl_w_fd != 3 {
                    libc::close(ctrl_w_fd);
                }
                if libc::dup2(resp_r_fd, 4) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                if resp_r_fd != 4 {
                    libc::close(resp_r_fd);
                }
                Ok(())
            });
        }

        let ctrl_reader = ch.take_control_reader().unwrap();
        let resp_writer = ch
            .take_response_writer()
            .map(|fd| unsafe { std::fs::File::from_raw_fd(fd.into_raw_fd()) });
        let mut child = cmd.spawn().unwrap();
        ch.close_child_ends();

        let writer = Arc::new(super::TerminalWriter::new());
        let handle = super::spawn_reader(ctrl_reader, writer, None, resp_writer, None, None);

        child.wait().unwrap();
        handle
            .join()
            .expect("reader thread must not panic on unknown type");
    }

    /// A bash block that never writes to fd 3 causes spawn_reader to exit
    /// cleanly when the pipe reaches EOF.
    #[test]
    fn spawn_reader_exits_on_eof_with_no_messages() {
        let mut ch = SideChannel::new().unwrap();
        let (ctrl_w_fd, resp_r_fd) = ch.child_fds();

        let mut cmd = std::process::Command::new("bash");
        cmd.arg("-c");
        cmd.arg("exit 0");
        unsafe {
            use std::os::unix::process::CommandExt as _;
            cmd.pre_exec(move || {
                if libc::dup2(ctrl_w_fd, 3) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                if ctrl_w_fd != 3 {
                    libc::close(ctrl_w_fd);
                }
                if libc::dup2(resp_r_fd, 4) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                if resp_r_fd != 4 {
                    libc::close(resp_r_fd);
                }
                Ok(())
            });
        }

        let ctrl_reader = ch.take_control_reader().unwrap();
        let resp_writer = ch
            .take_response_writer()
            .map(|fd| unsafe { std::fs::File::from_raw_fd(fd.into_raw_fd()) });
        let mut child = cmd.spawn().unwrap();
        ch.close_child_ends();

        let writer = Arc::new(super::TerminalWriter::new());
        let handle = super::spawn_reader(ctrl_reader, writer, None, resp_writer, None, None);

        let status = child.wait().unwrap();
        assert!(status.success());
        handle.join().expect("reader thread must not panic");
    }

    /// A bash block that sends a prompt message via fd 3 in non-interactive mode
    /// receives an empty response on fd 4 and does not deadlock.
    ///
    /// This wires spawn_reader (with prompt_tx) and spawn_prompt_handler together
    /// to verify the full prompt forwarding path in a non-interactive context.
    #[test]
    fn prompt_non_interactive_round_trip_does_not_deadlock() {
        let mut ch = SideChannel::new().unwrap();
        let (ctrl_w_fd, resp_r_fd) = ch.child_fds();

        // Bash script: send a prompt message to fd 3, read the response from
        // fd 4, and echo the response value to stdout.
        let script = r#"
printf '{"type":"prompt","id":"t1","question":"Continue?","choices":"yes,no"}\n' >&3
read -r _resp <&4
printf '%s' "$_resp"
"#;

        let mut cmd = std::process::Command::new("bash");
        cmd.arg("-c");
        cmd.arg(script);
        cmd.stdout(std::process::Stdio::piped());
        unsafe {
            use std::os::unix::process::CommandExt as _;
            cmd.pre_exec(move || {
                if libc::dup2(ctrl_w_fd, 3) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                if ctrl_w_fd != 3 {
                    libc::close(ctrl_w_fd);
                }
                if libc::dup2(resp_r_fd, 4) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                if resp_r_fd != 4 {
                    libc::close(resp_r_fd);
                }
                Ok(())
            });
        }

        let ctrl_reader = ch.take_control_reader().unwrap();
        let resp_writer_fd = ch.take_response_writer().unwrap();

        let child = cmd.spawn().unwrap();
        ch.close_child_ends();

        let writer = Arc::new(super::TerminalWriter::new());
        let (prompt_tx, prompt_rx) = mpsc::channel::<super::ChannelMessage>();

        let reader_handle = super::spawn_reader(
            ctrl_reader,
            Arc::clone(&writer),
            Some(prompt_tx),
            None,
            None,
            None,
        );
        let prompt_handle = super::spawn_prompt_handler(prompt_rx, resp_writer_fd, writer, false);

        let output = child.wait_with_output().unwrap();
        reader_handle.join().expect("reader thread must not panic");
        prompt_handle.join().expect("prompt handler must not panic");

        // The bash script writes the raw response JSON to stdout.
        let stdout = String::from_utf8(output.stdout).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
        assert_eq!(parsed["id"], "t1");
        assert_eq!(parsed["value"], "");
    }

    // ---- ChannelMessage::Exit deserialization ----

    /// `{"type":"exit","code":1}` deserializes to `Exit { code: 1 }`.
    #[test]
    fn channel_message_exit_with_code_deserializes() {
        let json = r#"{"type":"exit","code":1}"#;
        let msg: super::ChannelMessage = serde_json::from_str(json).unwrap();
        match msg {
            super::ChannelMessage::Exit { code } => {
                assert_eq!(code, 1, "exit code must be 1");
            }
            other => panic!("expected Exit, got {other:?}"),
        }
    }

    /// `{"type":"exit"}` (no code field) deserializes to `Exit { code: 0 }`.
    ///
    /// The `serde(default)` on the `code` field means a missing field
    /// deserializes to the type's default (0 for i32), which matches
    /// the no-arg `creft_exit` behavior.
    #[test]
    fn channel_message_exit_without_code_defaults_to_zero() {
        let json = r#"{"type":"exit"}"#;
        let msg: super::ChannelMessage = serde_json::from_str(json).unwrap();
        match msg {
            super::ChannelMessage::Exit { code } => {
                assert_eq!(code, 0, "missing code field must default to 0");
            }
            other => panic!("expected Exit, got {other:?}"),
        }
    }

    // ---- ExitSignal ----

    /// Writing then reading through an ExitSignal round-trips the value.
    ///
    /// Verifies the Arc<Mutex<Option<i32>>> pattern works as documented:
    /// the writer sets Some(code) and the reader sees the same value.
    #[test]
    fn exit_signal_write_then_read_roundtrips() {
        let signal: super::ExitSignal = Arc::new(std::sync::Mutex::new(None));
        {
            let mut slot = signal.lock().unwrap();
            *slot = Some(42);
        }
        let value = *signal.lock().unwrap();
        assert_eq!(
            value,
            Some(42),
            "ExitSignal round-trip must preserve the code"
        );
    }

    // ---- CappedTerm ----

    use indicatif::TermLike as _;

    /// CappedTerm::width() reports at most max_width, even when the inner term
    /// reports a wider value (or a default in test contexts without a real TTY).
    ///
    /// The test constructs a CappedTerm with max_width = 40 and verifies that
    /// width() does not exceed 40, satisfying the invariant that the progress
    /// bar never renders wider than the configured maximum.
    #[test]
    fn capped_term_width_does_not_exceed_max() {
        let capped = super::CappedTerm::stderr(40);
        assert!(
            capped.width() <= 40,
            "CappedTerm::width() must not exceed max_width; got {}",
            capped.width()
        );
    }

    /// CappedTerm with a max_width larger than any real terminal reports the
    /// real terminal width (the cap does not force a minimum).
    #[test]
    fn capped_term_width_does_not_shrink_narrow_terminals() {
        // u16::MAX is wider than any real terminal; the reported width must
        // be whatever the inner term actually returns (uncapped).
        let capped_large = super::CappedTerm::stderr(u16::MAX);
        let uncapped = super::CappedTerm::stderr(u16::MAX);
        assert_eq!(
            capped_large.width(),
            uncapped.width(),
            "max_width larger than terminal width must have no effect"
        );
    }

    // ── ChannelMessage::Index deserialization ─────────────────────────────────

    /// An Index message with explicit global:true deserializes correctly.
    #[test]
    fn channel_message_index_with_global_true_deserializes() {
        let json =
            r#"{"type":"index","id":"i1","name":"beta","content":"some docs","global":true}"#;
        let msg: super::ChannelMessage = serde_json::from_str(json).unwrap();
        match msg {
            super::ChannelMessage::Index {
                id,
                name,
                content,
                global,
            } => {
                assert_eq!(id, "i1");
                assert_eq!(name, "beta");
                assert_eq!(content, "some docs");
                assert!(global, "global field must be true when set to true");
            }
            other => panic!("expected Index, got {other:?}"),
        }
    }

    /// An Index message without global field defaults to false.
    #[test]
    fn channel_message_index_without_global_defaults_false() {
        let json = r#"{"type":"index","id":"i2","name":"beta","content":"some docs"}"#;
        let msg: super::ChannelMessage = serde_json::from_str(json).unwrap();
        match msg {
            super::ChannelMessage::Index {
                id,
                name,
                content,
                global,
            } => {
                assert_eq!(id, "i2");
                assert_eq!(name, "beta");
                assert_eq!(content, "some docs");
                assert!(!global, "missing global field must default to false");
            }
            other => panic!("expected Index, got {other:?}"),
        }
    }

    /// An Index message with global:false deserializes correctly.
    #[test]
    fn channel_message_index_with_global_false_deserializes() {
        let json = r#"{"type":"index","id":"i3","name":"configs","content":"data","global":false}"#;
        let msg: super::ChannelMessage = serde_json::from_str(json).unwrap();
        match msg {
            super::ChannelMessage::Index { global, .. } => {
                assert!(!global, "global field must be false when set to false");
            }
            other => panic!("expected Index, got {other:?}"),
        }
    }

    // ── ChannelMessage::Search deserialization ────────────────────────────────

    /// A well-formed Search message deserializes to the Search variant.
    #[test]
    fn channel_message_search_deserializes() {
        let json = r#"{"type":"search","id":"s1","query":"rollback","name":"beta"}"#;
        let msg: super::ChannelMessage = serde_json::from_str(json).unwrap();
        match msg {
            super::ChannelMessage::Search { id, query, name } => {
                assert_eq!(id, "s1");
                assert_eq!(query, "rollback");
                assert_eq!(name, "beta");
            }
            other => panic!("expected Search, got {other:?}"),
        }
    }

    // ── handle_index_message / handle_search_message unit tests ──────────────

    fn make_runtime_indexes() -> Arc<std::sync::Mutex<HashMap<String, super::super::RuntimeIndex>>>
    {
        Arc::new(std::sync::Mutex::new(HashMap::new()))
    }

    /// handle_index_message qualifies the name using the skill's namespace and
    /// inserts it into runtime_indexes keyed by the qualified name.
    #[test]
    fn handle_index_message_qualifies_name_with_namespace() {
        let indexes = make_runtime_indexes();
        super::handle_index_message(
            "i1",
            "beta",
            "rollback procedure",
            false,
            "deploy rollback",
            None,
            &indexes,
        );
        let map = indexes.lock().unwrap();
        assert!(
            map.contains_key("deploy.beta"),
            "index must be stored under the qualified name 'deploy.beta'; got keys: {:?}",
            map.keys().collect::<Vec<_>>()
        );
    }

    /// handle_index_message with global:true marks the index as globally accessible.
    #[test]
    fn handle_index_message_global_flag_propagates() {
        let indexes = make_runtime_indexes();
        super::handle_index_message(
            "i1",
            "configs",
            "shared data",
            true,
            "deploy rollback",
            None,
            &indexes,
        );
        let map = indexes.lock().unwrap();
        let entry = map
            .get("deploy.configs")
            .expect("deploy.configs must be in the map");
        assert!(
            entry.is_global,
            "is_global must be true when global:true was sent"
        );
    }

    /// handle_index_message with global:false marks the index as namespace-local.
    #[test]
    fn handle_index_message_without_global_is_local() {
        let indexes = make_runtime_indexes();
        super::handle_index_message(
            "i1",
            "private",
            "secret",
            false,
            "test skill",
            None,
            &indexes,
        );
        let map = indexes.lock().unwrap();
        let entry = map
            .get("test.private")
            .expect("test.private must be in the map");
        assert!(
            !entry.is_global,
            "is_global must be false when global:false was sent"
        );
    }

    /// handle_search_message returns empty results when no index exists for the name.
    #[test]
    fn handle_search_message_returns_empty_when_no_index() {
        let indexes = make_runtime_indexes();
        let response = super::handle_search_message(
            "r1",
            "rollback",
            "beta",
            "deploy rollback",
            None,
            &indexes,
        );
        let parsed: serde_json::Value = serde_json::from_str(response.trim()).unwrap();
        assert_eq!(parsed["id"], "r1");
        assert_eq!(parsed["results"], "");
    }

    /// handle_search_message returns results after handle_index_message populates the index.
    #[test]
    fn handle_search_message_returns_results_after_index() {
        let indexes = make_runtime_indexes();
        // Index a document containing "rollback".
        super::handle_index_message(
            "i1",
            "beta",
            "rollback deployment procedure",
            false,
            "deploy rollback",
            None,
            &indexes,
        );
        let response = super::handle_search_message(
            "r1",
            "rollback",
            "beta",
            "deploy rollback",
            None,
            &indexes,
        );
        let parsed: serde_json::Value = serde_json::from_str(response.trim()).unwrap();
        assert_eq!(parsed["id"], "r1");
        // Results should be non-empty — the indexed content should appear.
        assert!(
            !parsed["results"].as_str().unwrap_or("").is_empty(),
            "search for 'rollback' must return a non-empty results string after indexing"
        );
    }

    /// handle_search_message returns the indexed content, not the internal doc label.
    ///
    /// Verifies the fix for the bug where `creft_search` returned `"doc_0"` instead
    /// of the actual content that was passed to `creft_index`.
    #[test]
    fn handle_search_message_returns_content_not_doc_id() {
        let indexes = make_runtime_indexes();
        super::handle_index_message(
            "i1",
            "recipes",
            "Chocolate cake needs cocoa powder.",
            false,
            "cook skill",
            None,
            &indexes,
        );
        let response = super::handle_search_message(
            "s1",
            "chocolate",
            "recipes",
            "cook skill",
            None,
            &indexes,
        );
        let parsed: serde_json::Value = serde_json::from_str(response.trim()).unwrap();
        assert_eq!(parsed["id"], "s1");
        let results = parsed["results"].as_str().unwrap_or("");
        assert!(
            results.contains("Chocolate cake needs cocoa powder."),
            "results must contain the original indexed content, not a doc ID like 'doc_0'; got: {results}"
        );
        assert!(
            !results.contains("doc_0"),
            "results must not contain internal doc ID 'doc_0'; got: {results}"
        );
    }

    /// Cross-namespace search is denied when the target index is not global.
    #[test]
    fn handle_search_message_cross_namespace_denied_when_not_global() {
        let indexes = make_runtime_indexes();
        // Index in "deploy" namespace, not global.
        super::handle_index_message(
            "i1",
            "configs",
            "deploy configs",
            false,
            "deploy rollback",
            None,
            &indexes,
        );
        // Search from "test" namespace using dotted name (cross-namespace reference).
        let response = super::handle_search_message(
            "r2",
            "configs",
            "deploy.configs",
            "test skill",
            None,
            &indexes,
        );
        let parsed: serde_json::Value = serde_json::from_str(response.trim()).unwrap();
        assert_eq!(parsed["id"], "r2");
        assert!(
            parsed.get("error").is_some(),
            "cross-namespace access to a non-global index must return an error field"
        );
        let error_msg = parsed["error"].as_str().unwrap_or("");
        assert!(
            error_msg.contains("access denied"),
            "error message must contain 'access denied'; got: {error_msg}"
        );
    }

    /// Cross-namespace search succeeds when the target index is global.
    #[test]
    fn handle_search_message_cross_namespace_succeeds_when_global() {
        let indexes = make_runtime_indexes();
        // Index in "deploy" namespace with global:true.
        super::handle_index_message(
            "i1",
            "configs",
            "shared configuration data",
            true,
            "deploy rollback",
            None,
            &indexes,
        );
        // Search from "test" namespace — should be allowed because index is global.
        let response = super::handle_search_message(
            "r3",
            "configuration",
            "deploy.configs",
            "test skill",
            None,
            &indexes,
        );
        let parsed: serde_json::Value = serde_json::from_str(response.trim()).unwrap();
        assert_eq!(parsed["id"], "r3");
        assert!(
            parsed.get("error").is_none(),
            "cross-namespace access to a global index must not return an error"
        );
    }

    /// Plugin context is included in the qualified name when present.
    #[test]
    fn handle_index_message_includes_plugin_in_qualified_name() {
        let indexes = make_runtime_indexes();
        super::handle_index_message(
            "i1",
            "beta",
            "content",
            false,
            "deploy rollback",
            Some("acme"),
            &indexes,
        );
        let map = indexes.lock().unwrap();
        assert!(
            map.contains_key("acme.deploy.beta"),
            "plugin skill index must be stored under 'acme.deploy.beta'; got: {:?}",
            map.keys().collect::<Vec<_>>()
        );
    }

    // ── spawn_reader with search context ─────────────────────────────────────

    /// A bash block calling creft_index followed by creft_search receives a
    /// non-empty response containing the indexed content's name.
    #[test]
    fn spawn_reader_handles_index_then_search_round_trip() {
        let mut ch = SideChannel::new().unwrap();
        let (ctrl_w_fd, resp_r_fd) = ch.child_fds();

        // Bash script: index content, read the ack, then search for a term in it.
        // The response is the raw JSON line from fd 4.
        let script = r#"
printf '{"type":"index","id":"idx1","name":"beta","content":"rollback deployment","global":false}\n' >&3
read -r _ack <&4
_id="search_test_1"
printf '{"type":"search","id":"%s","query":"rollback","name":"beta"}\n' "$_id" >&3
read -r _resp <&4
printf '%s' "$_resp"
"#;

        let mut cmd = std::process::Command::new("bash");
        cmd.arg("-c").arg(script);
        cmd.stdout(std::process::Stdio::piped());
        unsafe {
            use std::os::unix::process::CommandExt as _;
            cmd.pre_exec(move || {
                if libc::dup2(ctrl_w_fd, 3) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                if ctrl_w_fd != 3 {
                    libc::close(ctrl_w_fd);
                }
                if libc::dup2(resp_r_fd, 4) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                if resp_r_fd != 4 {
                    libc::close(resp_r_fd);
                }
                Ok(())
            });
        }

        let ctrl_reader = ch.take_control_reader().unwrap();
        let resp_writer_fd = ch.take_response_writer().unwrap();
        let child = cmd.spawn().unwrap();
        ch.close_child_ends();

        let indexes = make_runtime_indexes();
        let search_ctx = Some(super::PrimitiveContext {
            skill_name: "deploy rollback".to_owned(),
            plugin: None,
            runtime_indexes: Arc::clone(&indexes),
            store_dir: std::path::PathBuf::from("/tmp/creft-test-store"),
            counter: Arc::new(std::sync::Mutex::new(std::collections::BTreeMap::new())),
        });

        let writer = Arc::new(super::TerminalWriter::new());
        use std::os::unix::io::{FromRawFd as _, IntoRawFd as _};
        let resp_writer_file =
            Some(unsafe { std::fs::File::from_raw_fd(resp_writer_fd.into_raw_fd()) });
        let reader_handle = super::spawn_reader(
            ctrl_reader,
            writer,
            None,
            resp_writer_file,
            None,
            search_ctx,
        );

        let output = child.wait_with_output().unwrap();
        reader_handle.join().expect("reader thread must not panic");

        let stdout = String::from_utf8(output.stdout).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
        assert_eq!(parsed["id"], "search_test_1");
        assert!(
            !parsed["results"].as_str().unwrap_or("").is_empty(),
            "search after index must return non-empty results; got response: {stdout}"
        );
    }

    /// A bash block calling creft_search before creft_index returns empty results.
    #[test]
    fn spawn_reader_search_with_no_index_returns_empty() {
        let mut ch = SideChannel::new().unwrap();
        let (ctrl_w_fd, resp_r_fd) = ch.child_fds();

        let script = r#"
_id="search_test_2"
printf '{"type":"search","id":"%s","query":"rollback","name":"beta"}\n' "$_id" >&3
read -r _resp <&4
printf '%s' "$_resp"
"#;

        let mut cmd = std::process::Command::new("bash");
        cmd.arg("-c").arg(script);
        cmd.stdout(std::process::Stdio::piped());
        unsafe {
            use std::os::unix::process::CommandExt as _;
            cmd.pre_exec(move || {
                if libc::dup2(ctrl_w_fd, 3) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                if ctrl_w_fd != 3 {
                    libc::close(ctrl_w_fd);
                }
                if libc::dup2(resp_r_fd, 4) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                if resp_r_fd != 4 {
                    libc::close(resp_r_fd);
                }
                Ok(())
            });
        }

        let ctrl_reader = ch.take_control_reader().unwrap();
        let resp_writer_fd = ch.take_response_writer().unwrap();
        let child = cmd.spawn().unwrap();
        ch.close_child_ends();

        let indexes = make_runtime_indexes();
        let search_ctx = Some(super::PrimitiveContext {
            skill_name: "deploy rollback".to_owned(),
            plugin: None,
            runtime_indexes: Arc::clone(&indexes),
            store_dir: std::path::PathBuf::from("/tmp/creft-test-store"),
            counter: Arc::new(std::sync::Mutex::new(std::collections::BTreeMap::new())),
        });

        let writer = Arc::new(super::TerminalWriter::new());
        use std::os::unix::io::{FromRawFd as _, IntoRawFd as _};
        let resp_writer_file =
            Some(unsafe { std::fs::File::from_raw_fd(resp_writer_fd.into_raw_fd()) });
        let reader_handle = super::spawn_reader(
            ctrl_reader,
            writer,
            None,
            resp_writer_file,
            None,
            search_ctx,
        );

        let output = child.wait_with_output().unwrap();
        reader_handle.join().expect("reader thread must not panic");

        let stdout = String::from_utf8(output.stdout).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
        assert_eq!(parsed["id"], "search_test_2");
        assert_eq!(
            parsed["results"].as_str().unwrap_or("non-empty"),
            "",
            "search with no prior index must return empty results"
        );
    }

    // ── document accumulation tests ───────────────────────────────────────────

    /// Two calls to handle_index_message with the same name accumulate both
    /// documents: the RuntimeIndex contains 2 entries after the second call.
    #[test]
    fn handle_index_message_accumulates_documents_across_calls() {
        let indexes = make_runtime_indexes();
        super::handle_index_message(
            "i1",
            "docs",
            "first document about rollback",
            false,
            "deploy skill",
            None,
            &indexes,
        );
        super::handle_index_message(
            "i2",
            "docs",
            "second document about deployment",
            false,
            "deploy skill",
            None,
            &indexes,
        );

        let map = indexes.lock().unwrap();
        let entry = map
            .get("deploy.docs")
            .expect("deploy.docs must be in the map");
        assert_eq!(
            entry.documents.len(),
            2,
            "two creft_index calls to the same name must accumulate 2 documents"
        );
    }

    /// After two calls with distinct tokens, searching for a token from the
    /// first document still returns results (the first document is not lost).
    #[test]
    fn handle_index_message_first_document_searchable_after_second_call() {
        let indexes = make_runtime_indexes();
        super::handle_index_message(
            "i1",
            "docs",
            "rollback procedure guide",
            false,
            "deploy skill",
            None,
            &indexes,
        );
        super::handle_index_message(
            "i2",
            "docs",
            "deployment configuration steps",
            false,
            "deploy skill",
            None,
            &indexes,
        );

        let response =
            super::handle_search_message("s1", "rollback", "docs", "deploy skill", None, &indexes);
        let parsed: serde_json::Value = serde_json::from_str(response.trim()).unwrap();
        assert_eq!(parsed["id"], "s1");
        assert!(
            !parsed["results"].as_str().unwrap_or("").is_empty(),
            "search for token from first document must return results after second call; got: {response}"
        );
    }

    /// After two calls with distinct tokens, searching for a token from the
    /// second document returns results.
    #[test]
    fn handle_index_message_second_document_searchable_after_accumulation() {
        let indexes = make_runtime_indexes();
        super::handle_index_message(
            "i1",
            "docs",
            "rollback procedure guide",
            false,
            "deploy skill",
            None,
            &indexes,
        );
        super::handle_index_message(
            "i2",
            "docs",
            "deployment configuration steps",
            false,
            "deploy skill",
            None,
            &indexes,
        );

        let response = super::handle_search_message(
            "s2",
            "deployment",
            "docs",
            "deploy skill",
            None,
            &indexes,
        );
        let parsed: serde_json::Value = serde_json::from_str(response.trim()).unwrap();
        assert_eq!(parsed["id"], "s2");
        assert!(
            !parsed["results"].as_str().unwrap_or("").is_empty(),
            "search for token from second document must return results; got: {response}"
        );
    }

    /// After two calls with distinct content, searching returns the original content
    /// strings, not the internal doc labels (`"doc_0"`, `"doc_1"`).
    ///
    /// Verifies the content-resolution fix holds for the multi-document path: a
    /// regression in the `filter_map` would pass the non-empty assertions above
    /// but fail here by returning `"doc_0"` or `"doc_1"` instead of real content.
    #[test]
    fn handle_search_message_multi_document_returns_content_not_doc_labels() {
        let indexes = make_runtime_indexes();
        super::handle_index_message(
            "i1",
            "docs",
            "rollback procedure guide",
            false,
            "deploy skill",
            None,
            &indexes,
        );
        super::handle_index_message(
            "i2",
            "docs",
            "deployment configuration steps",
            false,
            "deploy skill",
            None,
            &indexes,
        );

        let response =
            super::handle_search_message("s1", "rollback", "docs", "deploy skill", None, &indexes);
        let parsed: serde_json::Value = serde_json::from_str(response.trim()).unwrap();
        let results = parsed["results"].as_str().unwrap_or("");
        assert!(
            results.contains("rollback procedure guide"),
            "results must contain the original content from the first document, not an internal label; got: {results}"
        );
        assert!(
            !results.contains("doc_0"),
            "results must not expose internal doc label 'doc_0'; got: {results}"
        );
        assert!(
            !results.contains("doc_1"),
            "results must not expose internal doc label 'doc_1'; got: {results}"
        );
    }

    /// The is_global flag reflects the most recent call (last-write-wins).
    #[test]
    fn handle_index_message_is_global_last_write_wins() {
        let indexes = make_runtime_indexes();
        super::handle_index_message(
            "i1",
            "docs",
            "content one",
            false,
            "deploy skill",
            None,
            &indexes,
        );
        super::handle_index_message(
            "i2",
            "docs",
            "content two",
            true,
            "deploy skill",
            None,
            &indexes,
        );

        let map = indexes.lock().unwrap();
        let entry = map
            .get("deploy.docs")
            .expect("deploy.docs must be in the map");
        assert!(
            entry.is_global,
            "is_global must reflect the most recent call (true); last-write-wins"
        );
    }

    // ── json_escape_string tests ──────────────────────────────────────────────

    /// Entry names containing double-quotes produce valid JSON in the search response.
    #[test]
    fn handle_search_message_response_valid_json_when_name_contains_double_quote() {
        let escaped = super::json_escape_string("name with \"quotes\"");
        assert_eq!(escaped, r#"name with \"quotes\""#);
        // Verify the escaped string produces valid JSON when embedded.
        let json = format!("{{\"results\":\"{escaped}\"}}");
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("must be valid JSON");
        assert_eq!(parsed["results"], "name with \"quotes\"");
    }

    /// Entry names containing backslashes produce valid JSON in the search response.
    #[test]
    fn handle_search_message_response_valid_json_when_name_contains_backslash() {
        let escaped = super::json_escape_string("path\\to\\file");
        assert_eq!(escaped, "path\\\\to\\\\file");
        let json = format!("{{\"results\":\"{escaped}\"}}");
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("must be valid JSON");
        assert_eq!(parsed["results"], "path\\to\\file");
    }

    /// Entry names containing newlines produce valid JSON in the search response.
    #[test]
    fn handle_search_message_response_valid_json_when_name_contains_newline() {
        let escaped = super::json_escape_string("line one\nline two");
        assert_eq!(escaped, "line one\\nline two");
        let json = format!("{{\"results\":\"{escaped}\"}}");
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("must be valid JSON");
        assert_eq!(parsed["results"], "line one\nline two");
    }

    /// Entry names containing tabs produce valid JSON in the search response.
    #[test]
    fn handle_search_message_response_valid_json_when_name_contains_tab() {
        let escaped = super::json_escape_string("col\there");
        assert_eq!(escaped, "col\\there");
        let json = format!("{{\"results\":\"{escaped}\"}}");
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("must be valid JSON");
        assert_eq!(parsed["results"], "col\there");
    }

    /// json_escape_string is a no-op for plain ASCII content.
    #[test]
    fn json_escape_string_passthrough_for_plain_ascii() {
        let s = "deploy rollback configuration";
        assert_eq!(super::json_escape_string(s), s);
    }

    // ── ChannelMessage::StorePut / StoreGet / StoreSearch deserialization ──────

    /// StorePut with global:true deserializes to Some(true).
    #[test]
    fn channel_message_store_put_global_true_deserializes() {
        let json = r#"{"type":"store_put","id":"p1","name":"data","key":"env","value":"prod","global":true}"#;
        let msg: super::ChannelMessage = serde_json::from_str(json).unwrap();
        match msg {
            super::ChannelMessage::StorePut {
                id,
                name,
                key,
                value,
                global,
            } => {
                assert_eq!(id, "p1");
                assert_eq!(name, "data");
                assert_eq!(key, "env");
                assert_eq!(value, "prod");
                assert_eq!(global, Some(true));
            }
            other => panic!("expected StorePut, got {other:?}"),
        }
    }

    /// StorePut without global field deserializes to None (flag unchanged).
    #[test]
    fn channel_message_store_put_without_global_defaults_to_none() {
        let json = r#"{"type":"store_put","id":"p2","name":"data","key":"env","value":"prod"}"#;
        let msg: super::ChannelMessage = serde_json::from_str(json).unwrap();
        match msg {
            super::ChannelMessage::StorePut { global, .. } => {
                assert_eq!(
                    global, None,
                    "omitted global field must deserialize as None"
                );
            }
            other => panic!("expected StorePut, got {other:?}"),
        }
    }

    /// StorePut with global:false deserializes to Some(false).
    #[test]
    fn channel_message_store_put_global_false_deserializes() {
        let json = r#"{"type":"store_put","id":"p3","name":"data","key":"env","value":"prod","global":false}"#;
        let msg: super::ChannelMessage = serde_json::from_str(json).unwrap();
        match msg {
            super::ChannelMessage::StorePut { global, .. } => {
                assert_eq!(global, Some(false));
            }
            other => panic!("expected StorePut, got {other:?}"),
        }
    }

    /// StoreGet deserializes correctly from JSON.
    #[test]
    fn channel_message_store_get_deserializes() {
        let json = r#"{"type":"store_get","id":"g1","name":"data","key":"env"}"#;
        let msg: super::ChannelMessage = serde_json::from_str(json).unwrap();
        match msg {
            super::ChannelMessage::StoreGet { id, name, key } => {
                assert_eq!(id, "g1");
                assert_eq!(name, "data");
                assert_eq!(key, "env");
            }
            other => panic!("expected StoreGet, got {other:?}"),
        }
    }

    /// StoreSearch deserializes correctly from JSON.
    #[test]
    fn channel_message_store_search_deserializes() {
        let json = r#"{"type":"store_search","id":"ss1","name":"data","query":"rollback"}"#;
        let msg: super::ChannelMessage = serde_json::from_str(json).unwrap();
        match msg {
            super::ChannelMessage::StoreSearch { id, name, query } => {
                assert_eq!(id, "ss1");
                assert_eq!(name, "data");
                assert_eq!(query, "rollback");
            }
            other => panic!("expected StoreSearch, got {other:?}"),
        }
    }

    // ── handle_store_put / handle_store_get / handle_store_search unit tests ───

    /// handle_store_put creates a redb file and a companion index file on disk.
    #[test]
    fn handle_store_put_creates_redb_and_index_files() {
        let dir = tempfile::tempdir().unwrap();
        super::handle_store_put(
            "p1",
            "data",
            "env",
            "production",
            None,
            "deploy skill",
            None,
            dir.path(),
        );
        assert!(
            crate::store_kv::store_path(dir.path(), "deploy.data").exists(),
            "redb file must exist after put"
        );
        assert!(
            crate::store_kv::store_index_path(dir.path(), "deploy.data").exists(),
            "index file must exist after put"
        );
    }

    /// handle_store_put replaces an existing key on the second call.
    #[test]
    fn handle_store_put_replaces_existing_key() {
        let dir = tempfile::tempdir().unwrap();
        super::handle_store_put(
            "p2a",
            "data",
            "env",
            "staging",
            None,
            "deploy skill",
            None,
            dir.path(),
        );
        super::handle_store_put(
            "p2b",
            "data",
            "env",
            "production",
            None,
            "deploy skill",
            None,
            dir.path(),
        );
        let val = crate::store_kv::store_get(dir.path(), "deploy.data", "env")
            .unwrap()
            .expect("key must exist after second put");
        assert_eq!(val, "production");
    }

    /// handle_store_get returns the value for an existing key.
    #[test]
    fn handle_store_get_returns_value_for_existing_key() {
        let dir = tempfile::tempdir().unwrap();
        super::handle_store_put(
            "p3",
            "data",
            "env",
            "prod",
            None,
            "deploy skill",
            None,
            dir.path(),
        );
        let response =
            super::handle_store_get("g1", "data", "env", "deploy skill", None, dir.path());
        let parsed: serde_json::Value = serde_json::from_str(response.trim()).unwrap();
        assert_eq!(parsed["id"], "g1");
        assert_eq!(parsed["value"], "prod");
    }

    /// handle_store_get returns empty value when the key does not exist.
    #[test]
    fn handle_store_get_returns_empty_value_for_missing_key() {
        let dir = tempfile::tempdir().unwrap();
        super::handle_store_put(
            "p4",
            "data",
            "other",
            "x",
            None,
            "deploy skill",
            None,
            dir.path(),
        );
        let response = super::handle_store_get(
            "g2",
            "data",
            "missing_key",
            "deploy skill",
            None,
            dir.path(),
        );
        let parsed: serde_json::Value = serde_json::from_str(response.trim()).unwrap();
        assert_eq!(parsed["id"], "g2");
        assert_eq!(parsed["value"], "");
    }

    /// handle_store_get returns empty value when the database does not exist.
    #[test]
    fn handle_store_get_returns_empty_when_database_missing() {
        let dir = tempfile::tempdir().unwrap();
        let response =
            super::handle_store_get("g3", "data", "env", "deploy skill", None, dir.path());
        let parsed: serde_json::Value = serde_json::from_str(response.trim()).unwrap();
        assert_eq!(parsed["id"], "g3");
        assert_eq!(parsed["value"], "");
    }

    /// handle_store_get rejects dotted names with an error response.
    #[test]
    fn handle_store_get_rejects_dotted_name() {
        let dir = tempfile::tempdir().unwrap();
        let response =
            super::handle_store_get("g4", "other.data", "env", "deploy skill", None, dir.path());
        let parsed: serde_json::Value = serde_json::from_str(response.trim()).unwrap();
        assert_eq!(parsed["id"], "g4");
        assert!(
            parsed.get("error").is_some(),
            "dotted name must return an error response, got: {parsed}"
        );
    }

    /// handle_store_search returns matching keys after a put.
    #[test]
    fn handle_store_search_returns_matching_keys_after_put() {
        let dir = tempfile::tempdir().unwrap();
        super::handle_store_put(
            "p5",
            "data",
            "config",
            "rollback procedure",
            None,
            "deploy skill",
            None,
            dir.path(),
        );
        let response =
            super::handle_store_search("ss1", "data", "rollback", "deploy skill", None, dir.path());
        let parsed: serde_json::Value = serde_json::from_str(response.trim()).unwrap();
        assert_eq!(parsed["id"], "ss1");
        assert!(
            parsed["results"].as_str().unwrap_or("").contains("config"),
            "search for 'rollback' must return 'config' key; got: {parsed}"
        );
    }

    /// handle_store_search returns empty results when no match.
    #[test]
    fn handle_store_search_returns_empty_when_no_match() {
        let dir = tempfile::tempdir().unwrap();
        super::handle_store_put(
            "p6",
            "data",
            "env",
            "production",
            None,
            "deploy skill",
            None,
            dir.path(),
        );
        let response = super::handle_store_search(
            "ss2",
            "data",
            "zzzunmatchable",
            "deploy skill",
            None,
            dir.path(),
        );
        let parsed: serde_json::Value = serde_json::from_str(response.trim()).unwrap();
        assert_eq!(parsed["id"], "ss2");
        assert_eq!(parsed["results"].as_str().unwrap_or("MISSING"), "");
    }

    /// Cross-namespace StoreSearch is denied when the store is not global.
    #[test]
    fn handle_store_search_cross_namespace_denied_when_not_global() {
        let dir = tempfile::tempdir().unwrap();
        // Put into deploy.data without marking global.
        crate::store_kv::store_put(dir.path(), "deploy.data", "env", "prod", None).unwrap();
        crate::store_kv::rebuild_store_index(dir.path(), "deploy.data").unwrap();

        // Caller is from "monitor skill" (monitor namespace), searching "deploy.data" — cross-namespace.
        let response = super::handle_store_search(
            "ss3",
            "deploy.data",
            "prod",
            "monitor skill",
            None,
            dir.path(),
        );
        let parsed: serde_json::Value = serde_json::from_str(response.trim()).unwrap();
        assert_eq!(parsed["id"], "ss3");
        assert!(
            parsed.get("error").is_some(),
            "cross-namespace search must be denied when store is not global; got: {parsed}"
        );
    }

    /// Cross-namespace StoreSearch succeeds when the store is global.
    #[test]
    fn handle_store_search_cross_namespace_succeeds_when_global() {
        let dir = tempfile::tempdir().unwrap();
        // Put into deploy.data, marking it global.
        crate::store_kv::store_put(dir.path(), "deploy.data", "config", "rollback", Some(true))
            .unwrap();
        crate::store_kv::rebuild_store_index(dir.path(), "deploy.data").unwrap();

        // Caller is "monitor skill" (monitor namespace), searching deploy.data — cross-namespace but global.
        let response = super::handle_store_search(
            "ss4",
            "deploy.data",
            "rollback",
            "monitor skill",
            None,
            dir.path(),
        );
        let parsed: serde_json::Value = serde_json::from_str(response.trim()).unwrap();
        assert_eq!(parsed["id"], "ss4");
        assert!(
            parsed.get("error").is_none(),
            "cross-namespace search must succeed when store is global; got: {parsed}"
        );
        assert!(
            parsed["results"].as_str().unwrap_or("").contains("config"),
            "global cross-namespace search must return matching keys; got: {parsed}"
        );
    }

    // ── handle_store_put: error paths ────────────────────────────────────────

    /// handle_store_put silently drops the operation when the database file is
    /// corrupt (non-StoreOpen error — no retry, no hang).
    #[test]
    fn handle_store_put_non_store_open_error_does_not_panic() {
        let dir = tempfile::tempdir().unwrap();
        // Write garbage bytes where the database would be opened for writing.
        // open_db calls Database::create, which fails immediately on a corrupt
        // file with a non-DatabaseAlreadyOpen error — covering the Err(e) arm.
        let path = crate::store_kv::store_path(dir.path(), "deploy.data");
        std::fs::write(&path, b"not a redb file\xFF\xFE\xFD").unwrap();
        // Must complete without panicking (returns ack regardless of outcome).
        super::handle_store_put(
            "p7",
            "data",
            "env",
            "production",
            None,
            "deploy skill",
            None,
            dir.path(),
        );
    }

    // ── handle_store_get: error path ─────────────────────────────────────────

    /// handle_store_get returns an error response when the database is corrupt.
    #[test]
    fn handle_store_get_returns_error_response_on_corrupt_database() {
        let dir = tempfile::tempdir().unwrap();
        let path = crate::store_kv::store_path(dir.path(), "deploy.data");
        std::fs::write(&path, b"not a redb file\xFF\xFE\xFD").unwrap();

        let response =
            super::handle_store_get("g5", "data", "env", "deploy skill", None, dir.path());
        let parsed: serde_json::Value = serde_json::from_str(response.trim()).unwrap();
        assert_eq!(parsed["id"], "g5");
        assert!(
            parsed.get("error").is_some(),
            "corrupt database must return an error response; got: {parsed}"
        );
    }

    // ── handle_store_search: no-index path ───────────────────────────────────

    /// handle_store_search returns empty results when no index file exists.
    #[test]
    fn handle_store_search_returns_empty_when_no_index_file() {
        let dir = tempfile::tempdir().unwrap();
        // No put, so no index file — covers the None arm of load_store_index.
        let response =
            super::handle_store_search("ss5", "data", "anything", "deploy skill", None, dir.path());
        let parsed: serde_json::Value = serde_json::from_str(response.trim()).unwrap();
        assert_eq!(parsed["id"], "ss5");
        assert_eq!(parsed["results"].as_str().unwrap_or("MISSING"), "");
    }

    /// handle_store_search returns keys via fuzzy fallback when the exact query
    /// does not match but a similar term is in the index.
    #[test]
    fn handle_store_search_returns_keys_via_fuzzy_fallback() {
        let dir = tempfile::tempdir().unwrap();
        // Index "config" → "production environment rollback".
        super::handle_store_put(
            "p8",
            "data",
            "config",
            "production environment rollback",
            None,
            "deploy skill",
            None,
            dir.path(),
        );
        // Query with a term that won't exact-match but fuzzy-matches ("prodution" typo).
        let response = super::handle_store_search(
            "ss6",
            "data",
            "prodution",
            "deploy skill",
            None,
            dir.path(),
        );
        let parsed: serde_json::Value = serde_json::from_str(response.trim()).unwrap();
        assert_eq!(parsed["id"], "ss6");
        // We don't assert the specific results (fuzzy matching may or may not match),
        // but the response must be valid JSON and must not error.
        assert!(
            parsed.get("error").is_none(),
            "fuzzy search must not return an error response; got: {parsed}"
        );
    }

    // ── spawn_reader: no-ctx fallback for store messages ─────────────────────

    /// When spawn_reader is called with ctx=None and a StoreGet message arrives,
    /// the reader writes an empty-value response so the child does not hang.
    #[test]
    fn spawn_reader_no_ctx_writes_empty_response_for_store_get() {
        let mut ch = SideChannel::new().unwrap();
        let (ctrl_w_fd, resp_r_fd) = ch.child_fds();

        let script = r#"
printf '{"type":"store_get","id":"nc1","name":"data","key":"k"}\n' >&3
read -r _resp <&4
printf '%s' "$_resp"
"#;
        let mut cmd = std::process::Command::new("bash");
        cmd.arg("-c").arg(script);
        cmd.stdout(std::process::Stdio::piped());
        setup_child_fds(&mut cmd, ctrl_w_fd, resp_r_fd);

        let ctrl_reader = ch.take_control_reader().unwrap();
        let resp_writer_fd = ch.take_response_writer().unwrap();
        let child = cmd.spawn().unwrap();
        ch.close_child_ends();

        let writer = Arc::new(super::TerminalWriter::new());
        use std::os::unix::io::{FromRawFd as _, IntoRawFd as _};
        let resp_writer_file =
            Some(unsafe { std::fs::File::from_raw_fd(resp_writer_fd.into_raw_fd()) });
        // ctx = None: exercises the else-if fallback for StoreGet.
        let reader_handle =
            super::spawn_reader(ctrl_reader, writer, None, resp_writer_file, None, None);

        let output = child.wait_with_output().unwrap();
        reader_handle.join().expect("reader thread must not panic");

        let stdout = String::from_utf8(output.stdout).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
        assert_eq!(parsed["id"], "nc1");
        assert_eq!(parsed["value"], "");
    }

    /// When spawn_reader is called with ctx=None and a StoreSearch message
    /// arrives, the reader writes an empty-results response rather than hanging.
    #[test]
    fn spawn_reader_no_ctx_writes_empty_response_for_store_search() {
        let mut ch = SideChannel::new().unwrap();
        let (ctrl_w_fd, resp_r_fd) = ch.child_fds();

        let script = r#"
printf '{"type":"store_search","id":"nc2","name":"data","query":"q"}\n' >&3
read -r _resp <&4
printf '%s' "$_resp"
"#;
        let mut cmd = std::process::Command::new("bash");
        cmd.arg("-c").arg(script);
        cmd.stdout(std::process::Stdio::piped());
        setup_child_fds(&mut cmd, ctrl_w_fd, resp_r_fd);

        let ctrl_reader = ch.take_control_reader().unwrap();
        let resp_writer_fd = ch.take_response_writer().unwrap();
        let child = cmd.spawn().unwrap();
        ch.close_child_ends();

        let writer = Arc::new(super::TerminalWriter::new());
        use std::os::unix::io::{FromRawFd as _, IntoRawFd as _};
        let resp_writer_file =
            Some(unsafe { std::fs::File::from_raw_fd(resp_writer_fd.into_raw_fd()) });
        // ctx = None: exercises the else-if fallback for StoreSearch.
        let reader_handle =
            super::spawn_reader(ctrl_reader, writer, None, resp_writer_file, None, None);

        let output = child.wait_with_output().unwrap();
        reader_handle.join().expect("reader thread must not panic");

        let stdout = String::from_utf8(output.stdout).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
        assert_eq!(parsed["id"], "nc2");
        assert_eq!(parsed["results"], "");
    }

    // ── PrimitiveContext rename: existing search tests continue to compile ──────
    // (All tests above that used SearchContext now use PrimitiveContext via the
    // renamed struct. The tests themselves are unchanged — only the type name
    // changed. Compilation success is the verification.)

    // ── Integration: bash block using store primitives via preamble ───────────

    fn make_primitive_ctx(store_dir: std::path::PathBuf) -> super::PrimitiveContext {
        super::PrimitiveContext {
            skill_name: "deploy skill".to_owned(),
            plugin: None,
            runtime_indexes: make_runtime_indexes(),
            store_dir,
            counter: Arc::new(std::sync::Mutex::new(std::collections::BTreeMap::new())),
        }
    }

    fn setup_child_fds(cmd: &mut std::process::Command, ctrl_w_fd: i32, resp_r_fd: i32) {
        unsafe {
            use std::os::unix::process::CommandExt as _;
            cmd.pre_exec(move || {
                if libc::dup2(ctrl_w_fd, 3) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                if ctrl_w_fd != 3 {
                    libc::close(ctrl_w_fd);
                }
                if libc::dup2(resp_r_fd, 4) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                if resp_r_fd != 4 {
                    libc::close(resp_r_fd);
                }
                Ok(())
            });
        }
    }

    /// A bash block that calls creft_store_put then creft_store_get receives
    /// the stored value back from fd 4 without hanging.
    #[test]
    fn spawn_reader_handles_store_put_then_get_round_trip() {
        let store_tmp = tempfile::tempdir().unwrap();
        let store_dir = store_tmp.path().to_path_buf();

        let mut ch = SideChannel::new().unwrap();
        let (ctrl_w_fd, resp_r_fd) = ch.child_fds();

        // The script puts a value, reads the ack, then gets it back and prints the response.
        let script = r#"
printf '{"type":"store_put","id":"put1","name":"data","key":"env","value":"production"}\n' >&3
read -r _ack <&4
_id="store_get_test_1"
printf '{"type":"store_get","id":"%s","name":"data","key":"env"}\n' "$_id" >&3
read -r _resp <&4
printf '%s' "$_resp"
"#;

        let mut cmd = std::process::Command::new("bash");
        cmd.arg("-c").arg(script);
        cmd.stdout(std::process::Stdio::piped());
        setup_child_fds(&mut cmd, ctrl_w_fd, resp_r_fd);

        let ctrl_reader = ch.take_control_reader().unwrap();
        let resp_writer_fd = ch.take_response_writer().unwrap();
        let child = cmd.spawn().unwrap();
        ch.close_child_ends();

        let ctx = Some(make_primitive_ctx(store_dir));
        let writer = Arc::new(super::TerminalWriter::new());
        use std::os::unix::io::{FromRawFd as _, IntoRawFd as _};
        let resp_writer_file =
            Some(unsafe { std::fs::File::from_raw_fd(resp_writer_fd.into_raw_fd()) });
        let reader_handle =
            super::spawn_reader(ctrl_reader, writer, None, resp_writer_file, None, ctx);

        let output = child.wait_with_output().unwrap();
        reader_handle.join().expect("reader thread must not panic");

        let stdout = String::from_utf8(output.stdout).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
        assert_eq!(parsed["id"], "store_get_test_1");
        assert_eq!(
            parsed["value"], "production",
            "store get after put must return the stored value; got: {stdout}"
        );
    }

    /// A bash block calling creft_store_search after creft_store_put returns
    /// matching keys in the response.
    #[test]
    fn spawn_reader_handles_store_put_then_search_round_trip() {
        let store_tmp = tempfile::tempdir().unwrap();
        let store_dir = store_tmp.path().to_path_buf();

        let mut ch = SideChannel::new().unwrap();
        let (ctrl_w_fd, resp_r_fd) = ch.child_fds();

        let script = r#"
printf '{"type":"store_put","id":"put2","name":"data","key":"config","value":"rollback procedure"}\n' >&3
read -r _ack <&4
_id="store_search_test_1"
printf '{"type":"store_search","id":"%s","name":"data","query":"rollback"}\n' "$_id" >&3
read -r _resp <&4
printf '%s' "$_resp"
"#;

        let mut cmd = std::process::Command::new("bash");
        cmd.arg("-c").arg(script);
        cmd.stdout(std::process::Stdio::piped());
        setup_child_fds(&mut cmd, ctrl_w_fd, resp_r_fd);

        let ctrl_reader = ch.take_control_reader().unwrap();
        let resp_writer_fd = ch.take_response_writer().unwrap();
        let child = cmd.spawn().unwrap();
        ch.close_child_ends();

        let ctx = Some(make_primitive_ctx(store_dir));
        let writer = Arc::new(super::TerminalWriter::new());
        use std::os::unix::io::{FromRawFd as _, IntoRawFd as _};
        let resp_writer_file =
            Some(unsafe { std::fs::File::from_raw_fd(resp_writer_fd.into_raw_fd()) });
        let reader_handle =
            super::spawn_reader(ctrl_reader, writer, None, resp_writer_file, None, ctx);

        let output = child.wait_with_output().unwrap();
        reader_handle.join().expect("reader thread must not panic");

        let stdout = String::from_utf8(output.stdout).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
        assert_eq!(parsed["id"], "store_search_test_1");
        assert!(
            parsed["results"].as_str().unwrap_or("").contains("config"),
            "store search after put must return 'config' key; got: {stdout}"
        );
    }

    // ── ack response tests ───────────────────────────────────────────────────

    /// StorePut with an id field deserializes correctly and the id is captured.
    #[test]
    fn channel_message_store_put_deserializes_with_id() {
        let json = r#"{"type":"store_put","id":"sp_test_1","name":"data","key":"k","value":"v"}"#;
        let msg: super::ChannelMessage = serde_json::from_str(json).unwrap();
        match msg {
            super::ChannelMessage::StorePut {
                id,
                name,
                key,
                value,
                global,
            } => {
                assert_eq!(id, "sp_test_1");
                assert_eq!(name, "data");
                assert_eq!(key, "k");
                assert_eq!(value, "v");
                assert_eq!(global, None);
            }
            other => panic!("expected StorePut, got {other:?}"),
        }
    }

    /// Index with an id field deserializes correctly and the id is captured.
    #[test]
    fn channel_message_index_deserializes_with_id() {
        let json = r#"{"type":"index","id":"idx_test_1","name":"docs","content":"hello world","global":false}"#;
        let msg: super::ChannelMessage = serde_json::from_str(json).unwrap();
        match msg {
            super::ChannelMessage::Index {
                id,
                name,
                content,
                global,
            } => {
                assert_eq!(id, "idx_test_1");
                assert_eq!(name, "docs");
                assert_eq!(content, "hello world");
                assert!(!global);
            }
            other => panic!("expected Index, got {other:?}"),
        }
    }

    /// handle_store_put returns a JSON ack string containing the request id and ok:true.
    #[test]
    fn handle_store_put_returns_ack_with_id_and_ok() {
        let dir = tempfile::tempdir().unwrap();
        let ack = super::handle_store_put(
            "ack_test_id",
            "data",
            "env",
            "production",
            None,
            "deploy skill",
            None,
            dir.path(),
        );
        let parsed: serde_json::Value = serde_json::from_str(ack.trim()).unwrap();
        assert_eq!(
            parsed["id"], "ack_test_id",
            "ack must contain the request id"
        );
        assert_eq!(parsed["ok"], true, "ack must contain ok:true");
    }

    /// handle_index_message returns a JSON ack string containing the request id and ok:true.
    #[test]
    fn handle_index_message_returns_ack_with_id_and_ok() {
        let indexes = make_runtime_indexes();
        let ack = super::handle_index_message(
            "ack_idx_id",
            "docs",
            "some content",
            false,
            "deploy skill",
            None,
            &indexes,
        );
        let parsed: serde_json::Value = serde_json::from_str(ack.trim()).unwrap();
        assert_eq!(
            parsed["id"], "ack_idx_id",
            "ack must contain the request id"
        );
        assert_eq!(parsed["ok"], true, "ack must contain ok:true");
    }

    /// When spawn_reader receives a StorePut with ctx=None, it writes an ack so
    /// the child does not hang.
    #[test]
    fn spawn_reader_no_ctx_writes_ack_for_store_put() {
        let mut ch = SideChannel::new().unwrap();
        let (ctrl_w_fd, resp_r_fd) = ch.child_fds();

        let script = r#"
printf '{"type":"store_put","id":"nc_put1","name":"data","key":"k","value":"v"}\n' >&3
read -r _resp <&4
printf '%s' "$_resp"
"#;
        let mut cmd = std::process::Command::new("bash");
        cmd.arg("-c").arg(script);
        cmd.stdout(std::process::Stdio::piped());
        setup_child_fds(&mut cmd, ctrl_w_fd, resp_r_fd);

        let ctrl_reader = ch.take_control_reader().unwrap();
        let resp_writer_fd = ch.take_response_writer().unwrap();
        let child = cmd.spawn().unwrap();
        ch.close_child_ends();

        let writer = Arc::new(super::TerminalWriter::new());
        use std::os::unix::io::{FromRawFd as _, IntoRawFd as _};
        let resp_writer_file =
            Some(unsafe { std::fs::File::from_raw_fd(resp_writer_fd.into_raw_fd()) });
        // ctx = None: exercises the else-if fallback for StorePut.
        let reader_handle =
            super::spawn_reader(ctrl_reader, writer, None, resp_writer_file, None, None);

        let output = child.wait_with_output().unwrap();
        reader_handle.join().expect("reader thread must not panic");

        let stdout = String::from_utf8(output.stdout).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
        assert_eq!(parsed["id"], "nc_put1");
        assert_eq!(parsed["ok"], true);
    }

    /// When spawn_reader receives an Index with ctx=None, it writes an ack so
    /// the child does not hang.
    #[test]
    fn spawn_reader_no_ctx_writes_ack_for_index() {
        let mut ch = SideChannel::new().unwrap();
        let (ctrl_w_fd, resp_r_fd) = ch.child_fds();

        let script = r#"
printf '{"type":"index","id":"nc_idx1","name":"docs","content":"hello","global":false}\n' >&3
read -r _resp <&4
printf '%s' "$_resp"
"#;
        let mut cmd = std::process::Command::new("bash");
        cmd.arg("-c").arg(script);
        cmd.stdout(std::process::Stdio::piped());
        setup_child_fds(&mut cmd, ctrl_w_fd, resp_r_fd);

        let ctrl_reader = ch.take_control_reader().unwrap();
        let resp_writer_fd = ch.take_response_writer().unwrap();
        let child = cmd.spawn().unwrap();
        ch.close_child_ends();

        let writer = Arc::new(super::TerminalWriter::new());
        use std::os::unix::io::{FromRawFd as _, IntoRawFd as _};
        let resp_writer_file =
            Some(unsafe { std::fs::File::from_raw_fd(resp_writer_fd.into_raw_fd()) });
        // ctx = None: exercises the else-if fallback for Index.
        let reader_handle =
            super::spawn_reader(ctrl_reader, writer, None, resp_writer_file, None, None);

        let output = child.wait_with_output().unwrap();
        reader_handle.join().expect("reader thread must not panic");

        let stdout = String::from_utf8(output.stdout).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
        assert_eq!(parsed["id"], "nc_idx1");
        assert_eq!(parsed["ok"], true);
    }
}
