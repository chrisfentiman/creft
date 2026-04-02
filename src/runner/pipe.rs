use std::io::Read as _;
use std::io::Write as _;

use crate::error::CreftError;
use crate::model::{CodeBlock, ParsedCommand};

use super::blocks::spawn_block;
use super::substitute::substitute;
use super::{EARLY_EXIT, RunContext, exit_code_of, make_execution_error, prepare_block_script};

#[cfg(unix)]
use super::signal::PipeSignalGuard;

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

/// Result from a single reaper thread (Unix pipe mode).
#[cfg(unix)]
pub(crate) struct ReaperResult {
    pub(crate) block_idx: usize,
    pub(crate) lang: String,
    pub(crate) status: Result<std::process::ExitStatus, std::io::Error>,
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
/// chains. The check is correct and will become the primary cancellation path
/// when `PipeSignalGuard` is eventually replaced.
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
    let SpongeChannels { pgid_tx, reaper_tx } = channels;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let buffered = match upstream {
            Some(mut reader) => {
                let mut buf = Vec::new();
                // Ignore read errors — if upstream crashed, reaper catches its exit status.
                let _ = reader.read_to_end(&mut buf);
                String::from_utf8_lossy(&buf).to_string()
            }
            None => {
                // Block 0: read from parent stdin.
                let mut buf = Vec::new();
                let _ = std::io::stdin().lock().read_to_end(&mut buf);
                String::from_utf8_lossy(&buf).to_string()
            }
        };
        let trimmed = buffered.trim_end().to_string();

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
                    status: Err(std::io::Error::other(e.to_string())),
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
                    status: Err(std::io::Error::other(e.to_string())),
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
                    status: Err(std::io::Error::other(e.to_string())),
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
                    status: Err(e),
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
                        // During pipe execution PipeSignalGuard overwrites the signal-hook
                        // handler, so this check fires primarily for single-block runs
                        // and between sequential blocks. It will become the primary
                        // cancellation path when PipeSignalGuard is replaced in a future phase.
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
            status,
        });
    }));

    if result.is_err() {
        // Sponge thread panicked — send error to prevent main thread hang on rx.recv().
        let _ = reaper_tx.send(ReaperResult {
            block_idx,
            lang: block.lang.clone(),
            status: Err(std::io::Error::other("sponge thread panicked")),
        });
    }
}

/// Wait for all children in a Unix pipe chain using concurrent reaper threads
/// and a buffered stdout relay.
///
/// Each child is moved into its own reaper thread that calls `child.wait()` and
/// sends the result through an mpsc channel. Results arrive in exit order, not
/// spawn order. The last block's stdout is relayed into a buffer by a dedicated
/// relay thread; output is only flushed to the terminal after all reapers have
/// reported and no exit 99 was detected.
///
/// The `tx`/`rx` channel pair is created by the caller (`run_pipe_chain`) so
/// that sponge threads can also send results through the same channel before
/// this function is called. The caller must drop its own `tx` clone before
/// calling this function so the channel closes when all reaper and sponge
/// threads finish.
///
/// This design guarantees zero leakage: if any block exits 99, the relay buffer
/// is discarded without ever writing to the terminal.
#[cfg(unix)]
fn wait_pipe_children_unix(
    children: Vec<(std::process::Child, usize, String)>,
    last_stdout: PipeStdout,
    child_pgid: Option<u32>,
    tx: std::sync::mpsc::Sender<ReaperResult>,
    rx: std::sync::mpsc::Receiver<ReaperResult>,
) -> Result<(Vec<PipeResult>, bool), CreftError> {
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
                    status,
                });
            })
            .expect("failed to spawn reaper thread");
    }
    // Drop the tx clone passed in from run_pipe_chain. Combined with sponge threads
    // dropping their clones and reaper threads dropping theirs, rx closes when all done.
    drop(tx);

    let mut results: Vec<PipeResult> = Vec::new();
    let mut early_exit = false;

    while let Ok(reaper_result) = rx.recv() {
        let status = reaper_result.status.map_err(CreftError::Io)?;

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
        }

        results.push(PipeResult {
            block: reaper_result.block_idx,
            lang: reaper_result.lang,
            status,
        });
    }

    // All reapers have exited. Join the relay thread to retrieve the buffered output.
    // unwrap_or_default: relay panic yields empty buffer (no output printed, no crash).
    let relay_buffer = relay_handle.join().unwrap_or_default();

    if !early_exit {
        let stdout = std::io::stdout();
        let mut lock = stdout.lock();
        // Ignore write errors: creft's own stdout may be a broken pipe.
        let _ = lock.write_all(&relay_buffer);
    }
    // If early_exit is true, relay_buffer drops here without writing. Zero leakage.

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

    // Join handles for sponge threads — joined after all reaper results collected.
    #[cfg(unix)]
    let mut sponge_handles: Vec<std::thread::JoinHandle<()>> = Vec::new();

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
                prev_stdout = Some(PipeStdout::Pipe(pipe_reader));
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
            prev_stdout = stdout.map(PipeStdout::Child);
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
        // Drop the main-thread tx clone so rx closes when all senders (sponge + reaper) drop.
        let tx = reaper_tx;
        let rx = reaper_rx;
        let outcome = wait_pipe_children_unix(children, last_stdout, child_pgid, tx, rx)?;
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
