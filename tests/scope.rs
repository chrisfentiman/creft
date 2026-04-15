//! Tests for local/global scope behavior.

mod helpers;

use helpers::{TwoScopeEnv, create_test_package, creft_two_scope};
use predicates::prelude::*;

// ── local/global scope integration tests ──────────────────────────────────────

// ── 1. add writes to local scope when .creft/ exists ──────────────────────────

/// When a `.creft/` directory exists in CWD, `creft add` places the skill in the
/// local `.creft/commands/` directory rather than the global one.
#[test]
fn test_scope_add_defaults_to_local_when_local_exists() {
    let env = TwoScopeEnv::new();

    creft_two_scope(&env)
        .args(["add"])
        .write_stdin(
            "---\nname: local-skill\ndescription: a local skill\n---\n\n```bash\necho local\n```\n",
        )
        .assert()
        .success()
        .stderr(predicate::str::contains("added: local-skill"));

    // Skill must be in local .creft/commands/, not in global.
    assert!(
        env.local_commands().join("local-skill.md").exists(),
        "skill should be in local .creft/commands/"
    );
    assert!(
        !env.global_commands().join("local-skill.md").exists(),
        "skill must NOT be in global ~/.creft/commands/"
    );
}

// ── 2. add --global forces global scope even when local exists ────────────────

/// `creft add --global` always writes to `~/.creft/commands/` even when a local
/// `.creft/` directory is present.
#[test]
fn test_scope_add_global_flag_forces_global() {
    let env = TwoScopeEnv::new();

    creft_two_scope(&env)
        .args(["add", "--global"])
        .write_stdin("---\nname: global-skill\ndescription: a global skill\n---\n\n```bash\necho global\n```\n")
        .assert()
        .success()
        .stderr(predicates::prelude::predicate::str::contains("added: global-skill"));

    // Skill must be in global ~/.creft/commands/, not local.
    assert!(
        env.global_commands().join("global-skill.md").exists(),
        "skill should be in global ~/.creft/commands/"
    );
    assert!(
        !env.local_commands().join("global-skill.md").exists(),
        "skill must NOT be in local .creft/commands/"
    );
}

// ── 3. list shows both local and global skills with scope indicators ──────────

/// `creft list` merges skills from both local and global scopes.
/// Scope annotations are NOT shown — users see the skill names and descriptions only.
#[test]
fn test_scope_list_shows_local_and_global_with_indicators() {
    let env = TwoScopeEnv::new();

    // Add a local skill.
    creft_two_scope(&env)
        .args(["add"])
        .write_stdin(
            "---\nname: local-only\ndescription: local skill\n---\n\n```bash\necho local\n```\n",
        )
        .assert()
        .success();

    // Add a global skill.
    creft_two_scope(&env)
        .args(["add", "--global"])
        .write_stdin(
            "---\nname: global-only\ndescription: global skill\n---\n\n```bash\necho global\n```\n",
        )
        .assert()
        .success();

    // List must show both skills. Scope annotations are dropped.
    creft_two_scope(&env)
        .args(["list"])
        .assert()
        .success()
        .stdout(predicates::prelude::predicate::str::contains("local-only"))
        .stdout(predicates::prelude::predicate::str::contains("global-only"))
        .stdout(predicates::prelude::predicate::str::contains("(local)").not())
        .stdout(predicates::prelude::predicate::str::contains("(global)").not());
}

// ── 4. local skill shadows global skill of same name ─────────────────────────

/// When both local and global have a skill with the same name, `creft list`
/// shows only the local one (local shadows global). No scope annotations shown.
#[test]
fn test_scope_list_local_shadows_global_same_name() {
    let env = TwoScopeEnv::new();

    // Add local version of "deploy".
    creft_two_scope(&env)
        .args(["add"])
        .write_stdin("---\nname: deploy\ndescription: local deploy\n---\n\n```bash\necho local-deploy\n```\n")
        .assert()
        .success();

    // Add global version of "deploy".
    creft_two_scope(&env)
        .args(["add", "--global"])
        .write_stdin("---\nname: deploy\ndescription: global deploy\n---\n\n```bash\necho global-deploy\n```\n")
        .assert()
        .success();

    let output = creft_two_scope(&env)
        .args(["list"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8_lossy(&output);

    // "deploy" should appear (local description wins due to shadowing).
    assert!(
        stdout.contains("deploy"),
        "deploy should appear at least once; got: {stdout:?}"
    );
    assert!(
        stdout.contains("local deploy"),
        "local deploy description should be visible; got: {stdout:?}"
    );
    // Global copy is shadowed — global description must not appear.
    assert!(
        !stdout.contains("global deploy"),
        "global deploy description should be shadowed; got: {stdout:?}"
    );
    // Scope annotations are dropped.
    assert!(
        !stdout.contains("(local)"),
        "list must not show (local) annotation; got: {stdout:?}"
    );
    assert!(
        !stdout.contains("(global)"),
        "list must not show (global) annotation; got: {stdout:?}"
    );
}

// ── 5. run resolves local skill before global ─────────────────────────────────

/// When both local and global have a skill with the same name, running it
/// executes the local version.
#[test]
fn test_scope_run_resolves_local_before_global() {
    let env = TwoScopeEnv::new();

    // Local version echoes "local-version".
    creft_two_scope(&env)
        .args(["add"])
        .write_stdin(
            "---\nname: greet\ndescription: greet\n---\n\n```bash\necho local-version\n```\n",
        )
        .assert()
        .success();

    // Global version echoes "global-version".
    creft_two_scope(&env)
        .args(["add", "--global"])
        .write_stdin(
            "---\nname: greet\ndescription: greet\n---\n\n```bash\necho global-version\n```\n",
        )
        .assert()
        .success();

    // Running "greet" must execute the local version.
    creft_two_scope(&env)
        .args(["greet"])
        .assert()
        .success()
        .stdout(predicates::prelude::predicate::str::contains(
            "local-version",
        ))
        .stdout(predicates::prelude::predicate::str::contains("global-version").not());
}

// ── 6. plugin install always goes to global plugins cache ─────────────────────

/// `creft plugin install <url>` always installs to the global plugins cache
/// (`~/.creft/plugins/`), regardless of whether a local `.creft/` directory
/// exists. Plugin install is always global — activation is scoped.
#[test]
fn test_scope_plugin_install_always_global() {
    let pkg_repo = create_test_package(
        "my-local-pkg",
        &[(
            "hello.md",
            "---\nname: hello\ndescription: say hello\n---\n\n```bash\necho hi\n```\n",
        )],
    );

    let env = TwoScopeEnv::new();

    creft_two_scope(&env)
        .args(["plugin", "install", pkg_repo.path().to_str().unwrap()])
        .assert()
        .success()
        .stderr(predicates::prelude::predicate::str::contains(
            "installed: my-local-pkg",
        ));

    // Plugin must be in global ~/.creft/plugins/, never in packages/.
    assert!(
        env.global_plugins().join("my-local-pkg").exists(),
        "plugin should be in global ~/.creft/plugins/"
    );
    assert!(
        !env.local_packages().join("my-local-pkg").exists(),
        "plugin must NOT be in local .creft/packages/"
    );
    assert!(
        !env.global_packages().join("my-local-pkg").exists(),
        "plugin must NOT be in global ~/.creft/packages/"
    );
}

// ── 7. deprecated install alias forwards with warning ─────────────────────────

/// `creft install <url>` (deprecated) forwards to `creft plugin install`,
/// which always installs to the global plugins cache.
#[test]
fn test_scope_plugin_install_never_uses_local_scope() {
    let pkg_repo = create_test_package(
        "my-global-pkg",
        &[(
            "hello.md",
            "---\nname: hello\ndescription: say hello\n---\n\n```bash\necho hi\n```\n",
        )],
    );

    let env = TwoScopeEnv::new();

    creft_two_scope(&env)
        .args(["plugin", "install", pkg_repo.path().to_str().unwrap()])
        .assert()
        .success()
        .stderr(predicates::prelude::predicate::str::contains(
            "installed: my-global-pkg",
        ));

    // Plugin is in the global plugins cache.
    assert!(
        env.global_plugins().join("my-global-pkg").exists(),
        "plugin should be in global ~/.creft/plugins/"
    );
    assert!(
        !env.local_packages().join("my-global-pkg").exists(),
        "plugin must NOT be in local .creft/packages/"
    );
}

// ── 8. update uses plugins subcommand ────────────────────────────────────────

/// `creft plugin update <name>` finds and updates a plugin in the global plugins cache.
/// (The old root-level `creft update` alias was removed in v0.3.0.)
#[test]
fn test_scope_deprecated_update_forwards_to_plugin_update() {
    let pkg_repo = create_test_package(
        "updatable-pkg",
        &[(
            "skill.md",
            "---\nname: skill\ndescription: a skill\n---\n\n```bash\necho v1\n```\n",
        )],
    );

    let env = TwoScopeEnv::new();

    // Install via the plugins namespace (always global).
    creft_two_scope(&env)
        .args(["plugin", "install", pkg_repo.path().to_str().unwrap()])
        .assert()
        .success();

    // Update via the plugins namespace.
    creft_two_scope(&env)
        .args(["plugin", "update", "updatable-pkg"])
        .assert()
        .success()
        .stderr(predicates::prelude::predicate::str::contains(
            "updated: updatable-pkg",
        ));
}

/// `creft plugin update <name>` finds and updates a plugin in the global cache.
#[test]
fn test_scope_update_finds_global_package() {
    let pkg_repo = create_test_package(
        "global-updatable-pkg",
        &[(
            "skill.md",
            "---\nname: skill\ndescription: a skill\n---\n\n```bash\necho v1\n```\n",
        )],
    );

    let env = TwoScopeEnv::new();

    // Install to global plugins cache.
    creft_two_scope(&env)
        .args(["plugin", "install", pkg_repo.path().to_str().unwrap()])
        .assert()
        .success();

    // Update should find the plugin in the global cache and succeed.
    creft_two_scope(&env)
        .args(["plugin", "update", "global-updatable-pkg"])
        .assert()
        .success()
        .stderr(predicates::prelude::predicate::str::contains(
            "updated: global-updatable-pkg",
        ));
}

// ── 9. uninstall uses plugins subcommand ─────────────────────────────────────

/// `creft plugin uninstall <name>` removes the plugin from the global plugins cache.
/// (The old root-level `creft uninstall` alias was removed in v0.3.0.)
#[test]
fn test_scope_deprecated_uninstall_removes_from_global_plugins() {
    let pkg_repo = create_test_package(
        "local-removable-pkg",
        &[(
            "skill.md",
            "---\nname: skill\ndescription: a skill\n---\n\n```bash\necho hi\n```\n",
        )],
    );

    let env = TwoScopeEnv::new();

    creft_two_scope(&env)
        .args(["plugin", "install", pkg_repo.path().to_str().unwrap()])
        .assert()
        .success();

    creft_two_scope(&env)
        .args(["plugin", "uninstall", "local-removable-pkg"])
        .assert()
        .success();

    // Verify the plugin directory is gone from the global cache.
    assert!(
        !env.global_plugins().join("local-removable-pkg").exists(),
        "plugin should have been removed from global ~/.creft/plugins/"
    );
}

/// `creft plugin uninstall <name>` removes a plugin from the global cache.
#[test]
fn test_scope_uninstall_finds_global_package() {
    let pkg_repo = create_test_package(
        "global-removable-pkg",
        &[(
            "skill.md",
            "---\nname: skill\ndescription: a skill\n---\n\n```bash\necho hi\n```\n",
        )],
    );

    let env = TwoScopeEnv::new();

    creft_two_scope(&env)
        .args(["plugin", "install", pkg_repo.path().to_str().unwrap()])
        .assert()
        .success();

    creft_two_scope(&env)
        .args(["plugin", "uninstall", "global-removable-pkg"])
        .assert()
        .success();

    // Plugin directory must be gone from the global cache.
    assert!(
        !env.global_plugins().join("global-removable-pkg").exists(),
        "plugin should have been removed from global ~/.creft/plugins/"
    );
}

// ── 10. Full lifecycle ────────────────────────────────────────────────────────

/// Full lifecycle: add local, add global, list both (with scope indicators),
/// run local (shadows global), rm local, verify global skill still runs.
#[test]
fn test_scope_full_lifecycle_add_run_rm() {
    let env = TwoScopeEnv::new();

    // Step 1: add local skill named "ping".
    creft_two_scope(&env)
        .args(["add"])
        .write_stdin(
            "---\nname: ping\ndescription: local ping\n---\n\n```bash\necho local-ping\n```\n",
        )
        .assert()
        .success();

    // Step 2: add global skill also named "ping".
    creft_two_scope(&env)
        .args(["add", "--global"])
        .write_stdin(
            "---\nname: ping\ndescription: global ping\n---\n\n```bash\necho global-ping\n```\n",
        )
        .assert()
        .success();

    // Step 3: list — both local and global appear (but only one "ping" row since
    //         local shadows global).
    let list_output = creft_two_scope(&env)
        .args(["list"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let list_str = String::from_utf8_lossy(&list_output);
    // Local skill "ping" should appear; scope annotations are dropped.
    assert!(
        list_str.contains("ping"),
        "list should show ping; got: {list_str:?}"
    );
    // Scope annotations must not appear.
    assert!(
        !list_str.contains("(local)"),
        "list must not show (local) annotation; got: {list_str:?}"
    );
    // Global "ping" is shadowed, so no "(global)" should appear for ping.
    let global_ping_line = list_str
        .lines()
        .any(|line| line.contains("ping") && line.contains("(global)"));
    assert!(
        !global_ping_line,
        "global ping should be shadowed in list output; got: {list_str:?}"
    );

    // Step 4: run "ping" — executes local version.
    creft_two_scope(&env)
        .args(["ping"])
        .assert()
        .success()
        .stdout(predicates::prelude::predicate::str::contains("local-ping"))
        .stdout(predicates::prelude::predicate::str::contains("global-ping").not());

    // Step 5: rm "ping" (removes the local copy).
    creft_two_scope(&env)
        .args(["remove", "ping"])
        .assert()
        .success();

    assert!(
        !env.local_commands().join("ping.md").exists(),
        "local ping.md should have been removed"
    );

    // Step 6: run "ping" again — now resolves global version.
    creft_two_scope(&env)
        .args(["ping"])
        .assert()
        .success()
        .stdout(predicates::prelude::predicate::str::contains("global-ping"))
        .stdout(predicates::prelude::predicate::str::contains("local-ping").not());

    // Step 7: list — "ping" still appears (now from global, but no scope annotation shown).
    creft_two_scope(&env)
        .args(["list"])
        .assert()
        .success()
        .stdout(predicates::prelude::predicate::str::contains("ping"))
        // Scope annotations are dropped — no (global) suffix.
        .stdout(predicates::prelude::predicate::str::contains("(global)").not());
}
