use std::io::Write;
use std::path::Path;
use std::sync::LazyLock;

use crate::error::CreftError;
use crate::model::{CodeBlock, ParsedCommand};

/// Exit code that signals early successful return — skip remaining blocks.
///
/// A block that exits 99 is treated as a successful early termination of
/// the pipeline. creft intercepts this code and returns 0 to the caller.
/// All other non-zero exit codes propagate as errors.
const EARLY_EXIT: i32 = 99;

static PLACEHOLDER_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"\{\{([a-zA-Z_][a-zA-Z0-9_-]*)(?:\|([^}]*))?\}\}").unwrap()
});

/// Returns `true` if the language tag uses a shell interpreter that performs
/// word splitting and metacharacter expansion on substituted values.
fn should_shell_escape(lang: &str) -> bool {
    matches!(lang, "bash" | "sh" | "zsh")
}

/// Substitute `{{placeholder}}` values in a template string.
///
/// Named args from the command definition are matched positionally
/// to the provided values. Supports `{{name|default}}` syntax.
///
/// # Shell escaping
///
/// When `lang` is a shell language (`bash`, `sh`, `zsh`), values from the
/// `args` slice are single-quote escaped via `shell_escape::escape` so that
/// shell metacharacters (`$()`, backticks, semicolons, etc.) are treated as
/// literal characters. Author-supplied default values in `{{name|default}}`
/// syntax are NOT escaped — they are considered intentional shell code.
///
/// Non-shell languages (`python`, `node`, etc.) receive raw values.
///
/// # Edge cases
///
/// - Empty string: produces `''` under escaping. This is the correct shell
///   representation of an empty argument and makes intent unambiguous.
/// - `{{prev}}` (previous block output) is in the `args` slice and IS escaped
///   for shell blocks, since it may contain user-influenced content.
pub fn substitute(template: &str, args: &[(&str, &str)], lang: &str) -> Result<String, CreftError> {
    let re = &*PLACEHOLDER_RE;
    let escape = should_shell_escape(lang);

    let mut missing: Vec<String> = Vec::new();

    let result = re.replace_all(template, |caps: &regex::Captures| {
        let name = &caps[1];
        let default_val = caps.get(2).map(|m| m.as_str());

        if let Some((_, val)) = args.iter().find(|(n, _)| *n == name) {
            if escape {
                shell_escape::escape((*val).into()).to_string()
            } else {
                val.to_string()
            }
        } else if let Some(d) = default_val {
            // Author-controlled default — no escaping regardless of language
            d.to_string()
        } else {
            missing.push(name.to_string());
            format!("{{{{{}}}}}", name)
        }
    });

    if !missing.is_empty() {
        return Err(CreftError::MissingArg(missing.join(", ")));
    }

    Ok(result.to_string())
}

/// Bound pairs (name → value) and any passthrough args.
pub type BindResult = (Vec<(String, String)>, Vec<String>);

/// Build a clap Command dynamically from a parsed command's definition.
fn build_clap_command(cmd: &ParsedCommand) -> clap::Command {
    let mut clap_cmd = clap::Command::new(cmd.def.name.clone())
        .no_binary_name(true)
        .disable_help_flag(true)
        .disable_version_flag(true);

    // Add defined positional args
    for arg_def in &cmd.def.args {
        let mut clap_arg = clap::Arg::new(arg_def.name.clone()).action(clap::ArgAction::Set);
        // An arg is required by clap only when the caller must provide it AND
        // there is no frontmatter default to fall back to.
        if arg_def.required && arg_def.default.is_none() {
            clap_arg = clap_arg.required(true);
        }
        clap_cmd = clap_cmd.arg(clap_arg);
    }

    // Add defined flags
    for flag_def in &cmd.def.flags {
        let mut clap_arg = clap::Arg::new(flag_def.name.clone()).long(flag_def.name.clone());
        if let Some(short) = &flag_def.short
            && let Some(ch) = short.chars().next()
        {
            clap_arg = clap_arg.short(ch);
        }
        if flag_def.r#type == "bool" {
            clap_arg = clap_arg.action(clap::ArgAction::SetTrue);
        } else {
            clap_arg = clap_arg.action(clap::ArgAction::Set);
        }
        clap_cmd = clap_cmd.arg(clap_arg);
    }

    clap_cmd
}

/// Parse raw CLI args using clap's builder API, validate, apply defaults.
///
/// Returns a vec of `(name, value)` pairs for template substitution.
/// Unknown flags cause a parse error (no silent passthrough).
pub fn parse_and_bind(cmd: &ParsedCommand, raw_args: &[String]) -> Result<BindResult, CreftError> {
    let clap_cmd = build_clap_command(cmd);
    let matches = clap_cmd
        .try_get_matches_from(raw_args)
        .map_err(|e| CreftError::MissingArg(e.to_string()))?;

    let mut pairs: Vec<(String, String)> = Vec::new();

    // Extract positional args
    for arg_def in &cmd.def.args {
        let val = if let Some(v) = matches.get_one::<String>(&arg_def.name) {
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

        // Regex validation — skip when optional arg was not provided (empty default)
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

    // Extract flags
    for flag_def in &cmd.def.flags {
        let val = if flag_def.r#type == "bool" {
            matches.get_flag(&flag_def.name).to_string()
        } else if let Some(v) = matches.get_one::<String>(&flag_def.name) {
            v.clone()
        } else if let Some(d) = &flag_def.default {
            d.clone()
        } else {
            // String flag with no default and not provided — bind to empty
            // string so {{flagname}} resolves to "" rather than erroring.
            String::new()
        };

        // Regex validation for string flags — skip when empty default (not provided)
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
fn interpreter(lang: &str) -> &str {
    match lang {
        "bash" => "bash",
        "sh" => "sh",
        "zsh" => "zsh",
        "python" => "python3",
        "python3" => "python3",
        "node" | "javascript" | "js" => "node",
        "typescript" | "ts" => "npx tsx",
        "ruby" | "rb" => "ruby",
        "perl" => "perl",
        other => other,
    }
}

/// File extension for a language.
fn extension(lang: &str) -> &str {
    match lang {
        "bash" | "sh" | "zsh" => "sh",
        "python" | "python3" => "py",
        "node" | "javascript" | "js" => "js",
        "typescript" | "ts" => "ts",
        "ruby" | "rb" => "rb",
        "perl" => "pl",
        other => other,
    }
}

/// Create a temporary script file for a code block.
///
/// Returns the temp file handle (must be kept alive until the child exits).
fn prepare_block_script(
    block: &CodeBlock,
    expanded_code: &str,
) -> Result<tempfile::NamedTempFile, CreftError> {
    let ext = extension(&block.lang);
    let mut tmp = tempfile::Builder::new()
        .prefix("creft-")
        .suffix(&format!(".{}", ext))
        .tempfile()
        .map_err(CreftError::Io)?;

    tmp.write_all(expanded_code.as_bytes())
        .map_err(CreftError::Io)?;
    tmp.flush().map_err(CreftError::Io)?;

    Ok(tmp)
}

/// Spawn a child process for a code block.
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
#[allow(clippy::too_many_arguments)] // private helper; grouping into a struct would add noise
fn spawn_block(
    block: &CodeBlock,
    script_path: &Path,
    env_vars: &[(&str, &str)],
    stdin_cfg: std::process::Stdio,
    stdout_cfg: std::process::Stdio,
    cwd: &Path,
    #[cfg(unix)] process_group: Option<u32>,
    #[cfg(unix)] ignore_sigint: bool,
) -> Result<(std::process::Child, Option<tempfile::TempDir>), CreftError> {
    // Stdio is not Copy, so we apply stdin/stdout directly rather than
    // using a reusable closure (which would require Fn, not FnOnce).
    let apply_common = |cmd: &mut std::process::Command| {
        cmd.current_dir(cwd);
        for (k, v) in env_vars {
            cmd.env(k, v);
        }
        cmd.stderr(std::process::Stdio::inherit());
    };

    // For node blocks with deps, install packages into a temp directory first
    // so that require() / import() can resolve them via NODE_PATH. The tempdir
    // must remain alive until the child exits; it is returned to the caller.
    //
    // A minimal package.json is written before running `npm install` so that
    // npm anchors the node_modules directory to this temp dir rather than
    // walking up the directory tree to find an existing package.json.
    let (node_deps_dir, node_modules_path): (
        Option<tempfile::TempDir>,
        Option<std::path::PathBuf>,
    ) = if !block.deps.is_empty() && matches!(block.lang.as_str(), "node" | "javascript" | "js") {
        let dir = tempfile::tempdir().map_err(CreftError::Io)?;
        // Write a stub package.json so npm installs into this directory.
        let pkg_json = dir.path().join("package.json");
        std::fs::write(&pkg_json, r#"{"private":true}"#).map_err(CreftError::Io)?;
        let status = std::process::Command::new("npm")
            .arg("install")
            .args(&block.deps)
            .current_dir(dir.path())
            .status()
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    CreftError::InterpreterNotFound(
                        "npm (install Node.js). Run 'creft doctor' to check.".to_string(),
                    )
                } else {
                    CreftError::Io(e)
                }
            })?;
        if !status.success() {
            return Err(CreftError::Setup(format!(
                "npm install failed for deps: {}",
                block.deps.join(", ")
            )));
        }
        let node_modules = dir.path().join("node_modules");
        (Some(dir), Some(node_modules))
    } else {
        (None, None)
    };

    let mut cmd: std::process::Command = if !block.deps.is_empty() {
        match block.lang.as_str() {
            "python" | "python3" => {
                let mut c = std::process::Command::new("uv");
                c.arg("run");
                for dep in &block.deps {
                    c.arg("--with").arg(dep);
                }
                c.arg("--").arg("python3").arg(script_path);
                c
            }
            "node" | "javascript" | "js" => {
                let mut c = std::process::Command::new("node");
                if let Some(ref node_modules) = node_modules_path {
                    c.env("NODE_PATH", node_modules);
                }
                c.arg(script_path);
                c
            }
            "bash" | "sh" | "zsh" => {
                for dep in &block.deps {
                    if which(dep).is_none() {
                        eprintln!("warning: '{}' not found on PATH", dep);
                    }
                }
                let interp = interpreter(&block.lang);
                let mut c = std::process::Command::new(interp);
                c.arg(script_path);
                c
            }
            _ => {
                let interp = interpreter(&block.lang);
                let mut c = std::process::Command::new(interp);
                c.arg(script_path);
                c
            }
        }
    } else {
        let interp = interpreter(&block.lang);
        let parts: Vec<&str> = interp.split_whitespace().collect();
        let mut c = std::process::Command::new(parts[0]);
        for part in &parts[1..] {
            c.arg(part);
        }
        c.arg(script_path);
        c
    };

    apply_common(&mut cmd);
    cmd.stdin(stdin_cfg);
    cmd.stdout(stdout_cfg);

    // Both operations must happen between fork() and exec() — exactly when
    // pre_exec() runs. setpgid(2) and signal(2) are both async-signal-safe.
    #[cfg(unix)]
    {
        let need_pre_exec = process_group.is_some() || ignore_sigint;
        if need_pre_exec {
            use std::os::unix::process::CommandExt;
            // SAFETY: Both setpgid(0, pgid) and signal(SIGINT, SIG_IGN) are
            // async-signal-safe (POSIX-required for use in the fork-exec window).
            // No Rust allocations or mutexes are touched. Captured values
            // (pgid via Option<u32>, ignore_sigint via bool) are Copy.
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
            _ => interpreter(&block.lang).to_string(),
        }
    } else {
        interpreter(&block.lang).to_string()
    };

    let child = cmd.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            CreftError::InterpreterNotFound(format!("{interp_name}. Run 'creft doctor' to check."))
        } else {
            // E2BIG (large env) and other OS errors get actionable messages.
            crate::error::enrich_io_error(e, "CREFT_PREV")
        }
    })?;
    Ok((child, node_deps_dir))
}

/// Return the plain exit code for a process status, or `None` if the process
/// was killed by a signal (Unix) or terminated abnormally (Windows).
///
/// On Unix this corresponds to `ExitStatusExt::code()`: `Some(n)` for a
/// voluntary exit, `None` for a signal kill.
fn exit_code_of(status: &std::process::ExitStatus) -> Option<i32> {
    status.code()
}

/// Build the appropriate `CreftError` for a failed child process.
///
/// On Unix, if the process was killed by a signal (`ExitStatus::code()` is
/// `None`), returns `ExecutionSignaled` with the signal number. Otherwise
/// returns `ExecutionFailed` with the exit code.
fn make_execution_error(block: usize, lang: &str, status: &std::process::ExitStatus) -> CreftError {
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

// ── Unix-only signal management for pipe chains ──────────────────────────────

/// Atomic storage for the child process group ID, used by the SIGINT
/// forwarding handler. Zero means "no active pipe chain".
#[cfg(unix)]
static PIPE_CHILD_PGID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Signal handler that forwards SIGINT to the child process group.
///
/// # Safety
///
/// This function is called by the OS as a signal handler. It must be
/// async-signal-safe. The only operations performed are:
/// - `AtomicU32::load` (async-signal-safe: no locks, no allocation)
/// - `libc::kill` (listed as async-signal-safe in POSIX)
#[cfg(unix)]
extern "C" fn sigint_forward_handler(_sig: libc::c_int) {
    let pgid = PIPE_CHILD_PGID.load(std::sync::atomic::Ordering::SeqCst);
    if pgid != 0 {
        // SAFETY: kill(-pgid, SIGINT) is async-signal-safe per POSIX.
        // pgid is non-zero (checked above). Negative pgid means "process group".
        unsafe {
            libc::kill(-(pgid as libc::pid_t), libc::SIGINT);
        }
    }
}

/// RAII guard that manages SIGINT handling during pipe chain execution.
///
/// Installs a signal handler that forwards SIGINT to the child process
/// group via `kill(-pgid, SIGINT)`. The original SIGINT disposition is
/// restored on drop.
///
/// Children are in their own process group (set up by `spawn_block`).
/// creft stays in the shell's process group and never transfers terminal
/// foreground ownership. This avoids races with shell job control (zsh
/// in particular) that cause SIGTTOU suspension.
#[cfg(unix)]
struct PipeSignalGuard {
    original_handler: libc::sighandler_t,
}

#[cfg(unix)]
impl PipeSignalGuard {
    fn new(child_pgid: u32) -> Self {
        // Store the child pgid for the forwarding handler.
        PIPE_CHILD_PGID.store(child_pgid, std::sync::atomic::Ordering::SeqCst);

        // Install the forwarding handler, saving the previous disposition for
        // restoration in Drop. creft ignores SIGINT while waiting for children;
        // the handler forwards any SIGINT to the child process group instead.
        //
        // SAFETY: sigint_forward_handler is an extern "C" fn and is
        // async-signal-safe (only calls atomic load and kill). Casting to
        // sighandler_t is the standard way to install a signal handler via
        // libc::signal.
        let original_handler = unsafe {
            libc::signal(
                libc::SIGINT,
                sigint_forward_handler as *const () as libc::sighandler_t,
            )
        };

        Self { original_handler }
    }
}

#[cfg(unix)]
impl Drop for PipeSignalGuard {
    fn drop(&mut self) {
        // Restore the original SIGINT handler.
        // SAFETY: libc::signal with the previously-saved handler value is
        // the standard way to restore signal disposition.
        unsafe {
            libc::signal(libc::SIGINT, self.original_handler);
        }
        // Clear the atomic so a stale handler (if somehow called after drop)
        // does not forward to a dead process group.
        PIPE_CHILD_PGID.store(0, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Execute all blocks in a pipe:true command concurrently with OS-level pipe
/// connections between stdout/stdin.
///
/// All blocks are spawned before any are waited on. Block N's stdout is
/// connected to block N+1's stdin via Stdio::from(ChildStdout). The last
/// block's stdout is inherited (prints to terminal).
///
/// Returns Ok(()) if the last block exits successfully. Earlier blocks dying
/// from SIGPIPE when the downstream consumer exits early is normal pipeline
/// behavior and is not reported as an error unless the last block also fails.
fn run_pipe_chain(
    cmd: &ParsedCommand,
    bound_refs: &[(&str, &str)],
    extra_env: &[(&str, &str)],
    cwd: &Path,
) -> Result<(), CreftError> {
    let n = cmd.blocks.len();

    // Temp files must outlive all child processes; keep them alive until after wait().
    let mut temp_files: Vec<tempfile::NamedTempFile> = Vec::with_capacity(n);
    for block in &cmd.blocks {
        // In pipe mode, no "prev" template arg (output is on stdin).
        let expanded = substitute(&block.code, bound_refs, &block.lang)?;
        let tmp = prepare_block_script(block, &expanded)?;
        temp_files.push(tmp);
    }

    // Pipe mode passes no CREFT_PREV/CREFT_BLOCK_N — output flows on stdin, not env.
    let env_vars: Vec<(&str, &str)> = extra_env.to_vec();
    // node_deps_dirs keeps npm-installed tempdir handles alive until all children exit.
    let mut node_deps_dirs: Vec<Option<tempfile::TempDir>> = Vec::with_capacity(n);
    let mut children: Vec<(std::process::Child, usize, String)> = Vec::with_capacity(n);
    let mut prev_stdout: Option<std::process::ChildStdout> = None;
    // PID of the first child, used as the process group ID for all pipe children.
    #[cfg(unix)]
    let mut child_pgid: Option<u32> = None;

    for (i, block) in cmd.blocks.iter().enumerate() {
        let script_path = temp_files[i].path();

        let stdin_cfg = match prev_stdout.take() {
            // Block 0: inherit parent stdin (or /dev/null if none).
            None => std::process::Stdio::inherit(),
            // Intermediate + last blocks: fd from previous child's stdout.
            Some(stdout) => std::process::Stdio::from(stdout),
        };

        let is_last = i == n - 1;
        let stdout_cfg = if is_last {
            // Last block: print directly to terminal.
            std::process::Stdio::inherit()
        } else {
            // Intermediate blocks: pipe stdout to next block.
            std::process::Stdio::piped()
        };

        // Some(0) → block 0 creates its own process group; Some(pgid) → join it.
        #[cfg(unix)]
        let pg = if i == 0 { Some(0u32) } else { child_pgid };

        // Non-first blocks ignore SIGINT so only the pipe head receives Ctrl+C.
        // When the head dies, downstream blocks get EOF/SIGPIPE and exit cleanly
        // without printing raw language-level tracebacks (e.g. Python KeyboardInterrupt).
        #[cfg(unix)]
        let sigint_ignored = i > 0;

        let (mut child, node_deps_dir) = spawn_block(
            block,
            script_path,
            &env_vars,
            stdin_cfg,
            stdout_cfg,
            cwd,
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

        // After spawning block 0, record its PID as the process group ID.
        // (The pre_exec setpgid(0, 0) makes block 0's PID its own PGID.)
        #[cfg(unix)]
        if i == 0 {
            child_pgid = Some(child.id());
        }

        if !is_last {
            prev_stdout = child.stdout.take().or_else(|| {
                // Should never happen for Stdio::piped(), but handle defensively.
                None
            });
            if prev_stdout.is_none() {
                // Programming error: Stdio::piped() must yield a ChildStdout.
                #[cfg(unix)]
                if let Some(pgid) = child_pgid {
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
        }

        children.push((child, i, block.lang.clone()));
    }

    // Install SIGINT handler for the pipe chain duration (Unix only).
    // The guard sets up SIGINT forwarding to the child process group and
    // restores the original handler when dropped (RAII).
    #[cfg(unix)]
    let _sigint_guard = child_pgid.map(PipeSignalGuard::new);

    struct PipeResult {
        block: usize,
        lang: String,
        status: std::process::ExitStatus,
    }

    let mut results: Vec<PipeResult> = Vec::with_capacity(n);
    for (mut child, block_idx, lang) in children {
        let status = child.wait().map_err(CreftError::Io)?;
        results.push(PipeResult {
            block: block_idx,
            lang,
            status,
        });
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

/// Execute a single code block, capturing and echoing stdout.
///
/// Returns captured stdout as a `String`. Output is also printed to the
/// terminal so the user sees it in real time.
fn execute_block(
    block: &CodeBlock,
    code: &str,
    block_idx: usize,
    env_vars: &[(&str, &str)],
    cwd: &Path,
) -> Result<String, CreftError> {
    let tmp = prepare_block_script(block, code)?;
    let tmp_path = tmp.path().to_path_buf();

    let (child, _node_deps_dir) = spawn_block(
        block,
        &tmp_path,
        env_vars,
        std::process::Stdio::inherit(),
        std::process::Stdio::piped(),
        cwd,
        #[cfg(unix)]
        None, // sequential mode: no process group management
        #[cfg(unix)]
        false, // sequential mode: do not suppress SIGINT
    )?;
    // _node_deps_dir kept alive here so the npm-installed node_modules directory
    // is not deleted before the child process finishes.
    let output = child.wait_with_output().map_err(CreftError::Io)?;

    if exit_code_of(&output.status) == Some(EARLY_EXIT) {
        // Print any output produced before the early exit so it is not lost.
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

/// Simple which(1) equivalent.
fn which(name: &str) -> Option<std::path::PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|p| p.join(name))
            .find(|p| p.exists())
    })
}

/// Run a full parsed command with additional environment variables injected
/// into every child process.
///
/// `extra_env` entries are prepended to the per-block env vars so they are
/// visible to every block, including the first. Block-specific vars
/// (`CREFT_PREV`, `CREFT_BLOCK_N`) follow after.
///
/// Use this when a runtime feature (e.g. dry-run delegation) needs to signal
/// to the child process that a special mode is active.
pub fn run_with_env(
    cmd: &ParsedCommand,
    raw_args: &[String],
    extra_env: &[(&str, &str)],
    cwd: &Path,
) -> Result<(), CreftError> {
    if cmd.blocks.is_empty() {
        return Err(CreftError::NoCodeBlocks);
    }

    check_env(cmd)?;

    let (bound, _passthrough) = parse_and_bind(cmd, raw_args)?;
    let bound_refs: Vec<(&str, &str)> = bound
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();

    // pipe:true requires 2+ blocks; a single-block pipe falls through to sequential.
    if cmd.def.pipe && cmd.blocks.len() > 1 {
        return run_pipe_chain(cmd, &bound_refs, extra_env, cwd);
    }

    let mut prev_output = String::new();
    let mut block_outputs: Vec<String> = Vec::new();

    for (i, block) in cmd.blocks.iter().enumerate() {
        let prev_owned = prev_output.trim_end().to_string();
        let mut args_with_prev = bound_refs.clone();
        args_with_prev.push(("prev", &prev_owned));

        let expanded = substitute(&block.code, &args_with_prev, &block.lang)?;

        // extra_env is prepended so CREFT_DRY_RUN (and any future injected vars)
        // are visible to every block, including block 0.
        let mut env_vars: Vec<(&str, &str)> = extra_env.to_vec();

        let prev_trimmed = prev_output.trim_end();
        if i > 0 {
            env_vars.push(("CREFT_PREV", prev_trimmed));
        }
        // Declared at loop scope so the string data outlives the env_vars references.
        let block_env_keys: Vec<String> = (0..block_outputs.len())
            .map(|idx| format!("CREFT_BLOCK_{}", idx + 1))
            .collect();
        for (idx, key) in block_env_keys.iter().enumerate() {
            env_vars.push((key.as_str(), block_outputs[idx].trim_end()));
        }

        let output = match execute_block(block, &expanded, i, &env_vars, cwd) {
            Ok(out) => out,
            // Exit 99: stop the pipeline and return success to the caller.
            Err(CreftError::EarlyExit) => return Ok(()),
            Err(e) => return Err(e),
        };

        prev_output = output.clone();
        block_outputs.push(output);
    }

    Ok(())
}

/// Write rendered (substituted) blocks to stderr for diagnostic inspection.
///
/// Called when `--verbose` is active. Each block is shown with `===` delimiters
/// so the output is visually distinct from `--dry-run`'s `---` format.
pub fn render_blocks(cmd: &ParsedCommand, bound_refs: &[(&str, &str)]) -> Result<(), CreftError> {
    for (i, block) in cmd.blocks.iter().enumerate() {
        let expanded = substitute(&block.code, bound_refs, &block.lang)?;
        eprintln!("=== block {} ({}) ===", i + 1, block.lang);
        eprintln!("{}", expanded);
        eprintln!("=== end ===");
    }
    Ok(())
}

/// Print the expanded code for each block without executing it.
pub fn dry_run(cmd: &ParsedCommand, raw_args: &[String], cwd: &Path) -> Result<(), CreftError> {
    check_env(cmd)?;
    let (bound, _passthrough) = parse_and_bind(cmd, raw_args)?;
    let bound_refs: Vec<(&str, &str)> = bound
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();

    eprintln!("cwd: {}", cwd.display());

    for (i, block) in cmd.blocks.iter().enumerate() {
        let expanded = substitute(&block.code, &bound_refs, &block.lang)?;
        if cmd.blocks.len() > 1 {
            eprintln!("--- block {} ({}) ---", i + 1, block.lang);
        }
        if !block.deps.is_empty() {
            eprintln!("deps: {}", block.deps.join(", "));
        }
        println!("{}", expanded);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn test_substitute_basic() {
        // Safe value: no shell escaping changes the output for plain alphanumeric
        let result = substitute("Hello, {{name}}!", &[("name", "World")], "bash").unwrap();
        assert_eq!(result, "Hello, World!");
    }

    #[test]
    fn test_substitute_multiple() {
        let result = substitute("{{a}} and {{b}}", &[("a", "foo"), ("b", "bar")], "bash").unwrap();
        assert_eq!(result, "foo and bar");
    }

    #[test]
    fn test_substitute_default() {
        // Default value — author-controlled, not escaped
        let result = substitute("Hello, {{name|World}}!", &[], "bash").unwrap();
        assert_eq!(result, "Hello, World!");
    }

    #[test]
    fn test_substitute_default_overridden() {
        // User-supplied value in bash: "Chris" has no metacharacters so output is unchanged
        let result = substitute("Hello, {{name|World}}!", &[("name", "Chris")], "bash").unwrap();
        assert_eq!(result, "Hello, Chris!");
    }

    #[test]
    fn test_substitute_missing() {
        let result = substitute("Hello, {{name}}!", &[], "bash");
        assert!(result.is_err());
    }

    #[test]
    fn test_substitute_no_double_replace() {
        // In bash mode, the substituted value '{{b}}' gets shell-escaped to `'{{b}}'`
        // which is fine — the point is that the inner {{b}} is NOT re-expanded
        let result = substitute("{{a}}", &[("a", "{{b}}"), ("b", "NOPE")], "bash").unwrap();
        // shell_escape wraps in single quotes: '{{b}}'
        assert_eq!(result, "'{{b}}'");
    }

    // ---- shell escaping tests ----

    #[test]
    fn test_shell_escape_subshell_injection_bash() {
        // Command injection attempt must be neutralized for bash
        let result = substitute("echo {{name}}", &[("name", "$(whoami)")], "bash").unwrap();
        // shell_escape produces single-quoted literal: '$(whoami)'
        assert_eq!(result, "echo '$(whoami)'");
    }

    #[test]
    fn test_shell_escape_subshell_injection_sh() {
        let result = substitute("echo {{name}}", &[("name", "$(whoami)")], "sh").unwrap();
        assert_eq!(result, "echo '$(whoami)'");
    }

    #[test]
    fn test_shell_escape_subshell_injection_zsh() {
        let result = substitute("echo {{name}}", &[("name", "$(whoami)")], "zsh").unwrap();
        assert_eq!(result, "echo '$(whoami)'");
    }

    #[test]
    fn test_no_shell_escape_python() {
        // Non-shell language: raw value, no escaping
        let result = substitute("print('{{name}}')", &[("name", "$(whoami)")], "python").unwrap();
        assert_eq!(result, "print('$(whoami)')");
    }

    #[test]
    fn test_no_shell_escape_node() {
        let result =
            substitute("console.log('{{name}}')", &[("name", "$(whoami)")], "node").unwrap();
        assert_eq!(result, "console.log('$(whoami)')");
    }

    #[test]
    fn test_no_shell_escape_python3() {
        let result = substitute("print('{{name}}')", &[("name", "$(whoami)")], "python3").unwrap();
        assert_eq!(result, "print('$(whoami)')");
    }

    #[test]
    fn test_shell_escape_default_not_escaped() {
        // Author-supplied default value is NOT escaped even in bash
        let result = substitute("echo {{name|default_val}}", &[], "bash").unwrap();
        assert_eq!(result, "echo default_val");
    }

    #[test]
    fn test_shell_escape_default_with_metachar_not_escaped() {
        // Author can put shell code in defaults — not escaped
        let result = substitute("echo {{name|$(date)}}", &[], "bash").unwrap();
        assert_eq!(result, "echo $(date)");
    }

    #[test]
    fn test_shell_escape_single_quote_in_value() {
        // Embedded single quote: O'Brien -> 'O'\''Brien'
        let result = substitute("echo {{name}}", &[("name", "O'Brien")], "bash").unwrap();
        // shell_escape handles embedded single quotes
        assert_eq!(result, "echo 'O'\\''Brien'");
    }

    #[test]
    fn test_shell_escape_empty_string() {
        // Empty string -> '' (documented behavior change: unambiguous empty arg)
        let result = substitute("echo {{name}}", &[("name", "")], "bash").unwrap();
        assert_eq!(result, "echo ''");
    }

    #[test]
    fn test_shell_escape_semicolon_injection() {
        // Semicolon would allow command chaining — must be escaped
        let result = substitute("echo {{name}}", &[("name", "hello; rm -rf /")], "bash").unwrap();
        assert_eq!(result, "echo 'hello; rm -rf /'");
    }

    #[test]
    fn test_shell_no_escape_for_unknown_lang() {
        // Unknown language: no escaping (treated as non-shell)
        let result = substitute("echo {{name}}", &[("name", "$(whoami)")], "ruby").unwrap();
        assert_eq!(result, "echo $(whoami)");
    }

    #[test]
    fn test_interpreter_mapping() {
        assert_eq!(interpreter("bash"), "bash");
        assert_eq!(interpreter("python"), "python3");
        assert_eq!(interpreter("node"), "node");
        assert_eq!(interpreter("unknown"), "unknown");
    }

    use crate::model::{Arg, CommandDef, Flag};

    fn make_cmd(args: Vec<Arg>, flags: Vec<Flag>) -> ParsedCommand {
        ParsedCommand {
            def: CommandDef {
                name: "test".into(),
                description: "test cmd".into(),
                args,
                flags,
                env: vec![],
                tags: vec![],
                pipe: false,
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
                pipe: false,
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
                pipe: false,
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
        assert_eq!(extension("ruby"), "rb");
        assert_eq!(extension("rb"), "rb");
        assert_eq!(extension("perl"), "pl");
        assert_eq!(extension("unknown"), "unknown");
    }

    // ---- interpreter mapping for ts/ruby/perl ----

    #[test]
    fn test_interpreter_mapping_all() {
        assert_eq!(interpreter("sh"), "sh");
        assert_eq!(interpreter("zsh"), "zsh");
        assert_eq!(interpreter("python3"), "python3");
        assert_eq!(interpreter("javascript"), "node");
        assert_eq!(interpreter("js"), "node");
        assert_eq!(interpreter("typescript"), "npx tsx");
        assert_eq!(interpreter("ts"), "npx tsx");
        assert_eq!(interpreter("ruby"), "ruby");
        assert_eq!(interpreter("rb"), "ruby");
        assert_eq!(interpreter("perl"), "perl");
    }

    // ---- should_shell_escape ----

    #[test]
    fn test_should_shell_escape_langs() {
        assert!(should_shell_escape("bash"));
        assert!(should_shell_escape("sh"));
        assert!(should_shell_escape("zsh"));
        assert!(!should_shell_escape("python"));
        assert!(!should_shell_escape("node"));
        assert!(!should_shell_escape("ruby"));
    }

    // ---- stdin thread: pipe mode tests ----

    use crate::model::CodeBlock;

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
                pipe: true,
                supports: vec![],
            },
            docs: None,
            blocks: vec![
                CodeBlock {
                    lang: "bash".into(),
                    code: block0_code.into(),
                    deps: vec![],
                },
                CodeBlock {
                    lang: "bash".into(),
                    code: block1_code.into(),
                    deps: vec![],
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
        let result = run_with_env(&cmd, &[], &[], cwd);
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
        let result = run_with_env(&cmd, &[], &[], cwd);
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
        let result = run_with_env(&cmd, &[], &[], cwd);
        assert!(
            result.is_ok(),
            "empty stdin pipe should succeed: {:?}",
            result
        );
    }

    #[test]
    fn test_non_pipe_mode_unaffected() {
        // A two-block non-pipe command must still work correctly.
        // Block 0 prints "hello". Block 1 prints "world".
        // No stdin threading should occur (prev_stdin is None for both blocks).
        let cmd = ParsedCommand {
            def: CommandDef {
                name: "test-nopipe".into(),
                description: "test non-pipe".into(),
                args: vec![],
                flags: vec![],
                env: vec![],
                tags: vec![],
                pipe: false,
                supports: vec![],
            },
            docs: None,
            blocks: vec![
                CodeBlock {
                    lang: "bash".into(),
                    code: "echo hello".into(),
                    deps: vec![],
                },
                CodeBlock {
                    lang: "bash".into(),
                    code: "echo world".into(),
                    deps: vec![],
                },
            ],
        };
        let cwd = std::path::Path::new("/tmp");
        let result = run_with_env(&cmd, &[], &[], cwd);
        assert!(
            result.is_ok(),
            "non-pipe multi-block command must still work: {:?}",
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
                pipe: true,
                supports: vec![],
            },
            docs: None,
            blocks: blocks
                .iter()
                .map(|code| CodeBlock {
                    lang: "bash".into(),
                    code: code.to_string(),
                    deps: vec![],
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
        let result = run_with_env(&cmd, &[], &[], cwd);
        assert!(
            result.is_ok(),
            "three-block pipe chain should succeed: {:?}",
            result
        );
    }

    #[test]
    fn test_pipe_single_block_passthrough() {
        // pipe: true with only one block falls through to sequential path.
        // The single block runs normally and its output reaches the terminal.
        let cmd = ParsedCommand {
            def: CommandDef {
                name: "test-pipe-single".into(),
                description: "single block pipe".into(),
                args: vec![],
                flags: vec![],
                env: vec![],
                tags: vec![],
                pipe: true, // pipe: true but only one block — should be a no-op
                supports: vec![],
            },
            docs: None,
            blocks: vec![CodeBlock {
                lang: "bash".into(),
                code: "echo single".into(),
                deps: vec![],
            }],
        };
        let cwd = std::path::Path::new("/tmp");
        let result = run_with_env(&cmd, &[], &[], cwd);
        assert!(
            result.is_ok(),
            "single-block pipe:true must succeed (falls through to sequential): {:?}",
            result
        );
    }

    #[test]
    fn test_pipe_block0_fails() {
        // Block 0 exits non-zero. Block 1 reads EOF on stdin (block 0 died),
        // and succeeds (wc -l prints 0). Because the LAST block succeeds,
        // the overall result is Ok (last block exit determines success).
        // When block 0 AND the last block both fail, the error is from block 0.
        let cmd = make_pipe_cmd_multi(&["exit 1", "wc -l"]);
        let cwd = std::path::Path::new("/tmp");
        let result = run_with_env(&cmd, &[], &[], cwd);
        // Block 1 (wc -l) succeeds because it just sees EOF on stdin.
        // Last block exit determines the result; wc -l exits 0, so overall Ok.
        assert!(
            result.is_ok(),
            "when last block succeeds, overall result is Ok even if block 0 fails: {:?}",
            result
        );
    }

    #[test]
    fn test_pipe_last_block_fails_reports_error() {
        // Block 0 succeeds. Block 1 exits non-zero. The last block's exit status
        // determines the result — should return ExecutionFailed.
        let cmd = make_pipe_cmd_multi(&["echo hello", "exit 1"]);
        let cwd = std::path::Path::new("/tmp");
        let result = run_with_env(&cmd, &[], &[], cwd);
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
        // This tests the signal detection path in execute_block (sequential mode).
        let cmd = ParsedCommand {
            def: CommandDef {
                name: "test-signal".into(),
                description: "signal test".into(),
                args: vec![],
                flags: vec![],
                env: vec![],
                tags: vec![],
                pipe: false,
                supports: vec![],
            },
            docs: None,
            blocks: vec![CodeBlock {
                lang: "bash".into(),
                // Self-terminate with SIGTERM. bash propagates the signal exit.
                code: "kill -TERM $$".into(),
                deps: vec![],
            }],
        };
        let cwd = std::path::Path::new("/tmp");
        let result = run_with_env(&cmd, &[], &[], cwd);
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
        let err = crate::error::enrich_io_error(e2big, "CREFT_PREV");
        assert!(
            matches!(err, crate::error::CreftError::Setup(_)),
            "E2BIG should produce a Setup error with guidance, got: {:?}",
            err
        );
        let msg = err.to_string();
        assert!(
            msg.contains("pipe: true"),
            "E2BIG message should suggest 'pipe: true', got: {msg}"
        );
        assert!(
            msg.contains("CREFT_PREV"),
            "E2BIG message should include context 'CREFT_PREV', got: {msg}"
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
        let result = run_with_env(&cmd, &[], &[], cwd);
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
}
