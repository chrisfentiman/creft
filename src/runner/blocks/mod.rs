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
mod shell;
mod stdin;

/// Whether a block is dispatched via file mode (temp script + path arg) or
/// stdin mode (no useful temp file; source piped to the child's stdin).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExecutionMode {
    File,
    Stdin,
}

/// Trait for language-specific block command building.
///
/// Each implementation knows how to construct a `Command` for its language
/// family. The shared scaffolding (cwd, env, stdin/stdout, process group setup)
/// lives in `spawn_block`; `BlockRunner::build_command` only handles the
/// language-specific `Command` construction.
pub(super) trait BlockRunner {
    /// Build a `Command` for the given block. Does NOT spawn it.
    ///
    /// `script_path` is always provided: in file mode it is the temp script
    /// the runner appends as its final argument; in stdin mode the runner
    /// ignores it (the body is delivered to the child's stdin by the
    /// caller). `flags` is the already-expanded, already-tokenised
    /// `# flags:` directive (empty when the directive is absent).
    ///
    /// Returns the Command and an optional TempDir that must outlive the
    /// child (used by `NodeRunner` for npm-installed `node_modules`).
    fn build_command(
        &self,
        block: &CodeBlock,
        script_path: &Path,
        flags: &[String],
    ) -> Result<(std::process::Command, Option<tempfile::TempDir>), CreftError>;
}

/// Pick the runner for a block based on its lang tag and directives.
///
/// `ShellRunner` is the generic file-mode interpreter runner: it serves
/// shell family (`bash`/`sh`/`zsh`), typescript family (`typescript`/`ts`),
/// and unknown tags that opted into file mode via `# extension:`. The
/// "shell" name is retained for source-tree continuity; the doc comment
/// on the struct documents its actual role.
pub(super) fn runner_for(block: &CodeBlock) -> Box<dyn BlockRunner> {
    match block.lang.as_str() {
        "bash" | "sh" | "zsh" | "typescript" | "ts" => return Box::new(shell::ShellRunner),
        "python" | "python3" => return Box::new(python::PythonRunner),
        "node" | "javascript" | "js" => return Box::new(node::NodeRunner),
        "llm" => return Box::new(llm::LlmRunner),
        _ => {}
    }
    // Unknown tag: file mode when the author set `# extension:`, stdin mode otherwise.
    if block.extension.is_some() {
        Box::new(shell::ShellRunner)
    } else {
        Box::new(stdin::StdinRunner)
    }
}

/// The execution mode for a block.
pub(crate) fn execution_mode_for(block: &CodeBlock) -> ExecutionMode {
    if crate::model::is_known_family(&block.lang) {
        return ExecutionMode::File;
    }
    if block.extension.is_some() {
        ExecutionMode::File
    } else {
        ExecutionMode::Stdin
    }
}

/// Expand `{{var}}` placeholders in a block's `# flags:` directive and
/// tokenise the result on whitespace.
///
/// The flags string is substituted with shell-escaping OFF regardless of the
/// block's language, because the tokens become discrete `Command::arg`
/// arguments (not shell source). Returns an empty vector when the directive
/// is absent.
///
/// `bound_refs` is `&[(&str, &str)]` to match the existing `substitute`
/// signature. Single-block `execute_block` already holds a slice in this
/// shape; the sponge thread owns a cloned `Vec<(String, String)>` for
/// thread-boundary reasons and builds a borrow view at the call site
/// (`refs.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect()`).
/// Making the signature generic over owned-vs-borrowed would be over-engineered
/// for two callers.
pub(crate) fn expand_and_split_flags(
    block: &CodeBlock,
    bound_refs: &[(&str, &str)],
) -> Result<Vec<String>, CreftError> {
    let raw = match &block.flags {
        Some(s) => s.as_str(),
        None => return Ok(Vec::new()),
    };
    // The literal tag "flags" is opaque to `should_shell_escape`, so values
    // pass through unescaped. This is deliberate: tokens are arguments, not
    // shell source.
    let expanded = super::substitute(raw, bound_refs, "flags")?;
    Ok(expanded.split_whitespace().map(str::to_string).collect())
}

/// Warn (on stderr) when a `# deps:` entry is not on PATH.
///
/// Shared by `ShellRunner` (which covers shell family, typescript, and
/// unknown-tag file mode) and `StdinRunner` — every runner whose block has
/// free-form `# deps:` entries that name binaries the user must have
/// installed. Family runners with package-manager-based deps (Python via
/// `uv`, Node via `npm`) use their own paths and do not call this helper.
pub(super) fn warn_missing_deps(block: &CodeBlock) {
    for dep in &block.deps {
        if super::which(dep).is_none() {
            eprintln!("warning: '{}' not found on PATH", dep);
        }
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
// spawn_block has 9 parameters by design: the first 6 are platform-independent
// and the last 3 are unix-only (process_group, ignore_sigint, side_channel).
// On non-unix targets the function has only 6 parameters and does not trigger
// this lint. Splitting the function would scatter the shared spawn scaffolding
// that the spec intentionally concentrates here.
#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_block(
    block: &CodeBlock,
    flags: &[String],
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

    let runner = runner_for(block);
    let (mut cmd, node_deps_dir) = runner.build_command(block, script_path, flags)?;

    cmd.current_dir(cwd);
    for (k, v) in &env_pairs {
        cmd.env(k, v);
    }
    // Advertise the side channel fd numbers as env vars so languages without
    // a preamble (or power users) can open the fds manually.
    #[cfg(unix)]
    if side_channel.is_some() {
        cmd.env("CREFT_CONTROL_FD", CONTROL_FD.to_string());
        cmd.env("CREFT_RESPONSE_FD", RESPONSE_FD.to_string());
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use crate::model::{CodeBlock, LlmConfig};

    use super::{ExecutionMode, execution_mode_for, expand_and_split_flags, runner_for};

    fn make_block(lang: &str) -> CodeBlock {
        CodeBlock {
            lang: lang.to_string(),
            code: String::new(),
            deps: vec![],
            extension: None,
            flags: None,
            llm_config: None,
            llm_parse_error: None,
        }
    }

    fn make_block_with_extension(lang: &str, ext: &str) -> CodeBlock {
        CodeBlock {
            lang: lang.to_string(),
            code: String::new(),
            deps: vec![],
            extension: Some(ext.to_string()),
            flags: None,
            llm_config: None,
            llm_parse_error: None,
        }
    }

    fn make_block_with_flags(lang: &str, flags_str: &str) -> CodeBlock {
        CodeBlock {
            lang: lang.to_string(),
            code: String::new(),
            deps: vec![],
            extension: None,
            flags: Some(flags_str.to_string()),
            llm_config: None,
            llm_parse_error: None,
        }
    }

    fn make_llm_block(provider: &str) -> CodeBlock {
        CodeBlock {
            lang: "llm".to_string(),
            code: String::new(),
            deps: vec![],
            extension: None,
            flags: None,
            llm_config: Some(LlmConfig {
                provider: provider.to_string(),
                model: String::new(),
                params: String::new(),
            }),
            llm_parse_error: None,
        }
    }

    /// Calls `runner_for(block)`, then `build_command` with a minimal block and
    /// a dummy path. Returns the command's program as a String.
    fn program_for(lang: &str) -> String {
        let block = make_block(lang);
        let runner = runner_for(&block);
        let script = Path::new("/tmp/test_script");
        let (cmd, _) = runner.build_command(&block, script, &[]).unwrap();
        cmd.get_program().to_str().unwrap().to_string()
    }

    /// Known-family langs route through their own runners; unknown tags without
    /// `# extension:` route through StdinRunner (program == lang tag).
    #[rstest]
    #[case::bash("bash", "bash")]
    #[case::sh("sh", "sh")]
    #[case::zsh("zsh", "zsh")]
    #[case::python("python", "python3")]
    #[case::python3("python3", "python3")]
    #[case::node("node", "node")]
    #[case::javascript("javascript", "node")]
    #[case::js("js", "node")]
    #[case::unknown_stdin_mode("mylangtag", "mylangtag")]
    fn runner_for_dispatches_to_expected_program(#[case] lang: &str, #[case] expected: &str) {
        assert_eq!(program_for(lang), expected);
    }

    #[test]
    fn runner_for_llm_uses_provider_as_program() {
        let block = make_llm_block("claude");
        let runner = runner_for(&block);
        let script = Path::new("/tmp/test_script");
        let (cmd, _) = runner.build_command(&block, script, &[]).unwrap();
        assert_eq!(cmd.get_program().to_str().unwrap(), "claude");
    }

    /// Unknown tag without extension → StdinRunner: script path NOT in args.
    #[test]
    fn runner_for_unknown_no_extension_is_stdin_runner() {
        let block = make_block("ruby");
        let runner = runner_for(&block);
        let script = Path::new("/tmp/creft-XXXXX.ruby");
        let (cmd, _) = runner.build_command(&block, script, &[]).unwrap();
        assert_eq!(cmd.get_program(), "ruby");
        let args: Vec<_> = cmd.get_args().collect();
        assert!(
            args.is_empty(),
            "StdinRunner must not include script path in args; got: {args:?}"
        );
    }

    /// Unknown tag with `# extension:` → ShellRunner: script path IS in args.
    #[test]
    fn runner_for_unknown_with_extension_is_shell_runner() {
        let block = make_block_with_extension("ruby", "rb");
        let runner = runner_for(&block);
        let script = Path::new("/tmp/creft-XXXXX.rb");
        let (cmd, _) = runner.build_command(&block, script, &[]).unwrap();
        assert_eq!(cmd.get_program(), "ruby");
        let args: Vec<_> = cmd.get_args().map(|a| a.to_str().unwrap()).collect();
        assert_eq!(
            args.last().copied(),
            Some("/tmp/creft-XXXXX.rb"),
            "ShellRunner must append script path as last arg"
        );
    }

    /// typescript is in the file-mode set → ShellRunner regardless of directives.
    #[test]
    fn runner_for_typescript_is_shell_runner() {
        let block = make_block("typescript");
        let runner = runner_for(&block);
        let script = Path::new("/tmp/creft-XXXXX.ts");
        let (cmd, _) = runner.build_command(&block, script, &[]).unwrap();
        // npx tsx → program is "npx"
        assert_eq!(cmd.get_program(), "npx");
        let args: Vec<_> = cmd.get_args().map(|a| a.to_str().unwrap()).collect();
        assert_eq!(
            args.last().copied(),
            Some("/tmp/creft-XXXXX.ts"),
            "typescript must append script path as last arg"
        );
    }

    /// Cartesian product: (known family / unknown) × (with extension / without).
    #[rstest]
    #[case::bash_no_ext("bash", None, ExecutionMode::File)]
    #[case::bash_with_ext("bash", Some("zip"), ExecutionMode::File)]
    #[case::python_no_ext("python", None, ExecutionMode::File)]
    #[case::python_with_ext("python", Some("pyi"), ExecutionMode::File)]
    #[case::node_no_ext("node", None, ExecutionMode::File)]
    #[case::typescript_no_ext("typescript", None, ExecutionMode::File)]
    #[case::llm_no_ext("llm", None, ExecutionMode::File)]
    #[case::unknown_no_ext("ruby", None, ExecutionMode::Stdin)]
    #[case::unknown_with_ext("ruby", Some("rb"), ExecutionMode::File)]
    #[case::zx_no_ext("zx", None, ExecutionMode::Stdin)]
    #[case::zx_with_ext("zx", Some("mjs"), ExecutionMode::File)]
    fn execution_mode_for_expected(
        #[case] lang: &str,
        #[case] ext: Option<&str>,
        #[case] expected: ExecutionMode,
    ) {
        let block = CodeBlock {
            lang: lang.to_string(),
            code: String::new(),
            deps: vec![],
            extension: ext.map(str::to_string),
            flags: None,
            llm_config: None,
            llm_parse_error: None,
        };
        assert_eq!(execution_mode_for(&block), expected);
    }

    #[test]
    fn expand_and_split_flags_none_returns_empty() {
        let block = make_block("ruby");
        let result = expand_and_split_flags(&block, &[]).unwrap();
        assert_eq!(result, Vec::<String>::new());
    }

    #[test]
    fn expand_and_split_flags_single_flag() {
        let block = make_block_with_flags("bash", "-x");
        let result = expand_and_split_flags(&block, &[]).unwrap();
        assert_eq!(result, ["-x"]);
    }

    #[test]
    fn expand_and_split_flags_multiple_flags() {
        let block = make_block_with_flags("ruby", "-e --no-pager");
        let result = expand_and_split_flags(&block, &[]).unwrap();
        assert_eq!(result, ["-e", "--no-pager"]);
    }

    /// Placeholder expansion without shell-escaping: "hello world" yields two tokens.
    #[test]
    fn expand_and_split_flags_placeholder_expands_and_splits_without_shell_escaping() {
        let block = make_block_with_flags("bash", "{{name}}");
        let refs: &[(&str, &str)] = &[("name", "hello world")];
        let result = expand_and_split_flags(&block, refs).unwrap();
        // Two tokens because split_whitespace splits on the space in "hello world".
        // Shell-escaping would produce "'hello world'" (one token); the absence
        // of escaping gives two, which is the documented behaviour for flags.
        assert_eq!(result, ["hello", "world"]);
    }

    /// Unbound optional placeholder collapses to empty via |default.
    #[test]
    fn expand_and_split_flags_missing_optional_placeholder_returns_empty() {
        let block = make_block_with_flags("bash", "{{maybe|}}");
        let result = expand_and_split_flags(&block, &[]).unwrap();
        assert_eq!(result, Vec::<String>::new());
    }

    /// runs_via_stdin is true exactly for unknown tags without extension.
    #[rstest]
    #[case::bash_no_ext("bash", None, false)]
    #[case::ruby_no_ext("ruby", None, true)]
    #[case::ruby_with_ext("ruby", Some("rb"), false)]
    #[case::llm_no_ext("llm", None, false)]
    #[case::zx_no_ext("zx", None, true)]
    fn runs_via_stdin_matches_expectation(
        #[case] lang: &str,
        #[case] ext: Option<&str>,
        #[case] expected: bool,
    ) {
        let block = CodeBlock {
            lang: lang.to_string(),
            code: String::new(),
            deps: vec![],
            extension: ext.map(str::to_string),
            flags: None,
            llm_config: None,
            llm_parse_error: None,
        };
        assert_eq!(block.runs_via_stdin(), expected);
    }

    /// needs_sponge covers LLM blocks AND stdin-mode blocks.
    #[rstest]
    #[case::llm("llm", None, true)]
    #[case::ruby_stdin("ruby", None, true)]
    #[case::ruby_file("ruby", Some("rb"), false)]
    #[case::bash("bash", None, false)]
    fn needs_sponge_matches_expectation(
        #[case] lang: &str,
        #[case] ext: Option<&str>,
        #[case] expected: bool,
    ) {
        let block = CodeBlock {
            lang: lang.to_string(),
            code: String::new(),
            deps: vec![],
            extension: ext.map(str::to_string),
            flags: None,
            llm_config: None,
            llm_parse_error: None,
        };
        assert_eq!(block.needs_sponge(), expected);
    }
}
