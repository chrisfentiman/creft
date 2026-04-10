//! Tests for stderr capture: child stderr does not leak to the terminal on
//! success, and appears in creft's error output on failure.

mod helpers;

use helpers::{creft_env, creft_with};
use predicates::prelude::*;

// ── single-block: stderr captured and discarded on success ────────────────────

/// A block that writes to stderr and exits 0 must produce no stderr output
/// on the parent terminal. The terminal stays clean.
#[test]
fn stderr_from_successful_block_is_discarded() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["add"])
        .write_stdin(concat!(
            "---\n",
            "name: noisy-success\n",
            "description: writes to stderr then succeeds\n",
            "---\n",
            "\n",
            "```bash\n",
            "echo stderr-noise >&2\n",
            "echo stdout-output\n",
            "```\n",
        ))
        .assert()
        .success();

    creft_with(&dir)
        .args(["noisy-success"])
        .assert()
        .success()
        .stdout(predicate::str::contains("stdout-output"))
        .stderr(predicate::str::contains("stderr-noise").not());
}

/// A block that writes to stderr and exits non-zero must surface the stderr
/// content in creft's error output so the user can see what went wrong.
#[test]
fn stderr_from_failed_block_appears_in_error_output() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["add"])
        .write_stdin(concat!(
            "---\n",
            "name: noisy-fail\n",
            "description: writes to stderr then fails\n",
            "---\n",
            "\n",
            "```bash\n",
            "echo error-detail >&2\n",
            "exit 1\n",
            "```\n",
        ))
        .assert()
        .success();

    let output = creft_with(&dir).args(["noisy-fail"]).output().unwrap();

    assert!(!output.status.success(), "block must fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("error-detail"),
        "child stderr must appear in creft's error output, got: {stderr:?}"
    );
}

// ── single-block: ANSI sequences from stderr do not reach the terminal ─────────

/// A block that writes ANSI escape sequences to stderr and exits 0 must not
/// let those sequences reach the terminal. This covers the ollama spinner case.
#[test]
fn ansi_sequences_in_stderr_are_captured_not_displayed() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["add"])
        .write_stdin(concat!(
            "---\n",
            "name: ansi-noise\n",
            "description: emits ANSI spinner noise to stderr\n",
            "---\n",
            "\n",
            "```bash\n",
            // ESC[2K (erase line) + CR — typical spinner pattern
            "printf '\\033[2K\\r' >&2\n",
            "echo clean-output\n",
            "```\n",
        ))
        .assert()
        .success();

    creft_with(&dir)
        .args(["ansi-noise"])
        .assert()
        .success()
        .stdout(predicate::str::contains("clean-output"))
        // The ANSI escape byte (ESC = 0x1b) must not appear on the terminal
        .stderr(predicate::str::contains("\x1b").not());
}

// ── pipe chain: stderr captured and discarded on success ──────────────────────

/// In a pipe chain, a block that writes to stderr and exits 0 must not leak
/// stderr to the terminal.
#[test]
fn stderr_from_successful_pipe_block_is_discarded() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["add"])
        .write_stdin(concat!(
            "---\n",
            "name: pipe-noisy-success\n",
            "description: pipe with noisy first block\n",
            "---\n",
            "\n",
            "```bash\n",
            "echo stderr-pipe-noise >&2\n",
            "echo hello\n",
            "```\n",
            "\n",
            "```bash\n",
            "cat\n",
            "```\n",
        ))
        .assert()
        .success();

    creft_with(&dir)
        .args(["pipe-noisy-success"])
        .assert()
        .success()
        .stdout(predicate::str::contains("hello"))
        .stderr(predicate::str::contains("stderr-pipe-noise").not());
}

/// In a pipe chain, the last block's stderr must appear in creft's error output
/// when that block fails.
#[test]
fn stderr_from_failed_last_pipe_block_appears_in_error_output() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["add"])
        .write_stdin(concat!(
            "---\n",
            "name: pipe-last-noisy-fail\n",
            "description: pipe where last block fails with stderr\n",
            "---\n",
            "\n",
            "```bash\n",
            "echo hello\n",
            "```\n",
            "\n",
            "```bash\n",
            "echo pipe-error-detail >&2\n",
            "exit 1\n",
            "```\n",
        ))
        .assert()
        .success();

    let output = creft_with(&dir)
        .args(["pipe-last-noisy-fail"])
        .output()
        .unwrap();

    assert!(!output.status.success(), "pipe chain must fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("pipe-error-detail"),
        "child stderr must appear in creft's error output, got: {stderr:?}"
    );
}

// ── stdout is unaffected ───────────────────────────────────────────────────────

/// Capturing stderr must not interfere with stdout: block output still reaches
/// the caller correctly.
#[test]
fn stdout_unaffected_by_stderr_capture() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["add"])
        .write_stdin(concat!(
            "---\n",
            "name: stdout-check\n",
            "description: verify stdout is captured correctly alongside stderr\n",
            "---\n",
            "\n",
            "```bash\n",
            "echo stderr-line >&2\n",
            "echo stdout-line\n",
            "```\n",
        ))
        .assert()
        .success();

    creft_with(&dir)
        .args(["stdout-check"])
        .assert()
        .success()
        .stdout(predicate::str::contains("stdout-line"))
        .stderr(predicate::str::contains("stderr-line").not());
}
