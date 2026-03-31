//! Tests for `creft edit` stdin behavior.

mod helpers;

use helpers::{create_test_package, creft_env, creft_with};
use predicates::prelude::*;

// ── edit stdin tests ──────────────────────────────────────────────────────────

/// `creft edit <name>` with piped stdin replaces the command content.
/// The new content appears when showing the command afterwards.
/// assert_cmd always pipes stdin, so is_terminal() == false in tests — this
/// exercises the stdin code path directly.
#[test]
fn test_edit_stdin_replaces_content() {
    let dir = creft_env();

    // Create the original command.
    creft_with(&dir)
        .args(["add"])
        .write_stdin(
            "---\nname: hello\ndescription: original\n---\n\n```bash\necho original\n```\n",
        )
        .assert()
        .success();

    // Pipe new content to `creft edit hello`.
    let new_content = "---\nname: hello\ndescription: updated\n---\n\n```bash\necho updated\n```\n";
    creft_with(&dir)
        .args(["edit", "hello"])
        .write_stdin(new_content)
        .assert()
        .success()
        .stderr(predicate::str::contains("edited: hello"));

    // The new content is visible in `creft show`.
    creft_with(&dir)
        .args(["show", "hello"])
        .assert()
        .success()
        .stdout(predicate::str::contains("description: updated"));
}

/// Piping content with invalid frontmatter (no delimiters) is rejected.
/// The original file must be preserved.
#[test]
fn test_edit_stdin_rejects_invalid_frontmatter() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["add"])
        .write_stdin(
            "---\nname: safe\ndescription: should not change\n---\n\n```bash\necho safe\n```\n",
        )
        .assert()
        .success();

    // Pipe content without frontmatter delimiters — this must fail.
    creft_with(&dir)
        .args(["edit", "safe"])
        .write_stdin("just some text without any frontmatter delimiters")
        .assert()
        .failure();

    // Original content must be preserved.
    creft_with(&dir)
        .args(["show", "safe"])
        .assert()
        .success()
        .stdout(predicate::str::contains("should not change"));
}

/// Piping empty stdin is rejected (frontmatter::parse fails on empty input).
#[test]
fn test_edit_stdin_rejects_empty_stdin() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["add"])
        .write_stdin("---\nname: notempty\ndescription: stays\n---\n\n```bash\necho stays\n```\n")
        .assert()
        .success();

    creft_with(&dir)
        .args(["edit", "notempty"])
        .write_stdin("")
        .assert()
        .failure();
}

/// Piping content to `creft edit nonexistent` fails with "command not found".
#[test]
fn test_edit_stdin_nonexistent_command_fails() {
    let dir = creft_env();

    let valid_content =
        "---\nname: nonexistent\ndescription: nope\n---\n\n```bash\necho nope\n```\n";
    creft_with(&dir)
        .args(["edit", "nonexistent"])
        .write_stdin(valid_content)
        .assert()
        .failure()
        .code(2);
}

/// Piping content to an installed package skill is rejected (read-only).
#[test]
fn test_edit_stdin_package_skill_is_rejected() {
    let pkg_repo = create_test_package(
        "stdin-edit-pkg",
        &[(
            "deploy.md",
            "---\nname: deploy\ndescription: Deploy\n---\n\n```bash\necho deploying\n```\n",
        )],
    );
    let creft_home = creft_env();

    creft_with(&creft_home)
        .args(["install", pkg_repo.path().to_str().unwrap()])
        .assert()
        .success();

    let new_content = "---\nname: deploy\ndescription: hacked\n---\n\n```bash\necho hacked\n```\n";
    creft_with(&creft_home)
        .args(["edit", "stdin-edit-pkg", "deploy"])
        .write_stdin(new_content)
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("read-only"));
}

/// Piping content with a description longer than 80 characters emits a warning
/// on stderr but exits with code 0 (warning, not error).
#[test]
fn test_edit_piped_long_description_warns() {
    let dir = creft_env();

    // Create the original command with a short description.
    creft_with(&dir)
        .args(["add"])
        .write_stdin("---\nname: longdesc\ndescription: short\n---\n\n```bash\necho hi\n```\n")
        .assert()
        .success();

    // Build a description that exceeds 80 characters.
    let long_desc = "a".repeat(81);
    let new_content = format!(
        "---\nname: longdesc\ndescription: {}\n---\n\n```bash\necho hi\n```\n",
        long_desc
    );

    creft_with(&dir)
        .args(["edit", "longdesc"])
        .write_stdin(new_content.as_str())
        .assert()
        .success()
        .stderr(predicate::str::contains("description is long"));
}

/// Frontmatter name mismatch is ignored — the file at the given path is
/// overwritten regardless of what name appears inside the frontmatter.
#[test]
fn test_edit_stdin_name_mismatch_still_writes() {
    let dir = creft_env();

    // Create command named "bar".
    creft_with(&dir)
        .args(["add"])
        .write_stdin("---\nname: bar\ndescription: original bar\n---\n\n```bash\necho bar\n```\n")
        .assert()
        .success();

    // Pipe content whose frontmatter says "name: foo" — editing "bar".
    let mismatched_content =
        "---\nname: foo\ndescription: replaced with foo name\n---\n\n```bash\necho foo\n```\n";
    creft_with(&dir)
        .args(["edit", "bar"])
        .write_stdin(mismatched_content)
        .assert()
        .success()
        .stderr(predicate::str::contains("edited: bar"));

    // `creft show bar` returns the new content (name mismatch is written as-is).
    creft_with(&dir)
        .args(["show", "bar"])
        .assert()
        .success()
        .stdout(predicate::str::contains("replaced with foo name"));
}
