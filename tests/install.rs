//! Integration tests for `creft plugin install/update/uninstall` and deprecated root aliases.

mod helpers;

use helpers::{create_test_package, creft_env, creft_with};
use predicates::prelude::*;
use rstest::rstest;
use tempfile::TempDir;

// ── plugin install tests ───────────────────────────────────────────────────────

/// `creft plugin install <local-git-repo-path>` installs a plugin to the global
/// plugins cache (`$CREFT_HOME/plugins/<name>/`).
#[test]
fn plugin_install_local_repo_succeeds() {
    let pkg_repo = create_test_package(
        "my-tools",
        &[(
            "hello.md",
            "---\nname: hello\ndescription: say hello\n---\n\n```bash\necho hi\n```\n",
        )],
    );
    let creft_home = creft_env();

    creft_with(&creft_home)
        .args(["plugin", "install", pkg_repo.path().to_str().unwrap()])
        .assert()
        .success()
        .stderr(predicate::str::contains("installed: my-tools"));

    // Plugin must be in the global plugins cache, not the old packages/ directory.
    let plugin_dir = creft_home.path().join("plugins").join("my-tools");
    assert!(
        plugin_dir.exists(),
        "plugin directory should exist at plugins/my-tools"
    );
    assert!(
        !creft_home.path().join("packages").join("my-tools").exists(),
        "plugin must not be placed in packages/"
    );
}

/// `creft plugin install` on a repo without a manifest fails with a clear error.
#[test]
fn plugin_install_repo_without_manifest_fails() {
    let dir = TempDir::new().unwrap();
    let path = dir.path();

    // Init a git repo but do NOT add creft.yaml.
    std::process::Command::new("git")
        .args(["init"])
        .current_dir(path)
        .output()
        .expect("git init failed");
    std::process::Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(path)
        .output()
        .expect("git config email failed");
    std::process::Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(path)
        .output()
        .expect("git config name failed");

    std::fs::write(path.join("README.md"), "no manifest here").unwrap();
    std::process::Command::new("git")
        .args(["add", "."])
        .current_dir(path)
        .output()
        .expect("git add failed");
    std::process::Command::new("git")
        .args(["commit", "-m", "init"])
        .current_dir(path)
        .output()
        .expect("git commit failed");

    let creft_home = creft_env();
    creft_with(&creft_home)
        .args(["plugin", "install", path.to_str().unwrap()])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("manifest not found"));
}

/// Installing the same plugin twice fails with "already installed".
#[test]
fn plugin_install_duplicate_fails() {
    let pkg_repo = create_test_package("dup-tools", &[]);
    let creft_home = creft_env();
    let url = pkg_repo.path().to_str().unwrap();

    creft_with(&creft_home)
        .args(["plugin", "install", url])
        .assert()
        .success();

    creft_with(&creft_home)
        .args(["plugin", "install", url])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("already installed"));
}

/// `creft plugin install` with an invalid git URL returns a git error.
#[test]
fn plugin_install_invalid_url_fails() {
    let creft_home = creft_env();

    creft_with(&creft_home)
        .args(["plugin", "install", "/nonexistent/path/that/does/not/exist"])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("git command failed"));
}

// ── plugin update tests ────────────────────────────────────────────────────────

/// `creft plugin update <name>` when no plugins are installed returns PackageNotFound.
#[test]
fn plugin_update_not_found() {
    let creft_home = creft_env();

    creft_with(&creft_home)
        .args(["plugin", "update", "nonexistent-plugin"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("package not found"));
}

/// `creft plugin update` (no args) with no plugins installed reports "no plugins installed".
#[test]
fn plugin_update_all_no_plugins() {
    let creft_home = creft_env();

    creft_with(&creft_home)
        .args(["plugin", "update"])
        .assert()
        .success()
        .stderr(predicate::str::contains("no plugins installed"));
}

/// `creft plugin update <name>` after modifying the source repo picks up changes.
#[test]
fn plugin_update_picks_up_new_version() {
    let pkg_repo = create_test_package("update-test-plugin", &[]);
    let repo_path = pkg_repo.path();
    let creft_home = creft_env();

    creft_with(&creft_home)
        .args(["plugin", "install", repo_path.to_str().unwrap()])
        .assert()
        .success();

    // Add a new commit to the source repo.
    std::fs::write(
        repo_path.join("creft.yaml"),
        "name: update-test-plugin\nversion: 0.2.0\ndescription: Updated plugin\n",
    )
    .unwrap();
    std::process::Command::new("git")
        .args(["add", "."])
        .current_dir(repo_path)
        .output()
        .expect("git add failed");
    std::process::Command::new("git")
        .args(["commit", "-m", "bump version"])
        .current_dir(repo_path)
        .output()
        .expect("git commit failed");

    creft_with(&creft_home)
        .args(["plugin", "update", "update-test-plugin"])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "updated: update-test-plugin (0.2.0)",
        ));
}

/// `creft plugin update` (no args) updates all installed plugins.
#[test]
fn plugin_update_all_updates_plugins() {
    let repo1 = create_test_package("all-update-plugin-a", &[]);
    let repo2 = create_test_package("all-update-plugin-b", &[]);
    let creft_home = creft_env();

    creft_with(&creft_home)
        .args(["plugin", "install", repo1.path().to_str().unwrap()])
        .assert()
        .success();
    creft_with(&creft_home)
        .args(["plugin", "install", repo2.path().to_str().unwrap()])
        .assert()
        .success();

    creft_with(&creft_home)
        .args(["plugin", "update"])
        .assert()
        .success()
        .stderr(
            predicate::str::contains("updated: all-update-plugin-a")
                .and(predicate::str::contains("updated: all-update-plugin-b")),
        );
}

// ── plugin uninstall tests ────────────────────────────────────────────────────

/// `creft plugin uninstall nonexistent` fails with "package not found".
#[test]
fn plugin_uninstall_not_found() {
    let creft_home = creft_env();

    creft_with(&creft_home)
        .args(["plugin", "uninstall", "nonexistent-plugin"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("package not found"));
}

/// `creft plugin uninstall <name>` removes the plugin directory from the cache.
#[test]
fn plugin_uninstall_removes_plugin() {
    let pkg_repo = create_test_package(
        "removable-plugin",
        &[(
            "hello.md",
            "---\nname: hello\ndescription: say hello\n---\n\n```bash\necho hi\n```\n",
        )],
    );
    let creft_home = creft_env();

    creft_with(&creft_home)
        .args(["plugin", "install", pkg_repo.path().to_str().unwrap()])
        .assert()
        .success();

    creft_with(&creft_home)
        .args(["plugin", "uninstall", "removable-plugin"])
        .assert()
        .success()
        .stderr(predicate::str::contains("uninstalled: removable-plugin"));

    let plugin_dir = creft_home.path().join("plugins").join("removable-plugin");
    assert!(
        !plugin_dir.exists(),
        "plugin directory should not exist after uninstall"
    );
}

/// After uninstalling, reinstalling the same plugin should succeed.
#[test]
fn plugin_uninstall_then_reinstall() {
    let pkg_repo = create_test_package("reinstall-plugin", &[]);
    let creft_home = creft_env();

    creft_with(&creft_home)
        .args(["plugin", "install", pkg_repo.path().to_str().unwrap()])
        .assert()
        .success();

    creft_with(&creft_home)
        .args(["plugin", "uninstall", "reinstall-plugin"])
        .assert()
        .success();

    creft_with(&creft_home)
        .args(["plugin", "install", pkg_repo.path().to_str().unwrap()])
        .assert()
        .success()
        .stderr(predicate::str::contains("installed: reinstall-plugin"));
}

// ── plugin list tests ──────────────────────────────────────────────────────────

/// `creft plugin list` with no plugins installed reports "no plugins installed".
#[test]
fn plugin_list_no_plugins() {
    let creft_home = creft_env();

    creft_with(&creft_home)
        .args(["plugin", "list"])
        .assert()
        .success()
        .stderr(predicate::str::contains("no plugins installed"));
}

/// `creft plugin list` shows installed plugin names.
#[test]
fn plugin_list_shows_installed_plugins() {
    let pkg_repo = create_test_package(
        "list-plugin",
        &[(
            "hello.md",
            "---\nname: hello\ndescription: say hello\n---\n\n```bash\necho hi\n```\n",
        )],
    );
    let creft_home = creft_env();

    creft_with(&creft_home)
        .args(["plugin", "install", pkg_repo.path().to_str().unwrap()])
        .assert()
        .success();

    creft_with(&creft_home)
        .args(["plugin", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("list-plugin"));
}

/// `creft plugin list <name>` shows commands within the named plugin.
#[test]
fn plugin_list_shows_plugin_commands() {
    let pkg_repo = create_test_package(
        "cmd-list-plugin",
        &[(
            "deploy.md",
            "---\nname: deploy\ndescription: Deploy\n---\n\n```bash\necho deploying\n```\n",
        )],
    );
    let creft_home = creft_env();

    creft_with(&creft_home)
        .args(["plugin", "install", pkg_repo.path().to_str().unwrap()])
        .assert()
        .success();

    creft_with(&creft_home)
        .args(["plugin", "list", "cmd-list-plugin"])
        .assert()
        .success()
        .stdout(predicate::str::contains("cmd-list-plugin deploy"));
}

// ── plugins_dir() isolation tests ─────────────────────────────────────────────

/// `plugins_dir()` respects `CREFT_HOME` for test isolation.
/// When CREFT_HOME is set, plugins go to `$CREFT_HOME/plugins/`.
#[test]
fn plugin_install_respects_creft_home() {
    let pkg_repo = create_test_package("isolation-plugin", &[]);
    let creft_home = creft_env();

    creft_with(&creft_home)
        .args(["plugin", "install", pkg_repo.path().to_str().unwrap()])
        .assert()
        .success();

    let expected_path = creft_home.path().join("plugins").join("isolation-plugin");
    assert!(
        expected_path.exists(),
        "plugin should be at $CREFT_HOME/plugins/isolation-plugin"
    );
}

// ── deprecated root alias tests ────────────────────────────────────────────────

/// `creft install <url>` (deprecated) forwards to `creft plugin install` with a warning.
#[test]
fn deprecated_install_forwards_with_warning() {
    let pkg_repo = create_test_package("deprecated-install-plugin", &[]);
    let creft_home = creft_env();

    creft_with(&creft_home)
        .args(["install", pkg_repo.path().to_str().unwrap()])
        .assert()
        .success()
        .stderr(predicate::str::contains("deprecated"))
        .stderr(predicate::str::contains("creft plugin install"))
        .stderr(predicate::str::contains(
            "installed: deprecated-install-plugin",
        ));
}

/// `creft update` (deprecated) forwards to `creft plugin update` with a warning.
#[test]
fn deprecated_update_forwards_with_warning() {
    let creft_home = creft_env();

    // No plugins installed — the forward still works and produces "no plugins installed".
    creft_with(&creft_home)
        .args(["update"])
        .assert()
        .success()
        .stderr(predicate::str::contains("deprecated"))
        .stderr(predicate::str::contains("creft plugin update"));
}

/// `creft uninstall <name>` (deprecated) forwards to `creft plugin uninstall` with a warning.
#[test]
fn deprecated_uninstall_forwards_with_warning() {
    let creft_home = creft_env();

    creft_with(&creft_home)
        .args(["uninstall", "nonexistent"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("deprecated"))
        .stderr(predicate::str::contains("creft plugin uninstall"));
}

// ── reserved name tests ────────────────────────────────────────────────────────

/// `install`, `update`, and `uninstall` are no longer reserved — skill authors
/// can use them now that plugin management lives under `creft plugin`.
#[rstest]
#[case::install("install")]
#[case::update("update")]
#[case::uninstall("uninstall")]
fn formerly_reserved_names_are_now_valid_skill_names(#[case] name: &str) {
    let creft_home = creft_env();
    let stdin = format!(
        "---\nname: {name}\ndescription: custom {name} skill\n---\n\n```bash\necho custom\n```\n"
    );

    creft_with(&creft_home)
        .args(["add"])
        .write_stdin(stdin.as_str())
        .assert()
        .success();
}

/// `plugin` is now a reserved name (it is a built-in subcommand).
#[test]
fn plugin_is_reserved() {
    let creft_home = creft_env();

    creft_with(&creft_home)
        .args(["add"])
        .write_stdin(
            "---\nname: plugin\ndescription: shadow plugin\n---\n\n```bash\necho oops\n```\n",
        )
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("reserved"));
}

// ── packages/ legacy resolution tests ─────────────────────────────────────────
//
// The packages/ directory is a backward-compat resolution path in store.rs that
// runs on every command dispatch. These tests exercise it directly — bypassing
// the plugin install flow — to verify the skill resolution logic is correct.

/// Skills in `$CREFT_HOME/packages/<pkg>/<skill>.md` can be invoked by name.
///
/// This exercises the `packages/` branch of `resolve_in_single_scope` in store.rs.
#[test]
fn package_skill_resolves_and_runs() {
    let creft_home = creft_env();
    let pkg_dir = creft_home.path().join("packages").join("my-legacy-pkg");
    std::fs::create_dir_all(&pkg_dir).unwrap();

    std::fs::write(
        pkg_dir.join("creft.yaml"),
        "name: my-legacy-pkg\nversion: 1.0.0\ndescription: legacy package\n",
    )
    .unwrap();
    std::fs::write(
        pkg_dir.join("greet.md"),
        "---\nname: greet\ndescription: print greeting\n---\n\n```bash\necho hello-from-pkg\n```\n",
    )
    .unwrap();

    creft_with(&creft_home)
        .args(["my-legacy-pkg", "greet"])
        .assert()
        .success()
        .stdout(predicate::str::contains("hello-from-pkg"));
}

/// Nested skills in `packages/<pkg>/<dir>/<skill>.md` resolve with three tokens.
#[test]
fn package_nested_skill_resolves_and_runs() {
    let creft_home = creft_env();
    let pkg_dir = creft_home
        .path()
        .join("packages")
        .join("nested-pkg")
        .join("deploy");
    std::fs::create_dir_all(&pkg_dir).unwrap();

    std::fs::write(
        creft_home
            .path()
            .join("packages")
            .join("nested-pkg")
            .join("creft.yaml"),
        "name: nested-pkg\nversion: 1.0.0\ndescription: nested package\n",
    )
    .unwrap();
    std::fs::write(
        pkg_dir.join("staging.md"),
        "---\nname: staging\ndescription: deploy to staging\n---\n\n```bash\necho deploy-staging\n```\n",
    )
    .unwrap();

    creft_with(&creft_home)
        .args(["nested-pkg", "deploy", "staging"])
        .assert()
        .success()
        .stdout(predicate::str::contains("deploy-staging"));
}

/// `creft list` shows packages from the packages/ directory with a skill count.
#[test]
fn package_appears_in_list_output() {
    let creft_home = creft_env();
    let pkg_dir = creft_home.path().join("packages").join("listable-pkg");
    std::fs::create_dir_all(&pkg_dir).unwrap();

    std::fs::write(
        pkg_dir.join("creft.yaml"),
        "name: listable-pkg\nversion: 1.0.0\ndescription: listable package\n",
    )
    .unwrap();
    std::fs::write(
        pkg_dir.join("alpha.md"),
        "---\nname: alpha\ndescription: first skill\n---\n\n```bash\necho alpha\n```\n",
    )
    .unwrap();
    std::fs::write(
        pkg_dir.join("beta.md"),
        "---\nname: beta\ndescription: second skill\n---\n\n```bash\necho beta\n```\n",
    )
    .unwrap();

    // `creft list` shows packages by name with a skill count summary.
    creft_with(&creft_home)
        .args(["list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("listable-pkg"));
}

/// A missing skill within an installed package returns CommandNotFound (exit 2).
#[test]
fn package_missing_skill_returns_command_not_found() {
    let creft_home = creft_env();
    let pkg_dir = creft_home.path().join("packages").join("err-pkg");
    std::fs::create_dir_all(&pkg_dir).unwrap();

    std::fs::write(
        pkg_dir.join("creft.yaml"),
        "name: err-pkg\nversion: 1.0.0\ndescription: error package\n",
    )
    .unwrap();

    creft_with(&creft_home)
        .args(["err-pkg", "nonexistent"])
        .assert()
        .failure()
        .code(2);
}
