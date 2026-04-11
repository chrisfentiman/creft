use std::io::Read as _;
use std::io::Write as _;

use crate::error::CreftError;
use crate::model::{CodeBlock, ParsedCommand};

use super::blocks::spawn_block;
use super::substitute::substitute;
use super::{EARLY_EXIT, RunContext, exit_code_of, make_execution_error, prepare_block_script};

#[cfg(unix)]
use super::signal::PipeSignalGuard;

/// A duplicated pipe file descriptor that implements `Read`.
///
/// Created by `dup_pipe_stdout` to hold a second handle to an inter-block pipe
/// buffer. Kept alive so the kernel pipe buffer survives after the downstream
/// block is killed. On exit 99, drain this reader to recover the exit-99
/// block's unread output.
#[cfg(unix)]
struct DupedPipeReader {
    fd: std::os::unix::io::OwnedFd,
}

#[cfg(unix)]
impl std::io::Read for DupedPipeReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        use std::os::unix::io::AsRawFd as _;
        // SAFETY: fd is valid and owned by this struct. read(2) is a standard
        // POSIX call that reads up to buf.len() bytes from the fd.
        let ret = unsafe { libc::read(self.fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
        if ret < 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(ret as usize)
        }
    }
}

#[cfg(unix)]
impl DupedPipeReader {
    /// Drain available bytes from the pipe using poll() with a per-iteration
    /// timeout, collecting into `out`. Returns when the pipe is empty or all
    /// write ends are closed (EOF). Never blocks indefinitely: if no data
    /// arrives within `timeout_ms` milliseconds per poll call, drain stops.
    ///
    /// This prevents the drain from hanging when an orphaned grandchild of
    /// the exit-99 block still holds the pipe's write end open after the
    /// process group was killed.
    fn drain_with_timeout(&mut self, out: &mut Vec<u8>, timeout_ms: i32) {
        use std::os::unix::io::AsRawFd as _;
        let raw_fd = self.fd.as_raw_fd();
        let mut buf = [0u8; 8192];
        loop {
            let mut pfd = libc::pollfd {
                fd: raw_fd,
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: poll with a single pollfd and bounded timeout is standard POSIX.
            // raw_fd is valid for the lifetime of this call (owned by self.fd).
            let ret = unsafe { libc::poll(&mut pfd, 1, timeout_ms) };
            if ret == 0 {
                // Timeout — no data within the window; stop draining to avoid
                // blocking on a grandchild that still holds the write end.
                break;
            }
            if ret < 0 {
                let err = std::io::Error::last_os_error();
                if err.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                break;
            }
            if pfd.revents & (libc::POLLIN | libc::POLLHUP) != 0 {
                // SAFETY: raw_fd is valid; buf is a valid mutable slice.
                // read(2) is async-signal-safe and returns ≥0 on success.
                let n = unsafe { libc::read(raw_fd, buf.as_mut_ptr().cast(), buf.len()) };
                if n <= 0 {
                    // EOF (0) or error (<0) — done.
                    break;
                }
                out.extend_from_slice(&buf[..n as usize]);
            }
            if pfd.revents & libc::POLLERR != 0 {
                break;
            }
        }
    }
}

/// Duplicate the read end of an inter-block pipe so its kernel buffer stays
/// alive after the downstream process is killed.
///
/// # Errors
///
/// Returns an error if `dup(2)` fails (e.g. EMFILE — too many open fds).
#[cfg(unix)]
fn dup_pipe_stdout(stdout: &PipeStdout) -> std::io::Result<DupedPipeReader> {
    use std::os::unix::io::{AsRawFd as _, FromRawFd as _, OwnedFd};
    let raw_fd = match stdout {
        PipeStdout::Child(c) => c.as_raw_fd(),
        PipeStdout::Pipe(p) => p.as_raw_fd(),
    };
    // SAFETY: raw_fd is valid (obtained from a live ChildStdout or PipeReader).
    // dup(2) returns a new fd that the caller owns exclusively. No other code
    // holds or closes this new fd. OwnedFd will close it on drop.
    let duped = unsafe { libc::dup(raw_fd) };
    if duped < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: duped is a freshly allocated fd from dup(2) that we own exclusively.
    let owned = unsafe { OwnedFd::from_raw_fd(duped) };
    Ok(DupedPipeReader { fd: owned })
}

/// A stdout handle from a pipe chain stage.
///
/// Normal blocks produce a `ChildStdout`; sponge stages produce a
/// `PipeReader` from an `os_pipe::pipe()` pair. Both can be converted to
/// `Stdio` for the next block's stdin, and both implement `Read` for the
/// relay thread.
pub(super) enum PipeStdout {
    Child(std::process::ChildStdout),
    #[cfg(unix)]
    Pipe(os_pipe::PipeReader),
}

impl PipeStdout {
    pub(super) fn into_stdio(self) -> std::process::Stdio {
        match self {
            PipeStdout::Child(c) => std::process::Stdio::from(c),
            #[cfg(unix)]
            PipeStdout::Pipe(p) => std::process::Stdio::from(p),
        }
    }
}

impl std::io::Read for PipeStdout {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            PipeStdout::Child(c) => c.read(buf),
            #[cfg(unix)]
            PipeStdout::Pipe(p) => p.read(buf),
        }
    }
}

/// Result from a completed pipe block.
pub(super) struct PipeResult {
    pub(super) block: usize,
    pub(super) lang: String,
    pub(super) status: std::process::ExitStatus,
    /// Captured stderr bytes from the child process.
    pub(super) stderr: Vec<u8>,
}

/// Outcome of a single block in a pipe chain.
///
/// Replaces `Result<ExitStatus, io::Error>` in `ReaperResult` to accommodate
/// blocks that were cancelled before spawning (e.g. upstream exit 99).
#[cfg(unix)]
pub(crate) enum BlockOutcome {
    /// Block spawned and exited with a status.
    Exited(std::process::ExitStatus),
    /// Block failed to spawn or wait.
    Error(std::io::Error),
    /// Block was cancelled before spawning (upstream exit 99).
    Cancelled,
}

/// Result from a single reaper thread (Unix pipe mode).
#[cfg(unix)]
pub(crate) struct ReaperResult {
    pub(crate) block_idx: usize,
    pub(crate) lang: String,
    pub(crate) outcome: BlockOutcome,
    /// Captured stderr bytes from the child process.
    pub(crate) stderr: Vec<u8>,
}

/// Communication channels for a sponge stage thread.
///
/// Groups the mpsc channels used to report back to the pipe orchestrator,
/// keeping `sponge_stage`'s parameter count within clippy's limit.
#[cfg(unix)]
pub(super) struct SpongeChannels {
    /// When this sponge is block 0, carries the spawned process's PID back to
    /// `run_pipe_chain` so subsequent non-sponge blocks can join the process group.
    /// `None` for non-first blocks (pgid is already set from block 0).
    pub(super) pgid_tx: Option<std::sync::mpsc::SyncSender<Result<u32, ()>>>,
    /// Sends the block's exit status to the reaper collector in `run_pipe_chain`.
    pub(super) reaper_tx: std::sync::mpsc::Sender<ReaperResult>,
    /// Receiver for the upstream block's exit-99 determination.
    /// The upstream reaper or upstream sponge sends `true` if it exited 99,
    /// `false` otherwise. `None` when this sponge is block 0.
    pub(super) cancel_rx: Option<std::sync::mpsc::Receiver<bool>>,
    /// Sender to notify the downstream sponge of this block's exit-99
    /// determination. `None` when the next block is not a sponge.
    pub(super) cancel_tx_downstream: Option<std::sync::mpsc::Sender<bool>>,
}

/// Block until the upstream reaper reports whether the upstream block exited 99.
///
/// Returns `true` if the sponge should cancel (upstream exit 99 or SIGINT
/// already fired). Returns `false` if the sponge should proceed.
///
/// When `cancel_rx` is `None` (block 0 or upstream is a sponge), falls back
/// to the shared cancel flag only.
#[cfg(unix)]
fn should_cancel_sponge(
    ctx: &RunContext,
    cancel_rx: &Option<std::sync::mpsc::Receiver<bool>>,
) -> bool {
    if ctx.is_cancelled() {
        return true;
    }
    match cancel_rx {
        Some(rx) => {
            // Block until the reaper sends its determination. The reaper
            // always sends: true (exit 99) or false (normal exit). RecvError
            // means the reaper panicked — proceed rather than silently cancel.
            rx.recv().unwrap_or(false)
        }
        None => false,
    }
}

/// Sponge stage for a buffered block in a pipe chain.
///
/// Buffers all upstream output, performs template substitution with the
/// buffered content as `{{prev}}`, builds a `Command` via the block's runner,
/// spawns the process, pipes the expanded content to stdin, and relays the
/// process's stdout to `pipe_writer`. Sends the process's exit status through
/// `channels.reaper_tx`.
///
/// The sponge is generic — it works with any block type whose runner produces
/// a command that reads its input from stdin. For LLM blocks, the runner builds
/// the provider CLI command; future block types that need full upstream buffering
/// before starting (e.g. `buffered: true` metadata) can reuse the same path.
///
/// No `pre_exec` hooks are used. Sponge spawns use `posix_spawn()` (no
/// `fork()`) to avoid EPERM failures from non-main threads in CI environments
/// (GitHub Actions macOS launchd sessions, Ubuntu containers). The spawned
/// process is not placed in the pipe chain's process group and does not have
/// SIGINT ignored. The sponge thread manages the process's lifecycle directly
/// via pipe ownership and `child.wait()`.
///
/// When `upstream` is `None` (block 0), the sponge reads from the parent's
/// stdin. When block 0 is a sponge, `channels.pgid_tx` carries the process PID
/// back to `run_pipe_chain` so subsequent non-sponge blocks can join the process
/// group.
///
/// On cancellation (upstream exit 99), the sponge returns any buffered upstream
/// bytes as `Some(data)` via the `JoinHandle` return value. The main thread joins
/// the handle and writes the data to stdout deterministically, eliminating the
/// race where a direct stdout write from the sponge thread could lose data during
/// process teardown.
///
/// Note: during pipe execution `PipeSignalGuard` overwrites the signal-hook
/// handler, so `ctx.is_cancelled()` will not fire from SIGINT during pipe
/// chains.
#[cfg(unix)]
pub(super) fn sponge_stage(
    upstream: Option<PipeStdout>,
    pipe_writer: os_pipe::PipeWriter,
    block: &CodeBlock,
    bound_refs: Vec<(String, String)>,
    ctx: RunContext,
    block_idx: usize,
    channels: SpongeChannels,
) -> Option<Vec<u8>> {
    let SpongeChannels {
        pgid_tx,
        reaper_tx,
        cancel_rx,
        cancel_tx_downstream,
    } = channels;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Option<Vec<u8>> {
        // Keep raw bytes alongside the String so cancel paths can send the
        // exact bytes the sponge consumed, without re-encoding through lossy UTF-8.
        let raw_upstream_vec: Vec<u8> = match upstream {
            Some(mut reader) => {
                let mut buf = Vec::new();
                // Ignore read errors — if upstream crashed, reaper catches its exit status.
                let _ = reader.read_to_end(&mut buf);
                buf
            }
            None => {
                // Block 0: read from parent stdin.
                let mut buf = Vec::new();
                let _ = std::io::stdin().lock().read_to_end(&mut buf);
                buf
            }
        };
        let buffered = String::from_utf8_lossy(&raw_upstream_vec).to_string();
        let trimmed = buffered.trim_end().to_string();
        // Wrap in Option so .take() in cancel paths avoids a clone: first check
        // takes the vec; second check takes whatever is left.
        let mut raw_upstream = Some(raw_upstream_vec);

        // If the pipeline was cancelled (upstream exit 99 or SIGINT), bail without
        // spawning the provider. Return the buffered upstream bytes via the JoinHandle
        // so the main thread can write them to stdout after joining — avoids the race
        // where a direct thread write loses data during process teardown.
        if should_cancel_sponge(&ctx, &cancel_rx) {
            drop(pipe_writer);
            let data = raw_upstream.take().unwrap_or_default();
            if let Some(ref tx) = cancel_tx_downstream {
                let _ = tx.send(true);
            }
            if let Some(tx) = pgid_tx {
                let _ = tx.send(Err(()));
            }
            let _ = reaper_tx.send(ReaperResult {
                block_idx,
                lang: block.lang.clone(),
                outcome: BlockOutcome::Cancelled,
                stderr: vec![],
            });
            return if data.is_empty() { None } else { Some(data) };
        }

        let ref_pairs: Vec<(&str, &str)> = bound_refs
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .chain(std::iter::once(("prev", trimmed.as_str())))
            .collect();
        let expanded = match substitute(&block.code, &ref_pairs, &block.lang) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("error: sponge block {}: {}", block_idx + 1, e);
                drop(pipe_writer);
                if let Some(ref tx) = cancel_tx_downstream {
                    let _ = tx.send(true);
                }
                if let Some(tx) = pgid_tx {
                    let _ = tx.send(Err(()));
                }
                let _ = reaper_tx.send(ReaperResult {
                    block_idx,
                    lang: block.lang.clone(),
                    outcome: BlockOutcome::Error(std::io::Error::other(e.to_string())),
                    stderr: vec![],
                });
                return None;
            }
        };

        // prepare_block_script creates a temp file; LLM runners ignore it (prompt
        // is delivered via stdin), but it must exist for the trait signature.
        let tmp = match prepare_block_script(block, &expanded) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("error: sponge block {}: {}", block_idx + 1, e);
                drop(pipe_writer);
                if let Some(ref tx) = cancel_tx_downstream {
                    let _ = tx.send(true);
                }
                if let Some(tx) = pgid_tx {
                    let _ = tx.send(Err(()));
                }
                let _ = reaper_tx.send(ReaperResult {
                    block_idx,
                    lang: block.lang.clone(),
                    outcome: BlockOutcome::Error(std::io::Error::other(e.to_string())),
                    stderr: vec![],
                });
                return None;
            }
        };
        let runner = super::blocks::runner_for(&block.lang);
        let (mut cmd, _node_deps_dir) = match runner.build_command(block, tmp.path()) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("error: sponge block {}: {}", block_idx + 1, e);
                drop(pipe_writer);
                if let Some(ref tx) = cancel_tx_downstream {
                    let _ = tx.send(true);
                }
                if let Some(tx) = pgid_tx {
                    let _ = tx.send(Err(()));
                }
                let _ = reaper_tx.send(ReaperResult {
                    block_idx,
                    lang: block.lang.clone(),
                    outcome: BlockOutcome::Error(std::io::Error::other(e.to_string())),
                    stderr: vec![],
                });
                return None;
            }
        };

        // No pre_exec hooks — posix_spawn() compatibility for non-main threads.
        cmd.current_dir(ctx.cwd());
        for (k, v) in ctx.env_pairs() {
            cmd.env(k, v);
        }
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        // Second cancellation check — catches SIGINT that arrived during template
        // expansion and command building. The channel verdict was consumed by the
        // first check; calling recv() again would block forever.
        if ctx.is_cancelled() {
            drop(pipe_writer);
            let data = raw_upstream.take().unwrap_or_default();
            if let Some(ref tx) = cancel_tx_downstream {
                let _ = tx.send(true);
            }
            if let Some(tx) = pgid_tx {
                let _ = tx.send(Err(()));
            }
            let _ = reaper_tx.send(ReaperResult {
                block_idx,
                lang: block.lang.clone(),
                outcome: BlockOutcome::Cancelled,
                stderr: vec![],
            });
            return if data.is_empty() { None } else { Some(data) };
        }

        // For the error message, use the provider name for LLM blocks so the
        // user knows which CLI is missing, not just "llm".
        let display_name = if block.lang == "llm" {
            block
                .llm_config
                .as_ref()
                .map(|c| {
                    if c.provider.is_empty() {
                        "claude".to_string()
                    } else {
                        c.provider.clone()
                    }
                })
                .unwrap_or_else(|| "claude".to_string())
        } else {
            block.lang.clone()
        };
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                let msg = if e.kind() == std::io::ErrorKind::NotFound {
                    format!(
                        "'{}' not found on PATH. Install the provider CLI.",
                        display_name
                    )
                } else {
                    e.to_string()
                };
                eprintln!("error: sponge block {}: {}", block_idx + 1, msg);
                drop(pipe_writer);
                if let Some(ref tx) = cancel_tx_downstream {
                    let _ = tx.send(true);
                }
                if let Some(tx) = pgid_tx {
                    let _ = tx.send(Err(()));
                }
                let _ = reaper_tx.send(ReaperResult {
                    block_idx,
                    lang: block.lang.clone(),
                    outcome: BlockOutcome::Error(e),
                    stderr: vec![],
                });
                return None;
            }
        };

        // When this sponge is block 0, report the child's PID.
        if let Some(tx) = pgid_tx {
            let _ = tx.send(Ok(child.id()));
        }

        if let Some(mut stdin) = child.stdin.take() {
            // Ignore BrokenPipe — child's exit status is the authoritative error signal.
            let _ = stdin.write_all(expanded.as_bytes());
        }

        let mut pipe_writer = pipe_writer;
        if let Some(mut stdout) = child.stdout.take() {
            let mut buf = [0u8; 8192];
            loop {
                match stdout.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        // PipeSignalGuard overwrites the signal-hook handler during pipe
                        // execution, so this check fires only when the guard is absent
                        // (single-block sponge runs).
                        if ctx.is_cancelled() {
                            break;
                        }
                        if pipe_writer.write_all(&buf[..n]).is_err() {
                            // Downstream closed its stdin early — stop relaying.
                            break;
                        }
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
        }
        // Drop pipe_writer to signal EOF to the downstream block.
        drop(pipe_writer);

        // Drain stderr before wait() to prevent deadlock: if the child filled the
        // OS stderr pipe buffer (~64KB) it would block on write(), and wait() would
        // never return.
        let captured_stderr = {
            let mut buf = Vec::new();
            if let Some(mut err) = child.stderr.take() {
                let _ = std::io::Read::read_to_end(&mut err, &mut buf);
            }
            buf
        };

        let status = child.wait();

        let exit_99 = status.as_ref().ok().and_then(crate::runner::exit_code_of)
            == Some(crate::runner::EARLY_EXIT);
        if let Some(ref tx) = cancel_tx_downstream {
            let _ = tx.send(exit_99);
        }

        let _ = reaper_tx.send(ReaperResult {
            block_idx,
            lang: block.lang.clone(),
            outcome: match status {
                Ok(s) => BlockOutcome::Exited(s),
                Err(e) => BlockOutcome::Error(e),
            },
            stderr: captured_stderr,
        });

        // Normal path: provider's output went through the pipe. No data to recover.
        None
    }));

    match result {
        Ok(captured) => captured,
        Err(_) => {
            // Sponge thread panicked — send error to prevent main thread hang on rx.recv().
            // Send true downstream so the downstream sponge does not proceed with empty input.
            if let Some(ref tx) = cancel_tx_downstream {
                let _ = tx.send(true);
            }
            let _ = reaper_tx.send(ReaperResult {
                block_idx,
                lang: block.lang.clone(),
                outcome: BlockOutcome::Error(std::io::Error::other("sponge thread panicked")),
                stderr: vec![],
            });
            None
        }
    }
}

/// Parent-side setpgid for the POSIX double-setpgid pattern.
///
/// Called after spawn() returns. The child also calls setpgid in pre_exec.
/// Whichever runs first wins; the loser gets a harmless EACCES or ESRCH.
#[cfg(unix)]
fn parent_setpgid(child_pid: u32, pgid: u32) {
    // SAFETY: setpgid is async-signal-safe and both PIDs are valid
    // (just-spawned child, known group leader).
    let ret = unsafe { libc::setpgid(child_pid as libc::pid_t, pgid as libc::pid_t) };
    if ret == -1 {
        let err = std::io::Error::last_os_error();
        match err.raw_os_error() {
            // EACCES: child already exec'd and set its own pgid.
            // ESRCH: child already exited.
            Some(libc::EACCES) | Some(libc::ESRCH) => {}
            _ => {
                eprintln!(
                    "warning: parent setpgid({}, {}) failed: {}",
                    child_pid, pgid, err
                );
            }
        }
    }
}

/// Attempt killpg. Returns true if the group was successfully signaled.
#[cfg(unix)]
fn kill_group(pgid: Option<u32>) -> bool {
    if let Some(pgid) = pgid {
        // SAFETY: kill(-pgid, SIGKILL) is standard POSIX.
        // Negative first argument means "signal all processes in process group pgid".
        let ret = unsafe { libc::kill(-(pgid as libc::pid_t), libc::SIGKILL) };
        ret == 0
    } else {
        false
    }
}

/// Kill the pipe chain's process group, falling back to per-process kills.
#[cfg(unix)]
fn kill_pipe_group(
    pgid: Option<u32>,
    children: &[(
        std::process::Child,
        usize,
        String,
        Option<std::sync::mpsc::Sender<bool>>,
    )],
) {
    if kill_group(pgid) {
        return;
    }
    for (child, _, _, _) in children {
        // SAFETY: kill(pid, SIGKILL) is standard POSIX. Harmless ESRCH if already dead.
        unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGKILL) };
    }
}

/// Kill the pipe chain's process group, falling back to per-PID kills.
#[cfg(unix)]
fn kill_pipe_group_by_pids(pgid: Option<u32>, pids: &[u32]) {
    if kill_group(pgid) {
        return;
    }
    for &pid in pids {
        // SAFETY: kill(pid, SIGKILL) is standard POSIX. Harmless ESRCH if already dead.
        unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
    }
}

/// Arguments for `wait_pipe_children_unix`, grouped to stay within clippy's
/// argument count limit.
#[cfg(unix)]
struct WaitArgs {
    children: Vec<(
        std::process::Child,
        usize,
        String,
        Option<std::sync::mpsc::Sender<bool>>,
    )>,
    child_pids: Vec<u32>,
    last_stdout: PipeStdout,
    child_pgid: Option<u32>,
    tx: std::sync::mpsc::Sender<ReaperResult>,
    rx: std::sync::mpsc::Receiver<ReaperResult>,
    last_block_idx: usize,
    exit99_drains: std::collections::HashMap<usize, DupedPipeReader>,
    sponge_handles: Vec<std::thread::JoinHandle<Option<Vec<u8>>>>,
}

/// Wait for all children in a Unix pipe chain using concurrent reaper threads
/// and a buffered stdout relay.
///
/// Each child is moved into its own reaper thread that calls `child.wait()` and
/// sends the result through an mpsc channel. Results arrive in exit order, not
/// spawn order. The last block's stdout is relayed into a buffer by a dedicated
/// relay thread; the buffer is flushed to the terminal only after all reapers
/// have reported and the exit-99 check passes.
///
/// The `tx`/`rx` channel pair is created by the caller (`run_pipe_chain`) so
/// that sponge threads can also send results through the same channel before
/// this function is called. The caller must drop its own `tx` clone before
/// calling this function so the channel closes when all reaper and sponge
/// threads finish.
///
/// Relay buffer flush is selective: if an exit-99 block is the last block in
/// the chain, its output has already been captured in the relay buffer and is
/// flushed to the terminal. If the exit-99 block is a middle block, the
/// duplicated pipe fd in `exit99_drains` is drained and flushed instead. Sponge
/// handles are joined here (before the relay thread) so that any captured
/// upstream bytes returned by cancelled sponge threads are available for the
/// output selection decision.
#[cfg(unix)]
fn wait_pipe_children_unix(
    args: WaitArgs,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Result<(Vec<PipeResult>, bool), CreftError> {
    let WaitArgs {
        children,
        child_pids,
        last_stdout,
        child_pgid,
        tx,
        rx,
        last_block_idx,
        mut exit99_drains,
        sponge_handles,
    } = args;
    let cancel_relay = std::sync::Arc::clone(&cancel);
    // Never writes to the terminal — the main thread decides flush vs. discard.
    // Uses poll() with 100ms timeout so the cancel flag is checked between iterations.
    // This ensures the relay exits promptly when killpg kills the child but an orphaned
    // grandchild still holds the pipe's write end open.
    let relay_handle = std::thread::Builder::new()
        .name("creft-relay".to_owned())
        .spawn(move || {
            use std::os::unix::io::AsRawFd as _;
            let mut reader = last_stdout;
            let raw_fd = match &reader {
                PipeStdout::Child(c) => c.as_raw_fd(),
                PipeStdout::Pipe(p) => p.as_raw_fd(),
            };
            let mut buf = [0u8; 8192];
            let mut output: Vec<u8> = Vec::new();
            loop {
                let mut pfd = libc::pollfd {
                    fd: raw_fd,
                    events: libc::POLLIN,
                    revents: 0,
                };
                // SAFETY: poll with a single fd and a 100ms timeout is standard POSIX.
                let ret = unsafe { libc::poll(&mut pfd, 1, 100) };

                if cancel_relay.load(std::sync::atomic::Ordering::Relaxed) {
                    break;
                }
                if ret == 0 {
                    // Timeout — no data yet, loop to check cancel again.
                    continue;
                }
                if ret < 0 {
                    let err = std::io::Error::last_os_error();
                    if err.kind() == std::io::ErrorKind::Interrupted {
                        continue;
                    }
                    break;
                }
                // POLLHUP: all write ends closed — drain remaining data, then break.
                // POLLIN: data available — read it.
                // On macOS, POLLHUP may be set without POLLIN when the write end closes.
                if pfd.revents & (libc::POLLIN | libc::POLLHUP) != 0 {
                    match reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => output.extend_from_slice(&buf[..n]),
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(_) => break,
                    }
                }
                if pfd.revents & libc::POLLERR != 0 {
                    break;
                }
            }
            output
        })
        .expect("failed to spawn relay thread");

    for (i, (child, block_idx, lang, cancel_tx)) in children.into_iter().enumerate() {
        let tx = tx.clone();
        let pgid = child_pgid; // Option<u32> is Copy — no Arc needed
        std::thread::Builder::new()
            .name(format!("creft-reaper-{i}"))
            .spawn(move || {
                let mut child = child;
                // Drain stderr before wait() to prevent deadlock: a child that fills
                // the stderr pipe buffer blocks on write(), and wait() never returns.
                let captured_stderr = {
                    let mut buf = Vec::new();
                    if let Some(mut err) = child.stderr.take() {
                        let _ = std::io::Read::read_to_end(&mut err, &mut buf);
                    }
                    buf
                };
                let status = child.wait();
                let exit_99 = status.as_ref().ok().and_then(crate::runner::exit_code_of)
                    == Some(crate::runner::EARLY_EXIT);
                // Kill the process group immediately on exit 99, before any channel hop.
                // The main thread's kill is a redundant safety net.
                if exit_99 {
                    kill_group(pgid);
                }
                // Always send the exit-99 determination to the downstream sponge so
                // it can unblock from recv(). Send before the ReaperResult so the
                // sponge unblocks before the main thread begins its kill/cancel cascade.
                // Ignore send error: sponge already exited or was never spawned.
                if let Some(cancel) = cancel_tx {
                    let _ = cancel.send(exit_99);
                }
                // Ignore send error: main thread dropped rx only if it panicked.
                let _ = tx.send(ReaperResult {
                    block_idx,
                    lang,
                    outcome: match status {
                        Ok(s) => BlockOutcome::Exited(s),
                        Err(e) => BlockOutcome::Error(e),
                    },
                    stderr: captured_stderr,
                });
            })
            .expect("failed to spawn reaper thread");
    }
    // Drop the tx clone passed in from run_pipe_chain. Combined with sponge threads
    // dropping their clones and reaper threads dropping theirs, rx closes when all done.
    drop(tx);

    let mut results: Vec<PipeResult> = Vec::new();
    let mut early_exit = false;
    // Captured stdout from a middle-block exit 99, recovered via the dup'd fd.
    let mut exit99_captured: Option<Vec<u8>> = None;

    while let Ok(reaper_result) = rx.recv() {
        match reaper_result.outcome {
            BlockOutcome::Cancelled => {
                // Sponge cancelled before spawning — not a failure, not an exit.
                // Do not push to results; the block did not participate.
                continue;
            }
            BlockOutcome::Error(e) => {
                return Err(CreftError::Io(e));
            }
            BlockOutcome::Exited(status) => {
                if exit_code_of(&status) == Some(EARLY_EXIT) && !early_exit {
                    early_exit = true;
                    // Kill all processes in the pipe group so grandchildren are also killed.
                    // Falls back to per-PID kills if killpg fails.
                    kill_pipe_group_by_pids(child_pgid, &child_pids);
                    // Signal sponge threads to bail before spawning.
                    cancel.store(true, std::sync::atomic::Ordering::Relaxed);

                    // Drain the dup'd pipe fd for this block (if it's not the last block).
                    // The exit-99 block has already exited, so its write end is closed and
                    // read_to_end returns promptly with whatever remains in the pipe buffer.
                    // Drain errors are ignored — same behavior as today if drain is absent.
                    if let Some(mut drain_reader) = exit99_drains.remove(&reaper_result.block_idx) {
                        let mut buf = Vec::new();
                        // Use poll()-based drain to avoid blocking if an orphaned
                        // grandchild still holds the pipe write end open after killpg.
                        drain_reader.drain_with_timeout(&mut buf, 500);
                        if !buf.is_empty() {
                            exit99_captured = Some(buf);
                        }
                    }
                }

                results.push(PipeResult {
                    block: reaper_result.block_idx,
                    lang: reaper_result.lang,
                    status,
                    stderr: reaper_result.stderr,
                });
            }
        }
    }

    // Close all remaining dup'd fds (those not consumed by drain above).
    // These are independent inter-block pipes; dropping them does not affect
    // the relay thread's pipe to the last block.
    drop(exit99_drains);

    // All reaper results received. Join sponge threads to collect any data they
    // captured from the cancel path. Joining here is a synchronization barrier:
    // after join() returns, the data is on the main thread's stack with no timing
    // window. handle.join().ok().flatten(): Err (panic) → None, Ok(None) → None,
    // Ok(Some(data)) → Some(data).
    let sponge_captured: Option<Vec<u8>> = sponge_handles
        .into_iter()
        .filter_map(|h| h.join().ok().flatten())
        .find(|d| !d.is_empty());

    // All reapers have exited. Join the relay thread to retrieve the buffered output.
    // unwrap_or_default: relay panic yields empty buffer (no output printed, no crash).
    let relay_buffer = relay_handle.join().unwrap_or_default();

    if early_exit {
        // Find which block exited 99. If none found (shouldn't happen), output nothing.
        let exit99_idx = results
            .iter()
            .find(|r| exit_code_of(&r.status) == Some(EARLY_EXIT))
            .map(|r| r.block);

        // Select output source by priority:
        // 1. Exit-99 block is last → relay buffer has its output.
        // 2. Dup'd fd drain → kernel pipe buffer residue (downstream didn't consume it).
        // 3. Sponge captured data → buffered upstream bytes returned via JoinHandle.
        let output: &[u8] = match exit99_idx {
            Some(idx) if idx == last_block_idx => &relay_buffer,
            Some(_) => {
                // Middle or first block. Use dup'd fd drain if non-empty, then fall
                // back to sponge captured data (upstream bytes the sponge buffered).
                exit99_captured
                    .as_deref()
                    .filter(|d| !d.is_empty())
                    .or(sponge_captured.as_deref().filter(|d| !d.is_empty()))
                    .unwrap_or(&[])
            }
            None => &[],
        };

        if !output.is_empty() {
            let stdout = std::io::stdout();
            let mut lock = stdout.lock();
            let _ = lock.write_all(output);
        }
    } else {
        let stdout = std::io::stdout();
        let mut lock = stdout.lock();
        // Ignore write errors: creft's own stdout may be a broken pipe.
        let _ = lock.write_all(&relay_buffer);
    }

    // Sort by spawn order before post-loop checks; channel delivers in exit order.
    results.sort_by_key(|r| r.block);

    Ok((results, early_exit))
}

/// Wait for all children in a non-Unix pipe chain using sequential wait calls.
///
/// Fallback for platforms without process groups or kill(-pgid). Has the same
/// sequential-wait race window as the original implementation.
#[cfg(not(unix))]
fn wait_pipe_children_fallback(
    children: Vec<Option<(std::process::Child, usize, String)>>,
) -> Result<(Vec<PipeResult>, bool), CreftError> {
    let mut children = children;
    let n = children.len();
    let mut results: Vec<PipeResult> = Vec::with_capacity(n);
    let mut early_exit = false;

    for i in 0..children.len() {
        let (mut child, block_idx, lang) = children[i].take().expect("child taken once");
        // Drain stderr before wait() to prevent deadlock: a child that fills the
        // stderr pipe buffer blocks on write(), and wait() never returns.
        let captured_stderr = {
            let mut buf = Vec::new();
            if let Some(mut err) = child.stderr.take() {
                let _ = std::io::Read::read_to_end(&mut err, &mut buf);
            }
            buf
        };
        let status = child.wait().map_err(CreftError::Io)?;

        if exit_code_of(&status) == Some(EARLY_EXIT) {
            for remaining in children.iter_mut().skip(i + 1) {
                if let Some((mut c, _, _)) = remaining.take() {
                    let _ = c.kill();
                    // Drain stderr from killed children to prevent deadlock.
                    // The bytes are discarded — killed children's output is not actionable.
                    if let Some(mut err) = c.stderr.take() {
                        let mut discard = Vec::new();
                        let _ = std::io::Read::read_to_end(&mut err, &mut discard);
                    }
                    let _ = c.wait();
                }
            }
            early_exit = true;
            results.push(PipeResult {
                block: block_idx,
                lang,
                status,
                stderr: captured_stderr,
            });
            break;
        }

        results.push(PipeResult {
            block: block_idx,
            lang,
            status,
            stderr: captured_stderr,
        });
    }

    Ok((results, early_exit))
}

/// Execute all blocks in a multi-block command concurrently with OS-level pipe
/// connections between stdout/stdin.
///
/// All blocks are spawned before any are waited on. Block N's stdout is
/// connected to block N+1's stdin via Stdio::from(PipeStdout). On Unix, the
/// last block's stdout is buffered by a relay thread; output is flushed to the
/// terminal only after confirming no block exited 99.
///
/// Blocks that return `true` from `needs_sponge()` participate as sponge stages:
/// each sponge thread reads all upstream output, performs template substitution,
/// spawns the block's process, and relays the process's stdout to the next block
/// via an `os_pipe` pair.
///
/// Returns Ok(()) if the last block exits successfully. Earlier blocks dying
/// from SIGPIPE when the downstream consumer exits early is normal pipeline
/// behavior and is not reported as an error unless the last block also fails.
pub(super) fn run_pipe_chain(
    cmd: &ParsedCommand,
    bound_refs: &[(&str, &str)],
    ctx: &RunContext,
) -> Result<(), CreftError> {
    let n = cmd.blocks.len();

    // Temp files for non-sponge blocks. Sponge blocks use prepare_block_script
    // internally inside sponge_stage. Use Option so indices align with block indices.
    let mut temp_files: Vec<Option<tempfile::NamedTempFile>> = Vec::with_capacity(n);
    for block in &cmd.blocks {
        if block.needs_sponge() {
            temp_files.push(None);
        } else {
            // In pipe mode, no "prev" template arg (output is on stdin).
            let expanded = substitute(&block.code, bound_refs, &block.lang)?;
            let tmp = prepare_block_script(block, &expanded)?;
            temp_files.push(Some(tmp));
        }
    }

    // node_deps_dirs keeps npm-installed tempdir handles alive until all children exit.
    let mut node_deps_dirs: Vec<Option<tempfile::TempDir>> = Vec::with_capacity(n);
    let mut children: Vec<(
        std::process::Child,
        usize,
        String,
        Option<std::sync::mpsc::Sender<bool>>,
    )> = Vec::with_capacity(n);
    let mut prev_stdout: Option<PipeStdout> = None;
    // PID of the first child, used as the process group ID for all pipe children.
    #[cfg(unix)]
    let mut child_pgid: Option<u32> = None;
    // Last block's stdout handle (Unix only) — piped for buffered relay.
    #[cfg(unix)]
    let mut last_child_stdout: Option<PipeStdout> = None;

    // Reaper channel created here so sponge threads can also send results.
    // The main thread drops its tx clone before calling wait_pipe_children_unix.
    #[cfg(unix)]
    let (reaper_tx, reaper_rx) = std::sync::mpsc::channel::<ReaperResult>();

    // Join handles for sponge threads — joined inside wait_pipe_children_unix
    // so returned data is available when the output selection decision is made.
    #[cfg(unix)]
    let mut sponge_handles: Vec<std::thread::JoinHandle<Option<Vec<u8>>>> = Vec::new();

    // Dup'd read ends of inter-block pipes, keyed by block index.
    // Only middle blocks (not the last) get entries. The last block's output
    // is captured by the relay thread; duping it would create a competing reader.
    #[cfg(unix)]
    let mut exit99_drains: std::collections::HashMap<usize, DupedPipeReader> =
        std::collections::HashMap::new();

    // Cancel receivers for sponge blocks, keyed by sponge block index.
    // Created when a non-sponge block precedes a sponge block; the sender goes
    // into the children tuple for the upstream block's reaper thread.
    #[cfg(unix)]
    let mut sponge_cancel_rxs: std::collections::HashMap<
        usize,
        std::sync::mpsc::Receiver<bool>,
    > = std::collections::HashMap::new();

    for (i, block) in cmd.blocks.iter().enumerate() {
        let is_last = i == n - 1;

        #[cfg(unix)]
        if block.needs_sponge() {
            let (pipe_reader, pipe_writer) = os_pipe::pipe().map_err(CreftError::Io)?;

            let upstream = prev_stdout.take();
            let is_pgid_creator = i == 0;

            // When block 0 is a sponge, synchronize pgid via a channel so
            // subsequent blocks can join the correct process group.
            type PgidChannel = (
                std::sync::mpsc::SyncSender<Result<u32, ()>>,
                std::sync::mpsc::Receiver<Result<u32, ()>>,
            );
            let pgid_channel: Option<PgidChannel> = if is_pgid_creator {
                Some(std::sync::mpsc::sync_channel(1))
            } else {
                None
            };
            let pgid_tx = pgid_channel.as_ref().map(|(tx, _)| tx.clone());

            // Retrieve the cancel receiver placed here by the upstream non-sponge
            // block's spawn logic. None when block 0 is a sponge (no upstream
            // reaper) or when the upstream is itself a sponge.
            let cancel_rx = sponge_cancel_rxs.remove(&i);

            // When the next block is also a sponge, create a direct cancel channel
            // so this sponge thread can send its exit-99 determination to the
            // downstream sponge without going through the reaper.
            let cancel_tx_downstream: Option<std::sync::mpsc::Sender<bool>> = {
                let next_is_sponge = cmd.blocks.get(i + 1).is_some_and(|b| b.needs_sponge());
                if next_is_sponge {
                    let (tx, rx) = std::sync::mpsc::channel::<bool>();
                    sponge_cancel_rxs.insert(i + 1, rx);
                    Some(tx)
                } else {
                    None
                }
            };

            let owned_block = block.clone();
            let owned_bound_refs: Vec<(String, String)> = bound_refs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            let ctx_clone = ctx.clone();
            let reaper_tx_clone = reaper_tx.clone();

            let handle = std::thread::Builder::new()
                .name(format!("creft-sponge-{i}"))
                .spawn(move || {
                    sponge_stage(
                        upstream,
                        pipe_writer,
                        &owned_block,
                        owned_bound_refs,
                        ctx_clone,
                        i,
                        SpongeChannels {
                            pgid_tx,
                            reaper_tx: reaper_tx_clone,
                            cancel_rx,
                            cancel_tx_downstream,
                        },
                    )
                })
                .expect("failed to spawn sponge thread");
            sponge_handles.push(handle);

            // If block 0 is a sponge, wait for the process to spawn before
            // continuing. The provider PID is not used as the process group ID —
            // posix_spawn() is used (no pre_exec setpgid), so it is not a pgid
            // leader. The first non-sponge block creates the process group instead.
            // Provider failure is handled by the reaper channel — continue regardless.
            if is_pgid_creator && let Some((_, pgid_rx)) = pgid_channel {
                let _ = pgid_rx.recv();
            }

            node_deps_dirs.push(None);

            // The pipe_reader is the "stdout" of this sponge stage.
            if !is_last {
                let pipe_stdout = PipeStdout::Pipe(pipe_reader);
                // Dup the read end so the kernel buffer survives if the downstream
                // block is killed on exit 99. Failure kills already-spawned children.
                let duped = dup_pipe_stdout(&pipe_stdout).inspect_err(|_| {
                    kill_pipe_group(child_pgid, &children);
                    drop(children.drain(..));
                    drop(node_deps_dirs.drain(..));
                })?;
                exit99_drains.insert(i, duped);
                prev_stdout = Some(pipe_stdout);
            } else {
                last_child_stdout = Some(PipeStdout::Pipe(pipe_reader));
            }
            continue;
        }

        let script_path = temp_files[i]
            .as_ref()
            .expect("non-sponge block must have temp file")
            .path();

        let stdin_cfg = match prev_stdout.take() {
            // Block 0: inherit parent stdin (or /dev/null if none).
            None => std::process::Stdio::inherit(),
            // Intermediate + last blocks: fd from previous stage's stdout.
            Some(ps) => ps.into_stdio(),
        };

        // On Unix, all blocks use Stdio::piped(): intermediate blocks feed the
        // next block's stdin, the last block's stdout goes to the relay thread.
        // On non-Unix, last block inherits stdout (v1 behavior).
        #[cfg(unix)]
        let stdout_cfg = std::process::Stdio::piped();
        #[cfg(not(unix))]
        let stdout_cfg = if is_last {
            std::process::Stdio::inherit()
        } else {
            std::process::Stdio::piped()
        };

        // Some(0) → first spawned block creates its own process group; Some(pgid) → join it.
        // child_pgid is None when no non-sponge block has been spawned yet (including when
        // block 0 is a sponge — the sponge's provider is not a pgid leader).
        #[cfg(unix)]
        let pg = if child_pgid.is_none() {
            Some(0u32)
        } else {
            child_pgid
        };

        // Non-first blocks ignore SIGINT so only the pipe head receives Ctrl+C.
        // When the head dies, downstream blocks get EOF/SIGPIPE and exit cleanly
        // without printing raw language-level tracebacks (e.g. Python KeyboardInterrupt).
        #[cfg(unix)]
        let sigint_ignored = i > 0;

        let (mut child, node_deps_dir) = spawn_block(
            block,
            script_path,
            ctx,
            stdin_cfg,
            stdout_cfg,
            #[cfg(unix)]
            pg,
            #[cfg(unix)]
            sigint_ignored,
        )
        .inspect_err(|_| {
            #[cfg(unix)]
            kill_pipe_group(child_pgid, &children);
            drop(children.drain(..));
            drop(node_deps_dirs.drain(..));
        })?;
        node_deps_dirs.push(node_deps_dir);

        // After spawning the first non-sponge block, record its PID as the
        // process group ID. setpgid(0, 0) in pre_exec makes its PID its own PGID.
        #[cfg(unix)]
        if child_pgid.is_none() {
            child_pgid = Some(child.id());
        }

        // POSIX double-setpgid: parent calls setpgid after spawn returns,
        // child also calls it in pre_exec. Whichever runs first wins.
        #[cfg(unix)]
        parent_setpgid(
            child.id(),
            child_pgid.expect("child_pgid set above or just set"),
        );

        if !is_last {
            let stdout = child.stdout.take();
            if stdout.is_none() {
                // Stdio::piped() must always yield a ChildStdout — this path is unreachable
                // under normal conditions, but guard against it to avoid a silent hang.
                #[cfg(unix)]
                kill_pipe_group(child_pgid, &children);
                drop(children.drain(..));
                return Err(CreftError::Setup(format!(
                    "internal: failed to capture stdout for block {} ({})",
                    i, block.lang
                )));
            }
            #[cfg(unix)]
            {
                let pipe_stdout = PipeStdout::Child(stdout.expect("checked above"));
                // Dup the read end before passing it to the next block as stdin.
                // The kernel buffer stays alive after the downstream block is killed.
                let duped = dup_pipe_stdout(&pipe_stdout).inspect_err(|_| {
                    kill_pipe_group(child_pgid, &children);
                    drop(children.drain(..));
                    drop(node_deps_dirs.drain(..));
                })?;
                exit99_drains.insert(i, duped);
                prev_stdout = Some(pipe_stdout);
            }
            #[cfg(not(unix))]
            {
                prev_stdout = stdout.map(PipeStdout::Child);
            }
        } else {
            // Last block: extract its stdout for the relay thread (Unix only).
            // Must be done before the child is moved into the reaper thread.
            #[cfg(unix)]
            {
                last_child_stdout = child.stdout.take().map(PipeStdout::Child);
            }
        }

        // When the next block is a sponge, create a direct cancellation channel.
        // The sender is held by this block's reaper thread and dropped on exit 99.
        // The receiver is stored and later passed to the sponge thread via SpongeChannels.
        #[cfg(unix)]
        let cancel_tx: Option<std::sync::mpsc::Sender<bool>> = {
            let next_is_sponge = cmd.blocks.get(i + 1).is_some_and(|b| b.needs_sponge());
            if next_is_sponge {
                let (tx, rx) = std::sync::mpsc::channel::<bool>();
                sponge_cancel_rxs.insert(i + 1, rx);
                Some(tx)
            } else {
                None
            }
        };

        children.push((
            child,
            i,
            block.lang.clone(),
            #[cfg(unix)]
            cancel_tx,
            #[cfg(not(unix))]
            None,
        ));
    }

    // Install SIGINT handler for the pipe chain duration (Unix only).
    // The guard sets up SIGINT forwarding to the child process group and
    // restores the original handler when dropped (RAII).
    //
    // Note: PipeSignalGuard uses libc::signal(), which overwrites signal-hook's
    // sigaction-based handler. The cancel token is therefore NOT set by SIGINT
    // during pipe execution. On drop, the guard restores signal-hook's handler
    // so the token is set again for any SIGINT received after the pipe chain exits.
    #[cfg(unix)]
    let _sigint_guard = child_pgid.map(PipeSignalGuard::new);

    #[cfg(unix)]
    let (results, early_exit) = {
        let last_stdout = last_child_stdout.ok_or_else(|| {
            CreftError::Setup("internal: failed to capture last block stdout".to_owned())
        })?;
        // Drop the main-thread tx clone so the channel closes when all senders
        // (sponge + reaper threads) drop their own clones.
        let tx = reaper_tx;
        let rx = reaper_rx;
        let child_pids: Vec<u32> = children.iter().map(|(c, _, _, _)| c.id()).collect();
        wait_pipe_children_unix(
            WaitArgs {
                children,
                child_pids,
                last_stdout,
                child_pgid,
                tx,
                rx,
                last_block_idx: n - 1,
                exit99_drains,
                sponge_handles,
            },
            ctx.cancel_arc(),
        )?
    };

    #[cfg(not(unix))]
    let (results, early_exit) = {
        let children_opt: Vec<Option<(std::process::Child, usize, String)>> = children
            .into_iter()
            .map(|(c, i, l, _)| Some((c, i, l)))
            .collect();
        wait_pipe_children_fallback(children_opt)?
    };

    if early_exit {
        // Ensure the cancel token is set for any code that polls it after the
        // pipe chain returns (e.g. non-Unix paths or callers of run_pipe_chain).
        ctx.request_cancel();
        return Ok(());
    }

    // Check if any block was killed by SIGINT (signal 2) first.
    //
    // When non-first blocks have SIG_IGN for SIGINT, they exit cleanly via
    // EOF/SIGPIPE after the head dies — so the last block may exit 0 even
    // though the pipeline was interrupted. We must check for SIGINT-killed
    // blocks BEFORE the last-block-success check to preserve the shell
    // convention that Ctrl+C yields exit code 130.
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        let sigint_killed = results.iter().any(|r| r.status.signal() == Some(2));
        if sigint_killed {
            // Drop the guard first to restore the original SIGINT handler.
            // Then re-raise SIGINT so this process dies the same way.
            drop(_sigint_guard);
            // SAFETY: signal(SIGINT, SIG_DFL) and kill(getpid(), SIGINT) are
            // standard POSIX calls used to propagate signal death to the parent
            // shell. After kill(), the process receives SIGINT with default
            // handler (terminate), so the code after this block is unreachable.
            unsafe {
                libc::signal(libc::SIGINT, libc::SIG_DFL);
                libc::kill(libc::getpid(), libc::SIGINT);
            }
            // Unreachable in practice, but return an error as fallback.
            return Err(CreftError::ExecutionSignaled {
                block: 0,
                lang: results.first().map(|r| r.lang.clone()).unwrap_or_default(),
                signal: 2,
            });
        }
    }

    // Exit 99 from any block means early successful return — downstream blocks
    // will have received EOF/SIGPIPE and exited cleanly as a side-effect.
    if results
        .iter()
        .any(|r| exit_code_of(&r.status) == Some(EARLY_EXIT))
    {
        return Ok(());
    }

    // Under --verbose, emit captured stderr for every block that produced any,
    // regardless of exit status. This lets users see what the child wrote even
    // when the block succeeded. The `[block N stderr]` prefix makes multi-block
    // output attributable.
    if ctx.is_verbose() {
        for r in &results {
            if !r.stderr.is_empty() {
                let _ = writeln!(std::io::stderr(), "[block {} stderr]", r.block + 1);
                let _ = std::io::stderr().write_all(&r.stderr);
            }
        }
    }

    // Last block success wins: earlier blocks dying from SIGPIPE (consumer exited
    // early) is normal pipeline behavior, not an error.
    let last = results.last().expect("at least one block");
    if last.status.success() {
        return Ok(());
    }

    // Signal-killed upstream blocks (e.g. SIGPIPE when consumer exits early) are
    // side-effects, not root causes. Find the earliest non-signal failure instead.
    let root = results
        .iter()
        .find(|r| !r.status.success() && exit_code_of(&r.status).is_some())
        .unwrap_or(last);

    // When not in verbose mode, the root block's stderr has not yet been emitted.
    // In verbose mode it was already written above with the block prefix.
    if !ctx.is_verbose() && !root.stderr.is_empty() {
        let _ = std::io::stderr().write_all(&root.stderr);
    }

    Err(make_execution_error(root.block, &root.lang, &root.status))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    /// Verify that `dup_pipe_stdout` creates a readable copy of the pipe fd:
    /// after the original is consumed via `into_stdio()`, the dup'd fd can
    /// still read the data written to the write end.
    #[cfg(unix)]
    #[test]
    fn test_dup_pipe_stdout_returns_readable_fd() {
        use std::io::Write as _;
        let (pipe_reader, mut pipe_writer) = os_pipe::pipe().expect("pipe creation must succeed");
        let pipe_stdout = PipeStdout::Pipe(pipe_reader);

        // Dup before consuming the original.
        let mut duped = dup_pipe_stdout(&pipe_stdout).expect("dup must succeed");

        // Consume the original into a Stdio (simulates passing to next block's stdin).
        let _stdio = pipe_stdout.into_stdio();

        // Write data to the write end, then close it so the reader sees EOF.
        pipe_writer
            .write_all(b"hello from pipe")
            .expect("write must succeed");
        drop(pipe_writer);

        // The dup'd fd must still be able to read the data.
        let mut buf = Vec::new();
        duped
            .read_to_end(&mut buf)
            .expect("read from dup'd fd must succeed");
        assert_eq!(buf, b"hello from pipe");
    }
}
