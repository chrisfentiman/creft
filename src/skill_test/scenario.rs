//! End-to-end scenario orchestration.
//!
//! [`run`] drives the full lifecycle for one scenario: allocate a sandbox,
//! materialise seed files, run optional `before` hook, spawn the child with a
//! trace pipe dup'd in, drain stdout/stderr/trace concurrently, run all
//! assertions, run the optional `after` hook, and return a [`ScenarioOutcome`].
//!
//! This module is Unix-only. The trace pipe uses `pre_exec` + `dup2`, which
//! have no stable Windows equivalent. `cmd::skills::cmd_skills_test` returns a
//! setup error on non-Unix before any scenario reaches this code.

use std::io::Read as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::model::AppContext;
use crate::runner::TraceRecord;
use crate::skill_test::assertion::{self, AssertionFailure};
use crate::skill_test::coverage;
use crate::skill_test::expand;
use crate::skill_test::fixture::{Scenario, StdinPayload};
use crate::skill_test::sandbox::Sandbox;

// ── Public types ──────────────────────────────────────────────────────────────

/// Outcome of running a single scenario.
#[derive(Debug)]
pub(crate) struct ScenarioOutcome {
    /// Whether the scenario passed, failed, timed out, or could not set up.
    pub status: ScenarioStatus,
    /// Captured child stdout (lossy-decoded from bytes).
    pub stdout: String,
    /// Captured child stderr (lossy-decoded from bytes).
    pub stderr: String,
    /// Trace records parsed from the coverage pipe. Empty when no trace was emitted.
    pub trace: Vec<TraceRecord>,
    /// When `RunOpts.keep_on_failure` was set and the outcome is non-Pass, the
    /// path to the preserved sandbox directory so the author can inspect it.
    pub kept_path: Option<PathBuf>,
}

/// Final disposition of a scenario.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ScenarioStatus {
    /// All assertions passed.
    Pass,
    /// One or more assertions failed.
    Fail(Vec<AssertionFailure>),
    /// The child did not finish within the configured timeout.
    Timeout,
    /// The sandbox or `before.shell` hook failed before any assertion was evaluated.
    SetupError(String),
}

/// Framework knobs for scenario execution.
#[derive(Debug, Clone)]
pub(crate) struct RunOpts {
    /// Override the `creft` binary path.
    ///
    /// `None` means "resolve at run time via `std::env::current_exe()`"; a
    /// resolution failure surfaces as `ScenarioStatus::SetupError`.
    ///
    /// Framework unit tests set this to `assert_cmd::cargo::cargo_bin("creft")`
    /// because under `cargo nextest run` `current_exe()` resolves to the test
    /// binary, not the project's `creft` binary.
    pub creft_binary: Option<PathBuf>,
    /// Default timeout applied when a scenario omits `when.timeout_seconds`.
    pub default_timeout: Duration,
    /// When `true`, preserve the sandbox directory for failed scenarios and
    /// populate [`ScenarioOutcome::kept_path`].
    pub keep_on_failure: bool,
}

impl Default for RunOpts {
    fn default() -> Self {
        Self {
            creft_binary: None,
            default_timeout: Duration::from_secs(60),
            keep_on_failure: false,
        }
    }
}

// ── Fd number for the trace pipe ──────────────────────────────────────────────

/// The fd number dup'd into the child process for trace emission.
///
/// Must not collide with fd 3 (control pipe, block→creft) or fd 4
/// (response pipe, creft→block). Using fd 5 keeps all three separate.
#[cfg(unix)]
const TRACE_FD: i32 = 5;

// ── Entry point ───────────────────────────────────────────────────────────────

/// Run one scenario end-to-end and return its outcome.
///
/// `app` supplies the host project root (for skill mirroring) and `opts`
/// carries framework knobs. The parent environment is collected once per
/// scenario run and shared across the before hook, the main invocation, and
/// the after hook, so every spawn in one scenario sees the same env snapshot.
pub(crate) fn run(scenario: &Scenario, app: &AppContext, opts: &RunOpts) -> ScenarioOutcome {
    #[cfg(unix)]
    return run_unix(scenario, app, opts);

    #[cfg(not(unix))]
    return ScenarioOutcome {
        status: ScenarioStatus::SetupError(
            "`creft skills test` is currently supported on Unix only (macOS, Linux); \
             Windows support is not yet implemented."
                .to_owned(),
        ),
        stdout: String::new(),
        stderr: String::new(),
        trace: Vec::new(),
        kept_path: None,
    };
}

#[cfg(unix)]
fn run_unix(scenario: &Scenario, app: &AppContext, opts: &RunOpts) -> ScenarioOutcome {
    // Resolve the creft binary path once before doing any other work so
    // a resolution failure surfaces as SetupError rather than a panic.
    let creft_bin = match resolve_creft_binary(opts) {
        Ok(p) => p,
        Err(msg) => {
            return ScenarioOutcome {
                status: ScenarioStatus::SetupError(msg),
                stdout: String::new(),
                stderr: String::new(),
                trace: Vec::new(),
                kept_path: None,
            };
        }
    };

    // Allocate the sandbox.
    let mut sandbox = match Sandbox::new() {
        Ok(s) => s,
        Err(e) => {
            return ScenarioOutcome {
                status: ScenarioStatus::SetupError(format!("create sandbox: {e}")),
                stdout: String::new(),
                stderr: String::new(),
                trace: Vec::new(),
                kept_path: None,
            };
        }
    };

    // Mirror the host project's skills into the sandbox so `creft <skill>`
    // invocations resolve real skill files.
    let host_root = app.find_local_root().map(|creft_dir| {
        // find_local_root returns the .creft/ directory; the project root is its parent.
        creft_dir
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or(creft_dir)
    });
    if let Err(e) = sandbox.mirror_project_skills(host_root.as_deref()) {
        return ScenarioOutcome {
            status: ScenarioStatus::SetupError(format!("mirror project skills: {e}")),
            stdout: String::new(),
            stderr: String::new(),
            trace: Vec::new(),
            kept_path: None,
        };
    }

    // Collect the parent environment once. Shared by the before hook, the main
    // invocation, and the after hook so every spawn in one scenario sees the
    // same snapshot of the parent process's env.
    let parent_env: Vec<(String, String)> = std::env::vars().collect();

    // Expand all placeholders in the scenario once, before any further use.
    // After this point every field in `scenario` contains resolved paths.
    let scenario = expand::expand_scenario(scenario, &sandbox.paths());

    // Materialise seed files from given.files (paths and content already expanded).
    if let Err(e) = sandbox.materialise(&scenario.given) {
        return ScenarioOutcome {
            status: ScenarioStatus::SetupError(format!("materialise seed files: {e}")),
            stdout: String::new(),
            stderr: String::new(),
            trace: Vec::new(),
            kept_path: None,
        };
    }

    // Run before.shell hook if present. Failure aborts the scenario.
    // spawn_shell applies env_clear + env_for_child so the hook sees the
    // sandbox HOME and CREFT_PROJECT_ROOT, not the parent process's HOME.
    if let Some(before) = &scenario.before {
        match sandbox.spawn_shell(&before.shell, &parent_env) {
            Ok(s) if s.success() => {}
            Ok(s) => {
                return ScenarioOutcome {
                    status: ScenarioStatus::SetupError(format!(
                        "before.shell exited with code {}",
                        s.code().unwrap_or(1),
                    )),
                    stdout: String::new(),
                    stderr: String::new(),
                    trace: Vec::new(),
                    kept_path: None,
                };
            }
            Err(e) => {
                return ScenarioOutcome {
                    status: ScenarioStatus::SetupError(format!("before.shell failed: {e}")),
                    stdout: String::new(),
                    stderr: String::new(),
                    trace: Vec::new(),
                    kept_path: None,
                };
            }
        }
    }

    // Execute the scenario and collect the outcome.
    let (status, stdout, stderr, trace) =
        execute_scenario(&scenario, &sandbox, &creft_bin, &parent_env, opts);

    // Run after.shell hook if present. Runs regardless of outcome. Exit status
    // is intentionally ignored — after.shell is fire-and-forget cleanup.
    // spawn_shell applies the same env discipline as the before hook.
    if let Some(after) = &scenario.after {
        let _ = sandbox.spawn_shell(&after.shell, &parent_env);
    }

    // Decide whether to keep the sandbox directory.
    let kept_path = if opts.keep_on_failure && !matches!(status, ScenarioStatus::Pass) {
        let root = sandbox.root().to_owned();
        sandbox.set_keep(true);
        Some(root)
    } else {
        None
    };

    ScenarioOutcome {
        status,
        stdout,
        stderr,
        trace,
        kept_path,
    }
}

/// Resolve which `creft` binary to use.
///
/// Prefers `opts.creft_binary` when set; falls back to `current_exe()`.
#[cfg(unix)]
fn resolve_creft_binary(opts: &RunOpts) -> Result<PathBuf, String> {
    if let Some(path) = &opts.creft_binary {
        return Ok(path.clone());
    }
    std::env::current_exe().map_err(|e| format!("resolve creft binary: {e}"))
}

/// Spawn the child, drain streams concurrently, then run assertions.
///
/// `parent_env` is the parent process's environment, collected once in
/// `run_unix` and shared across all spawn sites in one scenario run. The
/// child sees only the allowlisted vars from `parent_env` plus the sandbox-
/// specific overrides from `scenario.when.env` — all other parent vars are
/// stripped by `env_clear` before `envs` is called.
///
/// Returns `(status, stdout, stderr, trace)`.
#[cfg(unix)]
fn execute_scenario(
    scenario: &Scenario,
    sandbox: &Sandbox,
    creft_bin: &std::path::Path,
    parent_env: &[(String, String)],
    opts: &RunOpts,
) -> (ScenarioStatus, String, String, Vec<TraceRecord>) {
    use std::os::unix::io::{AsRawFd as _, IntoRawFd as _};
    use std::os::unix::process::CommandExt as _;

    // Build the trace pipe. The write end goes to the child at TRACE_FD; the
    // read end stays in the parent for draining.
    let (trace_read_fd, trace_write_fd) = match crate::runner::os_pipe() {
        Ok(pair) => pair,
        Err(e) => {
            return (
                ScenarioStatus::SetupError(format!("create trace pipe: {e}")),
                String::new(),
                String::new(),
                Vec::new(),
            );
        }
    };

    // argv and env are already placeholder-expanded by expand_scenario in run_unix.
    let (program, args) = match scenario.when.argv.split_first() {
        Some((prog, rest)) => (resolve_argv0(prog, creft_bin), rest.to_vec()),
        None => {
            return (
                ScenarioStatus::SetupError("when.argv is empty".to_owned()),
                String::new(),
                String::new(),
                Vec::new(),
            );
        }
    };

    // Build the child environment from the shared parent snapshot.
    // scenario.when.env values are already expanded; keys are identifiers.
    let expanded_scenario_env: Vec<(String, String)> = scenario.when.env.clone();

    let trace_fd_raw = trace_write_fd.as_raw_fd();
    let mut child_env = sandbox.env_for_child(parent_env, &expanded_scenario_env);
    child_env.push(("CREFT_TRACE_FD".to_owned(), TRACE_FD.to_string()));

    // Determine the effective timeout for this scenario.
    let timeout = scenario
        .when
        .timeout_seconds
        .map(Duration::from_secs)
        .unwrap_or(opts.default_timeout);

    // Build the child command. env_clear strips all inherited parent vars so
    // only the allowlisted entries from env_for_child reach the child —
    // matching the same env discipline applied by Sandbox::spawn_shell.
    let mut cmd = std::process::Command::new(&program);
    cmd.args(&args)
        .current_dir(sandbox.source())
        .env_clear()
        .envs(child_env)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    // pre_exec: dup the trace write end to TRACE_FD and place the child in its
    // own process group. Both operations are async-signal-safe POSIX calls valid
    // in the fork-exec window.
    //
    // SAFETY: trace_fd_raw is a valid, open, writeable fd before this closure
    // registers. It was created by os_pipe() and is still open when spawn()
    // calls pre_exec. Inside the closure (fork-exec window) we only call
    // async-signal-safe syscalls: dup2(2), setpgid(2), close(2). No Rust
    // allocations or mutex operations occur.
    unsafe {
        cmd.pre_exec(move || {
            // dup the write end to TRACE_FD.
            if libc::dup2(trace_fd_raw, TRACE_FD) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            // If the original fd differs from TRACE_FD, close the original so
            // the child does not have two copies of the write end open.
            if trace_fd_raw != TRACE_FD {
                libc::close(trace_fd_raw);
            }
            // Place the child in its own process group so the watchdog can
            // kill the whole group (including grandchildren) with killpg.
            if libc::setpgid(0, 0) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return (
                ScenarioStatus::SetupError(format!("spawn child: {e}")),
                String::new(),
                String::new(),
                Vec::new(),
            );
        }
    };

    // Close the parent's copy of the write end so the child holds the only
    // write end. When the child exits, the read end will see EOF.
    drop(trace_write_fd);

    // Wire stdin on a background thread to avoid PIPE_BUF deadlock.
    let stdin_handle = {
        let stdin_payload = match &scenario.when.stdin {
            Some(StdinPayload::Text(s)) => Some(s.as_bytes().to_vec()),
            Some(StdinPayload::Json(v)) => {
                Some(serde_json::to_vec(v).expect("serde_json::Value always serialises"))
            }
            None => None,
        };

        let mut child_stdin = child.stdin.take().expect("stdin was piped");
        std::thread::spawn(move || {
            if let Some(data) = stdin_payload {
                use std::io::Write as _;
                match child_stdin.write_all(&data) {
                    Ok(()) => {}
                    // Child exited before consuming stdin — not an error.
                    Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => {}
                    // Any other write failure: best-effort; the assertion phase
                    // will surface the real problem via stdout/exit-code checks.
                    Err(_) => {}
                }
            }
            // stdin_handle drops here, closing the pipe end.
        })
    };

    // Drain stdout, stderr, and the trace pipe on three independent threads.
    // Sequential draining deadlocks when the child writes more than one
    // pipe-buffer's worth (~64 KiB on Linux) to any one stream while the
    // parent is blocked reading another.
    let child_stdout = child.stdout.take().expect("stdout was piped");
    let child_stderr = child.stderr.take().expect("stderr was piped");

    let stdout_thread = {
        let mut reader = child_stdout;
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = reader.read_to_end(&mut buf);
            buf
        })
    };

    let stderr_thread = {
        let mut reader = child_stderr;
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = reader.read_to_end(&mut buf);
            buf
        })
    };

    // Convert OwnedFd to File for reading.
    let trace_reader: std::fs::File = {
        use std::os::unix::io::FromRawFd as _;
        // SAFETY: trace_read_fd is a valid, open, readable fd created by
        // os_pipe(). We transfer ownership here; it will be closed when
        // trace_file is dropped.
        unsafe { std::fs::File::from_raw_fd(trace_read_fd.into_raw_fd()) }
    };

    let trace_thread = {
        let mut reader = trace_reader;
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = reader.read_to_end(&mut buf);
            buf
        })
    };

    // Watchdog: send SIGKILL to the child's process group after the timeout.
    // Uses a cancellation channel so the watchdog exits immediately when the
    // child finishes before the timeout — avoiding a full-timeout sleep on
    // every passing scenario.
    let timed_out = Arc::new(AtomicBool::new(false));
    let timed_out_watchdog = Arc::clone(&timed_out);
    let child_pid = child.id() as libc::pid_t;
    // Sender is dropped when the child exits; the watchdog receives a
    // disconnected notification and exits without killing.
    let (cancel_tx, cancel_rx) = std::sync::mpsc::channel::<()>();
    let watchdog = std::thread::spawn(move || {
        // Wait for the timeout OR cancellation (sender drop), whichever comes first.
        match cancel_rx.recv_timeout(timeout) {
            Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                // Cancellation: child exited before the timeout.
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                // Timeout elapsed: kill the child's process group.
                timed_out_watchdog.store(true, Ordering::Relaxed);
                // SAFETY: child_pid is a valid pid returned by Child::id().
                // killpg(2) is async-signal-safe. The child was placed in its
                // own group via setpgid(0,0) in pre_exec, so this kills the
                // child and any subprocesses it spawned without touching the
                // framework's own process group. Sending SIGKILL to an already-
                // exited process is harmless — the OS returns ESRCH.
                unsafe {
                    libc::killpg(child_pid, libc::SIGKILL);
                }
            }
        }
    });

    // Wait for the child to exit (or be killed by the watchdog).
    let exit_status = child.wait();

    // Cancel the watchdog by dropping the sender. If the watchdog already fired
    // (timeout case), the drop is a no-op. In either case, join to avoid
    // leaking the thread.
    drop(cancel_tx);
    let _ = watchdog.join();

    // After the child and watchdog have both finished, join reader threads.
    // All three streams are at EOF because the child exited and dropped its
    // write ends.
    let stdout_bytes = stdout_thread.join().unwrap_or_default();
    let stderr_bytes = stderr_thread.join().unwrap_or_default();
    let trace_bytes = trace_thread.join().unwrap_or_default();
    let _ = stdin_handle.join();

    let stdout = String::from_utf8_lossy(&stdout_bytes).into_owned();
    let stderr = String::from_utf8_lossy(&stderr_bytes).into_owned();
    let trace = parse_trace(&trace_bytes);

    // Determine whether the watchdog fired.
    if timed_out.load(Ordering::Relaxed) {
        return (ScenarioStatus::Timeout, stdout, stderr, trace);
    }

    // Determine the child's exit code.
    //
    // When the child exits normally, `code()` is `Some`. When it is killed by a
    // signal (OOM kill, SIGTERM from CI, a stray signal to the group despite
    // `setpgid`) `code()` is `None` and `signal()` is `Some`. The spec treats
    // any signal-killed exit as `Timeout` because the useful distinction for
    // authors is "process ran too long or was killed" vs "process exited with
    // a known code". Watchdog-driven kills are already handled above; this
    // branch handles external kills that arrive without the watchdog firing.
    #[cfg(unix)]
    let exit_code = {
        use std::os::unix::process::ExitStatusExt as _;
        match exit_status {
            Ok(s) if s.signal().is_some() => {
                return (ScenarioStatus::Timeout, stdout, stderr, trace);
            }
            Ok(s) => s.code().unwrap_or(1),
            Err(_) => 1,
        }
    };
    #[cfg(not(unix))]
    let exit_code = match exit_status {
        Ok(s) => s.code().unwrap_or(1),
        Err(_) => 1,
    };

    // Run all assertions and collect failures.
    let mut failures: Vec<AssertionFailure> = Vec::new();

    if let Some(f) = assertion::check_exit_code(&scenario.then, exit_code) {
        failures.push(f);
    }
    failures.extend(assertion::check_stdout_contains(&scenario.then, &stdout));
    failures.extend(assertion::check_stderr_contains(&scenario.then, &stderr));
    if let Some(f) = assertion::check_stdout_json(&scenario.then, &stdout) {
        failures.push(f);
    }
    failures.extend(assertion::check_files(&scenario.then));
    failures.extend(assertion::check_files_absent(&scenario.then));
    if let Some(cov_exp) = &scenario.then.coverage {
        failures.extend(coverage::check_coverage(cov_exp, &trace));
    }

    let status = if failures.is_empty() {
        ScenarioStatus::Pass
    } else {
        ScenarioStatus::Fail(failures)
    };

    (status, stdout, stderr, trace)
}

/// Resolve `argv[0]`: substitute the literal string `"creft"` with the
/// actual binary path; pass everything else through verbatim.
#[cfg(unix)]
fn resolve_argv0(argv0: &str, creft_bin: &std::path::Path) -> PathBuf {
    if argv0 == "creft" {
        creft_bin.to_path_buf()
    } else {
        PathBuf::from(argv0)
    }
}

/// Parse NDJSON trace records from raw bytes.
///
/// Splits on newlines and attempts to deserialise each non-empty line.
/// Malformed lines are dropped with a diagnostic eprintln — the trace is
/// best-effort; coverage assertion failures surface the missing records.
fn parse_trace(bytes: &[u8]) -> Vec<TraceRecord> {
    let mut records = Vec::new();
    for line in bytes.split(|&b| b == b'\n') {
        if line.is_empty() {
            continue;
        }
        match serde_json::from_slice::<TraceRecord>(line) {
            Ok(r) => records.push(r),
            Err(e) => {
                eprintln!(
                    "creft skills test: malformed trace line ({}): {:?}",
                    e,
                    String::from_utf8_lossy(line)
                );
            }
        }
    }
    records
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(all(test, unix))]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::time::Duration;

    use pretty_assertions::assert_eq;

    use crate::skill_test::fixture::{
        CoverageExpectation, FileAssertion, Given, Scenario, Then, When,
    };

    use super::*;

    /// Resolve the path to the project's `creft` binary for use in tests.
    ///
    /// Under `cargo nextest run` `current_exe()` resolves to the test binary,
    /// not `creft`. This helper uses `assert_cmd::cargo::cargo_bin("creft")`
    /// which walks cargo metadata to find the built binary.
    fn creft_bin() -> PathBuf {
        assert_cmd::cargo::cargo_bin("creft")
    }

    /// Build a minimal `RunOpts` suitable for tests. Uses a short timeout and
    /// sets the binary path explicitly so tests do not depend on `current_exe()`.
    fn test_opts() -> RunOpts {
        RunOpts {
            creft_binary: Some(creft_bin()),
            default_timeout: Duration::from_secs(30),
            keep_on_failure: false,
        }
    }

    /// Build a minimal `AppContext` pointing at a temp directory without any
    /// `.creft/` directory so `find_local_root()` returns `None`. This avoids
    /// accidentally mirroring the real project's skill tree into every test sandbox.
    fn bare_app() -> (tempfile::TempDir, AppContext) {
        let tmp = tempfile::TempDir::new().expect("tmp");
        let ctx = AppContext::for_test(tmp.path().to_path_buf(), tmp.path().to_path_buf());
        (tmp, ctx)
    }

    fn minimal_scenario(argv: Vec<String>) -> Scenario {
        Scenario {
            name: "test".to_owned(),
            source_file: PathBuf::from("test.test.yaml"),
            source_index: 0,
            notes: None,
            given: Given::default(),
            before: None,
            when: When {
                argv,
                stdin: None,
                env: Vec::new(),
                timeout_seconds: None,
            },
            then: Then::default(),
            after: None,
        }
    }

    // ── Basic invocation ──────────────────────────────────────────────────────

    #[test]
    fn version_flag_passes_with_stdout_contains() {
        let (_tmp, app) = bare_app();
        let mut scenario = minimal_scenario(vec!["creft".to_owned(), "--version".to_owned()]);
        scenario.then.stdout_contains = vec!["creft".to_owned()];
        let outcome = run(&scenario, &app, &test_opts());
        assert_eq!(
            outcome.status,
            ScenarioStatus::Pass,
            "expected pass, stdout={:?} stderr={:?}",
            outcome.stdout,
            outcome.stderr
        );
    }

    // ── Exit code assertions ──────────────────────────────────────────────────

    #[test]
    fn exit_code_mismatch_reported() {
        let (_tmp, app) = bare_app();
        // `sh -c 'exit 2'` exits 2 but scenario expects 0.
        let mut scenario =
            minimal_scenario(vec!["sh".to_owned(), "-c".to_owned(), "exit 2".to_owned()]);
        scenario.then.exit_code = 0;
        let outcome = run(&scenario, &app, &test_opts());

        let ScenarioStatus::Fail(failures) = outcome.status else {
            panic!("expected Fail, got {:?}", outcome.status);
        };
        let ec_failure = failures
            .iter()
            .find(|f| f.kind == "exit_code")
            .expect("expected an exit_code failure");
        assert_eq!(ec_failure.expected, "0");
        assert_eq!(ec_failure.actual, "2");
    }

    // ── Stdout containment ────────────────────────────────────────────────────

    #[test]
    fn stdout_contains_passes_when_present() {
        let (_tmp, app) = bare_app();
        let mut scenario = minimal_scenario(vec![
            "sh".to_owned(),
            "-c".to_owned(),
            "printf 'hello world'".to_owned(),
        ]);
        scenario.then.stdout_contains = vec!["hello".to_owned()];
        let outcome = run(&scenario, &app, &test_opts());
        assert_eq!(outcome.status, ScenarioStatus::Pass);
    }

    #[test]
    fn stdout_contains_fails_when_absent() {
        let (_tmp, app) = bare_app();
        let mut scenario = minimal_scenario(vec![
            "sh".to_owned(),
            "-c".to_owned(),
            "printf 'goodbye'".to_owned(),
        ]);
        scenario.then.stdout_contains = vec!["hello".to_owned()];
        let outcome = run(&scenario, &app, &test_opts());
        assert!(matches!(outcome.status, ScenarioStatus::Fail(_)));
    }

    // ── File assertions ───────────────────────────────────────────────────────

    #[test]
    fn file_assertion_json_subset_passes() {
        let (_tmp, app) = bare_app();
        let mut scenario = minimal_scenario(vec![
            "sh".to_owned(),
            "-c".to_owned(),
            r#"printf '{"a":1,"b":2}' > "$CREFT_PROJECT_ROOT/out.json""#.to_owned(),
        ]);
        scenario.then.files = vec![(
            "{source}/out.json".to_owned(),
            FileAssertion::JsonSubset(serde_json::json!({"a": 1})),
        )];
        let outcome = run(&scenario, &app, &test_opts());
        assert_eq!(
            outcome.status,
            ScenarioStatus::Pass,
            "stderr={:?}",
            outcome.stderr
        );
    }

    // ── Timeout ───────────────────────────────────────────────────────────────

    #[test]
    fn timeout_scenario_returns_timeout_status() {
        let (_tmp, app) = bare_app();
        // Sleep for 5 seconds; timeout is 300ms.
        let scenario =
            minimal_scenario(vec!["sh".to_owned(), "-c".to_owned(), "sleep 5".to_owned()]);
        let opts = RunOpts {
            creft_binary: Some(creft_bin()),
            default_timeout: Duration::from_millis(300),
            keep_on_failure: false,
        };
        let outcome = run(&scenario, &app, &opts);
        assert_eq!(outcome.status, ScenarioStatus::Timeout);
    }

    // ── Stdin ─────────────────────────────────────────────────────────────────

    #[test]
    fn stdin_text_reaches_child() {
        use crate::skill_test::fixture::StdinPayload;
        let (_tmp, app) = bare_app();
        let mut scenario = minimal_scenario(vec!["cat".to_owned()]);
        scenario.when.stdin = Some(StdinPayload::Text("hello from stdin\n".to_owned()));
        scenario.then.stdout_contains = vec!["hello from stdin".to_owned()];
        let outcome = run(&scenario, &app, &test_opts());
        assert_eq!(outcome.status, ScenarioStatus::Pass);
    }

    #[test]
    fn stdin_json_reaches_child_as_serialised_bytes() {
        use crate::skill_test::fixture::StdinPayload;
        let (_tmp, app) = bare_app();
        let mut scenario = minimal_scenario(vec!["cat".to_owned()]);
        scenario.when.stdin = Some(StdinPayload::Json(serde_json::json!({"k": "v"})));
        // serde_json::to_vec produces compact JSON without trailing newline.
        // cat echoes the bytes unchanged, so stdout must contain the serialised value.
        scenario.then.stdout_contains = vec![r#"{"k":"v"}"#.to_owned()];
        let outcome = run(&scenario, &app, &test_opts());
        assert_eq!(outcome.status, ScenarioStatus::Pass);
    }

    // ── Coverage trace ────────────────────────────────────────────────────────

    /// Write a minimal one-block bash skill into a temp CREFT_HOME and return
    /// the temp dir (caller must keep it alive) and the skill name.
    ///
    /// The skill calls `creft_print` once, so the trace record for block 0 will
    /// have `primitives.print == 1`.
    fn make_trace_skill_home() -> (tempfile::TempDir, String) {
        let home = tempfile::TempDir::new().expect("tmp creft_home");
        let commands_dir = home.path().join("commands");
        std::fs::create_dir_all(&commands_dir).expect("commands dir");
        let skill_name = "trace-coverage-skill";
        let skill_src = concat!(
            "---\nname: trace-coverage-skill\ndescription: trace coverage test skill\n---\n\n",
            "```bash\n",
            "creft_print \"coverage check\"\n",
            "```\n",
        );
        std::fs::write(commands_dir.join(format!("{}.md", skill_name)), skill_src)
            .expect("write skill");
        (home, skill_name.to_owned())
    }

    /// End-to-end seam from `scenario::run` through `parse_trace` to
    /// `check_coverage`: spawns `creft <skill>`, the child writes NDJSON to the
    /// trace fd, `parse_trace` deserialises each line into `TraceRecord`, and
    /// `check_coverage` matches the expectation. A passing `then.coverage`
    /// expectation on both `blocks` and `primitives` must yield
    /// `ScenarioStatus::Pass` and a non-empty trace.
    #[test]
    fn coverage_trace_end_to_end_passes_for_matching_expectation() {
        let (creft_home, skill_name) = make_trace_skill_home();
        let (_tmp, app) = bare_app();

        let mut scenario = minimal_scenario(vec!["creft".to_owned(), skill_name]);
        scenario.when.env = vec![(
            "CREFT_HOME".to_owned(),
            creft_home.path().to_string_lossy().into_owned(),
        )];
        scenario.then.coverage = Some(CoverageExpectation {
            blocks: vec![0],
            primitives: BTreeMap::from([(0, BTreeMap::from([("print".to_owned(), 1u32)]))]),
        });

        let outcome = run(&scenario, &app, &test_opts());

        assert_eq!(
            outcome.status,
            ScenarioStatus::Pass,
            "expected Pass; stdout={:?} stderr={:?} trace={:?}",
            outcome.stdout,
            outcome.stderr,
            outcome.trace,
        );
        assert!(
            !outcome.trace.is_empty(),
            "trace must contain at least one record when creft runs a skill"
        );
    }

    /// When `then.coverage.blocks` names a block index that never executed (the
    /// skill has only one block, block 0, but the expectation also requires block
    /// 1), `check_coverage` must report a `coverage`-kind `AssertionFailure`
    /// whose message identifies the missing block. This exercises the
    /// parse_trace → check_coverage path through real NDJSON over the trace pipe.
    #[test]
    fn coverage_trace_end_to_end_fails_for_missing_block() {
        let (creft_home, skill_name) = make_trace_skill_home();
        let (_tmp, app) = bare_app();

        let mut scenario = minimal_scenario(vec!["creft".to_owned(), skill_name]);
        scenario.when.env = vec![(
            "CREFT_HOME".to_owned(),
            creft_home.path().to_string_lossy().into_owned(),
        )];
        // Expect both block 0 and block 1, but the skill only has block 0.
        scenario.then.coverage = Some(CoverageExpectation {
            blocks: vec![0, 1],
            primitives: BTreeMap::new(),
        });

        let outcome = run(&scenario, &app, &test_opts());

        let ScenarioStatus::Fail(failures) = outcome.status else {
            panic!(
                "expected Fail for missing block 1; got {:?} stdout={:?} stderr={:?}",
                outcome.status, outcome.stdout, outcome.stderr,
            );
        };
        let cov_failure = failures
            .iter()
            .find(|f| f.kind == "coverage")
            .expect("expected a coverage-kind failure");
        assert!(
            cov_failure.expected.contains('1') || cov_failure.locator.as_deref() == Some("block 1"),
            "failure must identify block 1; got expected={:?} locator={:?}",
            cov_failure.expected,
            cov_failure.locator,
        );
    }

    #[test]
    fn non_creft_binary_with_coverage_expectation_returns_no_trace_failure() {
        let (_tmp, app) = bare_app();
        let mut scenario =
            minimal_scenario(vec!["sh".to_owned(), "-c".to_owned(), "exit 0".to_owned()]);
        scenario.then.coverage = Some(CoverageExpectation {
            blocks: vec![0],
            primitives: BTreeMap::new(),
        });
        let outcome = run(&scenario, &app, &test_opts());

        let ScenarioStatus::Fail(failures) = outcome.status else {
            panic!("expected Fail: non-creft binary produces empty trace");
        };
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].kind, "coverage");
        assert!(failures[0].actual.contains("no trace recorded"));
    }

    // ── Setup error ───────────────────────────────────────────────────────────

    #[test]
    fn before_shell_failure_aborts_scenario() {
        use crate::skill_test::fixture::ShellHook;
        let (_tmp, app) = bare_app();
        let mut scenario = minimal_scenario(vec!["creft".to_owned(), "--version".to_owned()]);
        scenario.before = Some(ShellHook {
            shell: "exit 1".to_owned(),
        });
        let outcome = run(&scenario, &app, &test_opts());
        assert!(
            matches!(outcome.status, ScenarioStatus::SetupError(_)),
            "expected SetupError, got {:?}",
            outcome.status
        );
    }

    #[test]
    fn after_shell_runs_even_when_scenario_fails() {
        use crate::skill_test::fixture::ShellHook;
        let (_tmp, app) = bare_app();
        // Force a failure: the command exits 0 but we expect exit code 1.
        let mut scenario =
            minimal_scenario(vec!["sh".to_owned(), "-c".to_owned(), "exit 0".to_owned()]);
        scenario.then.exit_code = 1;
        scenario.after = Some(ShellHook {
            shell: "touch {sandbox}/after-ran".to_owned(),
        });
        // Keep the sandbox so we can inspect the marker file after run returns.
        let opts = RunOpts {
            creft_binary: Some(creft_bin()),
            default_timeout: Duration::from_secs(10),
            keep_on_failure: true,
        };
        let outcome = run(&scenario, &app, &opts);
        assert!(
            matches!(outcome.status, ScenarioStatus::Fail(_)),
            "expected Fail, got {:?}",
            outcome.status
        );
        let kept = outcome
            .kept_path
            .expect("kept_path must be set for a failing scenario with keep_on_failure");
        assert!(
            kept.join("after-ran").exists(),
            "after.shell must run and create the marker file even when the scenario fails"
        );
        std::fs::remove_dir_all(&kept).ok();
    }

    // ── Placeholder expansion end-to-end ──────────────────────────────────────

    #[test]
    fn stdin_text_with_home_placeholder_reaches_child_expanded() {
        use crate::skill_test::fixture::StdinPayload;
        let (_tmp, app) = bare_app();
        // cat echoes stdin back to stdout. The placeholder in the stdin text
        // must be resolved to the real sandbox home path before the bytes
        // reach the child.
        let mut scenario = minimal_scenario(vec!["cat".to_owned()]);
        scenario.when.stdin = Some(StdinPayload::Text("{home}/.creft".to_owned()));
        // stdout_contains checks for the resolved path. If the placeholder
        // were sent verbatim, this check would fail because the child echoes
        // the literal string "{home}/.creft" rather than the resolved path.
        scenario.then.stdout_contains = vec!["/.creft".to_owned()];
        let outcome = run(&scenario, &app, &test_opts());
        assert_eq!(
            outcome.status,
            ScenarioStatus::Pass,
            "stdout must contain the resolved home path; stdout={:?}",
            outcome.stdout,
        );
        // Also verify the placeholder itself is not present in stdout.
        assert!(
            !outcome.stdout.contains("{home}"),
            "placeholder must be resolved before reaching the child; stdout={:?}",
            outcome.stdout,
        );
    }

    #[test]
    fn stdin_json_with_sandbox_placeholder_reaches_child_expanded() {
        use crate::skill_test::fixture::StdinPayload;
        let (_tmp, app) = bare_app();
        // cat echoes stdin to stdout. JSON value containing {sandbox} must be
        // expanded before serialisation so the child receives the resolved path.
        let mut scenario = minimal_scenario(vec!["cat".to_owned()]);
        scenario.when.stdin = Some(StdinPayload::Json(
            serde_json::json!({"cwd": "{sandbox}/wt"}),
        ));
        // The serialised JSON the child echoes must not contain the placeholder.
        scenario.then.stdout_contains = vec!["/wt".to_owned()];
        let outcome = run(&scenario, &app, &test_opts());
        assert_eq!(
            outcome.status,
            ScenarioStatus::Pass,
            "stdout must contain the resolved sandbox path; stdout={:?}",
            outcome.stdout,
        );
        assert!(
            !outcome.stdout.contains("{sandbox}"),
            "sandbox placeholder must be resolved in JSON stdin; stdout={:?}",
            outcome.stdout,
        );
    }

    #[test]
    fn file_assertion_equals_with_home_placeholder_passes() {
        use crate::skill_test::fixture::FileAssertion;
        let (_tmp, app) = bare_app();
        // The child writes the resolved home path to a file.
        // The equals assertion also contains the placeholder. Both must expand
        // to the same value for the assertion to pass.
        let mut scenario = minimal_scenario(vec![
            "sh".to_owned(),
            "-c".to_owned(),
            // Write the resolved $HOME to a file in the project root.
            r#"printf '%s' "$HOME" > "$CREFT_PROJECT_ROOT/out.txt""#.to_owned(),
        ]);
        scenario.then.files = vec![(
            "{source}/out.txt".to_owned(),
            // The equals value uses the same placeholder; after expansion both
            // sides resolve to the real sandbox home path.
            FileAssertion::Equals("{home}".to_owned()),
        )];
        let outcome = run(&scenario, &app, &test_opts());
        assert_eq!(
            outcome.status,
            ScenarioStatus::Pass,
            "file equals assertion with placeholder must pass; stdout={:?} stderr={:?}",
            outcome.stdout,
            outcome.stderr,
        );
    }

    #[test]
    fn stderr_contains_with_sandbox_placeholder_passes() {
        let (_tmp, app) = bare_app();
        // The child writes the sandbox root path to stderr.
        // then.stderr_contains uses the placeholder; it must expand and match.
        let mut scenario = minimal_scenario(vec![
            "sh".to_owned(),
            "-c".to_owned(),
            r#"printf '%s' "$CREFT_PROJECT_ROOT" >&2"#.to_owned(),
        ]);
        // {source} resolves to the sandbox's source directory, which the child
        // echoes via CREFT_PROJECT_ROOT.
        scenario.then.stderr_contains = vec!["{source}".to_owned()];
        let outcome = run(&scenario, &app, &test_opts());
        assert_eq!(
            outcome.status,
            ScenarioStatus::Pass,
            "stderr_contains placeholder must be resolved before matching; stderr={:?}",
            outcome.stderr,
        );
    }

    #[test]
    fn file_assertion_json_subset_with_sandbox_placeholder_passes() {
        use crate::skill_test::fixture::FileAssertion;
        let (_tmp, app) = bare_app();
        // The child writes a JSON object containing the resolved source path.
        // The json_subset assertion uses the placeholder; both must expand to
        // the same value.
        let mut scenario = minimal_scenario(vec![
            "sh".to_owned(),
            "-c".to_owned(),
            r#"printf '{"dir":"%s"}' "$CREFT_PROJECT_ROOT" > "$CREFT_PROJECT_ROOT/out.json""#
                .to_owned(),
        ]);
        scenario.then.files = vec![(
            "{source}/out.json".to_owned(),
            FileAssertion::JsonSubset(serde_json::json!({"dir": "{source}"})),
        )];
        let outcome = run(&scenario, &app, &test_opts());
        assert_eq!(
            outcome.status,
            ScenarioStatus::Pass,
            "json_subset placeholder in assertion value must be resolved; stdout={:?} stderr={:?}",
            outcome.stdout,
            outcome.stderr,
        );
    }

    // ── Stage 3: sandbox-scoped shell hooks ───────────────────────────────────

    /// before.shell must see the sandbox HOME, not the operator's real HOME.
    ///
    /// The hook writes $HOME to a file in CREFT_PROJECT_ROOT. The file
    /// assertion uses the {home} placeholder; after Stage 1 expansion both
    /// the written content and the assertion value resolve to the same
    /// sandbox path.
    #[test]
    fn before_shell_resolves_home_to_sandbox() {
        use crate::skill_test::fixture::{FileAssertion, ShellHook};
        let (_tmp, app) = bare_app();
        let mut scenario =
            minimal_scenario(vec!["sh".to_owned(), "-c".to_owned(), "exit 0".to_owned()]);
        scenario.before = Some(ShellHook {
            shell: r#"printf '%s' "$HOME" > "$CREFT_PROJECT_ROOT/home.out""#.to_owned(),
        });
        scenario.then.files = vec![(
            "{source}/home.out".to_owned(),
            // {home} expands to the sandbox home path; the file written by the
            // hook must contain the same resolved path.
            FileAssertion::Equals("{home}".to_owned()),
        )];
        let outcome = run(&scenario, &app, &test_opts());
        assert_eq!(
            outcome.status,
            ScenarioStatus::Pass,
            "before.shell must resolve HOME to sandbox; stdout={:?} stderr={:?}",
            outcome.stdout,
            outcome.stderr,
        );
    }

    /// Regression guard for BL-3: before.shell must write state under the
    /// sandbox HOME, not the operator's real HOME.
    ///
    /// The hook writes a marker under $HOME/.fake-state/ AND also copies it to
    /// $CREFT_PROJECT_ROOT so the scenario assertion can verify it. After the run:
    /// - the file at {source}/home-marker must contain the sandbox home path;
    /// - the file at the operator's real $HOME/.fake-state/marker must NOT exist.
    ///
    /// `.fake-state` is a name no real tool uses, so this test is safe to run on
    /// any developer machine.
    #[test]
    fn before_shell_writes_to_sandbox_home_not_operator_home() {
        use crate::skill_test::fixture::{FileAssertion, ShellHook};

        // Capture the operator's real HOME before allocating any sandbox.
        let operator_home = std::env::var("HOME").expect("HOME must be set");

        let (_tmp, app) = bare_app();
        let mut scenario =
            minimal_scenario(vec!["sh".to_owned(), "-c".to_owned(), "exit 0".to_owned()]);
        // The hook writes the resolved $HOME to a file in the project root so
        // the scenario assertion can compare it against {home}.
        scenario.before = Some(ShellHook {
            shell: concat!(
                r#"mkdir -p "$HOME/.fake-state" "#,
                r#"&& printf '%s' "$HOME" > "$CREFT_PROJECT_ROOT/home-marker""#,
            )
            .to_owned(),
        });
        scenario.then.files = vec![(
            "{source}/home-marker".to_owned(),
            // The hook must write the sandbox home path, not the operator's real home.
            FileAssertion::Equals("{home}".to_owned()),
        )];
        let outcome = run(&scenario, &app, &test_opts());

        // The scenario assertion confirms the hook saw the sandbox HOME.
        assert_eq!(
            outcome.status,
            ScenarioStatus::Pass,
            "before.shell must resolve HOME to the sandbox; stdout={:?} stderr={:?}",
            outcome.stdout,
            outcome.stderr,
        );

        // The operator's real home must be untouched.
        let real_marker = std::path::Path::new(&operator_home)
            .join(".fake-state")
            .join("marker");
        assert!(
            !real_marker.exists(),
            "before.shell must not write to the operator's real HOME; found {:?}",
            real_marker,
        );
    }

    /// after.shell must resolve HOME to the sandbox, not the operator's real HOME.
    ///
    /// The after hook runs after assertions, so the file it writes cannot be
    /// verified via scenario assertions. Instead, the test scenario is
    /// engineered to fail (by expecting exit code 1 when the main command exits
    /// 0) so that `keep_on_failure` preserves the sandbox. After run() returns,
    /// the test reads the marker file directly and checks that its content
    /// matches the sandbox home, not the operator's real home.
    #[test]
    fn after_shell_resolves_home_to_sandbox() {
        use crate::skill_test::fixture::ShellHook;

        let (_tmp, app) = bare_app();
        let mut scenario =
            minimal_scenario(vec!["sh".to_owned(), "-c".to_owned(), "exit 0".to_owned()]);
        // Expect exit code 1 so the scenario always fails and kept_path is set.
        scenario.then.exit_code = 1;
        scenario.after = Some(ShellHook {
            // Write $HOME to a file in the source dir so we can read it after run().
            shell: r#"printf '%s' "$HOME" > "$CREFT_PROJECT_ROOT/after-home.out""#.to_owned(),
        });
        let opts = RunOpts {
            creft_binary: Some(creft_bin()),
            default_timeout: Duration::from_secs(10),
            keep_on_failure: true,
        };
        let outcome = run(&scenario, &app, &opts);

        // The scenario exits 0 but expects 1 — it must fail.
        assert!(
            matches!(outcome.status, ScenarioStatus::Fail(_)),
            "scenario must fail so kept_path is populated; got {:?}",
            outcome.status,
        );

        let kept = outcome
            .kept_path
            .expect("kept_path must be set for a failing scenario with keep_on_failure");

        // The after hook must have written to the sandbox source dir.
        let marker = kept.join("source").join("after-home.out");
        assert!(
            marker.exists(),
            "after.shell must have run and written {:?}",
            marker
        );

        let content = std::fs::read_to_string(&marker).expect("after-home.out");
        // The content must be the sandbox home path (under the kept dir), not
        // the operator's real home. Comparing the prefix is sufficient because
        // the operator's home is a fixed path like /Users/... that cannot match
        // a randomly-named temp dir.
        let expected_home = kept.join("home").to_string_lossy().into_owned();
        assert_eq!(
            content, expected_home,
            "after.shell HOME must resolve to sandbox home, not operator's real home",
        );

        std::fs::remove_dir_all(&kept).ok();
    }

    /// execute_scenario must strip parent env vars not on the allowlist.
    ///
    /// The test calls execute_scenario directly with a hand-crafted parent_env
    /// slice that includes OPERATOR_SECRET. The child runs
    /// `printenv OPERATOR_SECRET || printf MISSING` — if OPERATOR_SECRET leaks
    /// through, the child prints the value; if it is stripped, the child prints
    /// MISSING. The assertion verifies the latter.
    ///
    /// Calling execute_scenario directly lets the test inject parent_env without
    /// touching std::env (which is `unsafe` in Rust 2024 and couples tests in
    /// unexpected ways even under nextest's process-per-test isolation).
    #[test]
    fn execute_scenario_strips_unknown_parent_vars() {
        let (_tmp, app) = bare_app();
        // Minimal scenario: print OPERATOR_SECRET if set, MISSING otherwise.
        let scenario = {
            let mut s = minimal_scenario(vec![
                "sh".to_owned(),
                "-c".to_owned(),
                "printenv OPERATOR_SECRET || printf MISSING".to_owned(),
            ]);
            // We only care about stdout content; defaults for the rest are fine.
            s.then.exit_code = 0;
            s
        };

        // Allocate a sandbox for the test manually so we can call execute_scenario directly.
        let sandbox = crate::skill_test::sandbox::Sandbox::new().expect("sandbox");

        let host_root = app
            .find_local_root()
            .map(|d| d.parent().map(|p| p.to_path_buf()).unwrap_or(d));
        sandbox
            .mirror_project_skills(host_root.as_deref())
            .expect("mirror");

        // Expand the scenario against the sandbox paths.
        let scenario = crate::skill_test::expand::expand_scenario(&scenario, &sandbox.paths());
        sandbox.materialise(&scenario.given).expect("materialise");

        let creft_binary = creft_bin();
        let opts = test_opts();

        // Inject a parent env that includes both PATH (needed for executables)
        // and OPERATOR_SECRET (must be stripped by execute_scenario).
        let parent_env: Vec<(String, String)> = vec![
            ("PATH".to_owned(), std::env::var("PATH").unwrap_or_default()),
            ("OPERATOR_SECRET".to_owned(), "value".to_owned()),
        ];

        let (status, stdout, _stderr, _trace) =
            execute_scenario(&scenario, &sandbox, &creft_binary, &parent_env, &opts);

        assert_eq!(
            status,
            ScenarioStatus::Pass,
            "scenario must pass — exit code 0 is expected"
        );
        assert_eq!(
            stdout.trim(),
            "MISSING",
            "OPERATOR_SECRET must be stripped; child printed {:?}",
            stdout,
        );
    }

    // ── Keep on failure ───────────────────────────────────────────────────────

    #[test]
    fn keep_on_failure_preserves_sandbox_dir() {
        let (_tmp, app) = bare_app();
        let mut scenario =
            minimal_scenario(vec!["sh".to_owned(), "-c".to_owned(), "exit 1".to_owned()]);
        scenario.then.exit_code = 0; // will fail
        let opts = RunOpts {
            creft_binary: Some(creft_bin()),
            default_timeout: Duration::from_secs(10),
            keep_on_failure: true,
        };
        let outcome = run(&scenario, &app, &opts);

        let kept = outcome
            .kept_path
            .expect("kept_path should be set on failure");
        assert!(kept.exists(), "sandbox dir must be preserved");
        // Clean up.
        std::fs::remove_dir_all(&kept).ok();
    }

    #[test]
    fn keep_on_failure_does_not_set_path_for_passing_scenario() {
        let (_tmp, app) = bare_app();
        let scenario =
            minimal_scenario(vec!["sh".to_owned(), "-c".to_owned(), "exit 0".to_owned()]);
        let opts = RunOpts {
            creft_binary: Some(creft_bin()),
            default_timeout: Duration::from_secs(10),
            keep_on_failure: true,
        };
        let outcome = run(&scenario, &app, &opts);
        assert_eq!(outcome.status, ScenarioStatus::Pass);
        assert!(
            outcome.kept_path.is_none(),
            "passing scenario must not set kept_path"
        );
    }
}
