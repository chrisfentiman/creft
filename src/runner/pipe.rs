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
}

/// Captured upstream content from a cancelled sponge stage.
///
/// When a sponge reads all upstream input via `read_to_end` and then detects
/// cancellation (upstream exit 99), it sends the buffered content here rather
/// than discarding it. This recovers exit-99 output that the sponge consumed
/// from the kernel pipe buffer before the cancel token could fire.
#[cfg(unix)]
pub(super) struct SpongeCapture {
    /// Index of the sponge block that captured the data.
    block_idx: usize,
    /// Raw upstream bytes the sponge buffered.
    data: Vec<u8>,
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
    /// Sends upstream bytes back to the main thread when the sponge detects
    /// cancellation after buffering. The main thread uses this to recover
    /// exit-99 output the sponge consumed from the kernel pipe buffer.
    pub(super) capture_tx: std::sync::mpsc::Sender<SpongeCapture>,
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
) {
    let SpongeChannels {
        pgid_tx,
        reaper_tx,
        capture_tx,
    } = channels;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
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

        // If the pipeline was cancelled (upstream exit 99), bail without spawning
        // the provider. Send the buffered upstream bytes so the main thread can
        // recover the exit-99 block's output that we consumed from the pipe.
        if ctx.is_cancelled() {
            drop(pipe_writer);
            let _ = capture_tx.send(SpongeCapture {
                block_idx,
                data: raw_upstream.take().unwrap_or_default(),
            });
            if let Some(tx) = pgid_tx {
                let _ = tx.send(Err(()));
            }
            let _ = reaper_tx.send(ReaperResult {
                block_idx,
                lang: block.lang.clone(),
                outcome: BlockOutcome::Cancelled,
            });
            return;
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
                if let Some(tx) = pgid_tx {
                    let _ = tx.send(Err(()));
                }
                let _ = reaper_tx.send(ReaperResult {
                    block_idx,
                    lang: block.lang.clone(),
                    outcome: BlockOutcome::Error(std::io::Error::other(e.to_string())),
                });
                return;
            }
        };

        // prepare_block_script creates a temp file; LLM runners ignore it (prompt
        // is delivered via stdin), but it must exist for the trait signature.
        let tmp = match prepare_block_script(block, &expanded) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("error: sponge block {}: {}", block_idx + 1, e);
                drop(pipe_writer);
                if let Some(tx) = pgid_tx {
                    let _ = tx.send(Err(()));
                }
                let _ = reaper_tx.send(ReaperResult {
                    block_idx,
                    lang: block.lang.clone(),
                    outcome: BlockOutcome::Error(std::io::Error::other(e.to_string())),
                });
                return;
            }
        };
        let runner = super::blocks::runner_for(&block.lang);
        let (mut cmd, _node_deps_dir) = match runner.build_command(block, tmp.path()) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("error: sponge block {}: {}", block_idx + 1, e);
                drop(pipe_writer);
                if let Some(tx) = pgid_tx {
                    let _ = tx.send(Err(()));
                }
                let _ = reaper_tx.send(ReaperResult {
                    block_idx,
                    lang: block.lang.clone(),
                    outcome: BlockOutcome::Error(std::io::Error::other(e.to_string())),
                });
                return;
            }
        };

        // No pre_exec hooks — posix_spawn() compatibility for non-main threads.
        cmd.current_dir(ctx.cwd());
        for (k, v) in ctx.env_pairs() {
            cmd.env(k, v);
        }
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::inherit());

        // Second cancellation check — shrinks the race window to near-zero.
        if ctx.is_cancelled() {
            drop(pipe_writer);
            let _ = capture_tx.send(SpongeCapture {
                block_idx,
                data: raw_upstream.take().unwrap_or_default(),
            });
            if let Some(tx) = pgid_tx {
                let _ = tx.send(Err(()));
            }
            let _ = reaper_tx.send(ReaperResult {
                block_idx,
                lang: block.lang.clone(),
                outcome: BlockOutcome::Cancelled,
            });
            return;
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
                if let Some(tx) = pgid_tx {
                    let _ = tx.send(Err(()));
                }
                let _ = reaper_tx.send(ReaperResult {
                    block_idx,
                    lang: block.lang.clone(),
                    outcome: BlockOutcome::Error(e),
                });
                return;
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

        let status = child.wait();
        let _ = reaper_tx.send(ReaperResult {
            block_idx,
            lang: block.lang.clone(),
            outcome: match status {
                Ok(s) => BlockOutcome::Exited(s),
                Err(e) => BlockOutcome::Error(e),
            },
        });
    }));

    if result.is_err() {
        // Sponge thread panicked — send error to prevent main thread hang on rx.recv().
        let _ = reaper_tx.send(ReaperResult {
            block_idx,
            lang: block.lang.clone(),
            outcome: BlockOutcome::Error(std::io::Error::other("sponge thread panicked")),
        });
    }
}

/// Arguments for `wait_pipe_children_unix`, grouped to stay within clippy's
/// argument count limit.
#[cfg(unix)]
struct WaitArgs {
    children: Vec<(std::process::Child, usize, String)>,
    last_stdout: PipeStdout,
    child_pgid: Option<u32>,
    tx: std::sync::mpsc::Sender<ReaperResult>,
    rx: std::sync::mpsc::Receiver<ReaperResult>,
    last_block_idx: usize,
    exit99_drains: std::collections::HashMap<usize, DupedPipeReader>,
    /// Receives buffered upstream bytes from sponge threads that were cancelled
    /// after buffering. Drained after all reapers finish to recover exit-99 output.
    capture_rx: std::sync::mpsc::Receiver<SpongeCapture>,
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
/// duplicated pipe fd in `exit99_drains` is drained and flushed instead.
#[cfg(unix)]
fn wait_pipe_children_unix(
    args: WaitArgs,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<(Vec<PipeResult>, bool), CreftError> {
    let WaitArgs {
        children,
        last_stdout,
        child_pgid,
        tx,
        rx,
        last_block_idx,
        mut exit99_drains,
        capture_rx,
    } = args;
    // Never writes to the terminal — the main thread decides flush vs. discard.
    let relay_handle = std::thread::Builder::new()
        .name("creft-relay".to_owned())
        .spawn(move || {
            let mut reader = last_stdout;
            let mut buf = [0u8; 8192];
            let mut output: Vec<u8> = Vec::new();
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => output.extend_from_slice(&buf[..n]),
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
            output
        })
        .expect("failed to spawn relay thread");

    for (i, (child, block_idx, lang)) in children.into_iter().enumerate() {
        let tx = tx.clone();
        std::thread::Builder::new()
            .name(format!("creft-reaper-{i}"))
            .spawn(move || {
                let mut child = child;
                let status = child.wait();
                // Ignore send error: main thread dropped rx only if it panicked.
                let _ = tx.send(ReaperResult {
                    block_idx,
                    lang,
                    outcome: match status {
                        Ok(s) => BlockOutcome::Exited(s),
                        Err(e) => BlockOutcome::Error(e),
                    },
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
                    if let Some(pgid) = child_pgid {
                        // SAFETY: kill(-pgid, SIGKILL) is a standard POSIX call.
                        // pgid is valid (obtained from block 0's PID after spawn).
                        // Negative pgid means "all processes in process group pgid".
                        unsafe {
                            libc::kill(-(pgid as libc::pid_t), libc::SIGKILL);
                        }
                    }
                    // Signal sponge threads to bail before spawning.
                    cancel.store(true, std::sync::atomic::Ordering::Relaxed);

                    // Drain the dup'd pipe fd for this block (if it's not the last block).
                    // The exit-99 block has already exited, so its write end is closed and
                    // read_to_end returns promptly with whatever remains in the pipe buffer.
                    // Drain errors are ignored — same behavior as today if drain is absent.
                    if let Some(mut drain_reader) = exit99_drains.remove(&reaper_result.block_idx) {
                        let mut buf = Vec::new();
                        let _ = drain_reader.read_to_end(&mut buf);
                        if !buf.is_empty() {
                            exit99_captured = Some(buf);
                        }
                    }
                }

                results.push(PipeResult {
                    block: reaper_result.block_idx,
                    lang: reaper_result.lang,
                    status,
                });
            }
        }
    }

    // Close all remaining dup'd fds (those not consumed by drain above).
    // These are independent inter-block pipes; dropping them does not affect
    // the relay thread's pipe to the last block.
    drop(exit99_drains);

    // All reapers have exited. Join the relay thread to retrieve the buffered output.
    // unwrap_or_default: relay panic yields empty buffer (no output printed, no crash).
    let relay_buffer = relay_handle.join().unwrap_or_default();

    // Drain sponge capture channel. Non-blocking: all sponge threads have finished
    // by this point (their reaper results have all been received before we exit the
    // rx.recv() loop, and captures are sent before reaper results in the cancel path).
    let mut sponge_captures: Vec<SpongeCapture> = Vec::new();
    while let Ok(cap) = capture_rx.try_recv() {
        sponge_captures.push(cap);
    }

    if early_exit {
        // Find which block exited 99. If none found (shouldn't happen), output nothing.
        let exit99_idx = results
            .iter()
            .find(|r| exit_code_of(&r.status) == Some(EARLY_EXIT))
            .map(|r| r.block);

        // Select output source by priority:
        // 1. Exit-99 block is last → relay buffer has its output
        // 2. Dup'd fd drain → kernel pipe buffer residue (downstream didn't consume it)
        // 3. Sponge capture → downstream sponge consumed the data before cancel fired
        //
        // Sources 2 and 3 are mutually exclusive in practice: if a sponge consumed
        // the pipe data, the dup'd fd is empty, and vice versa.
        let output: &[u8] = match exit99_idx {
            Some(idx) if idx == last_block_idx => &relay_buffer,
            Some(idx) => {
                // Middle or first block. Try dup'd fd drain first, then sponge capture.
                exit99_captured
                    .as_deref()
                    .filter(|d| !d.is_empty())
                    .or_else(|| {
                        sponge_captures
                            .iter()
                            .find(|c| c.block_idx > idx && !c.data.is_empty())
                            .map(|c| c.data.as_slice())
                    })
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
        let status = child.wait().map_err(CreftError::Io)?;

        if exit_code_of(&status) == Some(EARLY_EXIT) {
            for remaining in children.iter_mut().skip(i + 1) {
                if let Some((mut c, _, _)) = remaining.take() {
                    let _ = c.kill();
                    let _ = c.wait();
                }
            }
            early_exit = true;
            results.push(PipeResult {
                block: block_idx,
                lang,
                status,
            });
            break;
        }

        results.push(PipeResult {
            block: block_idx,
            lang,
            status,
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
    let mut children: Vec<(std::process::Child, usize, String)> = Vec::with_capacity(n);
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

    // Sponge capture channel: sponge threads send buffered upstream bytes here
    // when cancelled, so the main thread can recover exit-99 output the sponge
    // consumed from the kernel pipe buffer. Main thread's sender is dropped
    // before wait_pipe_children_unix so the channel closes when all sponges finish.
    #[cfg(unix)]
    let (capture_tx, capture_rx) = std::sync::mpsc::channel::<SpongeCapture>();

    // Join handles for sponge threads — joined after all reaper results collected.
    #[cfg(unix)]
    let mut sponge_handles: Vec<std::thread::JoinHandle<()>> = Vec::new();

    // Dup'd read ends of inter-block pipes, keyed by block index.
    // Only middle blocks (not the last) get entries. The last block's output
    // is captured by the relay thread; duping it would create a competing reader.
    #[cfg(unix)]
    let mut exit99_drains: std::collections::HashMap<usize, DupedPipeReader> =
        std::collections::HashMap::new();

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

            let owned_block = block.clone();
            let owned_bound_refs: Vec<(String, String)> = bound_refs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            let ctx_clone = ctx.clone();
            let reaper_tx_clone = reaper_tx.clone();
            let capture_tx_clone = capture_tx.clone();

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
                            capture_tx: capture_tx_clone,
                        },
                    );
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
                    if let Some(pgid) = child_pgid {
                        // SAFETY: kill(-pgid, SIGKILL) is a standard POSIX call.
                        // pgid is valid (obtained from block 0's PID after spawn).
                        unsafe {
                            libc::kill(-(pgid as libc::pid_t), libc::SIGKILL);
                        }
                    }
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
            if let Some(pgid) = child_pgid {
                // SAFETY: kill(-pgid, SIGKILL) is a standard POSIX call.
                // pgid is valid (we got it from child.id() which is always non-zero).
                unsafe {
                    libc::kill(-(pgid as libc::pid_t), libc::SIGKILL);
                }
            }
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

        if !is_last {
            let stdout = child.stdout.take();
            if stdout.is_none() {
                // Stdio::piped() must always yield a ChildStdout — this path is unreachable
                // under normal conditions, but guard against it to avoid a silent hang.
                #[cfg(unix)]
                if let Some(pgid) = child_pgid {
                    // SAFETY: kill(-pgid, SIGKILL) is a standard POSIX call.
                    // pgid is valid (we got it from child.id() which is always non-zero).
                    unsafe {
                        libc::kill(-(pgid as libc::pid_t), libc::SIGKILL);
                    }
                }
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
                    if let Some(pgid) = child_pgid {
                        // SAFETY: kill(-pgid, SIGKILL) is a standard POSIX call.
                        // pgid is valid (we got it from child.id() which is always non-zero).
                        unsafe {
                            libc::kill(-(pgid as libc::pid_t), libc::SIGKILL);
                        }
                    }
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

        children.push((child, i, block.lang.clone()));
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
        // Drop the main-thread tx clones so channels close when all senders
        // (sponge + reaper threads) drop their own clones.
        let tx = reaper_tx;
        let rx = reaper_rx;
        // Main thread doesn't send captures — drop its sender so the channel closes
        // when all sponge threads finish.
        drop(capture_tx);
        let outcome = wait_pipe_children_unix(
            WaitArgs {
                children,
                last_stdout,
                child_pgid,
                tx,
                rx,
                last_block_idx: n - 1,
                exit99_drains,
                capture_rx,
            },
            ctx.cancel_token(),
        )?;
        // Join sponge threads — they finished before sending their reaper results.
        for handle in sponge_handles {
            let _ = handle.join();
        }
        outcome
    };

    #[cfg(not(unix))]
    let (results, early_exit) = {
        let children_opt: Vec<Option<(std::process::Child, usize, String)>> =
            children.into_iter().map(Some).collect();
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

    /// Verify the SpongeCapture channel round-trips block index and raw bytes correctly.
    #[cfg(unix)]
    #[test]
    fn test_sponge_capture_channel_send_receive() {
        let (tx, rx) = std::sync::mpsc::channel::<SpongeCapture>();
        let data = b"captured-by-sponge\n".to_vec();
        tx.send(SpongeCapture {
            block_idx: 1,
            data: data.clone(),
        })
        .expect("send must succeed");
        drop(tx);
        let cap = rx.recv().expect("recv must succeed");
        assert_eq!(cap.block_idx, 1);
        assert_eq!(cap.data, data);
        // Channel should be empty after the single item.
        assert!(rx.try_recv().is_err());
    }
}
