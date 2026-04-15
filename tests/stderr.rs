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

// ── verbose mode: stderr from successful blocks is surfaced ───────────────────

/// With `--verbose`, stderr from a successful block must appear on the parent's
/// stderr prefixed with `[block N stderr]` so multi-block output is attributable.
#[test]
fn verbose_shows_stderr_from_successful_block() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["add"])
        .write_stdin(concat!(
            "---\n",
            "name: verbose-stderr-success\n",
            "description: writes stderr then succeeds; visible under verbose\n",
            "---\n",
            "\n",
            "```bash\n",
            "echo provider-diagnostic >&2\n",
            "echo stdout-output\n",
            "```\n",
        ))
        .assert()
        .success();

    let output = creft_with(&dir)
        .args(["verbose-stderr-success", "--verbose"])
        .output()
        .unwrap();

    assert!(output.status.success(), "block must succeed");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("[block 1 stderr]"),
        "verbose stderr must include block prefix; got: {stderr:?}"
    );
    assert!(
        stderr.contains("provider-diagnostic"),
        "verbose stderr must include the child's stderr content; got: {stderr:?}"
    );
}

/// Without `--verbose`, stderr from a successful block must not appear on the
/// parent's terminal. The terminal stays clean.
#[test]
fn non_verbose_hides_stderr_from_successful_block() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["add"])
        .write_stdin(concat!(
            "---\n",
            "name: non-verbose-stderr-success\n",
            "description: writes stderr then succeeds; hidden without verbose\n",
            "---\n",
            "\n",
            "```bash\n",
            "echo silent-diagnostic >&2\n",
            "echo stdout-output\n",
            "```\n",
        ))
        .assert()
        .success();

    creft_with(&dir)
        .args(["non-verbose-stderr-success"])
        .assert()
        .success()
        .stderr(predicate::str::contains("silent-diagnostic").not());
}

/// In a pipe chain with `--verbose`, stderr from all blocks appears in the
/// parent's stderr with `[block N stderr]` prefixes.
#[test]
fn verbose_shows_stderr_from_pipe_chain_blocks() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["add"])
        .write_stdin(concat!(
            "---\n",
            "name: verbose-pipe-stderr\n",
            "description: pipe chain where each block writes stderr\n",
            "---\n",
            "\n",
            "```bash\n",
            "echo block-one-diag >&2\n",
            "echo hello\n",
            "```\n",
            "\n",
            "```bash\n",
            "echo block-two-diag >&2\n",
            "cat\n",
            "```\n",
        ))
        .assert()
        .success();

    let output = creft_with(&dir)
        .args(["verbose-pipe-stderr", "--verbose"])
        .output()
        .unwrap();

    assert!(output.status.success(), "pipe chain must succeed");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("block-one-diag") && stderr.contains("block-two-diag"),
        "verbose stderr must include both blocks' diagnostic output; got: {stderr:?}"
    );
}

// ── skill name in failure message ─────────────────────────────────────────────

/// When a skill exits non-zero, creft prints `error: '<name>' exited with code N`
/// to stderr so the caller knows which skill failed and with what code.
#[test]
fn nonzero_exit_prints_skill_name_and_code() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["add"])
        .write_stdin(concat!(
            "---\n",
            "name: fail-with-code\n",
            "description: exits non-zero\n",
            "---\n",
            "\n",
            "```bash\n",
            "exit 42\n",
            "```\n",
        ))
        .assert()
        .success();

    let output = creft_with(&dir).args(["fail-with-code"]).output().unwrap();

    assert!(!output.status.success(), "skill must fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("error: 'fail-with-code' exited with code 42"),
        "stderr must contain skill name and exit code; got: {stderr:?}"
    );
    assert_eq!(
        output.status.code(),
        Some(42),
        "exit code must propagate unchanged"
    );
}

/// When a pipe-chain skill exits non-zero, the skill name (not block index)
/// appears in the error message.
#[test]
fn nonzero_exit_in_pipe_chain_prints_skill_name() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["add"])
        .write_stdin(concat!(
            "---\n",
            "name: pipe-fail\n",
            "description: pipe chain where last block fails\n",
            "---\n",
            "\n",
            "```bash\n",
            "echo hello\n",
            "```\n",
            "\n",
            "```bash\n",
            "exit 3\n",
            "```\n",
        ))
        .assert()
        .success();

    let output = creft_with(&dir).args(["pipe-fail"]).output().unwrap();

    assert!(!output.status.success(), "pipe chain must fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("error: 'pipe-fail' exited with code 3"),
        "stderr must name the skill, not a block index; got: {stderr:?}"
    );
}

/// A skill that uses exit 99 (early exit / creft_exit) must complete
/// successfully and produce no error message on stderr.
#[test]
fn exit_99_early_exit_is_silent() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["add"])
        .write_stdin(concat!(
            "---\n",
            "name: early-exit-silent\n",
            "description: stops early via exit 99\n",
            "---\n",
            "\n",
            "```bash\n",
            "exit 99\n",
            "```\n",
        ))
        .assert()
        .success();

    creft_with(&dir)
        .args(["early-exit-silent"])
        .assert()
        .success()
        .stderr(predicate::str::contains("error:").not());
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
