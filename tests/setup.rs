//! Tests for `creft up` (setup command).

use assert_cmd::Command;
use tempfile::TempDir;

// ── setup tests ───────────────────────────────────────────────────────────────

/// Two consecutive `creft up --local` runs: second run reports "skipped" for each installed system.
/// This verifies that the idempotency logic in `install()` works end-to-end through the CLI
/// in project-scope mode.
#[test]
fn up_local_idempotent() {
    let home_dir = TempDir::new().unwrap();
    let project_dir = TempDir::new().unwrap();

    // Create a .claude/ directory so Claude Code is detected.
    std::fs::create_dir_all(project_dir.path().join(".claude")).unwrap();

    // First run: should install (not skipped).
    let first_output = Command::cargo_bin("creft")
        .unwrap()
        .args(["up", "--local"])
        .env("CREFT_HOME", home_dir.path())
        .current_dir(project_dir.path())
        .assert()
        .success()
        .get_output()
        .stderr
        .clone();

    let first_stderr = String::from_utf8_lossy(&first_output);
    assert!(
        !first_stderr.contains("skipped:"),
        "first run should not report skipped; got: {first_stderr:?}"
    );

    // Second run: instructions are already current, so each system should be skipped.
    let second_output = Command::cargo_bin("creft")
        .unwrap()
        .args(["up", "--local"])
        .env("CREFT_HOME", home_dir.path())
        .current_dir(project_dir.path())
        .assert()
        .success()
        .get_output()
        .stderr
        .clone();

    let second_stderr = String::from_utf8_lossy(&second_output);
    assert!(
        second_stderr.contains("skipped:"),
        "second run should report skipped for already-current instructions; got: {second_stderr:?}"
    );
}

/// Bare `creft up` installs globally (Claude Code, Codex, Gemini).
/// Verifies the global-by-default behavior: output mentions "globally".
#[test]
fn up_bare_installs_globally() {
    let home_dir = TempDir::new().unwrap();
    let project_dir = TempDir::new().unwrap();

    let output = Command::cargo_bin("creft")
        .unwrap()
        .args(["up"])
        .env("CREFT_HOME", home_dir.path())
        .current_dir(project_dir.path())
        .assert()
        .success()
        .get_output()
        .stderr
        .clone();

    let stderr = String::from_utf8_lossy(&output);
    assert!(
        stderr.contains("globally"),
        "bare `creft up` must report global install; got: {stderr:?}"
    );
}

/// Second bare `creft up` reports skipped — global install is idempotent.
#[test]
fn up_bare_global_idempotent() {
    let home_dir = TempDir::new().unwrap();
    let project_dir = TempDir::new().unwrap();

    Command::cargo_bin("creft")
        .unwrap()
        .args(["up"])
        .env("CREFT_HOME", home_dir.path())
        .current_dir(project_dir.path())
        .assert()
        .success();

    let second_output = Command::cargo_bin("creft")
        .unwrap()
        .args(["up"])
        .env("CREFT_HOME", home_dir.path())
        .current_dir(project_dir.path())
        .assert()
        .success()
        .get_output()
        .stderr
        .clone();

    let second_stderr = String::from_utf8_lossy(&second_output);
    assert!(
        second_stderr.contains("skipped:"),
        "second global run should report skipped for already-current instructions; got: {second_stderr:?}"
    );
}

/// `creft up --local` with a `.claude/` directory in CWD installs for Claude Code locally.
#[test]
fn up_local_flag_long_detects_project_systems() {
    let home_dir = TempDir::new().unwrap();
    let project_dir = TempDir::new().unwrap();

    std::fs::create_dir_all(project_dir.path().join(".claude")).unwrap();

    let output = Command::cargo_bin("creft")
        .unwrap()
        .args(["up", "--local"])
        .env("CREFT_HOME", home_dir.path())
        .current_dir(project_dir.path())
        .assert()
        .success()
        .get_output()
        .stderr
        .clone();

    let stderr = String::from_utf8_lossy(&output);
    assert!(
        stderr.contains("Claude Code"),
        "`creft up --local` must detect and install Claude Code; got: {stderr:?}"
    );
    assert!(
        !stderr.contains("globally"),
        "`creft up --local` must not install globally; got: {stderr:?}"
    );
}

/// `-l` short flag is equivalent to `--local`.
#[test]
fn up_local_flag_short_detects_project_systems() {
    let home_dir = TempDir::new().unwrap();
    let project_dir = TempDir::new().unwrap();

    std::fs::create_dir_all(project_dir.path().join(".claude")).unwrap();

    let output = Command::cargo_bin("creft")
        .unwrap()
        .args(["up", "-l"])
        .env("CREFT_HOME", home_dir.path())
        .current_dir(project_dir.path())
        .assert()
        .success()
        .get_output()
        .stderr
        .clone();

    let stderr = String::from_utf8_lossy(&output);
    assert!(
        stderr.contains("Claude Code"),
        "`creft up -l` must detect and install Claude Code; got: {stderr:?}"
    );
}

/// `creft up claude-code` installs Claude Code globally by default.
#[test]
fn up_system_installs_globally_by_default() {
    let home_dir = TempDir::new().unwrap();
    let project_dir = TempDir::new().unwrap();

    let output = Command::cargo_bin("creft")
        .unwrap()
        .args(["up", "claude-code"])
        .env("CREFT_HOME", home_dir.path())
        .current_dir(project_dir.path())
        .assert()
        .success()
        .get_output()
        .stderr
        .clone();

    let stderr = String::from_utf8_lossy(&output);
    assert!(
        stderr.contains("Claude Code"),
        "`creft up claude-code` must install Claude Code; got: {stderr:?}"
    );
    // With CREFT_HOME set, both global and local resolve to the same temp dir,
    // but we can verify the command succeeds and mentions the system.
}

/// `creft up --local claude-code` installs Claude Code in project scope.
#[test]
fn up_local_with_system_installs_locally() {
    let home_dir = TempDir::new().unwrap();
    let project_dir = TempDir::new().unwrap();

    let output = Command::cargo_bin("creft")
        .unwrap()
        .args(["up", "--local", "claude-code"])
        .env("CREFT_HOME", home_dir.path())
        .current_dir(project_dir.path())
        .assert()
        .success()
        .get_output()
        .stderr
        .clone();

    let stderr = String::from_utf8_lossy(&output);
    assert!(
        stderr.contains("Claude Code"),
        "`creft up --local claude-code` must install Claude Code; got: {stderr:?}"
    );
}

/// `creft up --global` produces an unknown-flag error (intentional breaking change).
#[test]
fn up_global_flag_removed_returns_error() {
    let home_dir = TempDir::new().unwrap();
    let project_dir = TempDir::new().unwrap();

    Command::cargo_bin("creft")
        .unwrap()
        .args(["up", "--global"])
        .env("CREFT_HOME", home_dir.path())
        .current_dir(project_dir.path())
        .assert()
        .failure();
}
