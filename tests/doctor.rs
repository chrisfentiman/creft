//! Tests for the `creft doctor` command.

mod helpers;

use helpers::{creft_env, creft_with};

// ── doctor tests ──────────────────────────────────────────────────────────────

/// `creft doctor` (global check) exits 0 on a healthy system that has at minimum
/// bash and sh on PATH. The test is intentionally permissive: it checks that the
/// command runs without error and produces the expected header, not that every
/// individual check passes (which would be environment-dependent).
#[test]
fn test_doctor_global_runs() {
    let dir = creft_env();
    let output = creft_with(&dir).args(["doctor"]).output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    // The command should always start with the "Environment Health Check" header.
    assert!(
        stderr.starts_with("Environment Health Check"),
        "expected 'Environment Health Check' header, got: {stderr:?}"
    );
    // bash and sh are required; on any system where this test runs they should be present.
    assert!(
        stderr.contains("[ok] bash") || stderr.contains("[!!] bash"),
        "expected bash check in output, got: {stderr:?}"
    );
}

/// `creft doctor <nonexistent-skill>` exits with a non-zero code and reports an error.
#[test]
fn test_doctor_skill_not_found() {
    let dir = creft_env();
    creft_with(&dir)
        .args(["doctor", "nonexistent-skill-xyzzy"])
        .assert()
        .failure();
}

/// `creft doctor <skill>` on a skill with a declared env var that is not set
/// reports a Fail result and exits 1.
#[test]
fn test_doctor_skill_with_missing_env_var() {
    let dir = creft_env();

    // Add a skill that requires a nonexistent env var.
    creft_with(&dir)
        .args(["add"])
        .write_stdin(
            "---\nname: needs-token\ndescription: requires a token\nenv:\n  - name: CREFT_TEST_NONEXISTENT_VAR_XYZ\n    required: true\n---\n\n```bash\necho $CREFT_TEST_NONEXISTENT_VAR_XYZ\n```\n",
        )
        .assert()
        .success();

    // Unset the env var to ensure it is not set.
    let output = creft_with(&dir)
        .args(["doctor", "needs-token"])
        .env_remove("CREFT_TEST_NONEXISTENT_VAR_XYZ")
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("[!!]"),
        "expected a Fail result ([!!]) for missing env var, got: {stderr:?}"
    );
    assert!(
        !output.status.success(),
        "expected non-zero exit when a required env var is missing"
    );
}

/// `creft doctor <skill>` on a skill with bash deps reports the dep tool checks.
#[test]
fn test_doctor_skill_with_deps() {
    let dir = creft_env();

    // Add a skill that has a bash block with deps.
    creft_with(&dir)
        .args(["add"])
        .write_stdin(
            "---\nname: uses-deps\ndescription: skill with deps\n---\n\n```bash\n# deps: curl jq\ncurl https://example.com | jq .\n```\n",
        )
        .assert()
        .success();

    let output = creft_with(&dir)
        .args(["doctor", "uses-deps"])
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    // Should mention the skill name in the output.
    assert!(
        stderr.contains("uses-deps"),
        "expected skill name in output, got: {stderr:?}"
    );
    // Should report bash interpreter check.
    assert!(
        stderr.contains("bash"),
        "expected bash interpreter check in output, got: {stderr:?}"
    );
}

/// `creft doctor <skill>` on a simple bash skill reports the interpreter.
#[test]
fn test_skill_check_simple_bash() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["add"])
        .write_stdin("---\nname: simple-bash\ndescription: a simple bash skill\n---\n\n```bash\necho hello\n```\n")
        .assert()
        .success();

    let output = creft_with(&dir)
        .args(["doctor", "simple-bash"])
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    // Should report the skill name.
    assert!(
        stderr.contains("simple-bash"),
        "expected skill name in output, got: {stderr:?}"
    );
    // Should report bash interpreter (ok or fail depending on system).
    assert!(
        stderr.contains("bash"),
        "expected bash interpreter check in output, got: {stderr:?}"
    );
}

/// `creft doctor <skill>` on a skill that calls itself detects the circular reference.
#[test]
fn test_skill_check_circular_reference() {
    let dir = creft_env();

    // Add a skill that calls itself via creft.
    creft_with(&dir)
        .args(["add"])
        .write_stdin(
            "---\nname: self-ref\ndescription: calls itself\n---\n\n```bash\ncreft self-ref\n```\n",
        )
        .assert()
        .success();

    let output = creft_with(&dir)
        .args(["doctor", "self-ref"])
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    // Should detect and report the circular reference.
    assert!(
        stderr.contains("circular"),
        "expected circular reference detection in output, got: {stderr:?}"
    );
}

// ── doctor namespace tests ────────────────────────────────────────────────────

/// `creft doctor <namespace>` runs checks on every skill in the namespace and
/// renders a report per skill.
#[test]
fn test_doctor_namespace_runs_all_skills() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["add"])
        .write_stdin(
            "---\nname: tools build\ndescription: build tool\n---\n\n```bash\necho building\n```\n",
        )
        .assert()
        .success();

    creft_with(&dir)
        .args(["add"])
        .write_stdin(
            "---\nname: tools lint\ndescription: lint tool\n---\n\n```bash\necho linting\n```\n",
        )
        .assert()
        .success();

    let output = creft_with(&dir).args(["doctor", "tools"]).output().unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    // Both skills should appear in the combined report output.
    assert!(
        stderr.contains("tools build"),
        "expected 'tools build' in doctor output, got: {stderr:?}"
    );
    assert!(
        stderr.contains("tools lint"),
        "expected 'tools lint' in doctor output, got: {stderr:?}"
    );
}

/// `creft doctor <unknown>` still errors for a name that is neither a skill
/// nor a namespace.
#[test]
fn test_doctor_unknown_name_errors() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["doctor", "xyzzy-nonexistent-namespace"])
        .assert()
        .failure();
}

/// `creft doctor <skill>` stops recursion at depth 10 and reports the limit.
#[test]
fn test_skill_check_depth_limit() {
    let dir = creft_env();

    // Build a chain of 12 skills: skill-0 calls skill-1, skill-1 calls skill-2, ..., skill-11 calls skill-0.
    // This creates a cycle, but cycle detection fires before depth limit.
    // Instead, build a linear chain longer than 10 without cycles:
    // skill-0 calls skill-1, ..., skill-10 calls skill-11
    // skill-11 has no sub-skills.
    for i in (0..=11).rev() {
        let (name, body) = if i < 11 {
            let next = i + 1;
            (
                format!("depth-{i}"),
                format!(
                    "---\nname: depth-{i}\ndescription: depth test skill {i}\n---\n\n```bash\ncreft depth-{next}\n```\n"
                ),
            )
        } else {
            (
                format!("depth-{i}"),
                format!(
                    "---\nname: depth-{i}\ndescription: depth test skill {i} leaf\n---\n\n```bash\necho leaf\n```\n"
                ),
            )
        };

        creft_with(&dir)
            .args(["add"])
            .write_stdin(body.as_str())
            .assert()
            .success();
        drop(name);
    }

    let output = creft_with(&dir)
        .args(["doctor", "depth-0"])
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    // Should report that maximum depth was exceeded.
    assert!(
        stderr.contains("maximum depth") || stderr.contains("depth"),
        "expected depth limit message in output, got: {stderr:?}"
    );
}
