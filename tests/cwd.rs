//! Tests for working directory behavior in skill execution.

mod helpers;

use assert_cmd::Command;
use helpers::{TwoScopeEnv, creft_env, creft_two_scope, creft_with};

// ── CWD behavior integration tests ────────────────────────────────────────────
//
// These tests verify that subprocesses execute in the correct working directory
// based on skill scope, and that CREFT_PROJECT_ROOT is set appropriately.
//
// Local-scope tests use TwoScopeEnv (sets HOME, uses current_dir, no CREFT_HOME).
// Global-scope tests use creft_with (sets CREFT_HOME directly).
//
// Both patterns are fully parallel-safe without #[serial] because env vars are
// set on child processes only, not the test runner process.

/// A local skill's subprocess runs at the project root (the directory containing
/// `.creft/`), not the user's actual CWD.
///
/// Setup: project root has `.creft/`, CWD is set to the project root.
/// The skill prints `$PWD`. Output must equal the project root path.
#[test]
fn test_local_skill_cwd_is_project_root() {
    let env = TwoScopeEnv::new();

    // Write a skill directly into the local .creft/commands/ directory.
    // The skill prints its working directory.
    let skill_md =
        "---\nname: print-cwd\ndescription: print working dir\n---\n\n```bash\npwd\n```\n";
    std::fs::write(env.local_commands().join("print-cwd.md"), skill_md).unwrap();

    let output = creft_two_scope(&env)
        .args(["print-cwd"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8_lossy(&output);
    let project_root = env.project_dir.path().to_string_lossy().to_string();

    // Resolve any symlinks in the project root path (macOS /var -> /private/var).
    let resolved_project_root = std::fs::canonicalize(env.project_dir.path())
        .unwrap()
        .to_string_lossy()
        .to_string();

    // pwd output may have trailing newline; trim before comparing.
    let cwd_output = stdout.trim().to_string();

    assert!(
        cwd_output == project_root || cwd_output == resolved_project_root,
        "local skill must run at project root; expected {resolved_project_root:?} got {cwd_output:?}"
    );
}

/// A local skill invoked from a subdirectory still executes at the project root.
///
/// This is the core CWD feature: the subprocess CWD is always the project root
/// regardless of how deep within the project tree the user invoked creft.
#[test]
fn test_local_skill_cwd_is_project_root_from_subdirectory() {
    let env = TwoScopeEnv::new();

    // Create a subdirectory within the project.
    let subdir = env.project_dir.path().join("src").join("deep");
    std::fs::create_dir_all(&subdir).unwrap();

    // Write the skill directly into local .creft/commands/.
    let skill_md =
        "---\nname: print-cwd\ndescription: print working dir\n---\n\n```bash\npwd\n```\n";
    std::fs::write(env.local_commands().join("print-cwd.md"), skill_md).unwrap();

    // Invoke creft from the subdirectory, not the project root.
    let mut cmd = Command::cargo_bin("creft").unwrap();
    cmd.env("HOME", env.home_dir.path())
        .env_remove("CREFT_HOME")
        .current_dir(&subdir)
        .args(["print-cwd"]);

    let output = cmd.assert().success().get_output().stdout.clone();
    let stdout = String::from_utf8_lossy(&output);

    let resolved_project_root = std::fs::canonicalize(env.project_dir.path())
        .unwrap()
        .to_string_lossy()
        .to_string();

    let cwd_output = stdout.trim().to_string();

    assert!(
        cwd_output == resolved_project_root
            || cwd_output == env.project_dir.path().to_string_lossy(),
        "local skill invoked from subdirectory must run at project root; \
         expected {resolved_project_root:?} got {cwd_output:?}"
    );
}

/// A global skill's subprocess runs in the user's actual CWD at invocation time
/// (i.e., wherever the user ran creft from), not a fixed project root.
///
/// Uses CREFT_HOME so CREFT_HOME mode applies; CWD is the global test temp dir.
#[test]
fn test_global_skill_cwd_is_invocation_dir() {
    let dir = creft_env();

    // Add a global skill (CREFT_HOME mode) that prints its working directory.
    creft_with(&dir)
        .args(["add"])
        .write_stdin(
            "---\nname: print-cwd\ndescription: print working dir\n---\n\n```bash\npwd\n```\n",
        )
        .assert()
        .success();

    // Run the skill from a known directory — use dir.path() as CWD.
    let resolved_dir = std::fs::canonicalize(dir.path())
        .unwrap()
        .to_string_lossy()
        .to_string();

    let output = creft_with(&dir)
        .current_dir(dir.path())
        .args(["print-cwd"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8_lossy(&output);
    let cwd_output = stdout.trim().to_string();

    assert!(
        cwd_output == resolved_dir || cwd_output == dir.path().to_string_lossy(),
        "global skill must run at user's invocation CWD; \
         expected {resolved_dir:?} got {cwd_output:?}"
    );
}

/// `--dry-run` on a local skill prints a `cwd:` line to stderr showing the
/// project root path. The line appears before any block output.
#[test]
fn test_dry_run_shows_cwd_local_skill() {
    let env = TwoScopeEnv::new();

    let skill_md =
        "---\nname: show-it\ndescription: show cwd in dry-run\n---\n\n```bash\necho hello\n```\n";
    std::fs::write(env.local_commands().join("show-it.md"), skill_md).unwrap();

    let output = creft_two_scope(&env)
        .args(["show-it", "--dry-run"])
        .assert()
        .success()
        .get_output()
        .stderr
        .clone();

    let stderr = String::from_utf8_lossy(&output);

    // The `cwd:` line must appear in stderr.
    assert!(
        stderr.contains("cwd:"),
        "dry-run must print 'cwd:' line to stderr; got: {stderr:?}"
    );

    // The path shown must be the project root.
    let resolved_project_root = std::fs::canonicalize(env.project_dir.path())
        .unwrap()
        .to_string_lossy()
        .to_string();

    assert!(
        stderr.contains(&resolved_project_root)
            || stderr.contains(&env.project_dir.path().to_string_lossy().to_string()),
        "dry-run cwd line must contain project root path; \
         expected path {resolved_project_root:?} in stderr: {stderr:?}"
    );
}

/// `CREFT_PROJECT_ROOT` is set for local skills and equals the project root path.
///
/// A local skill that echoes `$CREFT_PROJECT_ROOT` must output the project root.
#[test]
fn test_creft_project_root_set_for_local_skill() {
    let env = TwoScopeEnv::new();

    let skill_md = "---\nname: show-root\ndescription: show project root env var\n---\n\n```bash\necho \"ROOT=$CREFT_PROJECT_ROOT\"\n```\n";
    std::fs::write(env.local_commands().join("show-root.md"), skill_md).unwrap();

    let output = creft_two_scope(&env)
        .args(["show-root"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8_lossy(&output);

    let resolved_project_root = std::fs::canonicalize(env.project_dir.path())
        .unwrap()
        .to_string_lossy()
        .to_string();

    assert!(
        stdout.contains(&format!("ROOT={}", resolved_project_root))
            || stdout.contains(&format!(
                "ROOT={}",
                env.project_dir.path().to_string_lossy()
            )),
        "CREFT_PROJECT_ROOT must equal project root for local skill; \
         expected ROOT={resolved_project_root:?} in stdout: {stdout:?}"
    );
}

/// `CREFT_PROJECT_ROOT` is NOT set for global skills.
///
/// A global skill (using CREFT_HOME) that echoes `$CREFT_PROJECT_ROOT` must
/// produce an empty value — the env var is absent.
#[test]
fn test_creft_project_root_absent_for_global_skill() {
    let dir = creft_env();

    // Global skill prints CREFT_PROJECT_ROOT; if unset the shell produces an empty string.
    creft_with(&dir)
        .args(["add"])
        .write_stdin(
            "---\nname: show-root\ndescription: show project root env var\n---\n\n```bash\necho \"ROOT=$CREFT_PROJECT_ROOT\"\n```\n",
        )
        .assert()
        .success();

    let output = creft_with(&dir)
        .args(["show-root"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8_lossy(&output);

    // The variable is unset, so bash expands it to empty string: "ROOT="
    assert!(
        stdout.contains("ROOT="),
        "stdout must contain ROOT= prefix; got: {stdout:?}"
    );
    // The value after ROOT= must be empty (no path follows).
    // Find "ROOT=" and check what follows on the same line.
    let root_value = stdout
        .lines()
        .find(|line| line.starts_with("ROOT="))
        .map(|line| line.trim_start_matches("ROOT=").trim())
        .unwrap_or("");

    assert!(
        root_value.is_empty(),
        "CREFT_PROJECT_ROOT must be empty/unset for global skill; got: {root_value:?}"
    );
}

/// When CWD is under HOME but no project-local `.creft/` exists, a global skill's
/// subprocess must run in the actual CWD, not in HOME.
///
/// Regression: `find_local_root` used to return `~/.creft/` which caused
/// `derive_cwd()` to set subprocess CWD to HOME.
#[test]
fn test_global_skill_cwd_not_home_when_no_local_creft() {
    let home = tempfile::tempdir().unwrap();
    // Create the global store.
    let global_commands = home.path().join(".creft").join("commands");
    std::fs::create_dir_all(&global_commands).unwrap();

    // A workdir under HOME with no .creft/ of its own.
    let workdir = home.path().join("myproject");
    std::fs::create_dir_all(&workdir).unwrap();

    // Skill prints its working directory.
    let skill_md =
        "---\nname: print-cwd\ndescription: print working dir\n---\n\n```bash\npwd\n```\n";
    std::fs::write(global_commands.join("print-cwd.md"), skill_md).unwrap();

    let output = Command::cargo_bin("creft")
        .unwrap()
        .env("HOME", home.path())
        .env_remove("CREFT_HOME")
        .env_remove("CREFT_PROJECT_ROOT")
        .current_dir(&workdir)
        .args(["print-cwd"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8_lossy(&output);

    // On macOS temp dirs may resolve through /private/var/... symlinks.
    let resolved_workdir = std::fs::canonicalize(&workdir)
        .unwrap()
        .to_string_lossy()
        .to_string();
    let raw_workdir = workdir.to_string_lossy().to_string();

    assert!(
        stdout.trim() == resolved_workdir || stdout.trim() == raw_workdir,
        "subprocess CWD must be the workdir, not HOME; got: {stdout:?}"
    );
}

/// When CWD is under HOME but no project-local `.creft/` exists, a global skill
/// must not receive `CREFT_PROJECT_ROOT` pointing at HOME.
///
/// Regression: `find_local_root` used to return `~/.creft/` which caused
/// `CREFT_PROJECT_ROOT` to be set to HOME.
#[test]
fn test_creft_project_root_not_home_when_no_local_creft() {
    let home = tempfile::tempdir().unwrap();
    // Create the global store.
    let global_commands = home.path().join(".creft").join("commands");
    std::fs::create_dir_all(&global_commands).unwrap();

    // A workdir under HOME with no .creft/ of its own.
    let workdir = home.path().join("myproject");
    std::fs::create_dir_all(&workdir).unwrap();

    // Skill echoes CREFT_PROJECT_ROOT.
    let skill_md = "---\nname: show-root\ndescription: show project root\n---\n\n```bash\necho \"ROOT=$CREFT_PROJECT_ROOT\"\n```\n";
    std::fs::write(global_commands.join("show-root.md"), skill_md).unwrap();

    let output = Command::cargo_bin("creft")
        .unwrap()
        .env("HOME", home.path())
        .env_remove("CREFT_HOME")
        .env_remove("CREFT_PROJECT_ROOT")
        .current_dir(&workdir)
        .args(["show-root"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8_lossy(&output);

    let root_value = stdout
        .lines()
        .find(|line| line.starts_with("ROOT="))
        .map(|line| line.trim_start_matches("ROOT=").trim())
        .unwrap_or("");

    assert!(
        root_value.is_empty(),
        "CREFT_PROJECT_ROOT must be empty when no local .creft/ exists; got: {root_value:?}"
    );
}
