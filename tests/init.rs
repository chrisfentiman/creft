//! Tests for `creft init`.

mod helpers;

use assert_cmd::Command;
use helpers::creft_env;
use predicates::prelude::*;

// ── init tests ────────────────────────────────────────────────────────────────

/// `creft init` creates `.creft/commands/` in the working directory.
#[test]
fn test_init_creates_directory() {
    let home = creft_env();
    let project = tempfile::tempdir().unwrap();

    Command::cargo_bin("creft")
        .unwrap()
        .env("CREFT_HOME", home.path())
        .current_dir(project.path())
        .args(["init"])
        .assert()
        .success()
        .stderr(predicate::str::contains("created"));

    assert!(
        project.path().join(".creft").join("commands").is_dir(),
        ".creft/commands/ should have been created"
    );
}

/// Running `creft init` twice succeeds and reports "already initialized" on the second run.
#[test]
fn test_init_idempotent() {
    let home = creft_env();
    let project = tempfile::tempdir().unwrap();

    // First run
    Command::cargo_bin("creft")
        .unwrap()
        .env("CREFT_HOME", home.path())
        .current_dir(project.path())
        .args(["init"])
        .assert()
        .success();

    // Second run — idempotent
    Command::cargo_bin("creft")
        .unwrap()
        .env("CREFT_HOME", home.path())
        .current_dir(project.path())
        .args(["init"])
        .assert()
        .success()
        .stderr(predicate::str::contains("already initialized"));
}

/// `creft init` in a subdirectory explains that a new root overlays the ancestor root.
#[test]
fn test_init_warns_parent() {
    let home = creft_env();
    let parent = tempfile::tempdir().unwrap();

    // Set up .creft/commands/ in parent
    std::fs::create_dir_all(parent.path().join(".creft").join("commands")).unwrap();

    // Create child directory
    let child = parent.path().join("child");
    std::fs::create_dir(&child).unwrap();

    Command::cargo_bin("creft")
        .unwrap()
        .env("CREFT_HOME", home.path())
        .current_dir(&child)
        .args(["init"])
        .assert()
        .success()
        .stderr(predicate::str::contains("ancestor .creft/ exists at"))
        .stderr(predicate::str::contains("overlay"));

    assert!(
        child.join(".creft").join("commands").is_dir(),
        ".creft/commands/ should have been created in the child directory"
    );
}

/// `creft init` from a CWD with multiple ancestor `.creft/` directories prints
/// the multi-ancestor message: "ancestor .creft/ directories exist at:" followed
/// by each path, then "this .creft/ will overlay them; closer scopes win on conflicts".
///
/// Spec test expectation: spec line 777.
#[test]
fn test_init_warns_multiple_parents() {
    let home = creft_env();

    // Create two ancestor roots: grandparent and parent.
    let grandparent = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(grandparent.path().join(".creft").join("commands")).unwrap();

    let parent = grandparent.path().join("parent");
    std::fs::create_dir_all(parent.join(".creft").join("commands")).unwrap();

    // Child directory has neither `.creft/` yet — this is where init runs.
    let child = parent.join("child");
    std::fs::create_dir(&child).unwrap();

    Command::cargo_bin("creft")
        .unwrap()
        .env("CREFT_HOME", home.path())
        .current_dir(&child)
        .args(["init"])
        .assert()
        .success()
        // Multi-ancestor header (plural form).
        .stderr(predicate::str::contains(
            "ancestor .creft/ directories exist at:",
        ))
        // Overlay message (shared between single- and multi-ancestor paths).
        .stderr(predicate::str::contains("overlay"));

    assert!(
        child.join(".creft").join("commands").is_dir(),
        ".creft/commands/ should have been created in the child directory"
    );
}

/// `creft init` in a clean directory does NOT warn about parent `.creft/`.
#[test]
fn test_init_no_warn_without_parent() {
    let home = creft_env();
    let project = tempfile::tempdir().unwrap();

    Command::cargo_bin("creft")
        .unwrap()
        .env("CREFT_HOME", home.path())
        .current_dir(project.path())
        .args(["init"])
        .assert()
        .success()
        .stderr(predicate::str::contains("ancestor").not());
}
