use std::path::Path;

use crate::error::CreftError;
use crate::model::CodeBlock;
use crate::shell as detect_shell;

use super::RunContext;
#[cfg(unix)]
use super::channel::{CONTROL_FD, RESPONSE_FD, SideChannel};

mod llm;
mod node;
mod python;
mod ruby;
mod shell;

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
        "llm" => Box::new(llm::LlmRunner),
        // Unknown language: fall back to ShellRunner which uses interpreter()
        // to resolve the command name (returns the lang tag verbatim for unknowns).
        _ => Box::new(shell::ShellRunner),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use crate::model::{CodeBlock, LlmConfig};

    use super::runner_for;

    fn make_block(lang: &str) -> CodeBlock {
        CodeBlock {
            lang: lang.to_string(),
            code: String::new(),
            deps: vec![],
            llm_config: None,
            llm_parse_error: None,
        }
    }

    fn make_llm_block(provider: &str) -> CodeBlock {
        CodeBlock {
            lang: "llm".to_string(),
            code: String::new(),
            deps: vec![],
            llm_config: Some(LlmConfig {
                provider: provider.to_string(),
                model: String::new(),
                params: String::new(),
            }),
            llm_parse_error: None,
        }
    }

    /// Calls `runner_for(lang)`, then `build_command` with a minimal block and
    /// a dummy path. Returns the command's program as a String.
    fn program_for(lang: &str) -> String {
        let runner = runner_for(lang);
        let block = make_block(lang);
        let script = Path::new("/tmp/test_script");
        let (cmd, _) = runner.build_command(&block, script).unwrap();
        cmd.get_program().to_str().unwrap().to_string()
    }

    #[rstest]
    #[case::bash("bash", "bash")]
    #[case::sh("sh", "sh")]
    #[case::zsh("zsh", "zsh")]
    #[case::python("python", "python3")]
    #[case::python3("python3", "python3")]
    #[case::node("node", "node")]
    #[case::javascript("javascript", "node")]
    #[case::js("js", "node")]
    #[case::ruby("ruby", "ruby")]
    #[case::rb("rb", "ruby")]
    #[case::unknown("mylangtag", "mylangtag")]
    fn runner_for_dispatches_to_expected_program(#[case] lang: &str, #[case] expected: &str) {
        assert_eq!(program_for(lang), expected);
    }

    #[test]
    fn runner_for_llm_uses_provider_as_program() {
        let runner = runner_for("llm");
        let block = make_llm_block("claude");
        let script = Path::new("/tmp/test_script");
        let (cmd, _) = runner.build_command(&block, script).unwrap();
        assert_eq!(cmd.get_program().to_str().unwrap(), "claude");
    }
}

/// Spawn a child process for a code block.
///
/// Delegates language-specific `Command` construction to the appropriate
/// `BlockRunner`, then applies shared configuration: cwd, env, stdio, and
/// on Unix, process group setup and SIGINT handling.
///
/// `stdin_cfg` and `stdout_cfg` control the stdio configuration.
/// stderr is always piped so child process output does not contaminate the
/// terminal. On failure the caller should surface `child.stderr`; on success
/// it should discard it.
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
// spawn_block has 8 parameters by design: the first 5 are platform-independent
// and the last 3 are unix-only (process_group, ignore_sigint, side_channel).
// On non-unix targets the function has only 5 parameters and does not trigger
// this lint. Splitting the function would scatter the shared spawn scaffolding
// that the spec intentionally concentrates here.
#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_block(
    block: &CodeBlock,
    script_path: &Path,
    ctx: &RunContext,
    stdin_cfg: std::process::Stdio,
    stdout_cfg: std::process::Stdio,
    #[cfg(unix)] process_group: Option<u32>,
    #[cfg(unix)] ignore_sigint: bool,
    #[cfg(unix)] side_channel: Option<&SideChannel>,
) -> Result<(std::process::Child, Option<tempfile::TempDir>), CreftError> {
    let env_pairs = ctx.env_pairs();
    let cwd = ctx.cwd();

    // Resolve shell preference: if the block's language is in the shell family
    // and the user's preferred shell is also in the shell family, substitute it.
    // This lets a zsh user run bash-tagged blocks under zsh, and vice versa.
    let resolved_block: CodeBlock;
    let block = if let Some(resolved_lang) =
        detect_shell::resolve_shell(&block.lang, ctx.shell_preference())
    {
        resolved_block = CodeBlock {
            lang: resolved_lang.to_string(),
            ..block.clone()
        };
        &resolved_block
    } else {
        block
    };

    let runner = runner_for(&block.lang);
    let (mut cmd, node_deps_dir) = runner.build_command(block, script_path)?;

    cmd.current_dir(cwd);
    for (k, v) in &env_pairs {
        cmd.env(k, v);
    }
    cmd.stdin(stdin_cfg);
    cmd.stdout(stdout_cfg);
    cmd.stderr(std::process::Stdio::piped());

    // Both operations must happen between fork() and exec() — exactly when
    // pre_exec() runs. setpgid(2), signal(2), dup2(2), and close(2) are all
    // async-signal-safe (POSIX-required for use in the fork-exec window).
    #[cfg(unix)]
    {
        // Extract raw fd values before entering the closure. The closure cannot
        // capture OwnedFd (non-Copy), so we capture i32 (Copy) directly.
        let side_channel_fds: Option<(i32, i32)> = side_channel.map(|ch| ch.child_fds());

        let need_pre_exec = process_group.is_some() || ignore_sigint || side_channel_fds.is_some();
        if need_pre_exec {
            use std::os::unix::process::CommandExt;
            // SAFETY: setpgid(0, pgid), signal(SIGINT, SIG_IGN), dup2, and
            // close are all async-signal-safe POSIX calls valid in the
            // fork-exec window. No Rust allocations or mutex operations occur.
            // All captured values (pgid via Option<u32>, bools, i32 fd values)
            // are Copy. The fd values were extracted from SideChannel before
            // this closure was registered; the parent still holds the OwnedFds,
            // keeping them valid across the fork.
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
                    if let Some((ctrl_write_fd, resp_read_fd)) = side_channel_fds {
                        // dup2 the control pipe write end to fd 3.
                        if libc::dup2(ctrl_write_fd, CONTROL_FD) == -1 {
                            return Err(std::io::Error::last_os_error());
                        }
                        if ctrl_write_fd != CONTROL_FD {
                            libc::close(ctrl_write_fd);
                        }
                        // dup2 the response pipe read end to fd 4.
                        if libc::dup2(resp_read_fd, RESPONSE_FD) == -1 {
                            return Err(std::io::Error::last_os_error());
                        }
                        if resp_read_fd != RESPONSE_FD {
                            libc::close(resp_read_fd);
                        }
                    }
                    Ok(())
                });
            }
        }
    }

    // Build a descriptive interpreter name for error messages.
    // For LLM blocks, name the provider CLI. For deps-based blocks, name the
    // package manager (uv/npm) since that is what actually needs to be on PATH.
    let interp_name = if block.lang == "llm" {
        let provider = block
            .llm_config
            .as_ref()
            .map(|c| {
                if c.provider.is_empty() {
                    "claude"
                } else {
                    c.provider.as_str()
                }
            })
            .unwrap_or("claude");
        format!("'{}' (LLM provider CLI)", provider)
    } else if !block.deps.is_empty() {
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
