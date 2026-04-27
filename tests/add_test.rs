//! Integration tests for `creft add test`.

mod helpers;

use assert_cmd::Command;
use helpers::TwoScopeEnv;
use predicates::prelude::*;

// ── Environment helpers ───────────────────────────────────────────────────────

/// Minimal skill markdown — used to create a skill `.md` file that the
/// existence check in `cmd_add_test` will find.
const SKILL_MD: &str = "---\nname: setup\ndescription: test skill\n---\n```bash\necho hi\n```\n";

/// Minimal valid scenario body (YAML that satisfies the fixture schema).
const MINIMAL_BODY: &str = "when:\n  argv: [sh, -c, exit 0]\nthen:\n  exit_code: 0\n";

/// Build a stdin envelope — the frontmatter + body format `creft add test` expects.
fn envelope(skill: &str, name: &str, body: &str) -> String {
    format!("---\nskill: {skill}\nname: {name}\n---\n{body}")
}

/// Create a `creft` command bound to the given two-scope environment.
fn creft_ts(env: &TwoScopeEnv) -> Command {
    let mut cmd = Command::cargo_bin("creft").unwrap();
    cmd.env("HOME", env.home_dir.path())
        .current_dir(env.project_dir.path());
    cmd.env_remove("CREFT_HOME");
    cmd
}

/// Write a skill `.md` file into the local commands directory so the skill
/// "exists" for the skill-existence check in `cmd_add_test`.
fn write_skill(env: &TwoScopeEnv, skill_name: &str) {
    let parts: Vec<&str> = skill_name.split_whitespace().collect();
    let mut path = env.local_commands();
    for part in &parts[..parts.len().saturating_sub(1)] {
        path = path.join(part);
    }
    std::fs::create_dir_all(&path).expect("create skill dirs");
    let leaf = parts.last().unwrap();
    std::fs::write(path.join(format!("{leaf}.md")), SKILL_MD).expect("write skill md");
}

/// Write a fixture file directly.
fn write_fixture(env: &TwoScopeEnv, skill_name: &str, content: &str) {
    let path = env.local_commands().join(format!("{skill_name}.test.yaml"));
    std::fs::write(&path, content).expect("write fixture");
}

// ── TTY / no-stdin test ───────────────────────────────────────────────────────

/// `creft add test` without piped stdin fails with an error. assert_cmd
/// connects a closed pipe by default (not a TTY), so this exercises the
/// no-input error path: empty input fails frontmatter parsing rather than the
/// IsTerminal branch, which is only reachable from a real terminal.
///
/// The invariant under test: invoking `creft add test` interactively (no
/// piped input) must never silently succeed or hang — it must produce an
/// error and exit non-zero.
#[test]
fn cmd_add_test_tty_stdin_rejected() {
    let env = TwoScopeEnv::new();
    creft_ts(&env)
        .args(["add", "test"])
        // No .write_stdin() — stdin is a closed empty pipe, not a TTY.
        .assert()
        .failure()
        .stderr(predicate::str::is_empty().not());
}

// ── Success message tests ─────────────────────────────────────────────────────

/// New fixture created: success message begins with "created:" and "added test:".
#[test]
fn add_test_new_fixture_emits_created_message() {
    let env = TwoScopeEnv::new();
    write_skill(&env, "setup");

    creft_ts(&env)
        .args(["add", "test"])
        .write_stdin(envelope("setup", "first scenario", MINIMAL_BODY))
        .assert()
        .success()
        .stderr(predicate::str::contains("created:"))
        .stderr(predicate::str::contains("added test:"));
}

/// Append to existing fixture: success message contains "added test:".
#[test]
fn add_test_append_emits_added_message() {
    let env = TwoScopeEnv::new();
    write_skill(&env, "setup");
    write_fixture(
        &env,
        "setup",
        "- name: existing\n  when:\n    argv: [sh, -c, exit 0]\n  then:\n    exit_code: 0\n",
    );

    creft_ts(&env)
        .args(["add", "test"])
        .write_stdin(envelope("setup", "second", MINIMAL_BODY))
        .assert()
        .success()
        .stderr(predicate::str::contains("added test:"))
        .stderr(predicate::str::contains("setup"))
        .stderr(predicate::str::contains("second"));
}

/// --force without collision: stderr contains both the warning and the "added test:" annotation.
#[test]
fn add_test_force_no_collision_emits_warning_and_added_annotation() {
    let env = TwoScopeEnv::new();
    write_skill(&env, "setup");
    write_fixture(
        &env,
        "setup",
        "- name: foo\n  when:\n    argv: [sh, -c, exit 0]\n  then:\n    exit_code: 0\n",
    );

    creft_ts(&env)
        .args(["add", "test", "--force"])
        .write_stdin(envelope("setup", "bar", MINIMAL_BODY))
        .assert()
        .success()
        // Warning is mandatory per spec — visible in CI logs.
        .stderr(predicate::str::contains(
            "--force given but no scenario named 'bar' exists",
        ))
        // Success line reports "added", not "replaced".
        .stderr(predicate::str::contains("added test:"))
        // The annotation distinguishes the no-collision --force path from a plain append.
        .stderr(predicate::str::contains(
            "--force matched no existing scenario; appended as new",
        ));
}

/// --force with collision: stderr contains "replaced test:" and the comment-loss note.
#[test]
fn add_test_force_with_collision_emits_replaced_message_and_comment_loss_note() {
    let env = TwoScopeEnv::new();
    write_skill(&env, "setup");
    write_fixture(
        &env,
        "setup",
        "- name: foo\n  when:\n    argv: [sh, -c, exit 0]\n  then:\n    exit_code: 0\n",
    );

    let replacement_body = "when:\n  argv: [sh, -c, exit 1]\nthen:\n  exit_code: 1\n";
    creft_ts(&env)
        .args(["add", "test", "--force"])
        .write_stdin(envelope("setup", "foo", replacement_body))
        .assert()
        .success()
        .stderr(predicate::str::contains("replaced test:"))
        // The comment-loss note is the contract with the user; they opted in by passing --force.
        .stderr(predicate::str::contains(
            "YAML comments may not be preserved",
        ));
}

// ── Error path tests ──────────────────────────────────────────────────────────

/// Collision without --force returns a non-zero exit and names the scenario.
#[test]
fn add_test_collision_without_force_fails() {
    let env = TwoScopeEnv::new();
    write_skill(&env, "setup");
    write_fixture(
        &env,
        "setup",
        "- name: foo\n  when:\n    argv: [sh, -c, exit 0]\n  then:\n    exit_code: 0\n",
    );

    creft_ts(&env)
        .args(["add", "test"])
        .write_stdin(envelope("setup", "foo", MINIMAL_BODY))
        .assert()
        .failure()
        .stderr(predicate::str::contains("foo"))
        .stderr(predicate::str::contains("already exists"));
}

/// Skill not found: non-zero exit and error names the skill.
#[test]
fn add_test_missing_skill_fails() {
    let env = TwoScopeEnv::new();
    // No skill written — "nonexistent" does not exist.
    creft_ts(&env)
        .args(["add", "test"])
        .write_stdin(envelope("nonexistent", "scenario", MINIMAL_BODY))
        .assert()
        .failure()
        .stderr(predicate::str::contains("nonexistent").or(predicate::str::contains("not found")));
}
