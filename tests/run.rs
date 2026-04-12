//! Tests for skill execution, help display, namespaced commands, optional args, and pipe behavior.

mod helpers;

use helpers::{creft_env, creft_with};
use predicates::prelude::*;
use pretty_assertions::assert_eq;

// ── run tests ─────────────────────────────────────────────────────────────────

/// Running a simple command with no args produces expected output.
#[test]
fn test_run_simple_command() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin("---\nname: greet\ndescription: say hello\n---\n\n```bash\necho hello\n```\n")
        .assert()
        .success();

    creft_with(&dir)
        .args(["greet"])
        .assert()
        .success()
        .stdout(predicate::str::contains("hello"));
}

/// Running a command that takes a positional arg substitutes it correctly.
#[test]
fn test_run_with_args() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(
            "---\nname: greet\ndescription: greet someone\nargs:\n  - name: who\n    description: name to greet\n---\n\n```bash\necho \"Hello, {{who}}!\"\n```\n",
        )
        .assert()
        .success();

    creft_with(&dir)
        .args(["greet", "World"])
        .assert()
        .success()
        // shell_escape wraps "World" in single quotes in the template, but bash
        // evaluates 'World' as the literal string World.
        .stdout(predicate::str::contains("Hello, World!"));
}

/// Running a command that does not exist exits with code 2.
#[test]
fn test_run_not_found() {
    let dir = creft_env();
    creft_with(&dir)
        .args(["nonexistent"])
        .assert()
        .failure()
        .code(2);
}

// ── help tests ────────────────────────────────────────────────────────────────

/// `creft<name> --help` prints the command's description and arg names.
#[test]
fn test_help_on_user_command() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(
            "---\nname: greet\ndescription: greet someone\nargs:\n  - name: who\n    description: name to greet\n---\n\n```bash\necho \"Hello, {{who}}!\"\n```\n",
        )
        .assert()
        .success();

    creft_with(&dir)
        .args(["greet", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("greet someone"))
        .stdout(predicate::str::contains("who"));
}

// ── namespaced command tests ──────────────────────────────────────────────────

/// Commands with a space in the name (e.g., "gh issue-body") are stored under
/// a sub-directory and can be invoked by passing each word as a separate arg.
#[test]
fn test_namespaced_command() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(
            "---\nname: gh issue-body\ndescription: fetch issue body\n---\n\n```bash\necho issue-output\n```\n",
        )
        .assert()
        .success();

    creft_with(&dir)
        .args(["gh", "issue-body"])
        .assert()
        .success()
        .stdout(predicate::str::contains("issue-output"));
}

/// A 4-token command (`hooks guard refs config`) routes to the deepest matching
/// file even when a shorter 3-token file (`hooks guard refs`) also exists.
/// Longest-match must win.
#[test]
fn test_four_token_command_routes_over_three_token_match() {
    let dir = creft_env();

    // Register the 3-token command first.
    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(concat!(
            "---\n",
            "name: hooks guard refs\n",
            "description: guard refs\n",
            "---\n",
            "\n",
            "```bash\n",
            "echo refs-output\n",
            "```\n",
        ))
        .assert()
        .success();

    // Register the 4-token command.
    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(concat!(
            "---\n",
            "name: hooks guard refs config\n",
            "description: guard refs config\n",
            "---\n",
            "\n",
            "```bash\n",
            "echo refs-config-output\n",
            "```\n",
        ))
        .assert()
        .success();

    // The 4-token invocation must route to the 4-token command, not the 3-token one.
    creft_with(&dir)
        .args(["hooks", "guard", "refs", "config"])
        .assert()
        .success()
        .stdout(predicate::str::contains("refs-config-output"));
}

/// The 3-token command still resolves correctly when the 4-token command also exists.
#[test]
fn test_three_token_command_still_resolves_when_four_token_sibling_exists() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(concat!(
            "---\n",
            "name: hooks guard refs\n",
            "description: guard refs\n",
            "---\n",
            "\n",
            "```bash\n",
            "echo refs-output\n",
            "```\n",
        ))
        .assert()
        .success();

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(concat!(
            "---\n",
            "name: hooks guard refs config\n",
            "description: guard refs config\n",
            "---\n",
            "\n",
            "```bash\n",
            "echo refs-config-output\n",
            "```\n",
        ))
        .assert()
        .success();

    creft_with(&dir)
        .args(["hooks", "guard", "refs"])
        .assert()
        .success()
        .stdout(predicate::str::contains("refs-output"));
}

// ── optional args tests ────────────────────────────────────────────────────────

/// A skill with `required: false` and a template default `{{count|5}}` runs
/// Optional arg omitted — parse_and_bind binds it to "". The bound "" takes
/// precedence over the template default, so `{{count|5}}` resolves to the
/// shell-escaped empty string, not "5".
#[test]
fn test_optional_arg_with_template_default_omitted() {
    let dir = creft_env();

    let markdown = "---\nname: opt-count\ndescription: counts things\nargs:\n  - name: count\n    description: how many\n    required: false\n---\n\n```bash\necho \"count={{count|5}}\"\n```\n";

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(markdown)
        .assert()
        .success();

    // Invoke without providing the arg — arg is bound to "", template default does not fire.
    creft_with(&dir)
        .args(["opt-count"])
        .assert()
        .success()
        .stdout(predicate::str::contains("count=''"));
}

/// Same skill as above, but the arg is provided — the provided value is used,
/// not the template default.
#[test]
fn test_optional_arg_with_template_default_provided() {
    let dir = creft_env();

    let markdown = "---\nname: opt-count2\ndescription: counts things\nargs:\n  - name: count\n    description: how many\n    required: false\n---\n\n```bash\necho \"count={{count|5}}\"\n```\n";

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(markdown)
        .assert()
        .success();

    // Invoke with a value — template default is overridden.
    creft_with(&dir)
        .args(["opt-count2", "42"])
        .assert()
        .success()
        .stdout(predicate::str::contains("count=42"));
}

/// A skill with `required: false`, no frontmatter default, and a bare `{{name}}`
/// template resolves to empty string when the arg is omitted. parse_and_bind
/// now binds optional args to "" so `{{name}}` substitutes as the shell-escaped
/// empty string rather than erroring.
#[test]
fn test_optional_arg_no_default_anywhere_errors() {
    let dir = creft_env();

    // Template uses {{name}} with no `|default` — resolves to '' when omitted.
    let markdown = "---\nname: opt-nodefault\ndescription: needs a name\nargs:\n  - name: name\n    description: a name\n    required: false\n---\n\n```bash\necho \"hello {{name}}\"\n```\n";

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(markdown)
        .assert()
        .success();

    // Invoke without the arg — succeeds, resolves to empty string.
    creft_with(&dir)
        .args(["opt-nodefault"])
        .assert()
        .success()
        .stdout(predicate::str::contains("hello ''"));
}

// ── pipe intermediate output suppression tests ────────────────────────────────

/// A two-block pipe skill must print only the last block's stdout to the
/// terminal. The intermediate block's output must NOT appear as a bare line —
/// it is forwarded silently as stdin to the next block.
#[test]
fn test_pipe_intermediate_output_suppressed() {
    let dir = creft_env();

    // Block 1 echoes "intermediate" to stdout.
    // Block 2 reads stdin (piped from block 1) and prints "final: <stdin>".
    // Only block 2's output should reach the terminal.
    let markdown = concat!(
        "---\n",
        "name: pipe-two-block\n",
        "description: pipe test\n",
        "---\n",
        "\n",
        "```bash\n",
        "echo intermediate\n",
        "```\n",
        "\n",
        "```bash\n",
        "stdin=$(cat)\n",
        "echo \"final: $stdin\"\n",
        "```\n",
    );

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(markdown)
        .assert()
        .success();

    let output = creft_with(&dir)
        .args(["pipe-two-block"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout_str = String::from_utf8_lossy(&output);

    // The final block's output is present.
    assert!(
        stdout_str.contains("final: intermediate"),
        "expected 'final: intermediate' in stdout, got: {:?}",
        stdout_str
    );

    // The intermediate block's output must NOT appear as a bare "intermediate"
    // line — it was forwarded silently. The only place "intermediate" should
    // appear is inside "final: intermediate".
    let lines: Vec<&str> = stdout_str.lines().collect();
    assert!(
        !lines.iter().any(|l| l.trim() == "intermediate"),
        "intermediate block stdout should not appear as a bare line; got: {:?}",
        stdout_str
    );
}

/// Multi-block skills pipe by default: block 1's stdout feeds block 2's stdin.
/// Block 2 reads stdin via `cat` (passing block 1's output through) then appends
/// its own line. Both markers must appear in the combined output.
#[test]
fn test_multi_block_default_pipes_integration() {
    let dir = creft_env();

    let markdown = concat!(
        "---\n",
        "name: non-pipe-two-block\n",
        "description: non-pipe test\n",
        "---\n",
        "\n",
        "```bash\n",
        "echo block-one-output\n",
        "```\n",
        "\n",
        "```bash\n",
        "cat\n",
        "echo block-two-output\n",
        "```\n",
    );

    creft_with(&dir)
        .args(["cmd", "add", "--no-validate"])
        .write_stdin(markdown)
        .assert()
        .success();

    creft_with(&dir)
        .args(["non-pipe-two-block"])
        .assert()
        .success()
        .stdout(predicate::str::contains("block-one-output"))
        .stdout(predicate::str::contains("block-two-output"));
}

// ── pipe env var and E2BIG fix tests ─────────────────────────────────────────

/// A pipe-mode skill whose first block emits output exceeding 256KB (the macOS
/// E2BIG threshold for execve environments) must succeed. Previously, creft
/// would unconditionally set CREFT_PREV and CREFT_BLOCK_N env vars containing
/// the full output, causing the OS to reject the child process spawn.
#[test]
fn test_pipe_large_output_no_e2big() {
    let dir = creft_env();

    // Block 1: generate ~300KB of output (well above the macOS 256KB env limit).
    // Block 2: read stdin and print the byte length.
    let markdown = concat!(
        "---\n",
        "name: pipe-large-output\n",
        "description: large pipe test\n",
        "---\n",
        "\n",
        "```python3\n",
        "print('x' * 300_000, end='')\n",
        "```\n",
        "\n",
        "```python3\n",
        "import sys\n",
        "data = sys.stdin.read()\n",
        "print(len(data))\n",
        "```\n",
    );

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(markdown)
        .assert()
        .success();

    creft_with(&dir)
        .args(["pipe-large-output"])
        .assert()
        .success()
        // Block 2 receives the 300000-byte string on stdin and prints its length.
        .stdout(predicate::str::contains("300000"));
}

/// In pipe mode, CREFT_PREV must NOT be set as an environment variable.
/// Blocks in pipe mode should read stdin instead.
#[test]
fn test_pipe_mode_no_creft_prev_env() {
    let dir = creft_env();

    let markdown = concat!(
        "---\n",
        "name: pipe-no-creft-prev\n",
        "description: verify CREFT_PREV absent in pipe mode\n",
        "---\n",
        "\n",
        "```bash\n",
        "echo some-value\n",
        "```\n",
        "\n",
        "```bash\n",
        // Print CREFT_PREV if set, otherwise print "EMPTY"
        "echo \"${CREFT_PREV:-EMPTY}\"\n",
        "```\n",
    );

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(markdown)
        .assert()
        .success();

    creft_with(&dir)
        .args(["pipe-no-creft-prev"])
        .assert()
        .success()
        // CREFT_PREV must not be set in pipe mode — block 2 should print EMPTY.
        .stdout(predicate::str::contains("EMPTY"));
}

/// In pipe mode, `{{prev}}` is not bound as a template arg (output is on stdin).
/// Unmatched placeholders pass through as literal text.
#[test]
fn test_pipe_mode_prev_placeholder_passes_through() {
    let dir = creft_env();

    let markdown = concat!(
        "---\n",
        "name: pipe-prev-placeholder\n",
        "description: prev placeholder in pipe mode\n",
        "---\n",
        "\n",
        "```bash\n",
        "echo hello\n",
        "```\n",
        "\n",
        "```bash\n",
        // {{prev}} is not bound in pipe mode — passes through as literal text.
        "echo '{{prev}}'\n",
        "```\n",
    );

    creft_with(&dir)
        .args(["cmd", "add", "--no-validate"])
        .write_stdin(markdown)
        .assert()
        .success();

    creft_with(&dir)
        .args(["pipe-prev-placeholder"])
        .assert()
        .success()
        .stdout(predicate::str::contains("{{prev}}"));
}

// ── subcommand-aware help tests ────────────────────────────────────────────────

/// When a skill and a subcommand share a name prefix, `creft <name> --help`
/// shows the skill's help text AND a Subcommands section listing the children.
#[test]
fn test_help_shows_subcommands_when_parent_and_child_exist() {
    let dir = creft_env();

    // Create the parent skill "test" with a positional arg.
    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(concat!(
            "---\n",
            "name: test\n",
            "description: Run project tests\n",
            "args:\n",
            "  - name: filter\n",
            "    description: test name or pattern to filter by\n",
            "---\n",
            "\n",
            "```bash\n",
            "echo \"filter={{filter}}\"\n",
            "```\n",
        ))
        .assert()
        .success();

    // Create the child skill "test mutants".
    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(concat!(
            "---\n",
            "name: test mutants\n",
            "description: Run mutation testing\n",
            "---\n",
            "\n",
            "```bash\n",
            "echo mutants-ran\n",
            "```\n",
        ))
        .assert()
        .success();

    // Requesting help on the parent skill should show both the skill description
    // and a Subcommands section listing the child.
    creft_with(&dir)
        .args(["test", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Run project tests"))
        .stdout(predicate::str::contains("Subcommands:"))
        .stdout(predicate::str::contains("test mutants"))
        .stdout(predicate::str::contains("Run mutation testing"));
}

// ── exit 99 pipe kill tests ───────────────────────────────────────────────────

/// When a block in a pipe chain exits 99, all subsequent blocks must be killed
/// immediately. creft must return 0, and no output from killed blocks should
/// appear. The test asserts the whole run completes well under 2 seconds even
/// though block 2 would have blocked for 5 seconds if not killed.
#[test]
fn test_pipe_exit_99_kills_remaining_blocks() {
    let dir = creft_env();

    let markdown = concat!(
        "---\n",
        "name: pipe-exit-99-kill\n",
        "description: exit 99 kills remaining blocks\n",
        "---\n",
        "\n",
        "```bash\n",
        "exit 99\n",
        "```\n",
        "\n",
        "```bash\n",
        "sleep 5 && echo 'should not appear'\n",
        "```\n",
    );

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(markdown)
        .assert()
        .success();

    let output = creft_with(&dir)
        .args(["pipe-exit-99-kill"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout_str = String::from_utf8_lossy(&output);
    assert!(
        !stdout_str.contains("should not appear"),
        "block 2 output must not appear after exit 99; got: {:?}",
        stdout_str
    );
}

/// When a **middle** block in a 3-block pipe chain exits 99, all subsequent
/// blocks must be killed immediately. Block 1 completes normally; block 2
/// reads stdin, prints something, then exits 99; block 3 (`sleep 5`) must be
/// killed before it produces output. creft must return 0 and the run must
/// finish well under 2 seconds.
#[test]
fn test_pipe_exit_99_middle_block() {
    let dir = creft_env();

    let markdown = concat!(
        "---\n",
        "name: pipe-mid-exit99\n",
        "description: middle block exit 99 test\n",
        "---\n",
        "\n",
        "```bash\n",
        "echo \"upstream data\"\n",
        "```\n",
        "\n",
        "```bash\n",
        "cat\n",
        "echo processed\n",
        "exit 99\n",
        "```\n",
        "\n",
        "```bash\n",
        "sleep 5\n",
        "echo 'should not appear'\n",
        "```\n",
    );

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(markdown)
        .assert()
        .success();

    let start = std::time::Instant::now();
    let output = creft_with(&dir)
        .args(["pipe-mid-exit99"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let elapsed = start.elapsed();

    let stdout_str = String::from_utf8_lossy(&output);
    assert!(
        !stdout_str.contains("should not appear"),
        "block 3 output must not appear after middle block exits 99; got: {:?}",
        stdout_str
    );
    assert!(
        elapsed.as_secs() < 2,
        "pipe must terminate quickly after middle block exits 99 (took {:?})",
        elapsed
    );
}

// ── upstream block failure reporting ─────────────────────────────────────────

/// When block 0 fails and block 1 succeeds, creft must surface the upstream
/// failure rather than treating last-block success as overall success.
#[test]
fn upstream_block_failure_reported_when_last_block_succeeds() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(concat!(
            "---\n",
            "name: upstream-fail-last-ok\n",
            "description: upstream block fails, downstream succeeds\n",
            "---\n",
            "\n",
            "```bash\n",
            "echo upstream-error-output >&2\n",
            "exit 1\n",
            "```\n",
            "\n",
            "```bash\n",
            "cat\n",
            "```\n",
        ))
        .assert()
        .success();

    let output = creft_with(&dir)
        .args(["upstream-fail-last-ok"])
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "skill must fail when upstream block exits 1"
    );
    assert_eq!(
        output.status.code(),
        Some(1),
        "exit code must match the upstream block's exit code"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("upstream-error-output"),
        "upstream block's stderr must appear in creft's output; got: {stderr:?}"
    );
}

/// When block 0 fails and block 1 also fails, the root cause is the earliest
/// failure (block 0), not the last block.
#[test]
fn earliest_failure_is_root_cause_when_multiple_blocks_fail() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(concat!(
            "---\n",
            "name: both-fail\n",
            "description: both blocks fail\n",
            "---\n",
            "\n",
            "```bash\n",
            "echo first-block-error >&2\n",
            "exit 1\n",
            "```\n",
            "\n",
            "```bash\n",
            "cat\n",
            "exit 2\n",
            "```\n",
        ))
        .assert()
        .success();

    let output = creft_with(&dir).args(["both-fail"]).output().unwrap();

    assert!(!output.status.success(), "skill must fail");
    assert_eq!(
        output.status.code(),
        Some(1),
        "exit code must be from the earliest (block 0) failure, not block 1"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("first-block-error"),
        "block 0's stderr must appear; got: {stderr:?}"
    );
}

/// When all blocks succeed, creft returns 0. The upstream failure check must
/// not fire for healthy pipelines.
#[test]
fn all_blocks_succeed_returns_ok() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(concat!(
            "---\n",
            "name: all-ok\n",
            "description: all blocks succeed\n",
            "---\n",
            "\n",
            "```bash\n",
            "echo hello\n",
            "```\n",
            "\n",
            "```bash\n",
            "cat\n",
            "```\n",
        ))
        .assert()
        .success();

    creft_with(&dir).args(["all-ok"]).assert().success();
}

/// When an upstream block produces more output than the consumer reads and
/// exits with code 0 (producer finishes before consumer stops reading), the
/// pipeline must succeed. This verifies the `find_root_cause` filter does not
/// misreport a clean exit as a failure.
///
/// Note: the inverse case — an upstream block killed by SIGPIPE because the
/// consumer exits early — is covered by `find_root_cause`'s `exit_code_of`
/// filter at the unit-test level. An end-to-end SIGPIPE integration test
/// requires creft to actively kill the upstream process group when the last
/// block exits, which is outside Stage 1's scope.
#[test]
fn upstream_exit_zero_with_partial_consumer_returns_ok() {
    let dir = creft_env();

    // Block 0: echo 5 lines. Block 1: read all of them (cat).
    // Both blocks exit 0. No root cause. creft must return 0.
    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(concat!(
            "---\n",
            "name: multi-line-cat\n",
            "description: multi-line producer with cat consumer\n",
            "---\n",
            "\n",
            "```bash\n",
            "printf 'line1\\nline2\\nline3\\nline4\\nline5\\n'\n",
            "```\n",
            "\n",
            "```bash\n",
            "cat\n",
            "```\n",
        ))
        .assert()
        .success();

    creft_with(&dir).args(["multi-line-cat"]).assert().success();
}

/// In a 3-block chain where the middle block fails and the last block gets
/// EOF and exits 0, the middle block is reported as the root cause.
#[test]
fn middle_block_failure_reported_in_three_block_chain() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(concat!(
            "---\n",
            "name: mid-fail-three\n",
            "description: middle block fails in three-block chain\n",
            "---\n",
            "\n",
            "```bash\n",
            "echo data\n",
            "```\n",
            "\n",
            "```bash\n",
            "echo mid-block-error >&2\n",
            "exit 3\n",
            "```\n",
            "\n",
            "```bash\n",
            "cat\n",
            "```\n",
        ))
        .assert()
        .success();

    let output = creft_with(&dir).args(["mid-fail-three"]).output().unwrap();

    assert!(
        !output.status.success(),
        "skill must fail when middle block exits 3"
    );
    assert_eq!(
        output.status.code(),
        Some(3),
        "exit code must be the middle block's exit code (3)"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("mid-block-error"),
        "middle block's stderr must appear; got: {stderr:?}"
    );
}

// ── parent/child coexistence tests ────────────────────────────────────────────

/// When both `test` (with a positional arg) and `test mutants` exist,
/// `creft test mutants` resolves to `test mutants` -- not `test` with `filter=mutants`.
#[test]
fn test_subcommand_resolved_over_parent_positional_arg() {
    let dir = creft_env();

    // Parent skill: test with positional arg `filter`.
    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(concat!(
            "---\n",
            "name: test\n",
            "description: Run project tests\n",
            "args:\n",
            "  - name: filter\n",
            "    description: test name or pattern\n",
            "---\n",
            "\n",
            "```bash\n",
            "echo \"filter={{filter}}\"\n",
            "```\n",
        ))
        .assert()
        .success();

    // Child skill: test mutants (stored as commands/test/mutants.md).
    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(concat!(
            "---\n",
            "name: test mutants\n",
            "description: Run mutation testing\n",
            "---\n",
            "\n",
            "```bash\n",
            "echo \"mutants-ran\"\n",
            "```\n",
        ))
        .assert()
        .success();

    // `creft test mutants` must route to the child, not the parent.
    creft_with(&dir)
        .args(["test", "mutants"])
        .assert()
        .success()
        .stdout(predicate::str::contains("mutants-ran"))
        .stdout(predicate::str::contains("filter=mutants").not());
}

/// When both `test` (with a positional arg) and `test mutants` exist,
/// `creft test myfilter` still resolves to `test` with `filter=myfilter`
/// because `myfilter` does not match any child command name.
#[test]
fn test_parent_with_positional_still_works_when_child_exists() {
    let dir = creft_env();

    // Parent skill: test with positional arg `filter`.
    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(concat!(
            "---\n",
            "name: test2\n",
            "description: Run project tests\n",
            "args:\n",
            "  - name: filter\n",
            "    description: test name or pattern\n",
            "---\n",
            "\n",
            "```bash\n",
            "echo \"filter={{filter}}\"\n",
            "```\n",
        ))
        .assert()
        .success();

    // Child skill: test2 mutants.
    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(concat!(
            "---\n",
            "name: test2 mutants\n",
            "description: Run mutation testing\n",
            "---\n",
            "\n",
            "```bash\n",
            "echo \"mutants-ran\"\n",
            "```\n",
        ))
        .assert()
        .success();

    // `creft test2 myfilter` must route to the parent with the positional arg.
    creft_with(&dir)
        .args(["test2", "myfilter"])
        .assert()
        .success()
        .stdout(predicate::str::contains("filter=myfilter"));
}

/// When `test mutants` has its own positional arg `target`, running
/// `creft test mutants error.rs` passes `error.rs` as `target` to the subcommand.
#[test]
fn test_subcommand_with_own_args() {
    let dir = creft_env();

    // Parent skill: test with positional arg `filter`.
    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(concat!(
            "---\n",
            "name: test3\n",
            "description: Run project tests\n",
            "args:\n",
            "  - name: filter\n",
            "    description: test name or pattern\n",
            "---\n",
            "\n",
            "```bash\n",
            "echo \"filter={{filter}}\"\n",
            "```\n",
        ))
        .assert()
        .success();

    // Child skill: test3 mutants with its own `target` positional arg.
    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(concat!(
            "---\n",
            "name: test3 mutants\n",
            "description: Run mutation testing\n",
            "args:\n",
            "  - name: target\n",
            "    description: file to target\n",
            "---\n",
            "\n",
            "```bash\n",
            "echo \"target={{target}}\"\n",
            "```\n",
        ))
        .assert()
        .success();

    // `creft test3 mutants error.rs` must resolve to test3 mutants with target=error.rs.
    creft_with(&dir)
        .args(["test3", "mutants", "error.rs"])
        .assert()
        .success()
        .stdout(predicate::str::contains("target=error.rs"));
}

/// `creft test mutants --help` shows the subcommand's own help, not the parent's.
#[test]
fn test_subcommand_help_shows_subcommand_details() {
    let dir = creft_env();

    // Parent skill: test4 with positional arg `filter`.
    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(concat!(
            "---\n",
            "name: test4\n",
            "description: Run project tests\n",
            "args:\n",
            "  - name: filter\n",
            "    description: test name or pattern\n",
            "---\n",
            "\n",
            "```bash\n",
            "echo \"filter={{filter}}\"\n",
            "```\n",
        ))
        .assert()
        .success();

    // Child skill: test4 mutants with description and `target` arg.
    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(concat!(
            "---\n",
            "name: test4 mutants\n",
            "description: Run mutation testing\n",
            "args:\n",
            "  - name: target\n",
            "    description: file to target\n",
            "---\n",
            "\n",
            "```bash\n",
            "echo \"target={{target}}\"\n",
            "```\n",
        ))
        .assert()
        .success();

    // `--help` on the subcommand shows the subcommand's own description and args.
    creft_with(&dir)
        .args(["test4", "mutants", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Run mutation testing"))
        .stdout(predicate::str::contains("target"));
}

/// When a skill has no child commands, `creft <name> --help` must NOT show a
/// Subcommands section.
#[test]
fn test_help_no_subcommands_section_for_leaf_skill() {
    let dir = creft_env();

    // Create a standalone skill with no children.
    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(concat!(
            "---\n",
            "name: hello\n",
            "description: say hello\n",
            "---\n",
            "\n",
            "```bash\n",
            "echo hello\n",
            "```\n",
        ))
        .assert()
        .success();

    creft_with(&dir)
        .args(["hello", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("say hello"))
        .stdout(predicate::str::contains("Subcommands:").not());
}

// ── flat file migration tests ─────────────────────────────────────────────────

/// A flat file `commands/test mutants.md` (with space in filename) is automatically
/// migrated to `commands/test/mutants.md` on first resolution.
#[test]
fn test_flat_file_migrated_on_resolution() {
    let dir = creft_env();

    // Create the commands directory and write a flat file with a space in the name.
    let commands_dir = dir.path().join("commands");
    std::fs::create_dir_all(&commands_dir).unwrap();

    let flat_content = concat!(
        "---\n",
        "name: test mutants\n",
        "description: run mutation tests\n",
        "---\n",
        "\n",
        "```bash\n",
        "echo migrated\n",
        "```\n",
    );
    std::fs::write(commands_dir.join("test mutants.md"), flat_content).unwrap();

    // Running the command should migrate the flat file and execute it successfully.
    creft_with(&dir)
        .args(["test", "mutants"])
        .assert()
        .success()
        .stdout(predicate::str::contains("migrated"))
        .stderr(predicate::str::contains("migrated:"));

    // After resolution, the flat file should be gone and directory version should exist.
    assert!(
        !commands_dir.join("test mutants.md").exists(),
        "flat file should have been removed after migration"
    );
    assert!(
        commands_dir.join("test").join("mutants.md").exists(),
        "directory-structured file should exist after migration"
    );
}

/// When both a flat file (`commands/test mutants.md`) and a directory version
/// (`commands/test/mutants.md`) exist, the directory version wins and the flat
/// file is left in place.
#[test]
fn test_flat_file_skipped_when_directory_version_exists() {
    let dir = creft_env();

    let commands_dir = dir.path().join("commands");
    std::fs::create_dir_all(commands_dir.join("test")).unwrap();

    // The directory version echoes "directory-version".
    let dir_content = concat!(
        "---\n",
        "name: test mutants\n",
        "description: directory version\n",
        "---\n",
        "\n",
        "```bash\n",
        "echo directory-version\n",
        "```\n",
    );
    std::fs::write(commands_dir.join("test").join("mutants.md"), dir_content).unwrap();

    // A flat file also exists with a different echo.
    let flat_content = concat!(
        "---\n",
        "name: test mutants\n",
        "description: flat version\n",
        "---\n",
        "\n",
        "```bash\n",
        "echo flat-version\n",
        "```\n",
    );
    std::fs::write(commands_dir.join("test mutants.md"), flat_content).unwrap();

    // The directory version should win, and a note should be emitted to stderr
    // alerting the user that the stale flat file is being ignored.
    creft_with(&dir)
        .args(["test", "mutants"])
        .assert()
        .success()
        .stdout(predicate::str::contains("directory-version"))
        .stdout(predicate::str::contains("flat-version").not())
        .stderr(predicate::str::contains("takes priority"));

    // The flat file should still be present (not deleted).
    assert!(
        commands_dir.join("test mutants.md").exists(),
        "flat file should be preserved when directory version wins"
    );
}

/// A single-token command (`hello`) does not trigger the flat file migration
/// check and runs normally.
#[test]
fn test_flat_file_not_triggered_for_single_token() {
    let dir = creft_env();

    // Add a normal single-token skill via creft add.
    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(concat!(
            "---\n",
            "name: hello-single\n",
            "description: say hello\n",
            "---\n",
            "\n",
            "```bash\n",
            "echo hello\n",
            "```\n",
        ))
        .assert()
        .success();

    // Running it should work with no migration messages on stderr.
    creft_with(&dir)
        .args(["hello-single"])
        .assert()
        .success()
        .stdout(predicate::str::contains("hello"))
        .stderr(predicate::str::contains("migrated:").not());
}

/// A flat file with two spaces (`commands/a b c.md`) is migrated to the correct
/// nested directory structure (`commands/a/b/c.md`).
#[test]
fn test_flat_file_nested_spaces_migrated() {
    let dir = creft_env();

    let commands_dir = dir.path().join("commands");
    std::fs::create_dir_all(&commands_dir).unwrap();

    let flat_content = concat!(
        "---\n",
        "name: a b c\n",
        "description: nested flat file\n",
        "---\n",
        "\n",
        "```bash\n",
        "echo nested-migrated\n",
        "```\n",
    );
    std::fs::write(commands_dir.join("a b c.md"), flat_content).unwrap();

    creft_with(&dir)
        .args(["a", "b", "c"])
        .assert()
        .success()
        .stdout(predicate::str::contains("nested-migrated"))
        .stderr(predicate::str::contains("migrated:"));

    // The flat file should be gone.
    assert!(
        !commands_dir.join("a b c.md").exists(),
        "flat file should have been removed after migration"
    );
    // The directory-structured file should exist.
    assert!(
        commands_dir.join("a").join("b").join("c.md").exists(),
        "nested directory-structured file should exist after migration"
    );
}

// ── SIGINT propagation in pipe chains ─────────────────────────────────────────

/// Helper: add a skill to a creft environment and return the CREFT_HOME path.
#[cfg(unix)]
fn add_skill_to_dir(dir: &tempfile::TempDir, markdown: &str) {
    creft_with(dir)
        .args(["cmd", "add"])
        .write_stdin(markdown)
        .assert()
        .success();
}

/// When SIGINT is sent to creft while running a two-block pipe skill, creft
/// should exit with code 130 (128 + SIGINT) and NOT print "was killed by signal".
///
/// The test spawns creft as a real subprocess, waits for children to start, then
/// sends SIGINT to the creft process. It verifies:
/// 1. creft exits with code 130
/// 2. stderr does not contain "was killed by signal"
#[test]
#[cfg(unix)]
fn test_pipe_sigint_clean_exit() {
    let dir = creft_env();

    // Block 0: sleep 10 (will be killed by SIGINT).
    // Block 1: cat (reads stdin, will also be killed).
    add_skill_to_dir(
        &dir,
        concat!(
            "---\n",
            "name: pipe-sigint-two\n",
            "description: SIGINT test skill\n",
            "---\n",
            "\n",
            "```bash\n",
            "sleep 10\n",
            "```\n",
            "\n",
            "```bash\n",
            "cat\n",
            "```\n",
        ),
    );

    // Spawn creft as a subprocess (not via assert_cmd — we need the PID to send signals).
    let mut child = std::process::Command::new(assert_cmd::cargo::cargo_bin("creft"))
        .env("CREFT_HOME", dir.path())
        .env_remove("CREFT_PROJECT_ROOT")
        .args(["pipe-sigint-two"])
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn creft");

    // Wait for children to start running.
    std::thread::sleep(std::time::Duration::from_millis(300));

    // Send SIGINT to creft.
    // SAFETY: kill(pid, SIGINT) is a standard POSIX call. child.id() is valid
    // for the duration of this scope (we have not yet waited on the child).
    unsafe {
        libc::kill(child.id() as libc::pid_t, libc::SIGINT);
    }

    // Wait for creft to exit (with a generous timeout).
    let status = child.wait().expect("failed to wait for creft");

    // creft should exit with code 130 (128 + SIGINT=2).
    // On Unix, when a process re-raises SIGINT after restoring SIG_DFL, the
    // shell sees it as signal-killed (code 130).
    let code = status.code().unwrap_or_else(|| {
        // If code() is None, the process was itself killed by a signal.
        // That also satisfies the test — the child group was cleaned up.
        use std::os::unix::process::ExitStatusExt;
        128 + status.signal().unwrap_or(2)
    });
    assert_eq!(
        code, 130,
        "creft should exit with code 130 after SIGINT, got {code}"
    );
}

/// When SIGINT is sent to creft while running a pipe chain, creft's own stderr
/// should NOT contain "was killed by signal" (ExecutionSignaled for SIGINT is quiet).
#[test]
#[cfg(unix)]
fn test_pipe_sigint_no_signal_message() {
    let dir = creft_env();

    add_skill_to_dir(
        &dir,
        concat!(
            "---\n",
            "name: pipe-sigint-quiet\n",
            "description: SIGINT quiet test skill\n",
            "---\n",
            "\n",
            "```bash\n",
            "sleep 10\n",
            "```\n",
            "\n",
            "```bash\n",
            "cat\n",
            "```\n",
        ),
    );

    let child = std::process::Command::new(assert_cmd::cargo::cargo_bin("creft"))
        .env("CREFT_HOME", dir.path())
        .env_remove("CREFT_PROJECT_ROOT")
        .args(["pipe-sigint-quiet"])
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn creft");

    std::thread::sleep(std::time::Duration::from_millis(300));

    unsafe {
        libc::kill(child.id() as libc::pid_t, libc::SIGINT);
    }

    let output = child.wait_with_output().expect("failed to wait for creft");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("was killed by signal"),
        "creft should not print 'was killed by signal' on SIGINT, got stderr: {stderr:?}"
    );
}

/// When the LAST block in a pipe chain is killed by SIGTERM (not SIGINT),
/// creft SHOULD print the "was killed by signal" message. Only SIGINT is suppressed.
///
/// Note: if an INTERMEDIATE block dies from a signal (e.g. SIGPIPE, SIGTERM)
/// and the LAST block still exits cleanly, creft reports success (normal
/// pipeline behavior). This test uses SIGTERM on the last block to ensure
/// the signal is visible.
#[test]
#[cfg(unix)]
fn test_pipe_non_sigint_signal_still_reported() {
    let dir = creft_env();

    // Block 0: echo some data.
    // Block 1 (last block): kills itself with SIGTERM while reading stdin.
    // When the last block dies from SIGTERM, creft should report it.
    add_skill_to_dir(
        &dir,
        concat!(
            "---\n",
            "name: pipe-sigterm-report\n",
            "description: SIGTERM should be reported\n",
            "---\n",
            "\n",
            "```bash\n",
            "echo hello\n",
            "```\n",
            "\n",
            "```bash\n",
            // Read stdin (consumes 'hello'), then kill self with SIGTERM.
            "read line; kill -TERM $$\n",
            "```\n",
        ),
    );

    creft_with(&dir)
        .args(["pipe-sigterm-report"])
        .assert()
        .failure()
        // SIGTERM death of last block is not quiet — creft should report it.
        .stderr(predicate::str::contains("was killed by signal"));
}

/// When a sequential (non-pipe) block is killed by SIGINT, creft reports the
/// signal (non-quiet). The quiet-on-SIGINT suppression is pipe-chain specific —
/// sequential mode has a different flow (the signal kills creft itself first).
///
/// This test verifies that the SIGTERM-reporting path works end-to-end in
/// sequential mode, ensuring test_pipe_non_sigint_signal_still_reported's
/// assertion about SIGTERM being reported is independently validated.
#[test]
#[cfg(unix)]
fn test_sequential_sigterm_block_reported() {
    let dir = creft_env();

    // A sequential (non-pipe) skill whose block kills itself with SIGTERM.
    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(concat!(
            "---\n",
            "name: seq-sigterm\n",
            "description: sequential SIGTERM test\n",
            "---\n",
            "\n",
            "```bash\n",
            "kill -TERM $$\n",
            "```\n",
        ))
        .assert()
        .success();

    creft_with(&dir)
        .args(["seq-sigterm"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("was killed by signal"));
}

/// When SIGINT is sent to creft while running a three-block pipe chain
/// (`sleep 10` | `cat` | `cat`), all three child processes must be dead
/// after creft exits. No zombies or orphans.
///
/// Each block writes its PID to a file so we can verify the processes are
/// gone after creft exits with code 130.
#[test]
#[cfg(unix)]
fn test_pipe_sigint_all_children_die() {
    let dir = creft_env();

    // Each block writes its PID to a known file in CREFT_HOME, then runs
    // a long-lived command. After SIGINT, we verify those PIDs are dead.
    let pid0_path = dir.path().join("block0.pid");
    let pid1_path = dir.path().join("block1.pid");
    let pid2_path = dir.path().join("block2.pid");

    let pid0_str = pid0_path.to_str().expect("valid UTF-8 path");
    let pid1_str = pid1_path.to_str().expect("valid UTF-8 path");
    let pid2_str = pid2_path.to_str().expect("valid UTF-8 path");

    let markdown = format!(
        concat!(
            "---\n",
            "name: pipe-sigint-three\n",
            "description: three-block SIGINT test\n",
            "---\n",
            "\n",
            "```bash\n",
            "echo $$ > {pid0}; sleep 10\n",
            "```\n",
            "\n",
            "```bash\n",
            "echo $$ > {pid1}; cat\n",
            "```\n",
            "\n",
            "```bash\n",
            "echo $$ > {pid2}; cat\n",
            "```\n",
        ),
        pid0 = pid0_str,
        pid1 = pid1_str,
        pid2 = pid2_str,
    );

    add_skill_to_dir(&dir, &markdown);

    let mut child = std::process::Command::new(assert_cmd::cargo::cargo_bin("creft"))
        .env("CREFT_HOME", dir.path())
        .env_remove("CREFT_PROJECT_ROOT")
        .args(["pipe-sigint-three"])
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn creft");

    // Wait for all three blocks to write their PIDs. Poll for up to 3 seconds.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        if pid0_path.exists() && pid1_path.exists() && pid2_path.exists() {
            break;
        }
        if std::time::Instant::now() >= deadline {
            // Send SIGINT anyway — best effort even if startup was slow.
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    // Read the PIDs written by the blocks.
    let read_pid = |path: &std::path::Path| -> Option<libc::pid_t> {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| s.trim().parse::<libc::pid_t>().ok())
    };
    let pid0 = read_pid(&pid0_path);
    let pid1 = read_pid(&pid1_path);
    let pid2 = read_pid(&pid2_path);

    // Send SIGINT to creft.
    // SAFETY: kill(pid, SIGINT) is a standard POSIX call. child.id() is valid
    // for the duration of this scope.
    unsafe {
        libc::kill(child.id() as libc::pid_t, libc::SIGINT);
    }

    let status = child.wait().expect("failed to wait for creft");

    let code = status.code().unwrap_or_else(|| {
        use std::os::unix::process::ExitStatusExt;
        128 + status.signal().unwrap_or(2)
    });
    assert_eq!(
        code, 130,
        "creft should exit with code 130 after SIGINT, got {code}"
    );

    // Give the OS a moment to reap the child processes.
    std::thread::sleep(std::time::Duration::from_millis(100));

    // Verify each recorded PID is no longer alive.
    // kill -0 returns 0 if the process exists, ESRCH if it does not.
    let is_alive = |pid: libc::pid_t| -> bool {
        // SAFETY: kill(pid, 0) probes process existence without sending a
        // signal. This is a standard POSIX idiom.
        unsafe { libc::kill(pid, 0) == 0 }
    };

    if let Some(p) = pid0 {
        assert!(
            !is_alive(p),
            "block 0 process (PID {p}) should be dead after SIGINT"
        );
    }
    if let Some(p) = pid1 {
        assert!(
            !is_alive(p),
            "block 1 process (PID {p}) should be dead after SIGINT"
        );
    }
    if let Some(p) = pid2 {
        assert!(
            !is_alive(p),
            "block 2 process (PID {p}) should be dead after SIGINT"
        );
    }
}

/// When creft is running a pipe skill with piped stdin (non-terminal mode),
/// sending SIGINT to creft must result in a clean exit (code 130) with no
/// hang or crash.
///
/// Non-terminal mode uses `sigint_forward_handler` (not `tcsetpgrp`) to
/// forward SIGINT to the child process group. This test exercises that path.
#[test]
#[cfg(unix)]
fn test_pipe_sigint_non_terminal() {
    let dir = creft_env();

    // A two-block pipe skill with long-lived blocks.
    add_skill_to_dir(
        &dir,
        concat!(
            "---\n",
            "name: pipe-sigint-nontty\n",
            "description: non-terminal SIGINT test\n",
            "---\n",
            "\n",
            "```bash\n",
            "sleep 10\n",
            "```\n",
            "\n",
            "```bash\n",
            "cat\n",
            "```\n",
        ),
    );

    // Spawn creft with piped stdin — this makes stdin non-terminal, so
    // `std::io::stdin().is_terminal()` returns false inside creft, activating
    // the `sigint_forward_handler` code path instead of `tcsetpgrp`.
    let mut child = std::process::Command::new(assert_cmd::cargo::cargo_bin("creft"))
        .env("CREFT_HOME", dir.path())
        .env_remove("CREFT_PROJECT_ROOT")
        .args(["pipe-sigint-nontty"])
        .stdin(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn creft");

    // Allow children time to start.
    std::thread::sleep(std::time::Duration::from_millis(300));

    // Send SIGINT to creft. In non-terminal mode creft's handler forwards
    // SIGINT to the child process group.
    // SAFETY: kill(pid, SIGINT) is a standard POSIX call. child.id() is valid
    // for the duration of this scope.
    unsafe {
        libc::kill(child.id() as libc::pid_t, libc::SIGINT);
    }

    // creft must exit within a reasonable time — not hang.
    let status = child.wait().expect("failed to wait for creft");

    let code = status.code().unwrap_or_else(|| {
        use std::os::unix::process::ExitStatusExt;
        128 + status.signal().unwrap_or(2)
    });
    assert_eq!(
        code, 130,
        "creft should exit with code 130 after SIGINT in non-terminal mode, got {code}"
    );
}

/// When SIGINT is sent to a pipe chain where block 1 is a Python script reading
/// stdin, Python must NOT print a KeyboardInterrupt traceback. Non-first blocks
/// have SIGINT set to SIG_IGN before exec, so Python ignores it and exits via
/// EOF/SIGPIPE when the upstream block dies — no traceback.
///
/// This is the direct regression test for the fix: `ignore_sigint: i > 0` in
/// `spawn_block` prevents downstream interpreters from catching SIGINT.
#[cfg(unix)]
#[test]
fn test_pipe_sigint_no_python_traceback() {
    if !helpers::tool_available("python3") {
        eprintln!("skipping test_pipe_sigint_no_python_traceback: python3 not on PATH");
        return;
    }

    let dir = helpers::creft_env();

    // Block 0: sleep 10 — a long-running process that holds the pipe open.
    // Block 1: python reads all of stdin. With old behavior it would raise
    //   KeyboardInterrupt and print a traceback when SIGINT arrived.
    //   With the fix it ignores SIGINT and exits when the pipe closes (EOF).
    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(concat!(
            "---\n",
            "name: pipe-sigint-python-norace\n",
            "description: python downstream SIGINT traceback regression\n",
            "---\n",
            "\n",
            "```bash\n",
            "sleep 10\n",
            "```\n",
            "\n",
            "```python3\n",
            "import sys\n",
            "raw = sys.stdin.read()\n",
            "print(len(raw))\n",
            "```\n",
        ))
        .assert()
        .success();

    let child = std::process::Command::new(assert_cmd::cargo::cargo_bin("creft"))
        .env("CREFT_HOME", dir.path())
        .env_remove("CREFT_PROJECT_ROOT")
        .args(["pipe-sigint-python-norace"])
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn creft");

    // Allow children to start before signalling.
    std::thread::sleep(std::time::Duration::from_millis(300));

    // Send SIGINT to creft. creft forwards it (via tcsetpgrp or kill(-pgid))
    // to the child process group. Block 0 (sleep) receives SIGINT and dies.
    // Block 1 (python) has SIG_IGN for SIGINT, so it ignores it and reads
    // EOF when the pipe from block 0 closes.
    //
    // SAFETY: kill(pid, SIGINT) is a standard POSIX call. child.id() is valid
    // for the duration of this scope.
    let child_pid = child.id();
    unsafe {
        libc::kill(child_pid as libc::pid_t, libc::SIGINT);
    }

    let output = child.wait_with_output().expect("failed to wait for creft");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("Traceback"),
        "creft stderr must not contain 'Traceback' after SIGINT in pipe chain, got: {stderr:?}"
    );
    assert!(
        !stderr.contains("KeyboardInterrupt"),
        "creft stderr must not contain 'KeyboardInterrupt' after SIGINT, got: {stderr:?}"
    );
}

// ── --verbose flag tests ────────────────────────────────────────────────────────

/// `--verbose` shows rendered blocks on stderr and still executes the skill.
#[test]
fn test_verbose_shows_rendered_blocks() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(concat!(
            "---\n",
            "name: verbose-hello\n",
            "description: greet with verbose\n",
            "args:\n",
            "  - name: who\n",
            "---\n",
            "\n",
            "```bash\n",
            "echo \"Hello, {{who}}!\"\n",
            "```\n",
        ))
        .assert()
        .success();

    let output = creft_with(&dir)
        .args(["verbose-hello", "World", "--verbose"])
        .assert()
        .success()
        .get_output()
        .clone();

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Stderr shows the rendered block with === delimiters.
    assert!(
        stderr.contains("=== block 1 (bash) ==="),
        "stderr should contain block header; got: {stderr:?}"
    );
    assert!(
        stderr.contains("=== end ==="),
        "stderr should contain block footer; got: {stderr:?}"
    );
    // The substituted value appears in the rendered block.
    assert!(
        stderr.contains("World"),
        "stderr should contain substituted arg value; got: {stderr:?}"
    );

    // Execution happened: stdout has the greeting.
    assert!(
        stdout.contains("Hello, World!"),
        "stdout should contain execution output; got: {stdout:?}"
    );
}

/// `--verbose --dry-run` shows rendered blocks on stderr and does NOT execute.
#[test]
fn test_verbose_dry_run_shows_rendered_no_execute() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(concat!(
            "---\n",
            "name: verbose-dry\n",
            "description: verbose dry-run test\n",
            "args:\n",
            "  - name: msg\n",
            "---\n",
            "\n",
            "```bash\n",
            "echo \"msg={{msg}}\"\n",
            "```\n",
        ))
        .assert()
        .success();

    let output = creft_with(&dir)
        .args(["verbose-dry", "hello", "--verbose", "--dry-run"])
        .assert()
        .success()
        .get_output()
        .clone();

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Stderr shows the rendered block.
    assert!(
        stderr.contains("=== block 1 (bash) ==="),
        "stderr should contain block header; got: {stderr:?}"
    );
    assert!(
        stderr.contains("hello"),
        "stderr should contain substituted value; got: {stderr:?}"
    );

    // Execution did NOT happen: stdout is empty.
    assert!(
        stdout.is_empty(),
        "stdout should be empty (no execution); got: {stdout:?}"
    );
}

/// Node block with `# deps: left-pad` installs the package via npm and makes
/// it available to `require()` via NODE_PATH. Verifies that module resolution
/// works end-to-end after the npx→npm-install change.
#[test]
fn test_node_deps_available_for_require() {
    if !helpers::tool_available("npm") {
        eprintln!("skipping test_node_deps_available_for_require: npm not on PATH");
        return;
    }
    if !helpers::tool_available("node") {
        eprintln!("skipping test_node_deps_available_for_require: node not on PATH");
        return;
    }

    let dir = creft_env();

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(concat!(
            "---\n",
            "name: node-leftpad\n",
            "description: test node deps via npm install\n",
            "---\n",
            "\n",
            "```node\n",
            "// deps: left-pad\n",
            "const lp = require('left-pad');\n",
            "console.log(lp('hello', 10));\n",
            "```\n",
        ))
        .assert()
        .success();

    creft_with(&dir)
        .args(["node-leftpad"])
        .assert()
        .success()
        .stdout(predicate::str::contains("     hello"));
}

/// `--verbose` with an optional arg omitted shows the empty substitution in stderr.
#[test]
fn test_verbose_without_args_shows_empty_defaults() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(concat!(
            "---\n",
            "name: verbose-optional\n",
            "description: optional arg verbose test\n",
            "args:\n",
            "  - name: thing\n",
            "    required: false\n",
            "---\n",
            "\n",
            "```bash\n",
            "echo \"thing={{thing}}\"\n",
            "```\n",
        ))
        .assert()
        .success();

    let output = creft_with(&dir)
        .args(["verbose-optional", "--verbose"])
        .assert()
        .success()
        .get_output()
        .clone();

    let stderr = String::from_utf8_lossy(&output.stderr);

    // Stderr shows the block with the empty-substituted (shell-escaped) value.
    assert!(
        stderr.contains("=== block 1 (bash) ==="),
        "stderr should contain block header; got: {stderr:?}"
    );
    // Empty string substitution produces '' in bash mode.
    assert!(
        stderr.contains("thing=''"),
        "stderr should show empty substitution as ''; got: {stderr:?}"
    );
}

// ── exit 99 early return tests ─────────────────────────────────────────────────

/// A block that exits 99 stops the pipeline and creft returns 0.
/// Blocks after the exiting block must not execute.
#[test]
fn test_early_exit_99_stops_remaining_blocks() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(concat!(
            "---\n",
            "name: early-exit-stop\n",
            "description: exit 99 stops the pipeline\n",
            "---\n",
            "\n",
            "```bash\n",
            "exit 99\n",
            "```\n",
            "\n",
            "```bash\n",
            "echo 'should not run'\n",
            "```\n",
        ))
        .assert()
        .success();

    creft_with(&dir)
        .args(["early-exit-stop"])
        .assert()
        .success()
        .stdout(predicate::str::contains("should not run").not());
}

/// A block that echoes output and then exits 99 preserves that output.
#[test]
fn test_early_exit_99_preserves_output() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(concat!(
            "---\n",
            "name: early-exit-output\n",
            "description: exit 99 preserves block output\n",
            "---\n",
            "\n",
            "```bash\n",
            "echo hello\n",
            "exit 99\n",
            "```\n",
        ))
        .assert()
        .success();

    creft_with(&dir)
        .args(["early-exit-output"])
        .assert()
        .success()
        .stdout(predicate::str::contains("hello"));
}

/// A block that exits 1 still causes creft to fail.
#[test]
fn test_normal_exit_1_still_fails() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(concat!(
            "---\n",
            "name: exit-one-fails\n",
            "description: exit 1 is an error\n",
            "---\n",
            "\n",
            "```bash\n",
            "exit 1\n",
            "```\n",
        ))
        .assert()
        .success();

    creft_with(&dir).args(["exit-one-fails"]).assert().failure();
}

/// In pipe mode, a block that exits 99 causes the pipeline to return 0.
#[test]
fn test_early_exit_99_in_pipe_mode() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(concat!(
            "---\n",
            "name: pipe-early-exit\n",
            "description: exit 99 in pipe mode\n",
            "---\n",
            "\n",
            "```bash\n",
            "exit 99\n",
            "```\n",
            "\n",
            "```bash\n",
            "cat\n",
            "```\n",
        ))
        .assert()
        .success();

    creft_with(&dir)
        .args(["pipe-early-exit"])
        .assert()
        .success();
}

/// Verify that output from a fast downstream block is suppressed when an
/// upstream block exits 99. This is the exact reproduction case from the v2
/// spec: block 0 exits immediately with 99, block 1 (fast bash echo) must NOT
/// produce output on creft's stdout.
///
/// This test is deterministic because the buffered relay never writes to the
/// terminal. Output is only flushed after all reapers have reported and
/// early_exit is confirmed false. Since block 0 exits 99, early_exit is true
/// and the buffer is dropped without writing.
#[test]
fn test_pipe_exit_99_fast_downstream_no_output() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(concat!(
            "---\n",
            "name: pipe-fast-exit99\n",
            "description: fast downstream must not leak output on exit 99\n",
            "---\n",
            "\n",
            "```bash\n",
            "exit 99\n",
            "```\n",
            "\n",
            "```bash\n",
            "echo \"LEAKED\"\n",
            "```\n",
        ))
        .assert()
        .success();

    let start = std::time::Instant::now();
    let output = creft_with(&dir)
        .args(["pipe-fast-exit99"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let elapsed = start.elapsed();

    let stdout_str = String::from_utf8_lossy(&output);
    assert!(
        !stdout_str.contains("LEAKED"),
        "fast downstream output must not appear after exit 99; got: {:?}",
        stdout_str
    );
    assert!(
        elapsed.as_secs() < 2,
        "pipe must complete quickly after exit 99 (took {:?})",
        elapsed
    );
}

/// Verify that downstream blocks with stdin-dependent side effects are killed
/// before those side effects occur when an upstream block exits 99.
///
/// Block 1 reads from stdin, then sleeps 500ms before touching the sentinel.
/// The sleep gives the reaper-side kill chain (microseconds of latency) ample
/// time to deliver SIGKILL before the side effect completes. The sentinel file
/// must never be created.
#[test]
fn test_pipe_exit_99_no_side_effects() {
    let dir = creft_env();
    let sentinel_dir = tempfile::TempDir::new().unwrap();
    let sentinel_path = sentinel_dir.path().join("sentinel");

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(concat!(
            "---\n",
            "name: pipe-exit99-sentinel\n",
            "description: exit 99 kills downstream before side effects\n",
            "---\n",
            "\n",
            "```bash\n",
            "exit 99\n",
            "```\n",
            "\n",
            "```bash\n",
            "read line; sleep 0.5; touch \"$CREFT_SENTINEL\"\n",
            "```\n",
        ))
        .assert()
        .success();

    let start = std::time::Instant::now();
    creft_with(&dir)
        .args(["pipe-exit99-sentinel"])
        .env("CREFT_SENTINEL", sentinel_path.display().to_string())
        .assert()
        .success();
    let elapsed = start.elapsed();

    assert!(
        !sentinel_path.exists(),
        "sentinel file must not be created when exit 99 kills downstream block"
    );
    assert!(
        elapsed.as_secs() < 2,
        "pipe must complete quickly after exit 99 (took {:?})",
        elapsed
    );
}

/// Verify that when a middle block in a 3-block pipe chain exits 99, the last
/// block's output is suppressed. Block 2 sleeps before echoing to ensure the
/// buffered relay + discard path is exercised rather than a timing coincidence.
#[test]
fn test_pipe_exit_99_middle_block_output_suppressed() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(concat!(
            "---\n",
            "name: pipe-mid-exit99-suppress\n",
            "description: middle exit 99 suppresses last block output\n",
            "---\n",
            "\n",
            "```bash\n",
            "echo \"data\"\n",
            "```\n",
            "\n",
            "```bash\n",
            "cat; exit 99\n",
            "```\n",
            "\n",
            "```bash\n",
            "sleep 1; echo \"LEAKED\"; cat\n",
            "```\n",
        ))
        .assert()
        .success();

    let start = std::time::Instant::now();
    let output = creft_with(&dir)
        .args(["pipe-mid-exit99-suppress"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let elapsed = start.elapsed();

    let stdout_str = String::from_utf8_lossy(&output);
    assert!(
        !stdout_str.contains("LEAKED"),
        "last block output must not appear after middle block exits 99; got: {:?}",
        stdout_str
    );
    assert!(
        elapsed.as_secs() < 2,
        "pipe must terminate quickly after middle block exits 99 (took {:?})",
        elapsed
    );
}

// ── pipe-by-default: legacy field tests ──────────────────────────────────────

/// A skill with `pipe: true` in YAML still works — the field is ignored by
/// serde (no deny_unknown_fields), and multi-block skills always pipe.
#[test]
fn test_legacy_pipe_true_ignored() {
    let dir = creft_env();

    let markdown = concat!(
        "---\n",
        "name: legacy-pipe-true\n",
        "description: legacy field is silently ignored\n",
        "pipe: true\n",
        "---\n",
        "\n",
        "```bash\n",
        "echo hello\n",
        "```\n",
        "\n",
        "```bash\n",
        "stdin=$(cat)\n",
        "echo \"got: $stdin\"\n",
        "```\n",
    );

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(markdown)
        .assert()
        .success();

    creft_with(&dir)
        .args(["legacy-pipe-true"])
        .assert()
        .success()
        .stdout(predicate::str::contains("got: hello"));
}

/// A skill with `sequential: true` in YAML pipes its blocks — the field is
/// ignored by serde (no deny_unknown_fields), and multi-block skills always pipe.
#[test]
fn test_legacy_sequential_true_ignored() {
    let dir = creft_env();

    let markdown = concat!(
        "---\n",
        "name: legacy-sequential-true\n",
        "description: legacy sequential field is silently ignored\n",
        "sequential: true\n",
        "---\n",
        "\n",
        "```bash\n",
        "echo piped\n",
        "```\n",
        "\n",
        "```bash\n",
        "stdin=$(cat)\n",
        "echo \"got: $stdin\"\n",
        "```\n",
    );

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(markdown)
        .assert()
        .success();

    creft_with(&dir)
        .args(["legacy-sequential-true"])
        .assert()
        .success()
        .stdout(predicate::str::contains("got: piped"));
}

// ── exit-99 relay flush regression tests ─────────────────────────────────────

/// When the LAST block in a pipe chain exits 99, its stdout must appear on the
/// terminal. The relay buffer contains valid output from the exit-99 block and
/// must be flushed, not discarded.
///
/// Regression guard for the stdout-swallowing bug: exit-99 output from the
/// last block was previously discarded unconditionally.
#[test]
fn test_pipe_exit_99_last_block_stdout_preserved() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(concat!(
            "---\n",
            "name: exit99-last-stdout\n",
            "description: last block exit 99 preserves stdout\n",
            "---\n",
            "\n",
            "```bash\n",
            "echo upstream\n",
            "```\n",
            "\n",
            "```bash\n",
            "cat >/dev/null\n",
            "echo final-output\n",
            "exit 99\n",
            "```\n",
        ))
        .assert()
        .success();

    let start = std::time::Instant::now();
    creft_with(&dir)
        .args(["exit99-last-stdout"])
        .assert()
        .success()
        .stdout(predicate::str::contains("final-output"));
    let elapsed = start.elapsed();

    assert!(
        elapsed.as_secs() < 5,
        "pipe must terminate quickly (took {:?})",
        elapsed
    );
}

/// When the last block exits 99 after printing multiple lines, all lines must
/// appear on stdout.
///
/// Regression guard: multi-line output from an exit-99 last block must not be
/// truncated or dropped.
#[test]
fn test_pipe_exit_99_last_block_multiline_stdout() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(concat!(
            "---\n",
            "name: exit99-last-multiline\n",
            "description: last block exit 99 preserves all lines\n",
            "---\n",
            "\n",
            "```bash\n",
            "echo input\n",
            "```\n",
            "\n",
            "```bash\n",
            "cat >/dev/null\n",
            "echo line1\n",
            "echo line2\n",
            "echo line3\n",
            "exit 99\n",
            "```\n",
        ))
        .assert()
        .success();

    creft_with(&dir)
        .args(["exit99-last-multiline"])
        .assert()
        .success()
        .stdout(predicate::str::contains("line1"))
        .stdout(predicate::str::contains("line2"))
        .stdout(predicate::str::contains("line3"));
}

// ── middle-block exit-99 stdout capture tests ─────────────────────────────────

/// When a middle block in a 3-block pipe chain exits 99 after writing to
/// stdout, its output must appear on creft's stdout.
///
/// Regression guard: before this fix, the middle block's output was in the
/// inter-block pipe buffer but was silently discarded when the downstream
/// block was killed.
#[test]
#[cfg(unix)]
fn test_pipe_exit_99_middle_block_stdout_captured() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(concat!(
            "---\n",
            "name: exit99-mid-capture\n",
            "description: middle block exit 99 stdout is captured\n",
            "---\n",
            "\n",
            "```bash\n",
            "echo \"input\"\n",
            "```\n",
            "\n",
            "```bash\n",
            "cat; echo \"middle-output\"; exit 99\n",
            "```\n",
            "\n",
            "```bash\n",
            "sleep 5; cat\n",
            "```\n",
        ))
        .assert()
        .success();

    creft_with(&dir)
        .args(["exit99-mid-capture"])
        .assert()
        .success()
        .stdout(predicate::str::contains("middle-output"));
}

/// When the first block in a multi-block chain exits 99, its stdout must
/// appear on creft's stdout.
///
/// Block 0 is treated as a "middle" block from the output-capture perspective:
/// it's not the last block, so its output is in an inter-block pipe and must
/// be recovered via the dup'd fd.
#[test]
#[cfg(unix)]
fn test_pipe_exit_99_first_block_stdout_captured() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(concat!(
            "---\n",
            "name: exit99-first-capture\n",
            "description: first block exit 99 stdout is captured\n",
            "---\n",
            "\n",
            "```bash\n",
            "echo \"first-output\"; exit 99\n",
            "```\n",
            "\n",
            "```bash\n",
            "sleep 5; cat\n",
            "```\n",
            "\n",
            "```bash\n",
            "sleep 5; cat\n",
            "```\n",
        ))
        .assert()
        .success();

    creft_with(&dir)
        .args(["exit99-first-capture"])
        .assert()
        .success()
        .stdout(predicate::str::contains("first-output"));
}

/// When a middle block exits 99 after printing multiple lines, all lines must
/// appear on stdout.
///
/// Regression guard: multi-line output from a middle exit-99 block must not
/// be truncated or partially captured.
#[test]
#[cfg(unix)]
fn test_pipe_exit_99_middle_block_multiline_stdout() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(concat!(
            "---\n",
            "name: exit99-mid-multiline\n",
            "description: middle block exit 99 captures all lines\n",
            "---\n",
            "\n",
            "```bash\n",
            "echo \"input\"\n",
            "```\n",
            "\n",
            "```bash\n",
            "cat >/dev/null; echo \"line1\"; echo \"line2\"; echo \"line3\"; exit 99\n",
            "```\n",
            "\n",
            "```bash\n",
            "sleep 5; cat\n",
            "```\n",
        ))
        .assert()
        .success();

    creft_with(&dir)
        .args(["exit99-mid-multiline"])
        .assert()
        .success()
        .stdout(predicate::str::contains("line1"))
        .stdout(predicate::str::contains("line2"))
        .stdout(predicate::str::contains("line3"));
}

/// When a middle block exits 99 without writing anything to stdout, creft
/// must return 0 with no output and no crash.
///
/// Regression guard: the drain path must handle the empty-output case without
/// writing spurious content or panicking.
#[test]
#[cfg(unix)]
fn test_pipe_exit_99_middle_block_no_stdout() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(concat!(
            "---\n",
            "name: exit99-mid-no-stdout\n",
            "description: middle block exit 99 with no stdout\n",
            "---\n",
            "\n",
            "```bash\n",
            "echo \"input\"\n",
            "```\n",
            "\n",
            "```bash\n",
            "cat >/dev/null; exit 99\n",
            "```\n",
            "\n",
            "```bash\n",
            "sleep 5; cat\n",
            "```\n",
        ))
        .assert()
        .success();

    let output = creft_with(&dir)
        .args(["exit99-mid-no-stdout"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    assert!(
        output.is_empty(),
        "stdout must be empty when middle block exits 99 with no output; got: {:?}",
        String::from_utf8_lossy(&output)
    );
}

// ── sequential SIGINT stderr suppression ──────────────────────────────────────

/// When SIGINT is sent to creft while running a sequential Python block, creft
/// must not dump the interpreter's KeyboardInterrupt traceback to stderr.
///
/// This validates that `execute_block` suppresses child stderr when the child
/// was killed by signal 2 (SIGINT), regardless of what the interpreter printed.
#[test]
#[cfg(unix)]
fn test_sequential_sigint_suppresses_python_traceback() {
    if !helpers::tool_available("python3") {
        eprintln!(
            "skipping test_sequential_sigint_suppresses_python_traceback: python3 not on PATH"
        );
        return;
    }

    let dir = creft_env();

    add_skill_to_dir(
        &dir,
        concat!(
            "---\n",
            "name: seq-sigint-python\n",
            "description: sequential Python SIGINT traceback suppression\n",
            "---\n",
            "\n",
            "```python3\n",
            "import time\n",
            "time.sleep(10)\n",
            "```\n",
        ),
    );

    // Spawn creft in its own process group (process_group(0)) so the SIGINT we
    // deliver to that group reaches creft and its sequential child without
    // propagating to the nextest process group.
    #[allow(unused_mut)]
    let mut cmd = std::process::Command::new(assert_cmd::cargo::cargo_bin("creft"));
    cmd.env("CREFT_HOME", dir.path())
        .env_remove("CREFT_PROJECT_ROOT")
        .args(["seq-sigint-python"])
        .stderr(std::process::Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let child = cmd.spawn().expect("failed to spawn creft");

    // Python needs slightly longer to start than bash.
    std::thread::sleep(std::time::Duration::from_millis(500));

    // Send SIGINT to creft's isolated process group. The child (python) inherits
    // the group and also receives the signal, producing a KeyboardInterrupt that
    // creft must suppress.
    //
    // SAFETY: kill(-pgid, SIGINT) is a standard POSIX call. child.id() is
    // creft's PID and also the process group ID (set via process_group(0)).
    // child.id() is valid for the duration of this scope.
    unsafe {
        libc::kill(-(child.id() as libc::pid_t), libc::SIGINT);
    }

    let output = child.wait_with_output().expect("failed to wait for creft");

    let code = output.status.code().unwrap_or_else(|| {
        use std::os::unix::process::ExitStatusExt;
        128 + output.status.signal().unwrap_or(2)
    });
    assert_eq!(
        code, 130,
        "creft should exit with code 130 after SIGINT, got {code}"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("Traceback"),
        "creft stderr must not contain 'Traceback' after sequential SIGINT; got: {stderr:?}"
    );
    assert!(
        !stderr.contains("KeyboardInterrupt"),
        "creft stderr must not contain 'KeyboardInterrupt' after sequential SIGINT; got: {stderr:?}"
    );
}

/// When SIGINT is sent to creft while running a sequential bash block, creft
/// must produce no error output on stderr.
#[test]
#[cfg(unix)]
fn test_sequential_sigint_suppresses_bash_stderr() {
    let dir = creft_env();

    add_skill_to_dir(
        &dir,
        concat!(
            "---\n",
            "name: seq-sigint-bash\n",
            "description: sequential bash SIGINT stderr suppression\n",
            "---\n",
            "\n",
            "```bash\n",
            "sleep 10\n",
            "```\n",
        ),
    );

    // Spawn creft in its own process group so SIGINT targets only this group
    // and does not propagate to the nextest runner.
    #[allow(unused_mut)]
    let mut cmd = std::process::Command::new(assert_cmd::cargo::cargo_bin("creft"));
    cmd.env("CREFT_HOME", dir.path())
        .env_remove("CREFT_PROJECT_ROOT")
        .args(["seq-sigint-bash"])
        .stderr(std::process::Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let child = cmd.spawn().expect("failed to spawn creft");

    std::thread::sleep(std::time::Duration::from_millis(300));

    // Send SIGINT to creft's isolated process group. The bash child inherits
    // the group and also receives the signal.
    //
    // SAFETY: kill(-pgid, SIGINT) is a standard POSIX call. child.id() is
    // creft's PID and also the process group ID (set via process_group(0)).
    // child.id() is valid for the duration of this scope.
    unsafe {
        libc::kill(-(child.id() as libc::pid_t), libc::SIGINT);
    }

    let output = child.wait_with_output().expect("failed to wait for creft");

    let code = output.status.code().unwrap_or_else(|| {
        use std::os::unix::process::ExitStatusExt;
        128 + output.status.signal().unwrap_or(2)
    });
    assert_eq!(
        code, 130,
        "creft should exit with code 130 after SIGINT, got {code}"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.is_empty(),
        "creft stderr must be empty after sequential bash SIGINT; got: {stderr:?}"
    );
}

/// When a sequential Python block exits with a non-zero code (not signal-killed),
/// its stderr is still written to the terminal — only SIGINT-killed stderr is
/// suppressed.
#[test]
#[cfg(unix)]
fn test_sequential_normal_failure_stderr_preserved() {
    if !helpers::tool_available("python3") {
        eprintln!("skipping test_sequential_normal_failure_stderr_preserved: python3 not on PATH");
        return;
    }

    let dir = creft_env();

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(concat!(
            "---\n",
            "name: seq-fail-stderr\n",
            "description: sequential failure preserves stderr\n",
            "---\n",
            "\n",
            "```python3\n",
            "import sys\n",
            "print('diagnostic error', file=sys.stderr)\n",
            "sys.exit(1)\n",
            "```\n",
        ))
        .assert()
        .success();

    creft_with(&dir)
        .args(["seq-fail-stderr"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("diagnostic error"));
}

/// When a bash block is killed by SIGTERM, creft must NOT suppress its stderr.
///
/// This is the negative case for the SIGINT suppression: only signal 2 (SIGINT)
/// triggers suppression. Signal 15 (SIGTERM) represents a genuine process
/// failure — any stderr the child produced is diagnostic information and must
/// be preserved.
///
/// The bash block sends SIGTERM to itself so that creft stays alive and can
/// collect and forward the child's stderr, proving the suppression logic does
/// not fire for non-SIGINT signals.
#[test]
#[cfg(unix)]
fn test_sequential_sigterm_preserves_stderr() {
    let dir = creft_env();

    // The bash block writes a sentinel to stderr then kills itself with SIGTERM.
    // creft stays alive, collects the child's captured stderr, and must write it
    // because suppress_stderr is false for SIGTERM.
    add_skill_to_dir(
        &dir,
        concat!(
            "---\n",
            "name: seq-sigterm-bash\n",
            "description: bash SIGTERM stderr preservation\n",
            "---\n",
            "\n",
            "```bash\n",
            "echo 'sigterm-sentinel' >&2\n",
            "kill -TERM $$\n",
            "```\n",
        ),
    );

    creft_with(&dir)
        .args(["seq-sigterm-bash"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("sigterm-sentinel"));
}

// ── env var injection tests ───────────────────────────────────────────────────

/// A flag is accessible as an environment variable ($FORMAT) in bash blocks.
#[test]
fn test_flag_injected_as_env_var() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(concat!(
            "---\n",
            "name: echo-format\n",
            "description: echo the format flag\n",
            "flags:\n",
            "  - name: format\n",
            "    description: output format\n",
            "    type: string\n",
            "---\n",
            "\n",
            "```bash\n",
            "echo \"$FORMAT\"\n",
            "```\n",
        ))
        .assert()
        .success();

    creft_with(&dir)
        .args(["echo-format", "--format", "json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("json"));
}

/// A positional arg is accessible as an environment variable ($TARGET) in bash blocks.
#[test]
fn test_arg_injected_as_env_var() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(concat!(
            "---\n",
            "name: echo-target\n",
            "description: echo the target arg\n",
            "args:\n",
            "  - name: target\n",
            "    description: deployment target\n",
            "---\n",
            "\n",
            "```bash\n",
            "echo \"$TARGET\"\n",
            "```\n",
        ))
        .assert()
        .success();

    creft_with(&dir)
        .args(["echo-target", "production"])
        .assert()
        .success()
        .stdout(predicate::str::contains("production"));
}

/// A hyphenated flag name is accessible as an env var with underscores ($ALWAYS_CONFIRM).
#[test]
fn test_hyphenated_flag_injected_with_underscores() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(concat!(
            "---\n",
            "name: echo-confirm\n",
            "description: echo the always-confirm flag\n",
            "flags:\n",
            "  - name: always-confirm\n",
            "    description: skip confirmation prompts\n",
            "    type: bool\n",
            "---\n",
            "\n",
            "```bash\n",
            "echo \"$ALWAYS_CONFIRM\"\n",
            "```\n",
        ))
        .assert()
        .success();

    creft_with(&dir)
        .args(["echo-confirm", "--always-confirm"])
        .assert()
        .success()
        .stdout(predicate::str::contains("true"));
}

/// Template substitution ({{format}}) and env var access ($FORMAT) both work in the same block.
#[test]
fn test_template_and_env_var_both_resolve() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(concat!(
            "---\n",
            "name: dual-access\n",
            "description: access flag via template and env var\n",
            "flags:\n",
            "  - name: format\n",
            "    description: output format\n",
            "    type: string\n",
            "---\n",
            "\n",
            "```bash\n",
            "echo \"template={{format}} env=$FORMAT\"\n",
            "```\n",
        ))
        .assert()
        .success();

    creft_with(&dir)
        .args(["dual-access", "--format", "yaml"])
        .assert()
        .success()
        .stdout(predicate::str::contains("template=yaml env=yaml"));
}

/// In a multi-block pipe chain, all blocks see the injected env vars.
#[test]
fn test_pipe_chain_blocks_see_env_vars() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(concat!(
            "---\n",
            "name: pipe-env-check\n",
            "description: verify env vars reach all pipe blocks\n",
            "flags:\n",
            "  - name: tag\n",
            "    description: a tag value\n",
            "    type: string\n",
            "---\n",
            "\n",
            "```bash\n",
            "echo \"block1:$TAG\"\n",
            "```\n",
            "\n",
            "```bash\n",
            "cat -\n",
            "echo \"block2:$TAG\"\n",
            "```\n",
        ))
        .assert()
        .success();

    creft_with(&dir)
        .args(["pipe-env-check", "--tag", "v1"])
        .assert()
        .success()
        .stdout(predicate::str::contains("block1:v1"))
        .stdout(predicate::str::contains("block2:v1"));
}
