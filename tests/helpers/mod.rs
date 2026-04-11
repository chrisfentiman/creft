//! Shared test utilities for creft integration tests.

// Each test binary only uses a subset of helpers. Suppress dead_code warnings
// that fire when a particular binary doesn't use every helper in this module.
#![allow(dead_code)]

use assert_cmd::Command;
use std::time::Duration;
use tempfile::TempDir;

/// Create an isolated CREFT_HOME directory.
pub fn creft_env() -> TempDir {
    TempDir::new().unwrap()
}

/// Create a creft command bound to the given CREFT_HOME directory.
/// Clears CREFT_PROJECT_ROOT to prevent env leakage from parent processes
/// (e.g., when tests run inside a creft skill like `creft coverage`).
pub fn creft_with(dir: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("creft").unwrap();
    cmd.env("CREFT_HOME", dir.path());
    cmd.env_remove("CREFT_PROJECT_ROOT");
    cmd
}

/// A helper to check whether a binary is on PATH. Used to skip tests that
/// require a specific interpreter when that interpreter is not available.
/// Returns true if the tool is found, false otherwise.
pub fn tool_available(name: &str) -> bool {
    std::process::Command::new("which")
        .arg(name)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Check whether the public internet is reachable by probing PyPI.
///
/// Used to gate registry integration tests so they are silently skipped in
/// air-gapped or offline environments instead of failing with a network error.
pub fn network_available() -> bool {
    ureq::Agent::new_with_config(
        ureq::config::Config::builder()
            .timeout_global(Some(Duration::from_secs(5)))
            .build(),
    )
    .head("https://pypi.org")
    .call()
    .is_ok()
}

// ── Two-scope test helpers ────────────────────────────────────────────────────
//
// The two-scope environment simulates having both a global ~/.creft/ and a
// local project-level .creft/. Layout:
//
//   home_dir/
//     .creft/
//       commands/
//       packages/
//   project_dir/
//     .creft/
//       commands/
//       packages/
//
// The `HOME` env var is set per-command invocation (child process only), so
// tests are fully parallel-safe without #[serial].

/// Struct holding both temp directories for a two-scope test.
pub struct TwoScopeEnv {
    pub home_dir: TempDir,
    pub project_dir: TempDir,
}

impl TwoScopeEnv {
    /// Create a new isolated two-scope environment.
    ///
    /// - `home_dir` contains the global `~/.creft/` storage.
    /// - `project_dir` contains a local `.creft/` directory.
    pub fn new() -> Self {
        let home_dir = TempDir::new().unwrap();
        let project_dir = TempDir::new().unwrap();

        // Pre-create the local .creft/ directory so creft sees a local root.
        std::fs::create_dir_all(project_dir.path().join(".creft").join("commands")).unwrap();
        std::fs::create_dir_all(project_dir.path().join(".creft").join("packages")).unwrap();

        // Pre-create global ~/.creft/ so global writes always succeed.
        std::fs::create_dir_all(home_dir.path().join(".creft").join("commands")).unwrap();
        std::fs::create_dir_all(home_dir.path().join(".creft").join("packages")).unwrap();

        TwoScopeEnv {
            home_dir,
            project_dir,
        }
    }

    /// Path to the local .creft/commands/ directory.
    pub fn local_commands(&self) -> std::path::PathBuf {
        self.project_dir.path().join(".creft").join("commands")
    }

    /// Path to the global ~/.creft/commands/ directory.
    pub fn global_commands(&self) -> std::path::PathBuf {
        self.home_dir.path().join(".creft").join("commands")
    }

    /// Path to the local .creft/packages/ directory.
    pub fn local_packages(&self) -> std::path::PathBuf {
        self.project_dir.path().join(".creft").join("packages")
    }

    /// Path to the global ~/.creft/packages/ directory.
    pub fn global_packages(&self) -> std::path::PathBuf {
        self.home_dir.path().join(".creft").join("packages")
    }

    /// Path to the global ~/.creft/plugins/ directory.
    pub fn global_plugins(&self) -> std::path::PathBuf {
        self.home_dir.path().join(".creft").join("plugins")
    }
}

/// Build a creft Command bound to a two-scope environment.
///
/// Sets HOME to `env.home_dir` and CWD to `env.project_dir`.
/// Does NOT set CREFT_HOME, so the two-tier resolution is active.
pub fn creft_two_scope(env: &TwoScopeEnv) -> Command {
    let mut cmd = Command::cargo_bin("creft").unwrap();
    cmd.env("HOME", env.home_dir.path())
        .current_dir(env.project_dir.path());
    // Unset CREFT_HOME so it does not bleed in from the test runner's environment.
    cmd.env_remove("CREFT_HOME");
    cmd
}

/// Create a temporary git repository containing a creft package.
///
/// Steps:
/// 1. Creates a TempDir.
/// 2. Runs `git init` inside it.
/// 3. Configures a dummy git identity (required for `git commit`).
/// 4. Writes `creft.yaml` with the given package name.
/// 5. Writes each skill file.
/// 6. Commits everything.
pub fn create_test_package(name: &str, skills: &[(&str, &str)]) -> TempDir {
    let dir = TempDir::new().unwrap();
    let path = dir.path();

    // git init
    std::process::Command::new("git")
        .args(["init"])
        .current_dir(path)
        .output()
        .expect("git init failed");

    // Configure a local identity so `git commit` works in CI and clean environments.
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

    // Write .creft/catalog.json (new format)
    let catalog_dir = path.join(".creft");
    std::fs::create_dir_all(&catalog_dir).unwrap();
    let catalog = format!(
        r#"{{"name":"{name}","description":"Test package","plugins":[{{"name":"{name}","source":".","description":"Test package","version":"0.1.0","tags":[]}}]}}"#
    );
    std::fs::write(catalog_dir.join("catalog.json"), catalog).unwrap();

    // Write skill files
    for (filename, content) in skills {
        // Create parent directories if the filename has path separators.
        let file_path = path.join(filename);
        if let Some(parent) = file_path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(file_path, content).unwrap();
    }

    // git add . && git commit
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

    dir
}

/// Create a git repo with a multi-plugin `.creft/catalog.json`.
///
/// `plugins` is a list of `(plugin_name, subdir_relative_to_repo_root)` pairs.
/// Each plugin directory gets a single `<name>.md` skill file.
pub fn create_multi_plugin_repo(plugins: &[(&str, &str)]) -> TempDir {
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
        .expect("git config email failed");
    std::process::Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(path)
        .output()
        .expect("git config name failed");

    // Build catalog entries.
    let entries: Vec<String> = plugins
        .iter()
        .map(|(name, subdir)| {
            format!(
                r#"{{"name":"{name}","source":"./{subdir}","description":"Plugin {name}","version":"0.1.0","tags":[]}}"#
            )
        })
        .collect();
    let catalog = format!(
        r#"{{"name":"multi-repo","description":"Multi plugin repo","plugins":[{}]}}"#,
        entries.join(",")
    );

    let catalog_dir = path.join(".creft");
    std::fs::create_dir_all(&catalog_dir).unwrap();
    std::fs::write(catalog_dir.join("catalog.json"), catalog).unwrap();

    // Create each plugin directory with a minimal skill file.
    for (name, subdir) in plugins {
        let plugin_dir = path.join(subdir);
        std::fs::create_dir_all(&plugin_dir).unwrap();
        let skill_content = format!(
            "---\nname: {name}\ndescription: {name} skill\n---\n\n```bash\necho {name}\n```\n"
        );
        std::fs::write(plugin_dir.join(format!("{name}.md")), skill_content).unwrap();
    }

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

    dir
}
