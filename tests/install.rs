//! Integration tests for `creft plugin install/update/uninstall` and deprecated root aliases.

mod helpers;

use helpers::{create_multi_plugin_repo, create_test_package, creft_env, creft_with};
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
        .args(["plugins", "install", pkg_repo.path().to_str().unwrap()])
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

    // Init a git repo without a .creft/catalog.json manifest.
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
        .args(["plugins", "install", path.to_str().unwrap()])
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
        .args(["plugins", "install", url])
        .assert()
        .success();

    creft_with(&creft_home)
        .args(["plugins", "install", url])
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
        .args([
            "plugins",
            "install",
            "/nonexistent/path/that/does/not/exist",
        ])
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
        .args(["plugins", "update", "nonexistent-plugin"])
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
        .args(["plugins", "update"])
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
        .args(["plugins", "install", repo_path.to_str().unwrap()])
        .assert()
        .success();

    // Add a new commit to the source repo with a bumped version.
    let catalog_dir = repo_path.join(".creft");
    std::fs::create_dir_all(&catalog_dir).unwrap();
    std::fs::write(
        catalog_dir.join("catalog.json"),
        r#"{"name":"update-test-plugin","description":"Updated plugin","plugins":[{"name":"update-test-plugin","source":".","description":"Updated plugin","version":"0.2.0","tags":[]}]}"#,
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
        .args(["plugins", "update", "update-test-plugin"])
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
        .args(["plugins", "install", repo1.path().to_str().unwrap()])
        .assert()
        .success();
    creft_with(&creft_home)
        .args(["plugins", "install", repo2.path().to_str().unwrap()])
        .assert()
        .success();

    creft_with(&creft_home)
        .args(["plugins", "update"])
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
        .args(["plugins", "uninstall", "nonexistent-plugin"])
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
        .args(["plugins", "install", pkg_repo.path().to_str().unwrap()])
        .assert()
        .success();

    creft_with(&creft_home)
        .args(["plugins", "uninstall", "removable-plugin"])
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
        .args(["plugins", "install", pkg_repo.path().to_str().unwrap()])
        .assert()
        .success();

    creft_with(&creft_home)
        .args(["plugins", "uninstall", "reinstall-plugin"])
        .assert()
        .success();

    creft_with(&creft_home)
        .args(["plugins", "install", pkg_repo.path().to_str().unwrap()])
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
        .args(["plugins", "list"])
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
        .args(["plugins", "install", pkg_repo.path().to_str().unwrap()])
        .assert()
        .success();

    creft_with(&creft_home)
        .args(["plugins", "list"])
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
        .args(["plugins", "install", pkg_repo.path().to_str().unwrap()])
        .assert()
        .success();

    creft_with(&creft_home)
        .args(["plugins", "list", "cmd-list-plugin"])
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
        .args(["plugins", "install", pkg_repo.path().to_str().unwrap()])
        .assert()
        .success();

    let expected_path = creft_home.path().join("plugins").join("isolation-plugin");
    assert!(
        expected_path.exists(),
        "plugin should be at $CREFT_HOME/plugins/isolation-plugin"
    );
}

// ── root alias removal tests ───────────────────────────────────────────────────

/// `creft install <url>` is no longer a recognized command in v0.3.0 — it resolves
/// as a user skill and fails with "command not found".
#[test]
fn install_root_alias_removed() {
    let pkg_repo = create_test_package("some-plugin", &[]);
    let creft_home = creft_env();

    creft_with(&creft_home)
        .args(["install", pkg_repo.path().to_str().unwrap()])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("command not found"));
}

/// `creft update` is no longer a recognized command in v0.3.0 — it resolves
/// as a user skill and fails with "command not found".
#[test]
fn update_root_alias_removed() {
    let creft_home = creft_env();

    creft_with(&creft_home)
        .args(["update"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("command not found"));
}

/// `creft uninstall <name>` is no longer a recognized command in v0.3.0 — it
/// resolves as a user skill and fails with "command not found".
#[test]
fn uninstall_root_alias_removed() {
    let creft_home = creft_env();

    creft_with(&creft_home)
        .args(["uninstall", "nonexistent"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("command not found"));
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
        .args(["cmd", "add"])
        .write_stdin(stdin.as_str())
        .assert()
        .success();
}

/// `plugins` is a reserved name (it is a built-in subcommand).
#[test]
fn plugins_is_reserved() {
    let creft_home = creft_env();

    creft_with(&creft_home)
        .args(["cmd", "add"])
        .write_stdin(
            "---\nname: plugins\ndescription: shadow plugins\n---\n\n```bash\necho oops\n```\n",
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
    std::fs::create_dir_all(pkg_dir.join(".creft")).unwrap();

    std::fs::write(
        pkg_dir.join(".creft").join("catalog.json"),
        r#"{"name":"my-legacy-pkg","description":"legacy package","plugins":[{"name":"my-legacy-pkg","source":".","description":"legacy package","version":"1.0.0","tags":[]}]}"#,
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

    let nested_catalog_dir = creft_home
        .path()
        .join("packages")
        .join("nested-pkg")
        .join(".creft");
    std::fs::create_dir_all(&nested_catalog_dir).unwrap();
    std::fs::write(
        nested_catalog_dir.join("catalog.json"),
        r#"{"name":"nested-pkg","description":"nested package","plugins":[{"name":"nested-pkg","source":".","description":"nested package","version":"1.0.0","tags":[]}]}"#,
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
    std::fs::create_dir_all(pkg_dir.join(".creft")).unwrap();

    std::fs::write(
        pkg_dir.join(".creft").join("catalog.json"),
        r#"{"name":"listable-pkg","description":"listable package","plugins":[{"name":"listable-pkg","source":".","description":"listable package","version":"1.0.0","tags":[]}]}"#,
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
        .args(["cmd", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("listable-pkg"));
}

// ── Stage 3: catalog-aware install ─────────────────────────────────────────────

/// Bare name without owner/repo separator returns a clear error.
#[test]
fn plugin_install_bare_name_without_slash_fails() {
    let creft_home = creft_env();
    creft_with(&creft_home)
        .args(["plugins", "install", "fetch"])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("not a valid plugin source"));
}

/// Multi-plugin repo without `--plugin` returns an error listing available plugins.
#[test]
fn plugin_install_multi_plugin_repo_without_filter_fails() {
    let repo = create_multi_plugin_repo(&[("alpha", "plugins/alpha"), ("beta", "plugins/beta")]);
    let creft_home = creft_env();

    creft_with(&creft_home)
        .args(["plugins", "install", repo.path().to_str().unwrap()])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("Use --plugin"));
}

/// Multi-plugin repo with `--plugin <name>` installs only the named plugin.
#[test]
fn plugin_install_multi_plugin_repo_with_filter_installs_selected() {
    let repo = create_multi_plugin_repo(&[("alpha", "plugins/alpha"), ("beta", "plugins/beta")]);
    let creft_home = creft_env();

    creft_with(&creft_home)
        .args([
            "plugins",
            "install",
            repo.path().to_str().unwrap(),
            "--plugin",
            "alpha",
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains("installed: alpha"));

    // Only alpha is in the plugins dir; beta is not.
    let plugins_dir = creft_home.path().join("plugins");
    assert!(plugins_dir.join("alpha").exists());
    assert!(!plugins_dir.join("beta").exists());
}

/// Multi-plugin repo with `--plugin <name>` that does not exist returns PluginNotInCatalog.
#[test]
fn plugin_install_multi_plugin_repo_nonexistent_plugin_fails() {
    let repo = create_multi_plugin_repo(&[("alpha", "plugins/alpha")]);
    let creft_home = creft_env();

    creft_with(&creft_home)
        .args([
            "plugins",
            "install",
            repo.path().to_str().unwrap(),
            "--plugin",
            "nonexistent",
        ])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("not found in catalog"));
}

/// A repo missing `.creft/catalog.json` returns ManifestNotFound.
#[test]
fn plugin_install_repo_without_catalog_fails() {
    let dir = TempDir::new().unwrap();
    let path = dir.path();

    std::process::Command::new("git")
        .args(["init"])
        .current_dir(path)
        .output()
        .expect("git init failed");
    std::process::Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(path)
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(path)
        .output()
        .unwrap();
    std::fs::write(path.join("README.md"), "no manifest").unwrap();
    std::process::Command::new("git")
        .args(["add", "."])
        .current_dir(path)
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["commit", "-m", "init"])
        .current_dir(path)
        .output()
        .unwrap();

    let creft_home = creft_env();
    creft_with(&creft_home)
        .args(["plugins", "install", path.to_str().unwrap()])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("manifest not found"));
}

/// `creft plugin install owner/repo` (not `creft`) is treated as GitHub shorthand.
///
/// This test verifies the routing logic by checking that the error message
/// references GitHub (the resolved URL) rather than "not a valid plugin source".
#[test]
fn plugin_install_github_shorthand_routes_to_github() {
    let creft_home = creft_env();
    creft_with(&creft_home)
        .args(["plugins", "install", "someowner/somerepo"])
        .assert()
        .failure()
        .code(1)
        // The error must come from git (cloning from github.com) not from input validation.
        .stderr(predicate::str::contains("git command failed").or(
            // On machines without network access, git may report a different error.
            predicate::str::contains("git").and(predicate::str::contains("failed")),
        ));
}

/// A missing skill within an installed package returns CommandNotFound (exit 2).
#[test]
fn package_missing_skill_returns_command_not_found() {
    let creft_home = creft_env();
    let pkg_dir = creft_home.path().join("packages").join("err-pkg");
    std::fs::create_dir_all(pkg_dir.join(".creft")).unwrap();

    std::fs::write(
        pkg_dir.join(".creft").join("catalog.json"),
        r#"{"name":"err-pkg","description":"error package","plugins":[{"name":"err-pkg","source":".","description":"error package","version":"1.0.0","tags":[]}]}"#,
    )
    .unwrap();

    creft_with(&creft_home)
        .args(["err-pkg", "nonexistent"])
        .assert()
        .failure()
        .code(2);
}
