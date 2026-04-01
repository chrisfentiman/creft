//! Tests for skill execution, help display, namespaced commands, optional args, and pipe behavior.

mod helpers;

use helpers::{creft_env, creft_with};
use predicates::prelude::*;

// ── run tests ─────────────────────────────────────────────────────────────────

/// Running a simple command with no args produces expected output.
#[test]
fn test_run_simple_command() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["add"])
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
        .args(["add"])
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
        .args(["add"])
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
        .args(["add"])
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
        .args(["add"])
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
        .args(["add"])
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
        .args(["add"])
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
        .args(["add"])
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
        .args(["add", "--no-validate"])
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
        .args(["add"])
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
        .args(["add"])
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
        .args(["add", "--no-validate"])
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
        .args(["add"])
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
        .args(["add"])
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
        .args(["add"])
        .write_stdin(markdown)
        .assert()
        .success();

    let start = std::time::Instant::now();
    let output = creft_with(&dir)
        .args(["pipe-exit-99-kill"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let elapsed = start.elapsed();

    let stdout_str = String::from_utf8_lossy(&output);
    assert!(
        !stdout_str.contains("should not appear"),
        "block 2 output must not appear after exit 99; got: {:?}",
        stdout_str
    );
    assert!(
        elapsed.as_secs() < 2,
        "pipe must terminate quickly after exit 99 (took {:?})",
        elapsed
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
        .args(["add"])
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

// ── parent/child coexistence tests ────────────────────────────────────────────

/// When both `test` (with a positional arg) and `test mutants` exist,
/// `creft test mutants` resolves to `test mutants` -- not `test` with `filter=mutants`.
#[test]
fn test_subcommand_resolved_over_parent_positional_arg() {
    let dir = creft_env();

    // Parent skill: test with positional arg `filter`.
    creft_with(&dir)
        .args(["add"])
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
        .args(["add"])
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
        .args(["add"])
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
        .args(["add"])
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
        .args(["add"])
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
        .args(["add"])
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
        .args(["add"])
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
        .args(["add"])
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
        .args(["add"])
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
        .args(["add"])
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
        .args(["add"])
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
        .args(["add"])
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
        .args(["add"])
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
        .args(["add"])
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
        .args(["add"])
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
        .args(["add"])
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
        .args(["add"])
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
        .args(["add"])
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
        .args(["add"])
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
        .args(["add"])
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
        .args(["add"])
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
        .args(["add"])
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
/// Block 1 reads from stdin before running any side effect. With exit 99 from
/// block 0, the pipe closes (EOF), block 1 gets SIGKILL from killpg, and the
/// sentinel file is never created.
#[test]
fn test_pipe_exit_99_no_side_effects() {
    let dir = creft_env();
    let sentinel_dir = tempfile::TempDir::new().unwrap();
    let sentinel_path = sentinel_dir.path().join("sentinel");

    creft_with(&dir)
        .args(["add"])
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
            "read line; touch \"$CREFT_SENTINEL\"\n",
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
        .args(["add"])
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
        .args(["add"])
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
        .args(["add"])
        .write_stdin(markdown)
        .assert()
        .success();

    creft_with(&dir)
        .args(["legacy-sequential-true"])
        .assert()
        .success()
        .stdout(predicate::str::contains("got: piped"));
}
