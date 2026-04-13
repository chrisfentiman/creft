//! Integration tests for `creft plugin search`.

mod helpers;

use helpers::{create_test_package, creft_env, creft_with};
use predicates::prelude::*;

// ── helpers ───────────────────────────────────────────────────────────────────

/// Install a plugin from a local git repo into creft_home.
fn install(creft_home: &tempfile::TempDir, pkg_repo: &tempfile::TempDir) {
    creft_with(creft_home)
        .args(["plugin", "install",pkg_repo.path().to_str().unwrap()])
        .assert()
        .success();
}

// ── no plugins installed ─────────────────────────────────────────────────────

/// `creft plugin search` with no plugins installed prints a clear message.
#[test]
fn search_no_plugins_installed_prints_message() {
    let creft_home = creft_env();

    creft_with(&creft_home)
        .args(["plugin", "search"])
        .assert()
        .success()
        .stderr(predicate::str::contains("no plugins installed"));
}

/// `creft plugin search <query>` with no plugins installed prints "no matching skills".
#[test]
fn search_query_no_plugins_installed_prints_no_matches() {
    let creft_home = creft_env();

    creft_with(&creft_home)
        .args(["plugin", "search","fetch"])
        .assert()
        .success()
        .stderr(predicate::str::contains("no matching skills found"));
}

// ── empty query (list all) ────────────────────────────────────────────────────

/// Empty query lists all skills across all installed plugins.
#[test]
fn search_empty_query_lists_all_skills() {
    let pkg = create_test_package(
        "my-tools",
        &[
            (
                "hello.md",
                "---\nname: hello\ndescription: say hello\ntags: [greeting]\n---\n\n```bash\necho hi\n```\n",
            ),
            (
                "deploy.md",
                "---\nname: deploy\ndescription: deploy to prod\ntags: [ops]\n---\n\n```bash\necho deploy\n```\n",
            ),
        ],
    );
    let creft_home = creft_env();
    install(&creft_home, &pkg);

    creft_with(&creft_home)
        .args(["plugin", "search"])
        .assert()
        .success()
        .stdout(predicate::str::contains("my-tools hello"))
        .stdout(predicate::str::contains("my-tools deploy"));
}

// ── name match ────────────────────────────────────────────────────────────────

/// Query matching a skill's name returns that skill.
#[test]
fn search_by_skill_name_returns_match() {
    let pkg = create_test_package(
        "my-tools",
        &[(
            "fetch.md",
            "---\nname: fetch\ndescription: fetch source code\n---\n\n```bash\necho fetch\n```\n",
        )],
    );
    let creft_home = creft_env();
    install(&creft_home, &pkg);

    creft_with(&creft_home)
        .args(["plugin", "search","fetch"])
        .assert()
        .success()
        .stdout(predicate::str::contains("my-tools fetch"));
}

/// Query that does not match any skill name, description, or tag returns no results.
#[test]
fn search_no_match_prints_no_matching_skills() {
    let pkg = create_test_package(
        "my-tools",
        &[(
            "hello.md",
            "---\nname: hello\ndescription: say hello\n---\n\n```bash\necho hi\n```\n",
        )],
    );
    let creft_home = creft_env();
    install(&creft_home, &pkg);

    creft_with(&creft_home)
        .args(["plugin", "search","nonexistent-query-xyz"])
        .assert()
        .success()
        .stderr(predicate::str::contains("no matching skills found"));
}

// ── description match ─────────────────────────────────────────────────────────

/// Query matching a skill's description returns that skill.
#[test]
fn search_by_description_returns_match() {
    let pkg = create_test_package(
        "ops-tools",
        &[(
            "deploy.md",
            "---\nname: deploy\ndescription: deploy to kubernetes cluster\n---\n\n```bash\necho deploy\n```\n",
        )],
    );
    let creft_home = creft_env();
    install(&creft_home, &pkg);

    creft_with(&creft_home)
        .args(["plugin", "search","kubernetes"])
        .assert()
        .success()
        .stdout(predicate::str::contains("ops-tools deploy"));
}

// ── tag match ─────────────────────────────────────────────────────────────────

/// Query matching a skill's tag returns that skill.
#[test]
fn search_by_tag_returns_match() {
    let pkg = create_test_package(
        "aws-tools",
        &[(
            "s3-sync.md",
            "---\nname: s3-sync\ndescription: sync files\ntags: [aws, storage]\n---\n\n```bash\necho sync\n```\n",
        )],
    );
    let creft_home = creft_env();
    install(&creft_home, &pkg);

    creft_with(&creft_home)
        .args(["plugin", "search","storage"])
        .assert()
        .success()
        .stdout(predicate::str::contains("aws-tools s3-sync"));
}

// ── case-insensitive ──────────────────────────────────────────────────────────

/// Search is case-insensitive for names, descriptions, and tags.
#[test]
fn search_is_case_insensitive() {
    let pkg = create_test_package(
        "my-tools",
        &[(
            "Deploy.md",
            "---\nname: Deploy\ndescription: Deploy to Production\ntags: [CI-CD]\n---\n\n```bash\necho deploy\n```\n",
        )],
    );
    let creft_home = creft_env();
    install(&creft_home, &pkg);

    creft_with(&creft_home)
        .args(["plugin", "search","deploy"])
        .assert()
        .success()
        .stdout(predicate::str::contains("my-tools"));

    creft_with(&creft_home)
        .args(["plugin", "search","PRODUCTION"])
        .assert()
        .success()
        .stdout(predicate::str::contains("my-tools"));

    creft_with(&creft_home)
        .args(["plugin", "search","ci-cd"])
        .assert()
        .success()
        .stdout(predicate::str::contains("my-tools"));
}

// ── multi-term AND semantics ──────────────────────────────────────────────────

/// Multiple query terms are ANDed: all must match.
#[test]
fn search_multiple_terms_all_must_match() {
    let pkg = create_test_package(
        "my-tools",
        &[
            (
                "fetch.md",
                "---\nname: fetch\ndescription: fetch source code\ntags: [research]\n---\n\n```bash\necho fetch\n```\n",
            ),
            (
                "deploy.md",
                "---\nname: deploy\ndescription: deploy to prod\ntags: [ops]\n---\n\n```bash\necho deploy\n```\n",
            ),
        ],
    );
    let creft_home = creft_env();
    install(&creft_home, &pkg);

    // "fetch" AND "source" — only fetch.md matches both
    creft_with(&creft_home)
        .args(["plugin", "search","fetch", "source"])
        .assert()
        .success()
        .stdout(predicate::str::contains("my-tools fetch"))
        .stdout(predicate::str::contains("my-tools deploy").not());
}

// ── multi-plugin ──────────────────────────────────────────────────────────────

/// Results from multiple plugins are all shown.
#[test]
fn search_across_multiple_plugins() {
    let pkg_a = create_test_package(
        "plugin-alpha",
        &[(
            "cmd-a.md",
            "---\nname: cmd-a\ndescription: alpha command\n---\n\n```bash\necho a\n```\n",
        )],
    );
    let pkg_b = create_test_package(
        "plugin-beta",
        &[(
            "cmd-b.md",
            "---\nname: cmd-b\ndescription: beta command\n---\n\n```bash\necho b\n```\n",
        )],
    );
    let creft_home = creft_env();
    install(&creft_home, &pkg_a);
    install(&creft_home, &pkg_b);

    creft_with(&creft_home)
        .args(["plugin", "search","command"])
        .assert()
        .success()
        .stdout(predicate::str::contains("plugin-alpha"))
        .stdout(predicate::str::contains("plugin-beta"));
}

/// Each result includes the plugin name as provenance.
#[test]
fn search_output_includes_plugin_name() {
    let pkg = create_test_package(
        "my-tools",
        &[(
            "hello.md",
            "---\nname: hello\ndescription: say hello\n---\n\n```bash\necho hi\n```\n",
        )],
    );
    let creft_home = creft_env();
    install(&creft_home, &pkg);

    creft_with(&creft_home)
        .args(["plugin", "search","hello"])
        .assert()
        .success()
        .stdout(predicate::str::contains("plugin: my-tools"));
}

// ── query matches only one of two skills ─────────────────────────────────────

/// A specific query returns only matching skills, not all skills in the plugin.
#[test]
fn search_returns_only_matching_skills_not_all() {
    let pkg = create_test_package(
        "my-tools",
        &[
            (
                "fetch.md",
                "---\nname: fetch\ndescription: fetch source code\n---\n\n```bash\necho fetch\n```\n",
            ),
            (
                "deploy.md",
                "---\nname: deploy\ndescription: deploy to prod\n---\n\n```bash\necho deploy\n```\n",
            ),
        ],
    );
    let creft_home = creft_env();
    install(&creft_home, &pkg);

    creft_with(&creft_home)
        .args(["plugin", "search","fetch"])
        .assert()
        .success()
        .stdout(predicate::str::contains("my-tools fetch"))
        .stdout(predicate::str::contains("my-tools deploy").not());
}
