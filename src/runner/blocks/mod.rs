use std::path::Path;

use crate::error::CreftError;
use crate::model::CodeBlock;

use super::RunContext;

mod llm;
mod node;
mod python;
mod ruby;
mod shell;

#[cfg(test)]
pub(crate) use llm::build_llm_command;
pub(crate) use llm::execute_llm_block;
#[cfg(unix)]
pub(super) use llm::sponge_thread;

/// Trait for language-specific block command building.
///
/// Each implementation knows how to construct a `Command` for its language
/// family. The shared scaffolding (cwd, env, stdin/stdout, process group setup)
/// lives in `spawn_block`; `BlockRunner::build_command` only handles the
/// language-specific `Command` construction.
pub(super) trait BlockRunner {
    /// Build a `Command` for the given block. Does NOT spawn it.
    ///
    /// `script_path` is the temp file containing the expanded code.
    ///
    /// Returns the Command and an optional TempDir that must be kept alive
    /// until the child exits (used by NodeRunner for npm-installed node_modules).
    fn build_command(
        &self,
        block: &CodeBlock,
        script_path: &Path,
    ) -> Result<(std::process::Command, Option<tempfile::TempDir>), CreftError>;
}

/// Return the appropriate runner for a language tag.
pub(super) fn runner_for(lang: &str) -> Box<dyn BlockRunner> {
    match lang {
        "bash" | "sh" | "zsh" => Box::new(shell::ShellRunner),
        "python" | "python3" => Box::new(python::PythonRunner),
        "node" | "javascript" | "js" => Box::new(node::NodeRunner),
        "ruby" | "rb" => Box::new(ruby::RubyRunner),
        // LLM blocks do not go through spawn_block — they use sponge_thread or
        // execute_llm_block directly. The fallback here covers any unknown
        // language by using the interpreter() name as the command (shell behavior).
        _ => Box::new(shell::ShellRunner),
    }
}

/// Spawn a child process for a code block.
///
/// Delegates language-specific `Command` construction to the appropriate
/// `BlockRunner`, then applies shared configuration: cwd, env, stdio, and
/// on Unix, process group setup and SIGINT handling.
///
/// `stdin_cfg` and `stdout_cfg` control the stdio configuration.
/// stderr is always inherited.
///
/// `process_group`: Unix-only parameter. When `Some(pgid)`, the child is
/// placed into the specified process group via `setpgid(0, pgid)` in a
/// `pre_exec` hook. Pass `Some(0)` for the first pipe-chain child (creates
/// a new group using the child's own PID). Pass `Some(first_child_pid)` for
/// subsequent children (joins the first child's group). Pass `None` for
/// sequential (non-pipe) execution — no process group changes.
///
/// `ignore_sigint`: Unix-only. When `true`, the child process will have
/// SIGINT set to `SIG_IGN` before exec. Use for non-first blocks in a pipe
/// chain: only the first block receives Ctrl+C; downstream blocks learn
/// the pipe broke via EOF/SIGPIPE and exit cleanly. `SIG_IGN` is inherited
/// across exec, so the spawned interpreter (e.g. Python) will also ignore it.
pub(crate) fn spawn_block(
    block: &CodeBlock,
    script_path: &Path,
    ctx: &RunContext,
    stdin_cfg: std::process::Stdio,
    stdout_cfg: std::process::Stdio,
    #[cfg(unix)] process_group: Option<u32>,
    #[cfg(unix)] ignore_sigint: bool,
) -> Result<(std::process::Child, Option<tempfile::TempDir>), CreftError> {
    let env_pairs = ctx.env_pairs();
    let cwd = ctx.cwd();

    let runner = runner_for(&block.lang);
    let (mut cmd, node_deps_dir) = runner.build_command(block, script_path)?;

    cmd.current_dir(cwd);
    for (k, v) in &env_pairs {
        cmd.env(k, v);
    }
    cmd.stdin(stdin_cfg);
    cmd.stdout(stdout_cfg);
    cmd.stderr(std::process::Stdio::inherit());

    // Both operations must happen between fork() and exec() — exactly when
    // pre_exec() runs. setpgid(2) and signal(2) are both async-signal-safe.
    #[cfg(unix)]
    {
        let need_pre_exec = process_group.is_some() || ignore_sigint;
        if need_pre_exec {
            use std::os::unix::process::CommandExt;
            // SAFETY: Both setpgid(0, pgid) and signal(SIGINT, SIG_IGN) are
            // async-signal-safe (POSIX-required for use in the fork-exec window).
            // No Rust allocations or mutex operations inside pre_exec. Captured
            // values (pgid via Option<u32>, ignore_sigint via bool) are Copy.
            unsafe {
                cmd.pre_exec(move || {
                    if let Some(pgid) = process_group {
                        // pgid=0: use child's own PID as the new process group ID.
                        // pgid=N: join existing process group N.
                        if libc::setpgid(0, pgid as libc::pid_t) == -1 {
                            return Err(std::io::Error::last_os_error());
                        }
                    }
                    if ignore_sigint {
                        // SIG_IGN is inherited across exec(2). This means the
                        // spawned interpreter (e.g. Python) will also ignore
                        // SIGINT, preventing spurious tracebacks when the pipe
                        // head dies from Ctrl+C and EOF propagates downstream.
                        libc::signal(libc::SIGINT, libc::SIG_IGN);
                    }
                    Ok(())
                });
            }
        }
    }

    // Build a descriptive interpreter name for error messages.
    // For deps-based blocks, name the package manager (uv/npm) since that
    // is what actually needs to be on PATH.
    let interp_name = if !block.deps.is_empty() {
        match block.lang.as_str() {
            "python" | "python3" => {
                "uv (install with: curl -LsSf https://astral.sh/uv/install.sh | sh)".to_string()
            }
            "node" | "javascript" | "js" => "npm (install Node.js)".to_string(),
            _ => super::interpreter(&block.lang).to_string(),
        }
    } else {
        super::interpreter(&block.lang).to_string()
    };

    let child = cmd.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            CreftError::InterpreterNotFound(format!("{interp_name}. Run 'creft doctor' to check."))
        } else {
            // E2BIG (large env) and other OS errors get actionable messages.
            crate::error::enrich_io_error(e, "environment")
        }
    })?;

    Ok((child, node_deps_dir))
}
