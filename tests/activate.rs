//! Integration tests for `creft plugin activate` and `creft plugin deactivate`.
//!
//! These tests verify the activation model: install to global cache, activate to
//! make commands visible, deactivate to hide them.

mod helpers;

use helpers::{TwoScopeEnv, create_test_package, creft_env, creft_two_scope};
use predicates::prelude::*;
use tempfile::TempDir;

// ── helpers ───────────────────────────────────────────────────────────────────

/// Install a plugin to `creft_home` using `creft plugin install`.
fn install_plugin(creft_home: &TempDir, pkg_repo: &TempDir) {
    helpers::creft_with(creft_home)
        .args(["plugin", "install", pkg_repo.path().to_str().unwrap()])
        .assert()
        .success();
}

/// Create a single-skill plugin repo and install it, returning both temp dirs.
fn install_single_skill_plugin(
    plugin_name: &str,
    skill_filename: &str,
    skill_content: &str,
) -> (TempDir, TempDir) {
    let pkg_repo = create_test_package(plugin_name, &[(skill_filename, skill_content)]);
    let creft_home = creft_env();
    install_plugin(&creft_home, &pkg_repo);
    (pkg_repo, creft_home)
}

const HELLO_SKILL: &str =
    "---\nname: hello\ndescription: say hello\n---\n\n```bash\necho hi\n```\n";
const DEPLOY_SKILL: &str =
    "---\nname: deploy\ndescription: deploy stuff\n---\n\n```bash\necho deploying\n```\n";

// ── activate: whole-plugin ─────────────────────────────────────────────────────

/// Activating an entire plugin makes all its commands visible.
#[test]
fn activate_all_commands_from_plugin() {
    let (_repo, creft_home) = install_single_skill_plugin("my-tools", "hello.md", HELLO_SKILL);

    helpers::creft_with(&creft_home)
        .args(["plugin", "activate", "my-tools"])
        .assert()
        .success()
        .stderr(predicate::str::contains("activated: my-tools"));

    // After activation, the command should appear in list output.
    helpers::creft_with(&creft_home)
        .args(["list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("hello"));
}

/// Activating a non-installed plugin returns PackageNotFound (exit code 2).
#[test]
fn activate_nonexistent_plugin_fails() {
    let creft_home = creft_env();

    helpers::creft_with(&creft_home)
        .args(["plugin", "activate", "ghost-tools"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("ghost-tools"));
}

// ── activate: specific command ─────────────────────────────────────────────────

/// Activating a specific command from a plugin makes only that command visible.
#[test]
fn activate_specific_command_from_plugin() {
    let pkg_repo = create_test_package(
        "k8s-tools",
        &[
            ("deploy.md", DEPLOY_SKILL),
            (
                "status.md",
                "---\nname: status\ndescription: check status\n---\n\n```bash\necho ok\n```\n",
            ),
        ],
    );
    let creft_home = creft_env();
    install_plugin(&creft_home, &pkg_repo);

    // Only activate "deploy", not "status".
    helpers::creft_with(&creft_home)
        .args(["plugin", "activate", "k8s-tools/deploy"])
        .assert()
        .success();

    // "deploy" should appear in list output.
    let list_output = helpers::creft_with(&creft_home)
        .args(["list"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let list_str = String::from_utf8_lossy(&list_output);
    assert!(
        list_str.contains("deploy"),
        "deploy should be visible after activation"
    );
    assert!(
        !list_str.contains("status"),
        "status should not be visible when only deploy is activated"
    );
}

/// Activating a non-existent command from an installed plugin fails (exit code 2).
#[test]
fn activate_nonexistent_command_fails() {
    let (_repo, creft_home) = install_single_skill_plugin("my-tools", "hello.md", HELLO_SKILL);

    helpers::creft_with(&creft_home)
        .args(["plugin", "activate", "my-tools/nonexistent"])
        .assert()
        .failure()
        .code(2);
}

// ── deactivate ────────────────────────────────────────────────────────────────

/// Deactivating a plugin removes its commands from the visible set.
#[test]
fn deactivate_removes_plugin_commands() {
    let (_repo, creft_home) = install_single_skill_plugin("my-tools", "hello.md", HELLO_SKILL);

    helpers::creft_with(&creft_home)
        .args(["plugin", "activate", "my-tools"])
        .assert()
        .success();

    helpers::creft_with(&creft_home)
        .args(["plugin", "deactivate", "my-tools"])
        .assert()
        .success()
        .stderr(predicate::str::contains("deactivated: my-tools"));

    // After deactivation, command should not appear in list.
    helpers::creft_with(&creft_home)
        .args(["list"])
        .assert()
        .success()
        .stdout(predicate::str::is_match("hello").unwrap().not());
}

/// Deactivating a plugin that was never activated returns an error.
#[test]
fn deactivate_not_activated_fails() {
    let (_repo, creft_home) = install_single_skill_plugin("my-tools", "hello.md", HELLO_SKILL);

    helpers::creft_with(&creft_home)
        .args(["plugin", "deactivate", "my-tools"])
        .assert()
        .failure()
        .code(1);
}

// ── scope: global vs local activation ─────────────────────────────────────────

/// Without `--global`, activation writes to the nearest local .creft/plugins/settings.json.
#[test]
fn activate_without_global_writes_to_local_scope() {
    let env = TwoScopeEnv::new();

    // Install the plugin (always global).
    let pkg_repo = create_test_package("my-tools", &[("hello.md", HELLO_SKILL)]);
    creft_two_scope(&env)
        .args(["plugin", "install", pkg_repo.path().to_str().unwrap()])
        .assert()
        .success();

    // Activate without --global: should write to local settings.json.
    creft_two_scope(&env)
        .args(["plugin", "activate", "my-tools"])
        .assert()
        .success();

    let local_settings_path = env
        .project_dir
        .path()
        .join(".creft")
        .join("plugins")
        .join("settings.json");
    assert!(
        local_settings_path.exists(),
        "local settings.json should be created after local activation"
    );

    let global_settings_path = env
        .home_dir
        .path()
        .join(".creft")
        .join("plugins")
        .join("settings.json");
    assert!(
        !global_settings_path.exists(),
        "global settings.json should not be created for a local activation"
    );
}

/// `--global` flag writes to `~/.creft/plugins/settings.json`.
#[test]
fn activate_with_global_flag_writes_to_global_scope() {
    let env = TwoScopeEnv::new();

    let pkg_repo = create_test_package("my-tools", &[("hello.md", HELLO_SKILL)]);
    creft_two_scope(&env)
        .args(["plugin", "install", pkg_repo.path().to_str().unwrap()])
        .assert()
        .success();

    creft_two_scope(&env)
        .args(["plugin", "activate", "--global", "my-tools"])
        .assert()
        .success();

    let global_settings_path = env
        .home_dir
        .path()
        .join(".creft")
        .join("plugins")
        .join("settings.json");
    assert!(
        global_settings_path.exists(),
        "global settings.json should be created after global activation"
    );
}

/// Local activation overrides global: command activated locally beats global activation.
#[test]
fn local_activation_is_visible_from_project_scope() {
    let env = TwoScopeEnv::new();

    let pkg_repo = create_test_package("my-tools", &[("hello.md", HELLO_SKILL)]);
    creft_two_scope(&env)
        .args(["plugin", "install", pkg_repo.path().to_str().unwrap()])
        .assert()
        .success();

    // Activate locally only.
    creft_two_scope(&env)
        .args(["plugin", "activate", "my-tools"])
        .assert()
        .success();

    // Command should appear in list from within the project.
    creft_two_scope(&env)
        .args(["list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("hello"));
}

// ── deactivate: scope semantics ────────────────────────────────────────────────

/// Deactivate without `--global` removes from all scopes.
#[test]
fn deactivate_without_global_removes_from_all_scopes() {
    let env = TwoScopeEnv::new();

    let pkg_repo = create_test_package("my-tools", &[("hello.md", HELLO_SKILL)]);
    creft_two_scope(&env)
        .args(["plugin", "install", pkg_repo.path().to_str().unwrap()])
        .assert()
        .success();

    // Activate in both scopes.
    creft_two_scope(&env)
        .args(["plugin", "activate", "my-tools"])
        .assert()
        .success();
    creft_two_scope(&env)
        .args(["plugin", "activate", "--global", "my-tools"])
        .assert()
        .success();

    // Deactivate without --global: should clear both scopes.
    creft_two_scope(&env)
        .args(["plugin", "deactivate", "my-tools"])
        .assert()
        .success();

    // Command should no longer appear in list.
    creft_two_scope(&env)
        .args(["list"])
        .assert()
        .success()
        .stdout(predicate::str::is_match("hello").unwrap().not());
}

/// Deactivate with `--global` removes only from global scope, leaving local intact.
#[test]
fn deactivate_with_global_flag_removes_only_from_global() {
    let env = TwoScopeEnv::new();

    let pkg_repo = create_test_package("my-tools", &[("hello.md", HELLO_SKILL)]);
    creft_two_scope(&env)
        .args(["plugin", "install", pkg_repo.path().to_str().unwrap()])
        .assert()
        .success();

    // Activate in both scopes.
    creft_two_scope(&env)
        .args(["plugin", "activate", "my-tools"])
        .assert()
        .success();
    creft_two_scope(&env)
        .args(["plugin", "activate", "--global", "my-tools"])
        .assert()
        .success();

    // Deactivate with --global: removes only global.
    creft_two_scope(&env)
        .args(["plugin", "deactivate", "--global", "my-tools"])
        .assert()
        .success();

    // The local activation should keep the command visible.
    creft_two_scope(&env)
        .args(["list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("hello"));

    // Global settings should not contain the plugin.
    let global_settings = std::fs::read_to_string(
        env.home_dir
            .path()
            .join(".creft")
            .join("plugins")
            .join("settings.json"),
    )
    .unwrap();
    assert!(
        !global_settings.contains("my-tools"),
        "global settings should not contain my-tools after global deactivation"
    );
}

// ── visibility: non-activated commands are hidden ─────────────────────────────

/// Commands from an installed but not activated plugin do not appear in list.
#[test]
fn non_activated_plugin_commands_are_not_visible() {
    let (_repo, creft_home) = install_single_skill_plugin("my-tools", "hello.md", HELLO_SKILL);

    // Plugin is installed but not activated.
    let list_out = helpers::creft_with(&creft_home)
        .args(["list"])
        .output()
        .unwrap();
    let list_str = String::from_utf8_lossy(&list_out.stdout);
    assert!(
        !list_str.contains("hello"),
        "non-activated plugin commands must not appear in list"
    );
}

/// Activated commands can be run directly.
#[test]
fn activated_command_can_be_run() {
    let (_repo, creft_home) = install_single_skill_plugin("my-tools", "hello.md", HELLO_SKILL);

    helpers::creft_with(&creft_home)
        .args(["plugin", "activate", "my-tools"])
        .assert()
        .success();

    helpers::creft_with(&creft_home)
        .args(["hello"])
        .assert()
        .success()
        .stdout(predicate::str::contains("hi"));
}

/// Non-activated commands from an installed plugin cannot be run.
#[test]
fn non_activated_command_cannot_be_run() {
    let (_repo, creft_home) = install_single_skill_plugin("my-tools", "hello.md", HELLO_SKILL);

    // Installed but not activated — should not be runnable.
    helpers::creft_with(&creft_home)
        .args(["hello"])
        .assert()
        .failure()
        .code(2);
}

// ── stale activations ─────────────────────────────────────────────────────────

/// After uninstalling a plugin, running its (still activated) command gives a clear error.
#[test]
fn stale_activation_produces_error_on_use() {
    let (_repo, creft_home) = install_single_skill_plugin("my-tools", "hello.md", HELLO_SKILL);

    helpers::creft_with(&creft_home)
        .args(["plugin", "activate", "my-tools"])
        .assert()
        .success();

    helpers::creft_with(&creft_home)
        .args(["plugin", "uninstall", "my-tools"])
        .assert()
        .success();

    // The command should no longer resolve (plugin dir gone = stale activation).
    helpers::creft_with(&creft_home)
        .args(["hello"])
        .assert()
        .failure()
        .code(2);
}

/// `creft doctor` reports stale activations when a plugin is uninstalled.
#[test]
fn doctor_reports_stale_activation() {
    let (_repo, creft_home) = install_single_skill_plugin("my-tools", "hello.md", HELLO_SKILL);

    helpers::creft_with(&creft_home)
        .args(["plugin", "activate", "my-tools"])
        .assert()
        .success();

    helpers::creft_with(&creft_home)
        .args(["plugin", "uninstall", "my-tools"])
        .assert()
        .success();

    // Doctor outputs to stderr.
    helpers::creft_with(&creft_home)
        .args(["doctor"])
        .assert()
        .stderr(predicate::str::contains("stale activation"));
}

// ── settings.json round-trip ──────────────────────────────────────────────────

/// `settings.json` is valid JSON and survives a full activate/deactivate cycle.
#[test]
fn settings_json_round_trips() {
    let (_repo, creft_home) = install_single_skill_plugin("my-tools", "hello.md", HELLO_SKILL);

    helpers::creft_with(&creft_home)
        .args(["plugin", "activate", "my-tools"])
        .assert()
        .success();

    let settings_path = creft_home.path().join("plugins").join("settings.json");
    let content = std::fs::read_to_string(&settings_path).unwrap();
    let parsed: serde_json::Value =
        serde_json::from_str(&content).expect("settings.json must be valid JSON after activation");
    assert!(
        parsed["activated"]["my-tools"].as_bool() == Some(true),
        "activated my-tools should be true in settings.json"
    );

    helpers::creft_with(&creft_home)
        .args(["plugin", "deactivate", "my-tools"])
        .assert()
        .success();

    let content_after = std::fs::read_to_string(&settings_path).unwrap();
    let parsed_after: serde_json::Value = serde_json::from_str(&content_after)
        .expect("settings.json must remain valid JSON after deactivation");
    assert!(
        parsed_after["activated"].get("my-tools").is_none(),
        "my-tools should be absent from activated map after deactivation"
    );
}

// ── plugin_settings_path ─────────────────────────────────────────────────────

/// `creft plugin activate` creates settings.json under `plugins/` (not directly in `.creft/`).
#[test]
fn settings_json_lives_under_plugins_subdirectory() {
    let (_repo, creft_home) = install_single_skill_plugin("my-tools", "hello.md", HELLO_SKILL);

    helpers::creft_with(&creft_home)
        .args(["plugin", "activate", "my-tools"])
        .assert()
        .success();

    let settings_path = creft_home.path().join("plugins").join("settings.json");
    assert!(
        settings_path.exists(),
        "settings.json should live at plugins/settings.json, not directly in CREFT_HOME"
    );

    // Should NOT exist at the root of CREFT_HOME.
    let root_settings = creft_home.path().join("settings.json");
    assert!(
        !root_settings.exists(),
        "settings.json should not be at the CREFT_HOME root"
    );
}
