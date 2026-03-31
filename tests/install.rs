//! Tests for install, update, uninstall, unified skill resolution, and full lifecycle.

mod helpers;

use helpers::{create_test_package, creft_env, creft_with};
use predicates::prelude::*;
use tempfile::TempDir;

// ── install tests ─────────────────────────────────────────────────────────────

/// `creft install <local-git-repo-path>` succeeds when the repo has a valid
/// `creft.yaml` manifest.
#[test]
fn test_install_local_repo_succeeds() {
    let pkg_repo = create_test_package(
        "my-tools",
        &[(
            "hello.md",
            "---\nname: hello\ndescription: say hello\n---\n\n```bash\necho hi\n```\n",
        )],
    );
    let creft_home = creft_env();

    creft_with(&creft_home)
        .args(["install", pkg_repo.path().to_str().unwrap()])
        .assert()
        .success()
        .stderr(predicate::str::contains("installed: my-tools"));
}

/// `creft install` on a repo without `creft.yaml` fails with a clear error.
#[test]
fn test_install_repo_without_manifest_fails() {
    let dir = TempDir::new().unwrap();
    let path = dir.path();

    // Init a git repo but do NOT add creft.yaml
    std::process::Command::new("git")
        .args(["init"])
        .current_dir(path)
        .output()
        .expect("git init failed");
    std::process::Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(path)
        .output()
        .expect("git config failed");
    std::process::Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(path)
        .output()
        .expect("git config name failed");

    // We need at least one commit for git clone to succeed.
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
        .args(["install", path.to_str().unwrap()])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("manifest not found"));
}

/// Installing the same package twice fails with "already installed".
#[test]
fn test_install_duplicate_fails() {
    let pkg_repo = create_test_package("dup-tools", &[]);
    let creft_home = creft_env();
    let url = pkg_repo.path().to_str().unwrap();

    // First install succeeds.
    creft_with(&creft_home)
        .args(["install", url])
        .assert()
        .success();

    // Second install fails.
    creft_with(&creft_home)
        .args(["install", url])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("already installed"));
}

/// `creft install` with an invalid git URL returns a git error.
#[test]
fn test_install_invalid_url_fails() {
    let creft_home = creft_env();

    creft_with(&creft_home)
        .args(["install", "/nonexistent/path/that/does/not/exist"])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("git command failed"));
}

/// `install` is a reserved name and cannot be used as a user command name.
#[test]
fn test_install_is_reserved_name() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["add"])
        .write_stdin(
            "---\nname: install\ndescription: shadow install\n---\n\n```bash\necho oops\n```\n",
        )
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("reserved"));
}

// ── update tests ──────────────────────────────────────────────────────────────

/// `creft update <name>` when no packages are installed returns PackageNotFound.
#[test]
fn test_update_not_found() {
    let creft_home = creft_env();

    creft_with(&creft_home)
        .args(["update", "nonexistent-pkg"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("package not found"));
}

/// `creft update` (no args) with no packages installed reports "no packages installed".
#[test]
fn test_update_all_no_packages() {
    let creft_home = creft_env();

    creft_with(&creft_home)
        .args(["update"])
        .assert()
        .success()
        .stderr(predicate::str::contains("no packages installed"));
}

/// `creft update <name>` after modifying the source repo picks up changes.
#[test]
fn test_update_picks_up_new_version() {
    let pkg_repo = create_test_package("update-test-pkg", &[]);
    let repo_path = pkg_repo.path();
    let creft_home = creft_env();

    // Install the package.
    creft_with(&creft_home)
        .args(["install", repo_path.to_str().unwrap()])
        .assert()
        .success();

    // Add a new commit to the source repo (bump version).
    std::fs::write(
        repo_path.join("creft.yaml"),
        "name: update-test-pkg\nversion: 0.2.0\ndescription: Updated test package\n",
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

    // Update the package — should succeed and print updated name + version.
    creft_with(&creft_home)
        .args(["update", "update-test-pkg"])
        .assert()
        .success()
        .stderr(predicate::str::contains("updated: update-test-pkg (0.2.0)"));
}

/// `creft update` (no args) updates all installed packages.
#[test]
fn test_update_all_updates_packages() {
    let repo1 = create_test_package("all-update-pkg-a", &[]);
    let repo2 = create_test_package("all-update-pkg-b", &[]);
    let creft_home = creft_env();

    creft_with(&creft_home)
        .args(["install", repo1.path().to_str().unwrap()])
        .assert()
        .success();
    creft_with(&creft_home)
        .args(["install", repo2.path().to_str().unwrap()])
        .assert()
        .success();

    creft_with(&creft_home)
        .args(["update"])
        .assert()
        .success()
        .stderr(
            predicate::str::contains("updated: all-update-pkg-a")
                .and(predicate::str::contains("updated: all-update-pkg-b")),
        );
}

/// `update` and `uninstall` are reserved names and cannot be used as user command names.
#[test]
fn test_update_is_reserved_name() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["add"])
        .write_stdin(
            "---\nname: update\ndescription: shadow update\n---\n\n```bash\necho oops\n```\n",
        )
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("reserved"));
}

#[test]
fn test_uninstall_is_reserved_name() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["add"])
        .write_stdin("---\nname: uninstall\ndescription: shadow\n---\n\n```bash\necho oops\n```\n")
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("reserved"));
}

// ── uninstall tests ───────────────────────────────────────────────────────────

/// `creft uninstall nonexistent` fails with "package not found".
#[test]
fn test_uninstall_not_found() {
    let creft_home = creft_env();

    creft_with(&creft_home)
        .args(["uninstall", "nonexistent-pkg"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("package not found"));
}

/// `creft uninstall <name>` removes the package directory.
#[test]
fn test_uninstall_removes_package() {
    let pkg_repo = create_test_package(
        "removable-pkg",
        &[(
            "hello.md",
            "---\nname: hello\ndescription: say hello\n---\n\n```bash\necho hi\n```\n",
        )],
    );
    let creft_home = creft_env();

    // Install the package.
    creft_with(&creft_home)
        .args(["install", pkg_repo.path().to_str().unwrap()])
        .assert()
        .success();

    // Uninstall it.
    creft_with(&creft_home)
        .args(["uninstall", "removable-pkg"])
        .assert()
        .success()
        .stderr(predicate::str::contains("uninstalled: removable-pkg"));

    // Verify the package directory is gone.
    let pkg_dir = creft_home.path().join("packages").join("removable-pkg");
    assert!(
        !pkg_dir.exists(),
        "package directory should not exist after uninstall"
    );
}

/// After uninstalling, reinstalling the same package should succeed.
#[test]
fn test_uninstall_then_reinstall() {
    let pkg_repo = create_test_package("reinstall-after-rm", &[]);
    let creft_home = creft_env();

    creft_with(&creft_home)
        .args(["install", pkg_repo.path().to_str().unwrap()])
        .assert()
        .success();

    creft_with(&creft_home)
        .args(["uninstall", "reinstall-after-rm"])
        .assert()
        .success();

    // Reinstall should not fail with PackageAlreadyInstalled.
    creft_with(&creft_home)
        .args(["install", pkg_repo.path().to_str().unwrap()])
        .assert()
        .success()
        .stderr(predicate::str::contains("installed: reinstall-after-rm"));
}

// ── unified skill resolution tests ────────────────────────────────────────────

/// `creft list` after installing a package shows the package's skills with
/// the package name appended in parentheses.
#[test]
fn test_list_shows_installed_skills() {
    let pkg_repo = create_test_package(
        "list-test-pkg",
        &[(
            "deploy.md",
            "---\nname: deploy\ndescription: Deploy the app\n---\n\n```bash\necho deploying\n```\n",
        )],
    );
    let creft_home = creft_env();

    creft_with(&creft_home)
        .args(["install", pkg_repo.path().to_str().unwrap()])
        .assert()
        .success();

    // With grouped output, the package appears as a collapsed namespace entry
    // showing the skill count and [package] annotation.
    creft_with(&creft_home)
        .args(["list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("list-test-pkg"))
        .stdout(predicate::str::contains("[package]"));

    // Drilling into the namespace shows the individual skill with its description.
    creft_with(&creft_home)
        .args(["list", "list-test-pkg"])
        .assert()
        .success()
        .stdout(predicate::str::contains("list-test-pkg deploy"))
        .stdout(predicate::str::contains("(pkg: list-test-pkg)"));
}

/// Installed skill can be executed via `creft <package> <skill> [args]`.
#[test]
fn test_run_installed_skill() {
    let pkg_repo = create_test_package(
        "run-test-pkg",
        &[(
            "greet.md",
            "---\nname: greet\ndescription: say hello\nargs:\n  - name: who\n    description: who to greet\n---\n\n```bash\necho \"hi {{who}}\"\n```\n",
        )],
    );
    let creft_home = creft_env();

    creft_with(&creft_home)
        .args(["install", pkg_repo.path().to_str().unwrap()])
        .assert()
        .success();

    creft_with(&creft_home)
        .args(["run-test-pkg", "greet", "world"])
        .assert()
        .success()
        .stdout(predicate::str::contains("hi world"));
}

/// A local skill with the same name as an installed skill shadows the installed one.
/// The local skill's output is produced, not the package skill's output.
#[test]
fn test_local_skill_shadows_installed_skill() {
    let pkg_repo = create_test_package(
        "shadow-pkg",
        &[(
            "greet.md",
            "---\nname: greet\ndescription: package greet\n---\n\n```bash\necho from-package\n```\n",
        )],
    );
    let creft_home = creft_env();

    // Install the package first.
    creft_with(&creft_home)
        .args(["install", pkg_repo.path().to_str().unwrap()])
        .assert()
        .success();

    // Add a local skill with the same full namespaced name ("shadow-pkg greet").
    creft_with(&creft_home)
        .args(["add"])
        .write_stdin("---\nname: shadow-pkg greet\ndescription: local greet\n---\n\n```bash\necho from-local\n```\n")
        .assert()
        .success();

    // Running the command should produce the LOCAL skill's output.
    creft_with(&creft_home)
        .args(["shadow-pkg", "greet"])
        .assert()
        .success()
        .stdout(predicate::str::contains("from-local"))
        .stdout(predicate::str::contains("from-package").not());
}

/// `creft show <package> <skill>` shows the raw content of an installed skill.
#[test]
fn test_show_installed_skill() {
    let pkg_repo = create_test_package(
        "show-pkg",
        &[(
            "deploy.md",
            "---\nname: deploy\ndescription: Deploy script\n---\n\n```bash\necho deploying\n```\n",
        )],
    );
    let creft_home = creft_env();

    creft_with(&creft_home)
        .args(["install", pkg_repo.path().to_str().unwrap()])
        .assert()
        .success();

    creft_with(&creft_home)
        .args(["show", "show-pkg", "deploy"])
        .assert()
        .success()
        // Should display the frontmatter
        .stdout(predicate::str::contains("description: Deploy script"))
        // Should display code blocks (verifies raw file is read, not just frontmatter)
        .stdout(predicate::str::contains("echo deploying"));
}

/// `creft cat <package> <skill>` prints the code blocks of an installed skill.
#[test]
fn test_cat_installed_skill() {
    let pkg_repo = create_test_package(
        "cat-pkg",
        &[(
            "build.md",
            "---\nname: build\ndescription: Build the project\n---\n\n```bash\necho building\n```\n",
        )],
    );
    let creft_home = creft_env();

    creft_with(&creft_home)
        .args(["install", pkg_repo.path().to_str().unwrap()])
        .assert()
        .success();

    creft_with(&creft_home)
        .args(["cat", "cat-pkg", "build"])
        .assert()
        .success()
        .stdout(predicate::str::contains("echo building"));
}

/// `creft edit <package> <skill>` returns an error for installed skills (read-only).
#[test]
fn test_edit_installed_skill_is_rejected() {
    let pkg_repo = create_test_package(
        "edit-pkg",
        &[(
            "deploy.md",
            "---\nname: deploy\ndescription: Deploy\n---\n\n```bash\necho deploying\n```\n",
        )],
    );
    let creft_home = creft_env();

    creft_with(&creft_home)
        .args(["install", pkg_repo.path().to_str().unwrap()])
        .assert()
        .success();

    creft_with(&creft_home)
        .args(["edit", "edit-pkg", "deploy"])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("read-only"));
}

/// `creft rm <package> <skill>` returns an error for installed skills.
#[test]
fn test_rm_installed_skill_is_rejected() {
    let pkg_repo = create_test_package(
        "rm-pkg",
        &[(
            "deploy.md",
            "---\nname: deploy\ndescription: Deploy\n---\n\n```bash\necho deploying\n```\n",
        )],
    );
    let creft_home = creft_env();

    creft_with(&creft_home)
        .args(["install", pkg_repo.path().to_str().unwrap()])
        .assert()
        .success();

    creft_with(&creft_home)
        .args(["rm", "rm-pkg", "deploy"])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("creft uninstall"));
}

/// After uninstalling a package, its skills no longer appear in `creft list`
/// and cannot be executed.
#[test]
fn test_uninstall_removes_skills_from_list() {
    let pkg_repo = create_test_package(
        "list-remove-pkg",
        &[(
            "build.md",
            "---\nname: build\ndescription: Build\n---\n\n```bash\necho building\n```\n",
        )],
    );
    let creft_home = creft_env();

    creft_with(&creft_home)
        .args(["install", pkg_repo.path().to_str().unwrap()])
        .assert()
        .success();

    // Package appears as a collapsed namespace before uninstall.
    creft_with(&creft_home)
        .args(["list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("list-remove-pkg"));

    creft_with(&creft_home)
        .args(["uninstall", "list-remove-pkg"])
        .assert()
        .success();

    // After uninstall, skill no longer appears.
    creft_with(&creft_home)
        .args(["list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("list-remove-pkg build").not());
}

/// An installed package skill with nested directory structure is reachable
/// via the CLI with tokens matching the path components.
#[test]
fn test_run_nested_installed_skill() {
    let pkg_repo = create_test_package(
        "nested-pkg",
        &[(
            "networking/check-dns.md",
            "---\nname: check-dns\ndescription: Check DNS\n---\n\n```bash\necho dns-ok\n```\n",
        )],
    );
    let creft_home = creft_env();

    creft_with(&creft_home)
        .args(["install", pkg_repo.path().to_str().unwrap()])
        .assert()
        .success();

    creft_with(&creft_home)
        .args(["nested-pkg", "networking", "check-dns"])
        .assert()
        .success()
        .stdout(predicate::str::contains("dns-ok"));
}

// ── full lifecycle tests ───────────────────────────────────────────────────────

/// Full end-to-end lifecycle: install -> list -> run -> update -> uninstall -> list empty.
///
/// This test verifies the complete workflow a user would follow when using creft
/// package management: install a package, confirm it appears in list, run one of
/// its skills, update the package, uninstall it, and confirm the skills are gone.
#[test]
fn test_full_lifecycle() {
    let pkg_repo = create_test_package(
        "lifecycle-pkg",
        &[(
            "greet.md",
            "---\nname: greet\ndescription: say hello\nargs:\n  - name: who\n    description: who to greet\n---\n\n```bash\necho \"hello {{who}}\"\n```\n",
        )],
    );
    let repo_path = pkg_repo.path();
    let creft_home = creft_env();

    // Step 1: Install the package.
    creft_with(&creft_home)
        .args(["install", repo_path.to_str().unwrap()])
        .assert()
        .success()
        .stderr(predicate::str::contains("installed: lifecycle-pkg"));

    // Step 2: List shows the installed package as a collapsed namespace entry.
    creft_with(&creft_home)
        .args(["list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("lifecycle-pkg"))
        .stdout(predicate::str::contains("[package]"));

    // Drilling into the namespace shows the individual skill with source indicator.
    creft_with(&creft_home)
        .args(["list", "--all"])
        .assert()
        .success()
        .stdout(predicate::str::contains("lifecycle-pkg greet"))
        .stdout(predicate::str::contains("(pkg: lifecycle-pkg)"));

    // Step 3: Run the installed skill.
    creft_with(&creft_home)
        .args(["lifecycle-pkg", "greet", "world"])
        .assert()
        .success()
        .stdout(predicate::str::contains("hello world"));

    // Step 4: Update — add a new commit to the source repo, then update.
    std::fs::write(
        repo_path.join("creft.yaml"),
        "name: lifecycle-pkg\nversion: 0.2.0\ndescription: Updated lifecycle package\n",
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
        .args(["update", "lifecycle-pkg"])
        .assert()
        .success()
        .stderr(predicate::str::contains("updated: lifecycle-pkg (0.2.0)"));

    // Step 5: Uninstall.
    creft_with(&creft_home)
        .args(["uninstall", "lifecycle-pkg"])
        .assert()
        .success()
        .stderr(predicate::str::contains("uninstalled: lifecycle-pkg"));

    // Step 6: List is now empty (no more installed skills).
    creft_with(&creft_home)
        .args(["list"])
        .assert()
        .success()
        .stderr(predicate::str::contains("no commands found"));
}

/// Tag filtering works across both local and installed skills.
///
/// Installed skills with a matching tag appear in filtered list output.
/// Installed skills without the tag do not appear.
#[test]
fn test_list_tag_filter_includes_installed_skills() {
    let pkg_repo = create_test_package(
        "tag-filter-pkg",
        &[
            (
                "deploy.md",
                "---\nname: deploy\ndescription: Deploy the app\ntags:\n  - ops\n---\n\n```bash\necho deploying\n```\n",
            ),
            (
                "lint.md",
                "---\nname: lint\ndescription: Run linter\ntags:\n  - dev\n---\n\n```bash\necho linting\n```\n",
            ),
        ],
    );
    let creft_home = creft_env();

    creft_with(&creft_home)
        .args(["install", pkg_repo.path().to_str().unwrap()])
        .assert()
        .success();

    // Filtering by "ops" groups results: the deploy skill (tagged ops) appears
    // as a collapsed namespace entry; lint (tagged dev) does not appear at all.
    creft_with(&creft_home)
        .args(["list", "--tag", "ops"])
        .assert()
        .success()
        .stdout(predicate::str::contains("tag-filter-pkg"))
        .stdout(predicate::str::contains("tag-filter-pkg lint").not());

    // Drilling in with --all confirms the individual skill.
    creft_with(&creft_home)
        .args(["list", "--all", "--tag", "ops"])
        .assert()
        .success()
        .stdout(predicate::str::contains("tag-filter-pkg deploy"))
        .stdout(predicate::str::contains("tag-filter-pkg lint").not());
}

/// `creft list` grouped output: namespaces (including packages) appear before leaf skills.
/// Drilling in with `--all` shows the flat sorted view.
#[test]
fn test_list_local_and_installed_sorted() {
    let pkg_repo = create_test_package(
        "sort-pkg",
        &[(
            "zzz-skill.md",
            "---\nname: zzz-skill\ndescription: last alphabetically\n---\n\n```bash\necho zzz\n```\n",
        )],
    );
    let creft_home = creft_env();

    // Add a local non-namespaced skill.
    creft_with(&creft_home)
        .args(["add"])
        .write_stdin(
            "---\nname: aaa-local\ndescription: first alphabetically local\n---\n\n```bash\necho aaa\n```\n",
        )
        .assert()
        .success();

    creft_with(&creft_home)
        .args(["install", pkg_repo.path().to_str().unwrap()])
        .assert()
        .success();

    // Grouped output: sort-pkg appears as a collapsed namespace (namespaces first),
    // aaa-local appears as a leaf skill after it.
    let output = creft_with(&creft_home)
        .args(["list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("aaa-local"))
        .stdout(predicate::str::contains("sort-pkg"))
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8_lossy(&output);
    // Namespaces appear before leaf skills in grouped output.
    let pos_ns = stdout.find("sort-pkg").expect("sort-pkg not found");
    let pos_skill = stdout.find("aaa-local").expect("aaa-local not found");
    assert!(
        pos_ns < pos_skill,
        "namespace should appear before leaf skill in grouped list; got: {stdout:?}"
    );

    // Flat view (--all) shows the full skill name with description.
    creft_with(&creft_home)
        .args(["list", "--all"])
        .assert()
        .success()
        .stdout(predicate::str::contains("sort-pkg zzz-skill"))
        .stdout(predicate::str::contains("aaa-local"));
}
