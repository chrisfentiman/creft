use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock};

use crate::error::CreftError;
use crate::model::{CodeBlock, LlmConfig, ParsedCommand};

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
        }
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

    /// Borrow the environment variable vec for cloning or inspection.
    pub(crate) fn env(&self) -> &Vec<(String, String)> {
        &self.env
    }
}

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
            // No matching arg/flag — leave as literal text.
            caps[0].to_string()
        }
    });

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
fn spawn_block(
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
    // Stdio is not Copy, so we apply stdin/stdout directly rather than
    // using a reusable closure (which would require Fn, not FnOnce).
    let apply_common = |cmd: &mut std::process::Command| {
        cmd.current_dir(cwd);
        for (k, v) in &env_pairs {
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
            crate::error::enrich_io_error(e, "environment")
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

/// Atomic storage for the child process group ID, used by the SIGINT
/// forwarding handler. Zero means "no active pipe chain".
#[cfg(unix)]
static PIPE_CHILD_PGID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Signal handler that forwards SIGINT to the child process group.
///
/// # Safety
///
/// Called by the OS as a signal handler; must be async-signal-safe.
/// Only async-signal-safe operations are performed:
/// - `AtomicU32::load` (no locks, no allocation)
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

/// Result from a single reaper thread (Unix pipe mode).
#[cfg(unix)]
struct ReaperResult {
    block_idx: usize,
    lang: String,
    status: Result<std::process::ExitStatus, std::io::Error>,
}

/// A stdout handle from a pipe chain stage.
///
/// Normal blocks produce a `ChildStdout`; sponge (LLM) stages produce a
/// `PipeReader` from an `os_pipe::pipe()` pair. Both can be converted to
/// `Stdio` for the next block's stdin, and both implement `Read` for the
/// relay thread.
enum PipeStdout {
    Child(std::process::ChildStdout),
    Pipe(os_pipe::PipeReader),
}

impl PipeStdout {
    fn into_stdio(self) -> std::process::Stdio {
        match self {
            PipeStdout::Child(c) => std::process::Stdio::from(c),
            PipeStdout::Pipe(p) => std::process::Stdio::from(p),
        }
    }
}

impl std::io::Read for PipeStdout {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            PipeStdout::Child(c) => c.read(buf),
            PipeStdout::Pipe(p) => p.read(buf),
        }
    }
}

/// Result from a completed pipe block.
struct PipeResult {
    block: usize,
    lang: String,
    status: std::process::ExitStatus,
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
///
/// Note: side effects that occur unconditionally at block startup (before stdin
/// is read) cannot be suppressed by exit 99, because all blocks are spawned
/// concurrently before any are waited on. This is inherent to pipe architecture
/// and applies equally to shell pipes (`false | touch /tmp/foo` creates the
/// file despite `false` exiting 1).
#[cfg(unix)]
fn wait_pipe_children_unix(
    children: Vec<(std::process::Child, usize, String)>,
    last_stdout: PipeStdout,
    child_pgid: Option<u32>,
    tx: std::sync::mpsc::Sender<ReaperResult>,
    rx: std::sync::mpsc::Receiver<ReaperResult>,
) -> Result<(Vec<PipeResult>, bool), CreftError> {
    use std::io::Read as _;

    // Spawn relay thread: reads the last block's piped stdout into a buffer.
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

    // Spawn one reaper thread per child. Each thread owns the Child and calls wait().
    for (i, (child, block_idx, lang)) in children.into_iter().enumerate() {
        let tx = tx.clone();
        std::thread::Builder::new()
            .name(format!("creft-reaper-{i}"))
            .spawn(move || {
                // child.wait() takes &mut self, so we need mut child.
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
        use std::io::Write as _;
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
fn sponge_thread(
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
    use std::io::{Read as _, Write as _};

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

/// Execute all blocks in a multi-block command concurrently with OS-level pipe
/// connections between stdout/stdin.
///
/// All blocks are spawned before any are waited on. Block N's stdout is
/// connected to block N+1's stdin via Stdio::from(PipeStdout). On Unix, the
/// last block's stdout is buffered by a relay thread; output is flushed to the
/// terminal only after confirming no block exited 99.
///
/// LLM blocks participate as sponge stages: each sponge thread reads all
/// upstream output, performs template substitution, spawns the LLM provider,
/// and relays the provider's stdout to the next block via an `os_pipe` pair.
///
/// Returns Ok(()) if the last block exits successfully. Earlier blocks dying
/// from SIGPIPE when the downstream consumer exits early is normal pipeline
/// behavior and is not reported as an error unless the last block also fails.
fn run_pipe_chain(
    cmd: &ParsedCommand,
    bound_refs: &[(&str, &str)],
    ctx: &RunContext,
) -> Result<(), CreftError> {
    let n = cmd.blocks.len();

    // Temp files for non-LLM blocks. LLM blocks do not use script files —
    // the prompt is written directly to the provider's stdin by the sponge thread.
    // Use Option<NamedTempFile> so the index aligns with block indices.
    let mut temp_files: Vec<Option<tempfile::NamedTempFile>> = Vec::with_capacity(n);
    for block in &cmd.blocks {
        if block.lang == "llm" {
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
        if block.lang == "llm" {
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

            let llm_config = block
                .llm_config
                .clone()
                .expect("llm block without llm_config; validation must gate this");
            let owned_bound_refs: Vec<(String, String)> = bound_refs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            let ctx_clone = ctx.clone();
            let reaper_tx_clone = reaper_tx.clone();
            let prompt_template = block.code.clone();

            let handle = std::thread::Builder::new()
                .name(format!("creft-sponge-{i}"))
                .spawn(move || {
                    sponge_thread(
                        upstream,
                        pipe_writer,
                        prompt_template,
                        llm_config,
                        owned_bound_refs,
                        ctx_clone,
                        i,
                        pgid_tx,
                        reaper_tx_clone,
                    );
                })
                .expect("failed to spawn sponge thread");
            sponge_handles.push(handle);

            // If block 0 is a sponge, wait for the provider to spawn before
            // continuing. The sponge's provider PID is not used as the process
            // group ID — posix_spawn() is used (no pre_exec setpgid), so the
            // provider is not a pgid leader. The first non-sponge block creates
            // the process group instead. Provider failure is handled by the
            // reaper channel — continue regardless.
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
            .expect("non-llm block must have temp file")
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

        // For intermediate blocks: pipe stdout feeds the next block's stdin.
        // For the last block on Unix: take() its stdout for the relay thread.
        // For the last block on non-Unix: stdout is already inherited.
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

    // Dispatch to platform-specific wait implementation.
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
    let tmp = prepare_block_script(block, code)?;
    let tmp_path = tmp.path().to_path_buf();

    let stdin_cfg = if stdin_data.is_some() {
        std::process::Stdio::piped()
    } else {
        std::process::Stdio::inherit()
    };

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
    )?;

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
fn build_llm_command(config: &LlmConfig) -> std::process::Command {
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

/// Execute an LLM block by piping the prompt to the provider CLI.
///
/// Returns captured stdout as a `String`. Output is also printed to the terminal.
fn execute_llm_block(
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

    // Multi-block skills always pipe. Single-block falls through to execute_block below.
    if cmd.blocks.len() > 1 {
        #[cfg(unix)]
        {
            return run_pipe_chain(cmd, &bound_refs, ctx);
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
            return run_pipe_chain(cmd, &bound_refs, ctx);
        }
    }

    // Single block execution.
    let block = &cmd.blocks[0];
    let expanded = substitute(&block.code, &bound_refs, &block.lang)?;

    if block.lang == "llm" {
        match execute_llm_block(block, &expanded, 0, ctx) {
            Ok(_) => Ok(()),
            Err(CreftError::EarlyExit) => Ok(()),
            Err(e) => Err(e),
        }
    } else {
        match execute_block(block, &expanded, 0, ctx, None) {
            Ok(_) => Ok(()),
            Err(CreftError::EarlyExit) => Ok(()),
            Err(e) => Err(e),
        }
    }
}

/// Run a full parsed command using a `RunContext`.
///
/// Returns immediately if cancellation has already been requested.
pub(crate) fn run_with_ctx(
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
pub(crate) fn dry_run_ctx(
    cmd: &ParsedCommand,
    raw_args: &[String],
    ctx: &RunContext,
) -> Result<(), CreftError> {
    dry_run(cmd, raw_args, ctx.cwd())
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
    use pretty_assertions::{assert_eq, assert_ne};

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

    /// Test helper: run a command with the given env and cwd, using a fresh (never-cancelled) RunContext.
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
        run_with_ctx(cmd, &args, &ctx)
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

        // Both instances share the same cancellation state.
        assert!(!ctx.is_cancelled());
        assert!(!cloned.is_cancelled());

        // Setting the flag is visible in both the original and clone.
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

    #[test]
    fn run_context_cancel_shared_via_arc() {
        // Setting the flag via the original Arc is visible through is_cancelled().
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
    fn test_substitute_unmatched_passes_through() {
        let result = substitute("Hello, {{name}}!", &[], "bash").unwrap();
        assert_eq!(result, "Hello, {{name}}!");
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
        // Block 0 exits non-zero. Block 1 reads EOF on stdin (block 0 died),
        // and succeeds (wc -l prints 0). Because the LAST block succeeds,
        // the overall result is Ok (last block exit determines success).
        // When block 0 AND the last block both fail, the error is from block 0.
        let cmd = make_pipe_cmd_multi(&["exit 1", "wc -l"]);
        let cwd = std::path::Path::new("/tmp");
        let result = run_for_test(&cmd, &[], &[], cwd);
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

    // ── build_llm_command tests ──────────────────────────────────────────────────

    fn llm_config(provider: &str, model: &str, params: &str) -> LlmConfig {
        LlmConfig {
            provider: provider.to_string(),
            model: model.to_string(),
            params: params.to_string(),
        }
    }

    // We test build_llm_command via format_llm_command (same logic, string form).
    // Direct Command inspection is not stable across Rust versions.

    #[test]
    fn test_build_llm_command_claude_default() {
        let config = llm_config("claude", "", "");
        let s = format_llm_command(&config);
        assert_eq!(s, "claude -p");
    }

    #[test]
    fn test_build_llm_command_claude_with_model() {
        let config = llm_config("claude", "haiku", "");
        let s = format_llm_command(&config);
        assert_eq!(s, "claude -p --model haiku");
    }

    #[test]
    fn test_build_llm_command_gemini() {
        let config = llm_config("gemini", "flash", "");
        let s = format_llm_command(&config);
        assert_eq!(s, "gemini -p -m flash");
    }

    #[test]
    fn test_build_llm_command_gemini_no_model() {
        let config = llm_config("gemini", "", "");
        let s = format_llm_command(&config);
        assert_eq!(s, "gemini -p");
    }

    #[test]
    fn test_build_llm_command_codex() {
        let config = llm_config("codex", "", "");
        let s = format_llm_command(&config);
        assert_eq!(s, "codex exec -");
    }

    #[test]
    fn test_build_llm_command_ollama() {
        let config = llm_config("ollama", "llama3", "");
        let s = format_llm_command(&config);
        assert_eq!(s, "ollama run llama3");
    }

    #[test]
    fn test_build_llm_command_ollama_no_model() {
        let config = llm_config("ollama", "", "");
        let s = format_llm_command(&config);
        assert_eq!(s, "ollama run");
    }

    #[test]
    fn test_build_llm_command_unknown_provider() {
        let config = llm_config("myai", "gpt4", "");
        let s = format_llm_command(&config);
        assert_eq!(s, "myai --model gpt4");
    }

    #[test]
    fn test_build_llm_command_unknown_provider_no_model() {
        let config = llm_config("myai", "", "");
        let s = format_llm_command(&config);
        assert_eq!(s, "myai");
    }

    #[test]
    fn test_build_llm_command_params_split() {
        let config = llm_config("claude", "", "--max-tokens 500");
        let s = format_llm_command(&config);
        assert_eq!(s, "claude -p --max-tokens 500");
    }

    #[test]
    fn test_build_llm_command_empty_provider_defaults_claude() {
        let config = llm_config("", "", "");
        let s = format_llm_command(&config);
        assert_eq!(s, "claude -p");
    }

    #[test]
    fn test_build_llm_command_params_multiple_tokens() {
        let config = llm_config("gemini", "flash", "--timeout 30 --retry 3");
        let s = format_llm_command(&config);
        assert_eq!(s, "gemini -p -m flash --timeout 30 --retry 3");
    }

    // Verify that build_llm_command actually constructs a Command with the right binary.
    #[test]
    fn test_build_llm_command_returns_correct_binary_claude() {
        let config = llm_config("claude", "", "");
        let _cmd = build_llm_command(&config);
        // Verify the binary name via format_llm_command which mirrors the match exactly.
        let formatted = format_llm_command(&config);
        assert!(
            formatted.starts_with("claude"),
            "claude binary should be first"
        );
    }

    #[test]
    fn test_build_llm_command_returns_correct_binary_ollama() {
        let config = llm_config("ollama", "mistral", "");
        let formatted = format_llm_command(&config);
        assert!(formatted.starts_with("ollama run mistral"));
    }

    #[test]
    fn test_sponge_substitute_prev() {
        // {{prev}} in an llm template must be replaced with upstream content.
        // The "llm" language tag must NOT shell-escape (no single-quoting of values).
        let result = substitute(
            "Process this: {{prev}}",
            &[("prev", "upstream content")],
            "llm",
        )
        .unwrap();
        assert_eq!(result, "Process this: upstream content");
    }

    #[test]
    fn test_sponge_substitute_prev_no_shell_escape() {
        // Shell metacharacters in prev must pass through unescaped in llm templates.
        let result =
            substitute("{{prev}}", &[("prev", "$(echo injected) `whoami`")], "llm").unwrap();
        assert_eq!(result, "$(echo injected) `whoami`");
    }
}
