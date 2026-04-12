//! Tests for dry-run behavior (print-only and delegated dry-run).

mod helpers;

use helpers::{creft_env, creft_with};
use predicates::prelude::*;

// ── dry-run tests ─────────────────────────────────────────────────────────────

/// `creft<name> --dry-run` prints the expanded code block to stdout without
/// executing it. We verify the code text appears and no side effects occur.
#[test]
fn test_dry_run() {
    let dir = creft_env();

    // The command writes to a file — we verify that file is NOT created in dry-run.
    let sentinel = dir.path().join("sentinel.txt");

    let markdown = format!(
        "---\nname: touch-file\ndescription: create a file\n---\n\n```bash\ntouch {}\n```\n",
        sentinel.display()
    );

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(markdown.as_str())
        .assert()
        .success();

    creft_with(&dir)
        .args(["touch-file", "--dry-run"])
        .assert()
        .success()
        // Dry-run prints the expanded code.
        .stdout(predicates::prelude::predicate::str::contains("touch"));

    // Sentinel file must NOT have been created.
    assert!(!sentinel.exists(), "dry-run must not execute the command");
}

// ── dry-run delegation tests ──────────────────────────────────────────────────

/// A command with `supports: [dry-run]` is executed (not just printed) when
/// `--dry-run` is passed. The child process receives `CREFT_DRY_RUN=1`.
#[test]
fn test_dry_run_delegated() {
    let dir = creft_env();

    // Script prints the value of CREFT_DRY_RUN so we can assert it was set.
    let markdown = "---\nname: dry-aware\ndescription: dry-run aware command\nsupports: [dry-run]\n---\n\n```bash\necho \"CREFT_DRY_RUN=$CREFT_DRY_RUN\"\n```\n";

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(markdown)
        .assert()
        .success();

    creft_with(&dir)
        .args(["dry-aware", "--dry-run"])
        .assert()
        .success()
        // The command was actually executed (not just printed), and it received
        // CREFT_DRY_RUN=1 from the injected env var.
        .stdout(predicates::prelude::predicate::str::contains(
            "CREFT_DRY_RUN=1",
        ));
}

/// A command without `supports` still gets print-only dry-run (existing behavior).
#[test]
fn test_dry_run_fallback() {
    let dir = creft_env();

    // The command writes a sentinel file. Without dry-run delegation, it should
    // NOT be executed -- the code should only be printed.
    let sentinel = dir.path().join("fallback-sentinel.txt");
    let markdown = format!(
        "---\nname: no-support\ndescription: no dry-run support\n---\n\n```bash\ntouch {}\n```\n",
        sentinel.display()
    );

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(markdown.as_str())
        .assert()
        .success();

    creft_with(&dir)
        .args(["no-support", "--dry-run"])
        .assert()
        .success()
        // Code text is printed.
        .stdout(predicates::prelude::predicate::str::contains("touch"));

    // File must NOT have been created -- command was not executed.
    assert!(
        !sentinel.exists(),
        "dry-run fallback must not execute the command"
    );
}

/// When a dry-run-delegated command exits non-zero, creft propagates the failure.
#[test]
fn test_dry_run_delegated_failure() {
    let dir = creft_env();

    // Script exits with code 1 when CREFT_DRY_RUN=1.
    let markdown = "---\nname: dry-fail\ndescription: fails on dry-run\nsupports: [dry-run]\n---\n\n```bash\nif [ \"$CREFT_DRY_RUN\" = \"1\" ]; then exit 1; fi\necho ok\n```\n";

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(markdown)
        .assert()
        .success();

    // Non-zero exit from the delegated dry-run must propagate.
    creft_with(&dir)
        .args(["dry-fail", "--dry-run"])
        .assert()
        .failure();
}

/// A multi-block command with `supports: [dry-run]` injects `CREFT_DRY_RUN=1`
/// into every block, not just the first. With pipe-by-default, block 1's stdout
/// feeds block 2's stdin. Block 2 reads stdin via `cat` (passing through block 1's
/// output) then appends its own marker. Both markers must appear in the final stdout.
#[test]
fn test_dry_run_delegated_multi_block() {
    let dir = creft_env();

    // Block 1 echoes its marker. Block 2 passes block 1's output through via `cat`
    // then appends its own marker. Both BLOCK1=1 and BLOCK2=1 must appear.
    let markdown = "---\nname: multi-dry\ndescription: multi-block dry-run\nsupports: [dry-run]\n---\n\n```bash\necho \"BLOCK1=$CREFT_DRY_RUN\"\n```\n\n```bash\ncat\necho \"BLOCK2=$CREFT_DRY_RUN\"\n```\n";

    creft_with(&dir)
        .args(["cmd", "add", "--no-validate"])
        .write_stdin(markdown)
        .assert()
        .success();

    creft_with(&dir)
        .args(["multi-dry", "--dry-run"])
        .assert()
        .success()
        // Both blocks executed and received CREFT_DRY_RUN=1.
        .stdout(predicates::prelude::predicate::str::contains("BLOCK1=1"))
        .stdout(predicates::prelude::predicate::str::contains("BLOCK2=1"));
}

/// A command with `supports: [dry-run]` and declared args correctly substitutes
/// arg values AND injects `CREFT_DRY_RUN=1` when `--dry-run` is passed.
#[test]
fn test_dry_run_delegated_with_args() {
    let dir = creft_env();

    let markdown = "---\nname: greet-dry\ndescription: dry-run aware greeting\nsupports: [dry-run]\nargs:\n  - name: who\n    description: who to greet\n---\n\n```bash\necho \"Hello {{who}} dry=$CREFT_DRY_RUN\"\n```\n";

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(markdown)
        .assert()
        .success();

    creft_with(&dir)
        .args(["greet-dry", "World", "--dry-run"])
        .assert()
        .success()
        // Arg was substituted correctly.
        .stdout(predicates::prelude::predicate::str::contains("Hello World"))
        // CREFT_DRY_RUN was injected.
        .stdout(predicates::prelude::predicate::str::contains("dry=1"));
}

/// When `supports` lists multiple features, e.g. `[dry-run, verbose]`, creft
/// still delegates dry-run correctly. The unrecognized feature is silently
/// ignored; no parse error or behavior change occurs.
#[test]
fn test_dry_run_supports_multiple_features() {
    let dir = creft_env();

    let markdown = "---\nname: multi-feat\ndescription: multiple features declared\nsupports: [dry-run, verbose]\n---\n\n```bash\necho \"DRY=$CREFT_DRY_RUN\"\n```\n";

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(markdown)
        .assert()
        .success();

    // dry-run is recognized and delegated; verbose is ignored.
    creft_with(&dir)
        .args(["multi-feat", "--dry-run"])
        .assert()
        .success()
        .stdout(predicates::prelude::predicate::str::contains("DRY=1"));

    // Without --dry-run, the command runs normally with no CREFT_DRY_RUN set.
    creft_with(&dir)
        .args(["multi-feat"])
        .assert()
        .success()
        // CREFT_DRY_RUN is unset in normal execution, so the echo produces "DRY=".
        .stdout(
            predicates::prelude::predicate::str::contains("DRY=")
                .and(predicates::prelude::predicate::str::contains("DRY=1").not()),
        );
}
