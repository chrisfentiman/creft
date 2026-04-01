use std::io::{Read as _, Write as _};

use crate::error::CreftError;
use crate::model::{CodeBlock, LlmConfig};

use super::super::substitute::substitute;
use super::super::{EARLY_EXIT, RunContext, exit_code_of, make_execution_error};

#[cfg(unix)]
use super::super::pipe::PipeStdout;
#[cfg(unix)]
use super::super::pipe::ReaperResult;

/// Build a `Command` for the given LLM provider.
///
/// Does NOT configure stdin/stdout/stderr or cwd — the caller does that.
/// Does NOT set env vars — the caller does that.
///
/// Provider patterns:
/// - `claude`: `claude -p [--model <model>]`
/// - `gemini`: `gemini -p [-m <model>]`
/// - `codex`: `codex exec -`
/// - `ollama`: `ollama run [<model>]`
/// - unknown: `<provider> [--model <model>]`
///
/// `params` is split on whitespace and appended as individual arguments.
pub(crate) fn build_llm_command(config: &LlmConfig) -> std::process::Command {
    let provider = if config.provider.is_empty() {
        "claude"
    } else {
        &config.provider
    };

    let mut cmd = match provider {
        "claude" => {
            let mut c = std::process::Command::new("claude");
            c.arg("-p");
            if !config.model.is_empty() {
                c.arg("--model").arg(&config.model);
            }
            c
        }
        "gemini" => {
            let mut c = std::process::Command::new("gemini");
            c.arg("-p");
            if !config.model.is_empty() {
                c.arg("-m").arg(&config.model);
            }
            c
        }
        "codex" => {
            let mut c = std::process::Command::new("codex");
            c.arg("exec").arg("-");
            // codex does not take a model flag in exec mode
            c
        }
        "ollama" => {
            let mut c = std::process::Command::new("ollama");
            c.arg("run");
            if !config.model.is_empty() {
                c.arg(&config.model);
            }
            c
        }
        unknown => {
            let mut c = std::process::Command::new(unknown);
            if !config.model.is_empty() {
                c.arg("--model").arg(&config.model);
            }
            c
        }
    };

    if !config.params.is_empty() {
        for token in config.params.split_whitespace() {
            cmd.arg(token);
        }
    }

    cmd
}

/// Execute an LLM block by piping the prompt to the provider CLI.
///
/// Returns captured stdout as a `String`. Output is also printed to the terminal.
pub(crate) fn execute_llm_block(
    block: &CodeBlock,
    prompt: &str,
    block_idx: usize,
    ctx: &RunContext,
) -> Result<String, CreftError> {
    // Check cancellation before spawning the provider — avoids starting a
    // potentially long-running LLM call when SIGINT already fired.
    if ctx.is_cancelled() {
        return Err(CreftError::EarlyExit);
    }

    let config = block
        .llm_config
        .as_ref()
        .expect("execute_llm_block called on block without llm_config; validation must gate this");

    let provider = if config.provider.is_empty() {
        "claude"
    } else {
        config.provider.as_str()
    };

    let mut cmd = build_llm_command(config);
    cmd.current_dir(ctx.cwd());
    for (k, v) in ctx.env_pairs() {
        cmd.env(k, v);
    }
    cmd.stdin(std::process::Stdio::piped());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::inherit());

    let mut child = cmd.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            CreftError::LlmProviderNotFound(format!(
                "'{}' not found on PATH. Install the provider CLI and ensure it is in your PATH.",
                provider
            ))
        } else {
            CreftError::Io(e)
        }
    })?;

    // Write prompt to stdin, then drop to close it.
    // LLM providers read the full prompt before producing output, so this is safe.
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(prompt.as_bytes()).map_err(CreftError::Io)?;
    }

    let output = child.wait_with_output().map_err(CreftError::Io)?;

    if exit_code_of(&output.status) == Some(EARLY_EXIT) {
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        print!("{}", stdout);
        return Err(CreftError::EarlyExit);
    }

    if !output.status.success() {
        return Err(make_execution_error(block_idx, &block.lang, &output.status));
    }

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    print!("{}", stdout);

    Ok(stdout)
}

/// Sponge stage for an LLM block in a pipe chain.
///
/// Reads all upstream output (the "sponge"), performs template substitution with
/// the buffered content as `{{prev}}`, spawns the LLM provider, and relays the
/// provider's output to `pipe_writer`. Sends the provider's exit status through
/// `reaper_tx` for the main thread to collect.
///
/// Runs on a dedicated thread (`creft-sponge-N`), owning the entire LLM
/// provider lifecycle. From the pipe chain's perspective, the sponge is just
/// another stage that produces output on a pipe fd.
///
/// When `upstream` is `None` (block 0), the sponge reads from the parent's
/// stdin. When block 0 is a sponge, `pgid_tx` carries the provider's PID
/// back to `run_pipe_chain` so subsequent non-sponge blocks can join the
/// process group.
///
/// No `pre_exec` hooks are used. Sponge spawns use `posix_spawn()` (no
/// `fork()`) to avoid EPERM failures from non-main threads in CI
/// environments. The provider is not placed in the pipe chain's process
/// group and does not have SIGINT ignored. The sponge thread manages the
/// provider's lifecycle directly via pipe ownership and `child.wait()`.
#[cfg(unix)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn sponge_thread(
    upstream: Option<PipeStdout>,
    pipe_writer: os_pipe::PipeWriter,
    prompt_template: String,
    config: crate::model::LlmConfig,
    bound_refs: Vec<(String, String)>,
    ctx: RunContext,
    block_idx: usize,
    pgid_tx: Option<std::sync::mpsc::SyncSender<Result<u32, ()>>>,
    reaper_tx: std::sync::mpsc::Sender<ReaperResult>,
) {
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

        // Template substitution with buffered content as {{prev}}.
        let ref_pairs: Vec<(&str, &str)> = bound_refs
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .chain(std::iter::once(("prev", trimmed.as_str())))
            .collect();
        let prompt = match substitute(&prompt_template, &ref_pairs, "llm") {
            Ok(p) => p,
            Err(e) => {
                eprintln!("error: sponge block {}: {}", block_idx + 1, e);
                drop(pipe_writer);
                if let Some(tx) = pgid_tx {
                    let _ = tx.send(Err(()));
                }
                let _ = reaper_tx.send(ReaperResult {
                    block_idx,
                    lang: "llm".to_string(),
                    status: Err(std::io::Error::other(e.to_string())),
                });
                return;
            }
        };

        let provider = if config.provider.is_empty() {
            "claude"
        } else {
            config.provider.as_str()
        };

        let mut cmd = build_llm_command(&config);
        cmd.current_dir(ctx.cwd());
        for (k, v) in ctx.env_pairs() {
            cmd.env(k, v);
        }
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::inherit());

        // No pre_exec hooks on sponge spawns. pre_exec forces Rust's Command
        // to use fork()+exec() instead of posix_spawn(). fork() from non-main
        // threads fails with EPERM in CI environments (GitHub Actions macOS
        // launchd sessions, Ubuntu containers). Without pre_exec, posix_spawn()
        // is used, which works reliably from any thread.
        //
        // Consequences of removing pre_exec:
        // - No setpgid: provider is not in the pipe chain's process group.
        //   exit 99 cleanup via killpg won't reach it, but the provider dies
        //   naturally when pipes close. The sponge thread owns the Child handle.
        // - No SIG_IGN: provider receives SIGINT on Ctrl+C. The sponge thread
        //   sees stdout close, stops relaying, and reports the exit status.
        //   The only user-visible effect is the provider may print an error to
        //   stderr before dying.

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                let msg = if e.kind() == std::io::ErrorKind::NotFound {
                    format!(
                        "'{}' not found on PATH. Install the provider CLI.",
                        provider
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
                    lang: "llm".to_string(),
                    status: Err(e),
                });
                return;
            }
        };

        // If this sponge created the process group, report the provider's PID as pgid.
        if let Some(tx) = pgid_tx {
            let _ = tx.send(Ok(child.id()));
        }

        if let Some(mut stdin) = child.stdin.take() {
            // Ignore BrokenPipe — provider's exit status is the authoritative error signal.
            let _ = stdin.write_all(prompt.as_bytes());
        }

        let mut pipe_writer = pipe_writer;
        if let Some(mut stdout) = child.stdout.take() {
            let mut buf = [0u8; 8192];
            loop {
                match stdout.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        // During pipe execution PipeSignalGuard overwrites the signal-hook
                        // handler, so this check fires primarily for single-block LLM runs
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
            lang: "llm".to_string(),
            status,
        });
    }));

    if result.is_err() {
        // Sponge thread panicked — send error to prevent main thread hang on rx.recv().
        let _ = reaper_tx.send(ReaperResult {
            block_idx,
            lang: "llm".to_string(),
            status: Err(std::io::Error::other("sponge thread panicked")),
        });
    }
}
