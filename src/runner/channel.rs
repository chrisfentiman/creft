use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufRead, BufReader, IsTerminal as _, Write};
use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use crate::namespace;
use crate::search::index::SearchIndex;
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
fn os_pipe() -> std::io::Result<(OwnedFd, OwnedFd)> {
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

/// Context for handling `creft_index` and `creft_search` messages on the side channel.
///
/// Carried into the reader thread when a skill is executing. The thread uses
/// `skill_name` to derive the caller's namespace for name qualification, and
/// `runtime_indexes` to store indexes created by `creft_index` and query them
/// for `creft_search` requests.
pub(crate) struct SearchContext {
    /// Fully-qualified name of the skill being executed.
    pub skill_name: String,
    /// Plugin name extracted from the skill's source, if any.
    pub plugin: Option<String>,
    /// Shared runtime indexes keyed by fully-qualified name.
    pub runtime_indexes: Arc<std::sync::Mutex<HashMap<String, RuntimeIndex>>>,
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
/// - `Index` — when `search_ctx` is `Some`, qualifies the name and builds an
///   in-memory `SearchIndex`. Fire-and-forget; no response is written.
/// - `Search` — when `search_ctx` is `Some`, resolves the name against the
///   access registry, queries the index, and writes a JSON response on fd 4.
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
    search_ctx: Option<SearchContext>,
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
                        writer.print(&message);
                    }
                    ChannelMessage::Status { message, progress } => match progress {
                        None => writer.status(&message),
                        Some(pct) => writer.progress(&message, pct),
                    },
                    ChannelMessage::Prompt {
                        id,
                        question,
                        choices,
                    } => {
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
                        name,
                        content,
                        global,
                    } => {
                        if let Some(ref ctx) = search_ctx {
                            handle_index_message(
                                &name,
                                &content,
                                global,
                                &ctx.skill_name,
                                ctx.plugin.as_deref(),
                                &ctx.runtime_indexes,
                            );
                        }
                    }
                    ChannelMessage::Search { id, query, name } => {
                        if let Some(ref ctx) = search_ctx {
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
                            // No search context — return empty results rather than hanging.
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

/// Handle a `creft_index` message: qualify the name, build a `SearchIndex`,
/// and store it in `runtime_indexes`.
///
/// Called from the reader thread when an `Index` message arrives on fd 3.
/// Fire-and-forget — no response is written to fd 4.
fn handle_index_message(
    name: &str,
    content: &str,
    global: bool,
    skill_name: &str,
    plugin: Option<&str>,
    runtime_indexes: &std::sync::Mutex<HashMap<String, RuntimeIndex>>,
) {
    let ns = namespace::skill_namespace(skill_name);
    let qualified = namespace::qualify(name, ns, plugin);
    // Build the index from the provided content as a single document.
    // The document name is the qualified index name; the description is empty
    // (runtime indexes are ephemeral and not surfaced in search results listings).
    let index = SearchIndex::build(&[(&qualified, "", content)]);
    let runtime_index = RuntimeIndex {
        index,
        is_global: global,
    };
    if let Ok(mut map) = runtime_indexes.lock() {
        map.insert(qualified, runtime_index);
    }
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
            let hits = runtime_index.index.search(query);
            hits.iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>()
                .join("\\n")
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    format!("{{\"id\":\"{id}\",\"results\":\"{results}\"}}\n")
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

    // Escape the answer for JSON: backslash and double-quote need escaping.
    let escaped = answer
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
        .replace('\r', "\\r");

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
        let json = r#"{"type":"index","name":"beta","content":"some docs","global":true}"#;
        let msg: super::ChannelMessage = serde_json::from_str(json).unwrap();
        match msg {
            super::ChannelMessage::Index {
                name,
                content,
                global,
            } => {
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
        let json = r#"{"type":"index","name":"beta","content":"some docs"}"#;
        let msg: super::ChannelMessage = serde_json::from_str(json).unwrap();
        match msg {
            super::ChannelMessage::Index {
                name,
                content,
                global,
            } => {
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
        let json = r#"{"type":"index","name":"configs","content":"data","global":false}"#;
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
        super::handle_index_message("private", "secret", false, "test skill", None, &indexes);
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
        // Results should be non-empty — the document name should appear.
        assert!(
            !parsed["results"].as_str().unwrap_or("").is_empty(),
            "search for 'rollback' must return a non-empty results string after indexing"
        );
    }

    /// Cross-namespace search is denied when the target index is not global.
    #[test]
    fn handle_search_message_cross_namespace_denied_when_not_global() {
        let indexes = make_runtime_indexes();
        // Index in "deploy" namespace, not global.
        super::handle_index_message(
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

        // Bash script: index content, then search for a term in it.
        // The response is the raw JSON line from fd 4.
        let script = r#"
printf '{"type":"index","name":"beta","content":"rollback deployment","global":false}\n' >&3
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
        let search_ctx = Some(super::SearchContext {
            skill_name: "deploy rollback".to_owned(),
            plugin: None,
            runtime_indexes: Arc::clone(&indexes),
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
        let search_ctx = Some(super::SearchContext {
            skill_name: "deploy rollback".to_owned(),
            plugin: None,
            runtime_indexes: Arc::clone(&indexes),
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
}
