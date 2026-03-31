//! Tests for `creft up` (setup command).

use assert_cmd::Command;
use tempfile::TempDir;

// ── setup tests ───────────────────────────────────────────────────────────────

/// Two consecutive `creft up` runs: second run reports "skipped" for each installed system.
/// This verifies that the idempotency logic in `install()` works end-to-end through the CLI.
#[test]
fn test_up_idempotent() {
    let home_dir = TempDir::new().unwrap();
    let project_dir = TempDir::new().unwrap();

    // Create a .claude/ directory so Claude Code is detected.
    std::fs::create_dir_all(project_dir.path().join(".claude")).unwrap();

    // First run: should install (not skipped).
    let first_output = Command::cargo_bin("creft")
        .unwrap()
        .args(["up"])
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
        "second run should report skipped for already-current instructions; got: {second_stderr:?}"
    );
}
