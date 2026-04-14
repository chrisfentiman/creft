use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::error::CreftError;
use crate::model::{CodeBlock, LlmConfig, ParsedCommand};

mod blocks;
#[cfg(unix)]
pub(crate) mod channel;
mod pipe;
mod preamble;
#[cfg(unix)]
mod signal;
mod substitute;

pub(crate) use self::blocks::spawn_block;
pub(crate) use self::substitute::substitute;

/// Execution context for a single skill invocation.
///
/// Carries all state that runner functions previously received as individual
/// parameters: working directory, environment variables, runtime flags, and
/// a cancellation token for cooperative shutdown.
///
/// Constructed once per skill invocation in `run_user_command`.
/// Shared across threads via `Arc` (sponge threads, reaper threads).
#[derive(Debug, Clone)]
pub(crate) struct RunContext {
    /// Cancellation token. Set to `true` by the SIGINT handler.
    /// Threads poll this to know when to stop.
    cancel: Arc<AtomicBool>,

    /// Working directory for subprocess execution.
    cwd: std::path::PathBuf,

    /// Extra environment variables injected into every child process.
    env: Vec<(String, String)>,

    /// Whether `--verbose` was passed. Controls block rendering before execution.
    verbose: bool,

    /// Whether `--dry-run` was passed. Controls execution vs. print-only.
    dry_run: bool,

    /// Caller's preferred shell (e.g., "zsh", "bash").
    ///
    /// When set, shell-family blocks (`bash`, `sh`, `zsh`) use this shell
    /// instead of the block's declared language tag. Set from shell detection
    /// at skill invocation time. `None` means use block language literally.
    shell_preference: Option<String>,
}

impl RunContext {
    pub(crate) fn new(
        cancel: Arc<AtomicBool>,
        cwd: std::path::PathBuf,
        env: Vec<(String, String)>,
        verbose: bool,
        dry_run: bool,
    ) -> Self {
        Self {
            cancel,
            cwd,
            env,
            verbose,
            dry_run,
            shell_preference: None,
        }
    }

    /// Set the caller's shell preference.
    ///
    /// When `Some(shell)`, shell-family code blocks run under the given shell
    /// rather than the shell named in the block's language tag.
    pub(crate) fn with_shell_preference(mut self, preference: Option<String>) -> Self {
        self.shell_preference = preference;
        self
    }

    /// Borrow the shell preference, if one was detected.
    pub(crate) fn shell_preference(&self) -> Option<&str> {
        self.shell_preference.as_deref()
    }

    /// Check whether cancellation has been requested.
    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// Borrow the working directory.
    pub(crate) fn cwd(&self) -> &std::path::Path {
        &self.cwd
    }

    /// Borrow the environment variables as a slice of string pairs.
    pub(crate) fn env_pairs(&self) -> Vec<(&str, &str)> {
        self.env
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect()
    }

    /// Whether verbose output was requested.
    pub(crate) fn is_verbose(&self) -> bool {
        self.verbose
    }

    /// Whether dry-run mode was requested.
    pub(crate) fn is_dry_run(&self) -> bool {
        self.dry_run
    }

    /// Borrow the environment variable slice for cloning or inspection.
    pub(crate) fn env(&self) -> &[(String, String)] {
        &self.env
    }

    /// Request cancellation. Sets the shared cancel token to `true`.
    ///
    /// Safe to call from any thread. All clones of this context will observe
    /// the cancellation on the next `is_cancelled()` poll.
    pub(crate) fn request_cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// Clone the underlying cancel Arc for passing to threads that require
    /// `'static` lifetime (e.g. relay threads in pipe chains).
    pub(super) fn cancel_arc(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.cancel)
    }
}

/// Exit code that signals early successful return — skip remaining blocks.
///
/// A block that exits 99 is treated as a successful early termination of
/// the pipeline. creft intercepts this code and returns 0 to the caller.
/// All other non-zero exit codes propagate as errors.
pub(crate) const EARLY_EXIT: i32 = 99;

/// Bound pairs (name → value) and any passthrough args.
pub type BindResult = (Vec<(String, String)>, Vec<String>);

/// Parse raw CLI args using lexopt, validate values, and apply frontmatter defaults.
///
/// Returns a vec of `(name, value)` pairs for template substitution plus any
/// passthrough args (currently always empty — all args must match the definition).
/// Unknown flags produce a parse error; there is no silent passthrough.
pub fn parse_and_bind(cmd: &ParsedCommand, raw_args: &[String]) -> Result<BindResult, CreftError> {
    use lexopt::prelude::*;

    // Raw parsed values collected during the lexopt loop.
    // Flags are keyed by their long name.
    let mut flag_values: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    // Tracks whether a bool flag was set (present = true).
    let mut flag_bools: std::collections::HashSet<String> = std::collections::HashSet::new();
    // Positional values in arrival order.
    let mut positional: Vec<String> = Vec::new();

    // Build a lookup from short char → flag name so we can handle -v, -q, etc.
    let short_to_flag: std::collections::HashMap<char, &str> = cmd
        .def
        .flags
        .iter()
        .filter_map(|f| {
            f.short
                .as_deref()
                .and_then(|s| s.chars().next())
                .map(|ch| (ch, f.name.as_str()))
        })
        .collect();

    let mut parser = lexopt::Parser::from_args(raw_args.iter().map(String::as_str));

    while let Some(arg) = parser
        .next()
        .map_err(|e| CreftError::MissingArg(e.to_string()))?
    {
        match arg {
            Long(name) => {
                // Convert to owned so we can call parser.value() without
                // conflicting borrows from the Long(&str) lifetime.
                let name = name.to_owned();
                if let Some(flag_def) = cmd.def.flags.iter().find(|f| f.name == name) {
                    if flag_def.r#type == "bool" {
                        flag_bools.insert(flag_def.name.clone());
                    } else {
                        let err_name = name.clone();
                        let val = parser
                            .value()
                            .map_err(|e| CreftError::MissingArg(e.to_string()))?
                            .into_string()
                            .map_err(|_| {
                                CreftError::MissingArg(format!(
                                    "--{err_name}: value contains invalid UTF-8"
                                ))
                            })?;
                        flag_values.insert(flag_def.name.clone(), val);
                    }
                } else {
                    return Err(CreftError::MissingArg(format!(
                        "unexpected option --{name}"
                    )));
                }
            }
            Short(ch) => {
                if let Some(&flag_name) = short_to_flag.get(&ch) {
                    let flag_def = cmd
                        .def
                        .flags
                        .iter()
                        .find(|f| f.name == flag_name)
                        .expect("short_to_flag built from cmd.def.flags; name must exist");
                    if flag_def.r#type == "bool" {
                        flag_bools.insert(flag_def.name.clone());
                    } else {
                        let val = parser
                            .value()
                            .map_err(|e| CreftError::MissingArg(e.to_string()))?
                            .into_string()
                            .map_err(|_| {
                                CreftError::MissingArg(format!(
                                    "-{ch}: value contains invalid UTF-8"
                                ))
                            })?;
                        flag_values.insert(flag_def.name.clone(), val);
                    }
                } else {
                    return Err(CreftError::MissingArg(format!("unexpected option -{ch}")));
                }
            }
            Value(v) => {
                let s = v.into_string().map_err(|_| {
                    CreftError::MissingArg("positional argument contains invalid UTF-8".to_string())
                })?;
                positional.push(s);
            }
        }
    }

    let mut pairs: Vec<(String, String)> = Vec::new();

    // Bind positional args in declaration order.
    for (i, arg_def) in cmd.def.args.iter().enumerate() {
        let val = if let Some(v) = positional.get(i) {
            v.clone()
        } else if let Some(d) = &arg_def.default {
            d.clone()
        } else if !arg_def.required {
            // Optional arg with no explicit default — bind to empty string so
            // {{name}} resolves to "" rather than erroring. Users who want a
            // non-empty fallback can still use {{name|fallback}} in the template.
            String::new()
        } else {
            return Err(CreftError::MissingArg(arg_def.name.clone()));
        };

        // Regex validation — skip when optional arg was not provided (value is empty).
        if let Some(pattern) = &arg_def.validation
            && (!val.is_empty() || arg_def.required)
        {
            let re = regex::Regex::new(pattern).map_err(|e| {
                CreftError::Frontmatter(format!("invalid validation regex '{}': {}", pattern, e))
            })?;
            if !re.is_match(&val) {
                return Err(CreftError::ValidationFailed {
                    name: arg_def.name.clone(),
                    value: val,
                    pattern: pattern.clone(),
                });
            }
        }

        pairs.push((arg_def.name.clone(), val));
    }

    // Bind flags in declaration order.
    for flag_def in &cmd.def.flags {
        let val = if flag_def.r#type == "bool" {
            flag_bools.contains(&flag_def.name).to_string()
        } else if let Some(v) = flag_values.get(&flag_def.name) {
            v.clone()
        } else if let Some(d) = &flag_def.default {
            d.clone()
        } else {
            // String flag with no default and not provided — bind to empty
            // string so {{flagname}} resolves to "" rather than erroring.
            String::new()
        };

        // Regex validation for string flags — skip when empty (not provided).
        if flag_def.r#type != "bool"
            && let Some(pattern) = &flag_def.validation
            && !val.is_empty()
        {
            let re = regex::Regex::new(pattern).map_err(|e| {
                CreftError::Frontmatter(format!("invalid validation regex '{}': {}", pattern, e))
            })?;
            if !re.is_match(&val) {
                return Err(CreftError::ValidationFailed {
                    name: flag_def.name.clone(),
                    value: val,
                    pattern: pattern.clone(),
                });
            }
        }

        pairs.push((flag_def.name.clone(), val));
    }

    Ok((pairs, vec![]))
}

/// Check that required env vars are set.
pub fn check_env(cmd: &ParsedCommand) -> Result<(), CreftError> {
    for var in &cmd.def.env {
        if var.required && std::env::var(&var.name).is_err() {
            return Err(CreftError::MissingEnvVar(var.name.clone()));
        }
    }
    Ok(())
}

/// Map a code block language tag to an interpreter command.
pub(crate) fn interpreter(lang: &str) -> &str {
    match lang {
        "bash" => "bash",
        "sh" => "sh",
        "zsh" => "zsh",
        "python" => "python3",
        "python3" => "python3",
        "node" | "javascript" | "js" => "node",
        "typescript" | "ts" => "npx tsx",
        "perl" => "perl",
        other => other,
    }
}

/// File extension for a language.
pub(crate) fn extension(lang: &str) -> &str {
    match lang {
        "bash" | "sh" | "zsh" => "sh",
        "python" | "python3" => "py",
        "node" | "javascript" | "js" => "js",
        "typescript" | "ts" => "ts",
        "perl" => "pl",
        other => other,
    }
}

/// Create a temporary script file for a code block.
///
/// When `preamble` is `Some`, the preamble is written before `expanded_code`.
/// If `expanded_code` starts with a shebang (`#!`), the shebang line is
/// written first and the preamble follows it — so the kernel's script loader
/// sees the interpreter directive at byte 0.
///
/// Returns the temp file handle (must be kept alive until the child exits).
pub(crate) fn prepare_block_script(
    block: &CodeBlock,
    expanded_code: &str,
    preamble: Option<&str>,
) -> Result<tempfile::NamedTempFile, CreftError> {
    let ext = extension(&block.lang);
    let mut tmp = tempfile::Builder::new()
        .prefix("creft-")
        .suffix(&format!(".{}", ext))
        .tempfile()
        .map_err(CreftError::Io)?;

    if let Some(pre) = preamble {
        // If the script opens with a shebang, the kernel requires it at
        // byte 0. Split the shebang off, write it, then inject the preamble
        // before the rest of the user code.
        if expanded_code.starts_with("#!") {
            let (shebang_line, rest) = expanded_code
                .split_once('\n')
                .unwrap_or((expanded_code, ""));
            tmp.write_all(shebang_line.as_bytes())
                .map_err(CreftError::Io)?;
            tmp.write_all(b"\n").map_err(CreftError::Io)?;
            tmp.write_all(pre.as_bytes()).map_err(CreftError::Io)?;
            tmp.write_all(rest.as_bytes()).map_err(CreftError::Io)?;
        } else {
            tmp.write_all(pre.as_bytes()).map_err(CreftError::Io)?;
            tmp.write_all(expanded_code.as_bytes())
                .map_err(CreftError::Io)?;
        }
    } else {
        tmp.write_all(expanded_code.as_bytes())
            .map_err(CreftError::Io)?;
    }

    tmp.flush().map_err(CreftError::Io)?;

    Ok(tmp)
}

/// Simple which(1) equivalent.
pub(crate) fn which(name: &str) -> Option<std::path::PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|p| p.join(name))
            .find(|p| p.exists())
    })
}

/// Return the plain exit code for a process status, or `None` if the process
/// was killed by a signal (Unix) or terminated abnormally (Windows).
///
/// On Unix this corresponds to `ExitStatusExt::code()`: `Some(n)` for a
/// voluntary exit, `None` for a signal kill.
pub(crate) fn exit_code_of(status: &std::process::ExitStatus) -> Option<i32> {
    status.code()
}

/// Build the appropriate `CreftError` for a failed child process.
///
/// On Unix, if the process was killed by a signal (`ExitStatus::code()` is
/// `None`), returns `ExecutionSignaled` with the signal number. Otherwise
/// returns `ExecutionFailed` with the exit code.
pub(crate) fn make_execution_error(
    block: usize,
    lang: &str,
    status: &std::process::ExitStatus,
) -> CreftError {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return CreftError::ExecutionSignaled {
                block,
                lang: lang.to_string(),
                signal: sig,
            };
        }
    }
    CreftError::ExecutionFailed {
        block,
        lang: lang.to_string(),
        code: status.code().unwrap_or(1),
    }
}

/// Format the command that `build_llm_command` would produce as a display string.
///
/// Used by dry-run and verbose output.
fn format_llm_command(config: &LlmConfig) -> String {
    let provider = if config.provider.is_empty() {
        "claude"
    } else {
        &config.provider
    };

    let mut parts: Vec<String> = vec![provider.to_string()];

    match provider {
        "claude" => {
            parts.push("-p".to_string());
            if !config.model.is_empty() {
                parts.push("--model".to_string());
                parts.push(config.model.clone());
            }
        }
        "gemini" => {
            parts.push("-p".to_string());
            if !config.model.is_empty() {
                parts.push("-m".to_string());
                parts.push(config.model.clone());
            }
        }
        "codex" => {
            parts.push("exec".to_string());
            parts.push("-".to_string());
        }
        "ollama" => {
            parts.push("run".to_string());
            if !config.model.is_empty() {
                parts.push(config.model.clone());
            }
        }
        _ => {
            if !config.model.is_empty() {
                parts.push("--model".to_string());
                parts.push(config.model.clone());
            }
        }
    }

    if !config.params.is_empty() {
        for token in config.params.split_whitespace() {
            parts.push(token.to_string());
        }
    }

    parts.join(" ")
}

/// Execute a single code block, capturing and echoing stdout.
///
/// Returns captured stdout as a `String`. Output is also printed to the
/// terminal so the user sees it in real time.
///
/// When `stdin_data` is `Some`, the child's stdin is piped and the data is
/// written on a background thread before `wait_with_output` drains stdout.
/// The background thread prevents the classic deadlock where a large stdin
/// payload fills the OS pipe buffer while the child is also blocked writing
/// to a full stdout pipe buffer. When `stdin_data` is `None`, the child
/// inherits the parent's stdin (terminal or upstream process).
fn execute_block(
    block: &CodeBlock,
    code: &str,
    block_idx: usize,
    ctx: &RunContext,
    stdin_data: Option<&[u8]>,
) -> Result<String, CreftError> {
    // Check cancellation before spawning any block — avoids starting a
    // potentially long-running process when SIGINT already fired.
    if ctx.is_cancelled() {
        return Err(CreftError::EarlyExit);
    }

    let pre = preamble::for_language(&block.lang);
    let tmp = prepare_block_script(block, code, pre.as_deref())?;
    let tmp_path = tmp.path().to_path_buf();

    let stdin_cfg = if stdin_data.is_some() {
        std::process::Stdio::piped()
    } else {
        std::process::Stdio::inherit()
    };

    // Create the side channel before spawning so the child's pipe ends are
    // ready for inheritance via pre_exec/dup2.
    #[cfg(unix)]
    let mut side_channel = channel::SideChannel::new().map_err(CreftError::Io)?;

    let (mut child, _node_deps_dir) = spawn_block(
        block,
        &tmp_path,
        ctx,
        stdin_cfg,
        std::process::Stdio::piped(),
        #[cfg(unix)]
        None, // single-block mode: no process group management
        #[cfg(unix)]
        false, // single-block mode: do not suppress SIGINT
        #[cfg(unix)]
        Some(&side_channel),
    )?;

    // After spawn, close the parent's copies of the child's pipe ends.
    // If the parent keeps them open, the control-pipe reader will never
    // see EOF (the parent's write end keeps the pipe alive).
    #[cfg(unix)]
    side_channel.close_child_ends();

    // Spawn the side-channel reader thread that parses NDJSON from fd 3
    // and renders print/status messages to the terminal via stderr.
    // Runs concurrently with stdout capture so neither pipe blocks.
    //
    // Interactive prompts are only valid when the child's stdin is the
    // terminal (stdin_data is None). If stdin is piped, creft is using
    // the child's stdin for data — interactive reading from the same
    // terminal is not possible.
    #[cfg(unix)]
    let (reader_thread, prompt_thread, exit_signal) = {
        let ctrl_reader = side_channel
            .take_control_reader()
            .expect("control reader taken exactly once per block");
        let resp_writer_fd = side_channel
            .take_response_writer()
            .expect("response writer taken exactly once per block");
        let writer = std::sync::Arc::new(channel::TerminalWriter::new());

        // Set up a channel so the reader thread can forward prompt messages
        // to the prompt handler thread.
        let (prompt_tx, prompt_rx) = std::sync::mpsc::channel::<channel::ChannelMessage>();

        // Interactive prompts require an unpiped stdin (terminal access).
        let interactive = stdin_data.is_none();

        let exit_signal: channel::ExitSignal =
            Arc::new(std::sync::Mutex::new(None));
        let reader = channel::spawn_reader(
            ctrl_reader,
            std::sync::Arc::clone(&writer),
            Some(prompt_tx),
            // The reader does not write responses directly — the prompt
            // handler thread owns the response writer.
            None,
            Some(Arc::clone(&exit_signal)),
        );
        let prompter =
            channel::spawn_prompt_handler(prompt_rx, resp_writer_fd, writer, interactive);
        (reader, prompter, exit_signal)
    };

    // When prev_output data must be written to the child's stdin, do it on a
    // background thread so that stdout draining (via wait_with_output) and
    // stdin writes proceed concurrently. This prevents deadlock when
    // prev_output is large enough to fill the OS pipe buffer (~64 KB on Linux,
    // ~16 KB on macOS) before the child has read any of it.
    let stdin_thread = if let Some(data) = stdin_data {
        let owned: Vec<u8> = data.to_vec();
        let mut stdin_handle = child
            .stdin
            .take()
            .expect("stdin was piped but handle is missing");
        Some(std::thread::spawn(move || {
            use std::io::ErrorKind;
            match std::io::Write::write_all(&mut stdin_handle, &owned) {
                Ok(()) => Ok(()),
                // BrokenPipe means the child exited before reading all input.
                // The child's exit status is the authoritative error signal.
                Err(e) if e.kind() == ErrorKind::BrokenPipe => Ok(()),
                Err(e) => Err(e),
            }
        }))
    } else {
        None
    };

    // _node_deps_dir kept alive here so the npm-installed node_modules directory
    // is not deleted before the child process finishes.
    let output = child.wait_with_output().map_err(CreftError::Io)?;

    if let Some(handle) = stdin_thread {
        handle
            .join()
            .expect("stdin write thread panicked")
            .map_err(CreftError::Io)?;
    }

    // Join the reader thread after the child has exited and stdout has been
    // drained. The child's exit closes its fd 3 write end; close_child_ends()
    // above closed the parent's copy, so the reader will have seen EOF.
    // When the reader exits it drops prompt_tx, which closes the mpsc channel
    // and causes the prompt handler thread to exit.
    // Joining here ensures all side-channel messages are rendered before
    // the function returns, and that the exit_signal slot is final before
    // we read it below.
    #[cfg(unix)]
    {
        reader_thread
            .join()
            .expect("side-channel reader thread panicked");
        // Join the prompt handler after the reader: the reader exits first
        // (closes prompt_tx), which signals the prompt handler to finish.
        prompt_thread
            .join()
            .expect("side-channel prompt handler thread panicked");
    }

    // Check the creft_exit side-channel signal before the exit-99 check.
    // creft_exit causes the block to exit 0, so exit_code_of returns Some(0),
    // not Some(99). There is no ambiguity between the two paths.
    #[cfg(unix)]
    {
        let signal_code = exit_signal
            .lock()
            .expect("exit signal lock poisoned")
            .take();
        if let Some(code) = signal_code {
            // Flush any stdout the block produced before creft_exit.
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            print!("{stdout}");
            if code == 0 {
                return Err(CreftError::EarlyExit);
            } else {
                return Err(CreftError::ExecutionFailed {
                    block: block_idx,
                    lang: block.lang.clone(),
                    code,
                });
            }
        }
    }

    if exit_code_of(&output.status) == Some(EARLY_EXIT) {
        eprintln!("warning: exit 99 is deprecated, use creft_exit instead");
        // Print any output produced before the early exit so it is not lost.
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        print!("{}", stdout);
        if ctx.is_verbose() && !output.stderr.is_empty() {
            let _ = writeln!(std::io::stderr(), "[block {} stderr]", block_idx + 1);
            let _ = std::io::stderr().write_all(&output.stderr);
        }
        return Err(CreftError::EarlyExit);
    }

    if !output.status.success() {
        // Suppress child stderr when killed by SIGINT: the user pressed Ctrl+C
        // deliberately and the interpreter's traceback (e.g., Python's
        // KeyboardInterrupt) is noise, not diagnostics.
        #[cfg(unix)]
        let suppress_stderr = {
            use std::os::unix::process::ExitStatusExt;
            output.status.signal() == Some(libc::SIGINT)
        };
        #[cfg(not(unix))]
        let suppress_stderr = false;

        if !suppress_stderr && !output.stderr.is_empty() {
            let _ = std::io::stderr().write_all(&output.stderr);
        }
        return Err(make_execution_error(block_idx, &block.lang, &output.status));
    }

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    print!("{}", stdout);

    if ctx.is_verbose() && !output.stderr.is_empty() {
        let _ = writeln!(std::io::stderr(), "[block {} stderr]", block_idx + 1);
        let _ = std::io::stderr().write_all(&output.stderr);
    }

    Ok(stdout)
}

/// Convert bound pairs from `parse_and_bind` into environment variable pairs.
///
/// Names are uppercased and hyphens are replaced with underscores so that
/// shell-invalid identifiers like `always-confirm` become valid env var names
/// (`ALWAYS_CONFIRM`). The original names in `bound` are left unchanged for
/// template substitution; only the env var keys are transformed here.
fn bound_pairs_to_env(pairs: &[(String, String)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(name, val)| {
            let env_name = name.replace('-', "_").to_uppercase();
            (env_name, val.clone())
        })
        .collect()
}

/// Core execution logic. Uses `RunContext` for all execution configuration.
fn run_inner(cmd: &ParsedCommand, raw_args: &[String], ctx: &RunContext) -> Result<(), CreftError> {
    if cmd.blocks.is_empty() {
        return Err(CreftError::NoCodeBlocks);
    }

    check_env(cmd)?;

    let (bound, _passthrough) = parse_and_bind(cmd, raw_args)?;
    let bound_refs: Vec<(&str, &str)> = bound
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();

    // Inject bound args and flags as environment variables into every child
    // process. Names are normalized (uppercase, hyphens → underscores) so
    // shell authors can write $FORMAT instead of only {{format}}.
    let mut extended_env = ctx.env().to_vec();
    extended_env.extend(bound_pairs_to_env(&bound));
    let ctx = RunContext::new(
        ctx.cancel_arc(),
        ctx.cwd().to_path_buf(),
        extended_env,
        ctx.is_verbose(),
        ctx.is_dry_run(),
    );

    if cmd.blocks.len() > 1 {
        #[cfg(unix)]
        {
            return pipe::run_pipe_chain(cmd, &bound_refs, &ctx);
        }
        #[cfg(not(unix))]
        {
            let has_llm_blocks = cmd.blocks.iter().any(|b| b.lang == "llm");
            if has_llm_blocks {
                return Err(CreftError::Setup(
                    "Multi-block skills with LLM blocks require Unix (macOS/Linux). \
                     LLM pipe stages use process groups which are not available on this platform."
                        .into(),
                ));
            }
            return pipe::run_pipe_chain(cmd, &bound_refs, &ctx);
        }
    }

    let block = &cmd.blocks[0];
    let expanded = substitute(&block.code, &bound_refs, &block.lang)?;

    // Sponge blocks (LLM) receive their expanded content via stdin.
    // Script-based blocks read from the temp file; stdin_data is None.
    let stdin_data = if block.needs_sponge() {
        Some(expanded.as_bytes())
    } else {
        None
    };

    match execute_block(block, &expanded, 0, &ctx, stdin_data) {
        Ok(_) => Ok(()),
        Err(CreftError::EarlyExit) => Ok(()),
        Err(e) => Err(e),
    }
}

/// Run a full parsed command using a `RunContext`.
///
/// Returns immediately if cancellation has already been requested.
pub(crate) fn run(
    cmd: &ParsedCommand,
    raw_args: &[String],
    ctx: &RunContext,
) -> Result<(), CreftError> {
    if ctx.is_cancelled() {
        return Ok(());
    }
    run_inner(cmd, raw_args, ctx)
}

/// Print the expanded code for each block without executing it, using a `RunContext`.
pub(crate) fn dry_run(
    cmd: &ParsedCommand,
    raw_args: &[String],
    ctx: &RunContext,
) -> Result<(), CreftError> {
    dry_run_inner(cmd, raw_args, ctx.cwd())
}

/// Write rendered (substituted) blocks to stderr for diagnostic inspection.
///
/// Called when `--verbose` is active. Each block is shown with `===` delimiters
/// so the output is visually distinct from `--dry-run`'s `---` format.
pub fn render_blocks(cmd: &ParsedCommand, bound_refs: &[(&str, &str)]) -> Result<(), CreftError> {
    for (i, block) in cmd.blocks.iter().enumerate() {
        let expanded = substitute(&block.code, bound_refs, &block.lang)?;
        if block.lang == "llm" {
            if let Some(config) = &block.llm_config {
                let command_str = format_llm_command(config);
                eprintln!("=== block {} (llm: {}) ===", i + 1, config.provider);
                eprintln!("command: {}", command_str);
                eprintln!("prompt:");
                eprintln!("{}", expanded);
                eprintln!("=== end ===");
            } else {
                eprintln!("=== block {} (llm) ===", i + 1);
                eprintln!("{}", expanded);
                eprintln!("=== end ===");
            }
        } else {
            eprintln!("=== block {} ({}) ===", i + 1, block.lang);
            eprintln!("{}", expanded);
            eprintln!("=== end ===");
        }
    }
    Ok(())
}

/// Print the expanded code for each block without executing it.
fn dry_run_inner(cmd: &ParsedCommand, raw_args: &[String], cwd: &Path) -> Result<(), CreftError> {
    check_env(cmd)?;
    let (bound, _passthrough) = parse_and_bind(cmd, raw_args)?;
    let bound_refs: Vec<(&str, &str)> = bound
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();

    eprintln!("cwd: {}", cwd.display());

    for (i, block) in cmd.blocks.iter().enumerate() {
        let expanded = substitute(&block.code, &bound_refs, &block.lang)?;
        if block.lang == "llm" {
            let provider = block
                .llm_config
                .as_ref()
                .map(|c| c.provider.as_str())
                .unwrap_or("claude");
            eprintln!("--- block {} (llm: {}) ---", i + 1, provider);
            if let Some(config) = &block.llm_config {
                let command_str = format_llm_command(config);
                eprintln!("command: {}", command_str);
            }
            eprintln!("prompt:");
            println!("{}", expanded);
        } else {
            if cmd.blocks.len() > 1 {
                eprintln!("--- block {} ({}) ---", i + 1, block.lang);
            }
            if !block.deps.is_empty() {
                eprintln!("deps: {}", block.deps.join(", "));
            }
            println!("{}", expanded);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    fn make_context() -> RunContext {
        RunContext::new(
            Arc::new(AtomicBool::new(false)),
            std::path::PathBuf::from("/tmp"),
            vec![
                ("FOO".to_string(), "bar".to_string()),
                ("BAZ".to_string(), "qux".to_string()),
            ],
            false,
            false,
        )
    }

    // Run a command with the given env and cwd using a fresh RunContext.
    fn run_for_test(
        cmd: &ParsedCommand,
        raw_args: &[&str],
        extra_env: &[(&str, &str)],
        cwd: &std::path::Path,
    ) -> Result<(), CreftError> {
        let args: Vec<String> = raw_args.iter().map(|s| s.to_string()).collect();
        let env: Vec<(String, String)> = extra_env
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let ctx = RunContext::new(
            Arc::new(AtomicBool::new(false)),
            cwd.to_path_buf(),
            env,
            false,
            false,
        );
        run(cmd, &args, &ctx)
    }

    #[test]
    fn run_context_new_cwd_accessible() {
        let ctx = make_context();
        assert_eq!(ctx.cwd(), std::path::Path::new("/tmp"));
    }

    #[test]
    fn run_context_is_cancelled_default_false() {
        let ctx = make_context();
        assert!(!ctx.is_cancelled());
    }

    #[test]
    fn run_context_cancel_shared_across_clone() {
        let cancel = Arc::new(AtomicBool::new(false));
        let ctx = RunContext::new(
            Arc::clone(&cancel),
            std::path::PathBuf::from("/tmp"),
            vec![],
            false,
            false,
        );
        let cloned = ctx.clone();

        assert!(!ctx.is_cancelled());
        assert!(!cloned.is_cancelled());

        cancel.store(true, Ordering::Relaxed);
        assert!(ctx.is_cancelled());
        assert!(cloned.is_cancelled());
    }

    #[test]
    fn run_context_env_pairs_returns_str_refs() {
        let ctx = make_context();
        let pairs = ctx.env_pairs();
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0], ("FOO", "bar"));
        assert_eq!(pairs[1], ("BAZ", "qux"));
    }

    #[test]
    fn run_context_clone_shares_cwd_and_env() {
        let ctx = make_context();
        let cloned = ctx.clone();
        assert_eq!(ctx.cwd(), cloned.cwd());
        assert_eq!(ctx.env_pairs(), cloned.env_pairs());
    }

    #[test]
    fn run_context_is_cancelled_true_after_flag_set() {
        let cancel = Arc::new(AtomicBool::new(false));
        let ctx = RunContext::new(
            Arc::clone(&cancel),
            std::path::PathBuf::from("/tmp"),
            vec![],
            false,
            false,
        );
        assert!(!ctx.is_cancelled());
        cancel.store(true, Ordering::Relaxed);
        assert!(ctx.is_cancelled());
    }

    use crate::model::{Arg, CodeBlock, CommandDef, Flag};

    fn make_cmd(args: Vec<Arg>, flags: Vec<Flag>) -> ParsedCommand {
        ParsedCommand {
            def: CommandDef {
                name: "test".into(),
                description: "test cmd".into(),
                args,
                flags,
                env: vec![],
                tags: vec![],
                supports: vec![],
            },
            docs: None,
            blocks: vec![],
        }
    }

    #[test]
    fn test_flag_equals_syntax() {
        let cmd = make_cmd(
            vec![],
            vec![Flag {
                name: "output".into(),
                short: Some("o".into()),
                description: "output format".into(),
                r#type: "string".into(),
                default: None,
                validation: None,
            }],
        );
        let raw = vec!["--output=json".to_string()];
        let (pairs, _) = parse_and_bind(&cmd, &raw).unwrap();
        assert_eq!(pairs.iter().find(|(k, _)| k == "output").unwrap().1, "json");
    }

    #[test]
    fn test_flag_bool_presence() {
        let cmd = make_cmd(
            vec![],
            vec![Flag {
                name: "verbose".into(),
                short: Some("v".into()),
                description: "verbose".into(),
                r#type: "bool".into(),
                default: None,
                validation: None,
            }],
        );
        // --verbose sets to true
        let raw = vec!["--verbose".to_string()];
        let (pairs, _) = parse_and_bind(&cmd, &raw).unwrap();
        assert_eq!(
            pairs.iter().find(|(k, _)| k == "verbose").unwrap().1,
            "true"
        );

        // absent defaults to false
        let raw2: Vec<String> = vec![];
        let (pairs2, _) = parse_and_bind(&cmd, &raw2).unwrap();
        assert_eq!(
            pairs2.iter().find(|(k, _)| k == "verbose").unwrap().1,
            "false"
        );
    }

    #[test]
    fn test_combined_short_flags_bool_only() {
        let cmd = make_cmd(
            vec![],
            vec![
                Flag {
                    name: "all".into(),
                    short: Some("a".into()),
                    description: "all".into(),
                    r#type: "bool".into(),
                    default: None,
                    validation: None,
                },
                Flag {
                    name: "verbose".into(),
                    short: Some("v".into()),
                    description: "verbose".into(),
                    r#type: "bool".into(),
                    default: None,
                    validation: None,
                },
                Flag {
                    name: "force".into(),
                    short: Some("f".into()),
                    description: "force".into(),
                    r#type: "bool".into(),
                    default: None,
                    validation: None,
                },
            ],
        );
        let raw = vec!["-avf".to_string()];
        let (pairs, _) = parse_and_bind(&cmd, &raw).unwrap();
        assert_eq!(pairs.iter().find(|(k, _)| k == "all").unwrap().1, "true");
        assert_eq!(
            pairs.iter().find(|(k, _)| k == "verbose").unwrap().1,
            "true"
        );
        assert_eq!(pairs.iter().find(|(k, _)| k == "force").unwrap().1, "true");
    }

    #[test]
    fn test_combined_short_with_value() {
        let cmd = make_cmd(
            vec![],
            vec![
                Flag {
                    name: "verbose".into(),
                    short: Some("v".into()),
                    description: "verbose".into(),
                    r#type: "bool".into(),
                    default: None,
                    validation: None,
                },
                Flag {
                    name: "format".into(),
                    short: Some("f".into()),
                    description: "output format".into(),
                    r#type: "string".into(),
                    default: None,
                    validation: None,
                },
            ],
        );
        // -vf json: v=true, f consumes next arg
        let raw = vec!["-vf".to_string(), "json".to_string()];
        let (pairs, _) = parse_and_bind(&cmd, &raw).unwrap();
        assert_eq!(
            pairs.iter().find(|(k, _)| k == "verbose").unwrap().1,
            "true"
        );
        assert_eq!(pairs.iter().find(|(k, _)| k == "format").unwrap().1, "json");
    }

    #[test]
    fn test_unknown_flag_errors() {
        let cmd = make_cmd(
            vec![],
            vec![Flag {
                name: "verbose".into(),
                short: Some("v".into()),
                description: "verbose".into(),
                r#type: "bool".into(),
                default: None,
                validation: None,
            }],
        );
        // Unknown flag causes an error, not silent passthrough
        let raw = vec!["--unknown".to_string()];
        assert!(parse_and_bind(&cmd, &raw).is_err());

        // Unknown short flag also errors
        let raw2 = vec!["-x".to_string()];
        assert!(parse_and_bind(&cmd, &raw2).is_err());
    }

    // ---- optional arg tests ----

    #[test]
    fn test_optional_arg_not_bound_when_absent() {
        // required: false, no default, no value provided → arg IS in pairs with value ""
        let cmd = make_cmd(
            vec![Arg {
                name: "count".into(),
                description: "how many".into(),
                default: None,
                required: false,
                validation: None,
            }],
            vec![],
        );
        let raw: Vec<String> = vec![];
        let (pairs, _) = parse_and_bind(&cmd, &raw).unwrap();
        assert_eq!(
            pairs
                .iter()
                .find(|(k, _)| k == "count")
                .map(|(_, v)| v.as_str()),
            Some(""),
            "optional arg with no value should be bound to empty string"
        );
    }

    #[test]
    fn test_optional_arg_uses_provided_value() {
        // required: false, no default, value provided → arg IS in pairs
        let cmd = make_cmd(
            vec![Arg {
                name: "count".into(),
                description: "how many".into(),
                default: None,
                required: false,
                validation: None,
            }],
            vec![],
        );
        let raw = vec!["42".to_string()];
        let (pairs, _) = parse_and_bind(&cmd, &raw).unwrap();
        assert_eq!(pairs.iter().find(|(k, _)| k == "count").unwrap().1, "42");
    }

    #[test]
    fn test_optional_arg_with_frontmatter_default() {
        // required: false, default: "10", no value provided → bound to "10"
        let cmd = make_cmd(
            vec![Arg {
                name: "count".into(),
                description: "how many".into(),
                default: Some("10".into()),
                required: false,
                validation: None,
            }],
            vec![],
        );
        let raw: Vec<String> = vec![];
        let (pairs, _) = parse_and_bind(&cmd, &raw).unwrap();
        assert_eq!(pairs.iter().find(|(k, _)| k == "count").unwrap().1, "10");
    }

    #[test]
    fn test_required_arg_missing_errors() {
        // required: true (default), no value → MissingArg error
        let cmd = make_cmd(
            vec![Arg {
                name: "name".into(),
                description: "a name".into(),
                default: None,
                required: true,
                validation: None,
            }],
            vec![],
        );
        let raw: Vec<String> = vec![];
        assert!(parse_and_bind(&cmd, &raw).is_err());
    }

    #[test]
    fn test_optional_arg_template_default_fires() {
        // When parse_and_bind binds an optional arg to "", the bound "" takes
        // precedence and {{count|5}} resolves to '' (shell-escaped empty string),
        // not "5". The template default only fires when the key is absent from
        // pairs entirely.
        let cmd = make_cmd(
            vec![Arg {
                name: "count".into(),
                description: "how many".into(),
                default: None,
                required: false,
                validation: None,
            }],
            vec![],
        );
        let raw: Vec<String> = vec![];
        let (pairs, _) = parse_and_bind(&cmd, &raw).unwrap();
        let bound_refs: Vec<(&str, &str)> = pairs
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        // count is bound to "" by parse_and_bind — template default does not fire
        let result = substitute("echo {{count|5}}", &bound_refs, "bash").unwrap();
        assert_eq!(result, "echo ''");
    }

    #[test]
    fn test_optional_arg_no_default_template_errors() {
        // With the new behavior, required: false + no default → arg bound to ""
        // by parse_and_bind. So {{name}} resolves to '' (empty string), not an error.
        let cmd = make_cmd(
            vec![Arg {
                name: "name".into(),
                description: "a name".into(),
                default: None,
                required: false,
                validation: None,
            }],
            vec![],
        );
        let raw: Vec<String> = vec![];
        let (pairs, _) = parse_and_bind(&cmd, &raw).unwrap();
        let bound_refs: Vec<(&str, &str)> = pairs
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        let result = substitute("echo {{name}}", &bound_refs, "bash").unwrap();
        assert_eq!(
            result, "echo ''",
            "optional arg with no default should resolve to empty string"
        );
    }

    // ---- Validation: arg regex ----

    #[test]
    fn test_arg_validation_valid_value_passes() {
        let cmd = make_cmd(
            vec![Arg {
                name: "env".into(),
                description: "environment".into(),
                default: None,
                required: true,
                validation: Some(r"^(staging|production)$".into()),
            }],
            vec![],
        );
        let raw = vec!["staging".to_string()];
        let (pairs, _) = parse_and_bind(&cmd, &raw).unwrap();
        assert_eq!(pairs[0].1, "staging");
    }

    #[test]
    fn test_arg_validation_invalid_value_errors() {
        let cmd = make_cmd(
            vec![Arg {
                name: "env".into(),
                description: "environment".into(),
                default: None,
                required: true,
                validation: Some(r"^(staging|production)$".into()),
            }],
            vec![],
        );
        let raw = vec!["dev".to_string()];
        let result = parse_and_bind(&cmd, &raw);
        assert!(
            matches!(
                result,
                Err(crate::error::CreftError::ValidationFailed { .. })
            ),
            "expected ValidationFailed, got: {:?}",
            result
        );
    }

    #[test]
    fn test_arg_validation_invalid_regex_errors() {
        let cmd = make_cmd(
            vec![Arg {
                name: "x".into(),
                description: "x".into(),
                default: None,
                required: true,
                validation: Some(r"[invalid(".into()),
            }],
            vec![],
        );
        let raw = vec!["anything".to_string()];
        let result = parse_and_bind(&cmd, &raw);
        assert!(
            matches!(result, Err(crate::error::CreftError::Frontmatter(_))),
            "invalid regex should produce Frontmatter error, got: {:?}",
            result
        );
    }

    #[test]
    fn test_flag_validation_valid_value_passes() {
        let cmd = make_cmd(
            vec![],
            vec![Flag {
                name: "format".into(),
                short: None,
                description: "output format".into(),
                r#type: "string".into(),
                default: None,
                validation: Some(r"^(json|text)$".into()),
            }],
        );
        let raw = vec!["--format=json".to_string()];
        let (pairs, _) = parse_and_bind(&cmd, &raw).unwrap();
        assert_eq!(pairs[0].1, "json");
    }

    #[test]
    fn test_flag_validation_invalid_value_errors() {
        let cmd = make_cmd(
            vec![],
            vec![Flag {
                name: "format".into(),
                short: None,
                description: "output format".into(),
                r#type: "string".into(),
                default: None,
                validation: Some(r"^(json|text)$".into()),
            }],
        );
        let raw = vec!["--format=xml".to_string()];
        let result = parse_and_bind(&cmd, &raw);
        assert!(
            matches!(
                result,
                Err(crate::error::CreftError::ValidationFailed { .. })
            ),
            "expected ValidationFailed, got: {:?}",
            result
        );
    }

    #[test]
    fn test_flag_validation_invalid_regex_errors() {
        let cmd = make_cmd(
            vec![],
            vec![Flag {
                name: "x".into(),
                short: None,
                description: "x".into(),
                r#type: "string".into(),
                default: None,
                validation: Some(r"[bad(".into()),
            }],
        );
        let raw = vec!["--x=val".to_string()];
        let result = parse_and_bind(&cmd, &raw);
        assert!(
            matches!(result, Err(crate::error::CreftError::Frontmatter(_))),
            "invalid flag regex should produce Frontmatter error, got: {:?}",
            result
        );
    }

    #[test]
    fn test_flag_string_with_default_used_when_absent() {
        // string flag with default — when not provided, default is used
        let cmd = make_cmd(
            vec![],
            vec![Flag {
                name: "format".into(),
                short: None,
                description: "format".into(),
                r#type: "string".into(),
                default: Some("json".into()),
                validation: None,
            }],
        );
        let raw: Vec<String> = vec![];
        let (pairs, _) = parse_and_bind(&cmd, &raw).unwrap();
        assert_eq!(pairs[0].1, "json");
    }

    #[test]
    fn test_flag_string_no_default_absent_skipped() {
        // string flag with no default, not provided → bound to "" in pairs
        let cmd = make_cmd(
            vec![],
            vec![Flag {
                name: "filter".into(),
                short: None,
                description: "filter".into(),
                r#type: "string".into(),
                default: None,
                validation: None,
            }],
        );
        let raw: Vec<String> = vec![];
        let (pairs, _) = parse_and_bind(&cmd, &raw).unwrap();
        assert_eq!(
            pairs
                .iter()
                .find(|(k, _)| k == "filter")
                .map(|(_, v)| v.as_str()),
            Some(""),
            "string flag with no default and not provided should be bound to empty string"
        );
    }

    #[test]
    fn test_optional_flag_no_default_binds_empty() {
        // string flag with required: false (implied by flag type), no default, not provided
        // → bound to "" so {{flagname}} resolves to empty string rather than erroring
        let cmd = make_cmd(
            vec![],
            vec![Flag {
                name: "format".into(),
                short: None,
                description: "output format".into(),
                r#type: "string".into(),
                default: None,
                validation: None,
            }],
        );
        let raw: Vec<String> = vec![];
        let (pairs, _) = parse_and_bind(&cmd, &raw).unwrap();
        let bound_refs: Vec<(&str, &str)> = pairs
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        let result = substitute("output {{format}}", &bound_refs, "bash").unwrap();
        assert_eq!(
            result, "output ''",
            "unset string flag should substitute as empty string"
        );
    }

    // ---- check_env ----

    #[test]
    fn test_check_env_missing_required_var_errors() {
        use crate::model::{CommandDef, EnvVar};
        let cmd = ParsedCommand {
            def: CommandDef {
                name: "test".into(),
                description: "test".into(),
                args: vec![],
                flags: vec![],
                env: vec![EnvVar {
                    name: "CREFT_TEST_MISSING_VAR_XYZ_12345".into(),
                    required: true,
                }],
                tags: vec![],
                supports: vec![],
            },
            docs: None,
            blocks: vec![],
        };
        let result = check_env(&cmd);
        assert!(
            matches!(result, Err(crate::error::CreftError::MissingEnvVar(_))),
            "missing required env var should error, got: {:?}",
            result
        );
    }

    #[test]
    fn test_check_env_optional_missing_ok() {
        use crate::model::{CommandDef, EnvVar};
        let cmd = ParsedCommand {
            def: CommandDef {
                name: "test".into(),
                description: "test".into(),
                args: vec![],
                flags: vec![],
                env: vec![EnvVar {
                    name: "CREFT_TEST_OPTIONAL_VAR_XYZ_12345".into(),
                    required: false,
                }],
                tags: vec![],
                supports: vec![],
            },
            docs: None,
            blocks: vec![],
        };
        // Optional missing var should not error
        let result = check_env(&cmd);
        assert!(result.is_ok());
    }

    // ---- extension mapping ----

    #[test]
    fn test_extension_mapping() {
        assert_eq!(extension("bash"), "sh");
        assert_eq!(extension("sh"), "sh");
        assert_eq!(extension("zsh"), "sh");
        assert_eq!(extension("python"), "py");
        assert_eq!(extension("python3"), "py");
        assert_eq!(extension("node"), "js");
        assert_eq!(extension("javascript"), "js");
        assert_eq!(extension("js"), "js");
        assert_eq!(extension("typescript"), "ts");
        assert_eq!(extension("ts"), "ts");
        assert_eq!(extension("perl"), "pl");
        assert_eq!(extension("unknown"), "unknown");
    }

    // ---- interpreter mapping for ts/perl ----

    #[rstest]
    #[case::sh("sh", "sh")]
    #[case::zsh("zsh", "zsh")]
    #[case::python3("python3", "python3")]
    #[case::javascript("javascript", "node")]
    #[case::js("js", "node")]
    #[case::typescript("typescript", "npx tsx")]
    #[case::ts("ts", "npx tsx")]
    #[case::perl("perl", "perl")]
    fn interpreter_maps_lang_to_executable(#[case] lang: &str, #[case] expected: &str) {
        assert_eq!(interpreter(lang), expected);
    }

    // ---- stdin thread: pipe mode tests ----

    /// Build a two-block pipe command. Block 0 uses `block0_code` and block 1
    /// uses `block1_code`. Both blocks run as bash.
    fn make_pipe_cmd(block0_code: &str, block1_code: &str) -> ParsedCommand {
        ParsedCommand {
            def: CommandDef {
                name: "test-pipe".into(),
                description: "test pipe cmd".into(),
                args: vec![],
                flags: vec![],
                env: vec![],
                tags: vec![],
                supports: vec![],
            },
            docs: None,
            blocks: vec![
                CodeBlock {
                    lang: "bash".into(),
                    code: block0_code.into(),
                    deps: vec![],
                    llm_config: None,
                    llm_parse_error: None,
                },
                CodeBlock {
                    lang: "bash".into(),
                    code: block1_code.into(),
                    deps: vec![],
                    llm_config: None,
                    llm_parse_error: None,
                },
            ],
        }
    }

    #[test]
    fn test_pipe_large_stdin_no_deadlock() {
        // Block 0 produces 128KB of output (well above the OS pipe buffer of ~64KB).
        // Block 1 reads all stdin and writes it to stdout.
        // Without the thread fix this test would deadlock; with it, it must complete.
        let block0 = r#"python3 -c "print('x' * 131072, end='')" "#;
        let block1 = "cat";
        let cmd = make_pipe_cmd(block0, block1);
        let cwd = std::path::Path::new("/tmp");
        let result = run_for_test(&cmd, &[], &[], cwd);
        assert!(
            result.is_ok(),
            "large pipe should not deadlock: {:?}",
            result
        );
    }

    #[test]
    fn test_pipe_broken_pipe_silenced() {
        // Block 0 produces substantial output. Block 1 exits immediately (exit 0)
        // without reading any stdin. The write thread gets BrokenPipe; this must
        // NOT surface as StdinWriteFailed — the command should succeed because
        // block 1 exits with code 0.
        let block0 = r#"python3 -c "print('x' * 65536, end='')" "#;
        let block1 = "exit 0";
        let cmd = make_pipe_cmd(block0, block1);
        let cwd = std::path::Path::new("/tmp");
        let result = run_for_test(&cmd, &[], &[], cwd);
        assert!(
            result.is_ok(),
            "BrokenPipe from early-exit child must not surface as error: {:?}",
            result
        );
    }

    #[test]
    fn test_pipe_empty_stdin_payload() {
        // Block 0 produces no output (empty string). Block 1 reads stdin and
        // echoes its line count. Should succeed with 0 lines.
        let block0 = "true"; // exits 0, no output
        let block1 = "wc -l"; // reads stdin, prints line count
        let cmd = make_pipe_cmd(block0, block1);
        let cwd = std::path::Path::new("/tmp");
        let result = run_for_test(&cmd, &[], &[], cwd);
        assert!(
            result.is_ok(),
            "empty stdin pipe should succeed: {:?}",
            result
        );
    }

    #[test]
    fn test_multi_block_default_pipes() {
        // Multi-block skills always pipe. Block 0 outputs "hello" on stdout,
        // which becomes stdin for block 1. Block 1 (cat) passes it through.
        // Both blocks run as part of a pipe chain — result is Ok.
        let cmd = ParsedCommand {
            def: CommandDef {
                name: "test-default-pipe".into(),
                description: "test multi-block pipes by default".into(),
                args: vec![],
                flags: vec![],
                env: vec![],
                tags: vec![],
                supports: vec![],
            },
            docs: None,
            blocks: vec![
                CodeBlock {
                    lang: "bash".into(),
                    code: "echo hello".into(),
                    deps: vec![],
                    llm_config: None,
                    llm_parse_error: None,
                },
                CodeBlock {
                    lang: "bash".into(),
                    code: "cat".into(),
                    deps: vec![],
                    llm_config: None,
                    llm_parse_error: None,
                },
            ],
        };
        let cwd = std::path::Path::new("/tmp");
        let result = run_for_test(&cmd, &[], &[], cwd);
        assert!(
            result.is_ok(),
            "multi-block command must pipe by default: {:?}",
            result
        );
    }

    /// Build a pipe command with an arbitrary number of bash blocks.
    fn make_pipe_cmd_multi(blocks: &[&str]) -> ParsedCommand {
        ParsedCommand {
            def: CommandDef {
                name: "test-pipe-multi".into(),
                description: "test multi-block pipe".into(),
                args: vec![],
                flags: vec![],
                env: vec![],
                tags: vec![],
                supports: vec![],
            },
            docs: None,
            blocks: blocks
                .iter()
                .map(|code| CodeBlock {
                    lang: "bash".into(),
                    code: code.to_string(),
                    deps: vec![],
                    llm_config: None,
                    llm_parse_error: None,
                })
                .collect(),
        }
    }

    #[test]
    fn test_pipe_three_blocks() {
        // Three-block concurrent pipe chain: block 0 echoes data, block 1
        // transforms it (tr a-z A-Z), block 2 reads and appends a marker.
        // This verifies the OS pipe chain connects all three blocks correctly.
        let cmd = make_pipe_cmd_multi(&[
            "echo hello",
            "tr a-z A-Z",
            r#"read line; echo "got: $line""#,
        ]);
        let cwd = std::path::Path::new("/tmp");
        let result = run_for_test(&cmd, &[], &[], cwd);
        assert!(
            result.is_ok(),
            "three-block pipe chain should succeed: {:?}",
            result
        );
    }

    #[test]
    fn test_pipe_single_block_passthrough() {
        // Single-block skills skip the pipe path entirely and run via execute_block.
        let cmd = ParsedCommand {
            def: CommandDef {
                name: "test-pipe-single".into(),
                description: "single block skill".into(),
                args: vec![],
                flags: vec![],
                env: vec![],
                tags: vec![],
                supports: vec![],
            },
            docs: None,
            blocks: vec![CodeBlock {
                lang: "bash".into(),
                code: "echo single".into(),
                deps: vec![],
                llm_config: None,
                llm_parse_error: None,
            }],
        };
        let cwd = std::path::Path::new("/tmp");
        let result = run_for_test(&cmd, &[], &[], cwd);
        assert!(
            result.is_ok(),
            "single-block skill must succeed: {:?}",
            result
        );
    }

    #[test]
    fn test_pipe_block0_fails() {
        // Block 0 exits non-zero. Block 1 reads EOF on stdin and succeeds
        // (wc -l prints 0). The upstream failure must be reported regardless
        // of whether the last block succeeded — creft reports the root cause,
        // not the last block's exit code.
        let cmd = make_pipe_cmd_multi(&["exit 1", "wc -l"]);
        let cwd = std::path::Path::new("/tmp");
        let result = run_for_test(&cmd, &[], &[], cwd);
        assert!(
            matches!(
                result,
                Err(crate::error::CreftError::ExecutionFailed { block: 0, .. })
            ),
            "block 0 failure must be reported even when the last block succeeds: {:?}",
            result
        );
    }

    #[test]
    fn test_pipe_last_block_fails_reports_error() {
        // Block 0 succeeds. Block 1 exits non-zero. The last block's exit status
        // determines the result — should return ExecutionFailed.
        let cmd = make_pipe_cmd_multi(&["echo hello", "exit 1"]);
        let cwd = std::path::Path::new("/tmp");
        let result = run_for_test(&cmd, &[], &[], cwd);
        assert!(
            matches!(
                result,
                Err(crate::error::CreftError::ExecutionFailed { block: 1, .. })
            ),
            "when last block fails, error should reference block 1: {:?}",
            result
        );
    }

    // ---- actionable OS error messages ----

    #[test]
    #[cfg(unix)]
    fn test_signal_detection() {
        // A block that kills itself with SIGTERM should produce ExecutionSignaled,
        // not ExecutionFailed. We use `kill -TERM $$` in bash to self-signal.
        let cmd = ParsedCommand {
            def: CommandDef {
                name: "test-signal".into(),
                description: "signal test".into(),
                args: vec![],
                flags: vec![],
                env: vec![],
                tags: vec![],
                supports: vec![],
            },
            docs: None,
            blocks: vec![CodeBlock {
                lang: "bash".into(),
                // Self-terminate with SIGTERM. bash propagates the signal exit.
                code: "kill -TERM $$".into(),
                deps: vec![],
                llm_config: None,
                llm_parse_error: None,
            }],
        };
        let cwd = std::path::Path::new("/tmp");
        let result = run_for_test(&cmd, &[], &[], cwd);
        assert!(
            matches!(
                result,
                Err(crate::error::CreftError::ExecutionSignaled { block: 0, .. })
            ),
            "self-SIGTERM should produce ExecutionSignaled, got: {:?}",
            result
        );
    }

    #[test]
    fn test_e2big_enrichment() {
        // Directly test enrich_io_error with a synthetic E2BIG error (raw_os_error 7).
        // We can't easily trigger a real E2BIG in a unit test, so we construct the
        // io::Error directly using from_raw_os_error.
        let e2big = std::io::Error::from_raw_os_error(7);
        let err = crate::error::enrich_io_error(e2big, "environment");
        assert!(
            matches!(err, crate::error::CreftError::Setup(_)),
            "E2BIG should produce a Setup error with guidance, got: {:?}",
            err
        );
        let msg = err.to_string();
        assert!(
            msg.contains("OS argument size limit"),
            "E2BIG message should mention OS argument size limit, got: {msg}"
        );
        assert!(
            msg.contains("environment"),
            "E2BIG message should include context 'environment', got: {msg}"
        );
    }

    #[test]
    fn test_enrich_io_error_other_errors_passthrough() {
        // Non-E2BIG, non-NotFound errors should pass through as CreftError::Io.
        let permission_denied = std::io::Error::from_raw_os_error(13 /* EACCES */);
        let err = crate::error::enrich_io_error(permission_denied, "ctx");
        assert!(
            matches!(err, crate::error::CreftError::Io(_)),
            "EACCES should pass through as Io, got: {:?}",
            err
        );
    }

    #[test]
    fn test_pipe_chain_multiple_failures() {
        // Both block 0 and block 1 fail. Block 0 exits non-zero, block 1 also
        // exits non-zero. The error should reference block 0 (the root cause —
        // earliest non-signal failure).
        //
        // Note: wc -l counts lines from stdin. When block 0 exits 1, its pipe
        // closes, block 1 gets EOF and also exits 1 via explicit `exit 1`.
        let cmd = make_pipe_cmd_multi(&["exit 1", "exit 1"]);
        let cwd = std::path::Path::new("/tmp");
        let result = run_for_test(&cmd, &[], &[], cwd);
        // Block 0 fails with exit code (not a signal), so it's the root cause.
        assert!(
            matches!(
                result,
                Err(crate::error::CreftError::ExecutionFailed { block: 0, .. })
            ),
            "when both blocks fail, block 0 should be reported as root cause: {:?}",
            result
        );
    }

    #[test]
    fn test_execution_signaled_exit_code() {
        // ExecutionSignaled.exit_code() should return 128 + signal (Unix convention).
        let err = crate::error::CreftError::ExecutionSignaled {
            block: 0,
            lang: "bash".into(),
            signal: 15, // SIGTERM
        };
        assert_eq!(err.exit_code(), 143, "128 + 15 (SIGTERM) = 143");
    }

    #[test]
    fn run_context_request_cancel_sets_flag() {
        let ctx = RunContext::new(
            Arc::new(AtomicBool::new(false)),
            std::path::PathBuf::from("/tmp"),
            vec![],
            false,
            false,
        );
        assert_eq!(ctx.is_cancelled(), false);
        ctx.request_cancel();
        assert_eq!(ctx.is_cancelled(), true);
    }

    #[test]
    fn run_context_request_cancel_visible_to_clones() {
        let cancel = Arc::new(AtomicBool::new(false));
        let ctx = RunContext::new(
            Arc::clone(&cancel),
            std::path::PathBuf::from("/tmp"),
            vec![],
            false,
            false,
        );
        let cloned = ctx.clone();
        ctx.request_cancel();
        assert_eq!(cloned.is_cancelled(), true);
    }

    #[cfg(unix)]
    #[test]
    fn make_execution_error_signal_returns_execution_signaled() {
        use std::os::unix::process::ExitStatusExt;
        // Raw value N means the process was killed by signal N.
        let status = std::process::ExitStatus::from_raw(9);
        let err = make_execution_error(2, "bash", &status);
        assert!(
            matches!(
                err,
                crate::error::CreftError::ExecutionSignaled {
                    block: 2,
                    signal: 9,
                    ..
                }
            ),
            "signal kill should produce ExecutionSignaled, got: {:?}",
            err
        );
    }

    #[cfg(unix)]
    #[test]
    fn make_execution_error_exit_code_returns_execution_failed() {
        use std::os::unix::process::ExitStatusExt;
        // Raw value N << 8 means the process exited with code N.
        let status = std::process::ExitStatus::from_raw(42 << 8);
        let err = make_execution_error(1, "python", &status);
        assert!(
            matches!(
                err,
                crate::error::CreftError::ExecutionFailed {
                    block: 1,
                    code: 42,
                    ..
                }
            ),
            "non-zero exit code should produce ExecutionFailed, got: {:?}",
            err
        );
    }

    // ---- bound_pairs_to_env ----

    #[test]
    fn bound_pairs_to_env_empty_input_produces_empty_output() {
        let result = bound_pairs_to_env(&[]);
        assert_eq!(result, Vec::<(String, String)>::new());
    }

    #[rstest]
    #[case::lowercase("format", "json", "FORMAT", "json")]
    #[case::hyphenated("always-confirm", "true", "ALWAYS_CONFIRM", "true")]
    #[case::multi_hyphen("output-file-path", "out.txt", "OUTPUT_FILE_PATH", "out.txt")]
    #[case::already_upper("TARGET", "prod", "TARGET", "prod")]
    #[case::mixed_case("myFlag", "val", "MYFLAG", "val")]
    #[case::underscore_unchanged("my_flag", "1", "MY_FLAG", "1")]
    fn bound_pairs_to_env_name_normalization(
        #[case] input_name: &str,
        #[case] input_val: &str,
        #[case] expected_name: &str,
        #[case] expected_val: &str,
    ) {
        let pairs = vec![(input_name.to_string(), input_val.to_string())];
        let result = bound_pairs_to_env(&pairs);
        assert_eq!(result.len(), 1);
        assert_eq!(
            result[0],
            (expected_name.to_string(), expected_val.to_string())
        );
    }

    #[test]
    fn bound_pairs_to_env_preserves_order_and_count() {
        let pairs = vec![
            ("alpha".to_string(), "1".to_string()),
            ("beta-flag".to_string(), "2".to_string()),
            ("gamma".to_string(), "3".to_string()),
        ];
        let result = bound_pairs_to_env(&pairs);
        assert_eq!(result.len(), 3);
        assert_eq!(result[0], ("ALPHA".to_string(), "1".to_string()));
        assert_eq!(result[1], ("BETA_FLAG".to_string(), "2".to_string()));
        assert_eq!(result[2], ("GAMMA".to_string(), "3".to_string()));
    }

    #[test]
    fn bound_pairs_to_env_empty_value_preserved() {
        let pairs = vec![("format".to_string(), String::new())];
        let result = bound_pairs_to_env(&pairs);
        assert_eq!(result, vec![("FORMAT".to_string(), String::new())]);
    }

    // ---- prepare_block_script ----

    fn bash_block() -> CodeBlock {
        CodeBlock {
            lang: "bash".into(),
            code: String::new(),
            deps: vec![],
            llm_config: None,
            llm_parse_error: None,
        }
    }

    /// With no preamble, the file contains exactly the expanded code.
    #[test]
    fn prepare_block_script_none_preamble_writes_code_only() {
        let block = bash_block();
        let code = "echo hello\n";
        let tmp = prepare_block_script(&block, code, None).unwrap();
        let contents = std::fs::read_to_string(tmp.path()).unwrap();
        assert_eq!(contents, code);
    }

    /// With a preamble, the preamble is written before the code.
    #[test]
    fn prepare_block_script_some_preamble_prepends_preamble() {
        let block = bash_block();
        let preamble = "# preamble\n";
        let code = "echo hello\n";
        let tmp = prepare_block_script(&block, code, Some(preamble)).unwrap();
        let contents = std::fs::read_to_string(tmp.path()).unwrap();
        assert_eq!(contents, "# preamble\necho hello\n");
    }

    /// When the code starts with a shebang, the shebang is placed first,
    /// then the preamble, then the remainder of the code.
    #[test]
    fn prepare_block_script_shebang_placed_before_preamble() {
        let block = bash_block();
        let preamble = "# preamble\n";
        let code = "#!/bin/bash\necho hello\n";
        let tmp = prepare_block_script(&block, code, Some(preamble)).unwrap();
        let contents = std::fs::read_to_string(tmp.path()).unwrap();
        assert_eq!(contents, "#!/bin/bash\n# preamble\necho hello\n");
    }

    /// A shebang-only script (no trailing newline after shebang) is handled
    /// without panicking — remainder is empty.
    #[test]
    fn prepare_block_script_shebang_only_no_panic() {
        let block = bash_block();
        let preamble = "# preamble\n";
        let code = "#!/bin/bash";
        let tmp = prepare_block_script(&block, code, Some(preamble)).unwrap();
        let contents = std::fs::read_to_string(tmp.path()).unwrap();
        assert_eq!(contents, "#!/bin/bash\n# preamble\n");
    }

    /// Without a preamble, a shebang-prefixed code is written unchanged.
    #[test]
    fn prepare_block_script_shebang_without_preamble_unchanged() {
        let block = bash_block();
        let code = "#!/bin/bash\necho hi\n";
        let tmp = prepare_block_script(&block, code, None).unwrap();
        let contents = std::fs::read_to_string(tmp.path()).unwrap();
        assert_eq!(contents, code);
    }

    /// The file extension matches the block's language.
    #[test]
    fn prepare_block_script_file_has_correct_extension() {
        let block = CodeBlock {
            lang: "python".into(),
            code: String::new(),
            deps: vec![],
            llm_config: None,
            llm_parse_error: None,
        };
        let tmp = prepare_block_script(&block, "print('hi')\n", None).unwrap();
        assert!(
            tmp.path().extension().map(|e| e == "py").unwrap_or(false),
            "python blocks must produce .py temp files"
        );
    }
}
