//! Integration tests for `creft skills test --filter` output-visible behaviour.
//!
//! Tests that need to inspect stdout (--where listings, zero-scenarios
//! messages) run `creft` as a subprocess via assert_cmd so the output is
//! fully captured. Unit-level filter tests that only need pass/fail live in
//! `src/cmd/skills.rs`.

mod helpers;

use assert_cmd::Command;
use helpers::TwoScopeEnv;
use predicates::prelude::*;

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Create a `creft` command bound to the given two-scope environment.
fn creft_ts(env: &TwoScopeEnv) -> Command {
    let mut cmd = Command::cargo_bin("creft").unwrap();
    cmd.env("HOME", env.home_dir.path())
        .current_dir(env.project_dir.path());
    cmd.env_remove("CREFT_HOME");
    cmd
}

/// Write a fixture file into the project's `.creft/commands/` directory.
fn write_fixture(env: &TwoScopeEnv, skill_name: &str, content: &str) {
    let path = env.local_commands().join(format!("{skill_name}.test.yaml"));
    std::fs::write(&path, content).expect("write fixture");
}

/// A fixture with three scenarios: one that would fail if run, and two
/// merge-prefixed scenarios that pass.
const THREE_SCENARIO_FIXTURE: &str = r#"
- name: fresh-install
  when:
    argv: ["sh", "-c", "exit 1"]
  then:
    exit_code: 0
- name: merge-clean
  when:
    argv: ["sh", "-c", "exit 0"]
  then:
    exit_code: 0
- name: merge-conflict
  when:
    argv: ["sh", "-c", "exit 0"]
  then:
    exit_code: 0
"#;

// ── --where listing reflects the filter ──────────────────────────────────────

/// `--where` listing with `--filter "merge*"` shows only the two merge-prefixed
/// scenarios and excludes fresh-install.
///
/// This test verifies the integration seam: the scenario filter is applied
/// before the --where listing, so the listing reflects only matching names.
#[cfg(unix)]
#[test]
fn cmd_skills_test_filter_under_where_lists_only_matching() {
    let env = TwoScopeEnv::new();
    write_fixture(&env, "setup", THREE_SCENARIO_FIXTURE);

    creft_ts(&env)
        .args(["skills", "test", "setup", "--filter", "merge*", "--where"])
        .assert()
        .success()
        .stdout(predicate::str::contains("merge-clean"))
        .stdout(predicate::str::contains("merge-conflict"))
        .stdout(predicate::str::contains("fresh-install").not());
}

// ── Zero-scenarios message ────────────────────────────────────────────────────

/// When the filter matches no scenarios in any discovered fixture, the output
/// must contain the "0 scenarios: filter matched no scenarios" message and
/// exit 0.
///
/// This verifies that the empty-filter path emits the expected diagnostic
/// message and does not exit non-zero (no scenarios to fail).
#[cfg(unix)]
#[test]
fn cmd_skills_test_filter_no_match_prints_zero_scenarios() {
    let env = TwoScopeEnv::new();
    write_fixture(
        &env,
        "setup",
        r#"
- name: alpha
  when:
    argv: ["sh", "-c", "exit 0"]
  then:
    exit_code: 0
"#,
    );

    creft_ts(&env)
        .args(["skills", "test", "--filter", "nonexistent"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "0 scenarios: filter matched no scenarios",
        ));
}
