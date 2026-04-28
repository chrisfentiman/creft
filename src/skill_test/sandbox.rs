//! Hermetic sandbox lifecycle for a single test scenario.
//!
//! A [`Sandbox`] is a temporary directory tree with a fixed layout:
//!
//! ```text
//! {sandbox}/
//!   source/    cwd for the child; project root the child sees
//!   home/      HOME for the child
//! ```
//!
//! On [`Drop`], the temp dir is removed unless [`Sandbox::set_keep`] was
//! called with `true`. The keep decision belongs to the caller — the scenario
//! runner evaluates the outcome and flips the flag when `--keep` is active
//! and the scenario failed.

use std::path::{Path, PathBuf};

use tempfile::TempDir;

use crate::skill_test::fixture::{FileContent, Given};
use crate::skill_test::placeholder::Paths;

// ── Error ─────────────────────────────────────────────────────────────────────

/// Errors that can occur during sandbox operations.
#[derive(Debug, thiserror::Error)]
pub(crate) enum SandboxError {
    /// The temp directory or its subdirectories could not be created.
    #[error("create sandbox: {0}")]
    Create(#[source] std::io::Error),

    /// Mirroring the host project's skill tree into the sandbox failed.
    #[error("mirror project skills: {0}")]
    Mirror(#[source] std::io::Error),

    /// Writing a seed file into the sandbox failed.
    #[error("write seed file {path}: {source}")]
    Materialise {
        path: PathBuf,
        source: std::io::Error,
    },
}

// ── Sandbox ───────────────────────────────────────────────────────────────────

/// Owned hermetic sandbox for a single scenario.
///
/// The path layout is fixed:
///
/// ```text
/// {sandbox}/
///   source/        cwd for the child; project root the child sees
///   home/          HOME for the child
/// ```
///
/// On [`Drop`], removes the temp dir unless [`set_keep`][Self::set_keep] has
/// been called with `true`.
pub(crate) struct Sandbox {
    tempdir: TempDir,
    source: PathBuf,
    home: PathBuf,
}

impl Sandbox {
    /// Allocate a fresh sandbox under `std::env::temp_dir()`.
    ///
    /// Creates `source/` and `home/` subdirectories immediately.
    /// Cleanup-on-drop is enabled by default; call [`set_keep(true)`][Self::set_keep]
    /// to preserve the directory across [`Drop`].
    pub(crate) fn new() -> Result<Self, SandboxError> {
        let tempdir = TempDir::new_in(std::env::temp_dir()).map_err(SandboxError::Create)?;

        let source = tempdir.path().join("source");
        let home = tempdir.path().join("home");

        std::fs::create_dir(&source).map_err(SandboxError::Create)?;
        std::fs::create_dir(&home).map_err(SandboxError::Create)?;

        Ok(Self {
            tempdir,
            source,
            home,
        })
    }

    /// Mark the sandbox to be preserved on `Drop`.
    ///
    /// When `keep` is `true`, the underlying `TempDir` skips cleanup and the
    /// directory survives `Drop`. When `keep` is `false` (the default at
    /// construction), `TempDir`'s own `Drop` removes the directory.
    ///
    /// This method implements the mechanism. The policy — when to call it —
    /// belongs to the caller (the scenario runner).
    pub(crate) fn set_keep(&mut self, keep: bool) {
        self.tempdir.disable_cleanup(keep);
    }

    /// Mirror the host project's `.creft/commands/` into the sandbox.
    ///
    /// Makes `{sandbox}/source/.creft/commands/<skill>.md` identical to the
    /// host file, so that `creft <skill>` invocations from inside the sandbox
    /// resolve to project-local skill files.
    ///
    /// `host_project_root` is the directory that contains `.creft/`; pass
    /// `None` to skip the mirror entirely (used in tests that supply their own
    /// skills via `Given.files`).
    pub(crate) fn mirror_project_skills(
        &self,
        host_project_root: Option<&Path>,
    ) -> Result<(), SandboxError> {
        let host_root = match host_project_root {
            Some(r) => r,
            None => return Ok(()),
        };

        let host_commands = host_root.join(".creft").join("commands");
        if !host_commands.exists() {
            return Ok(());
        }

        let dest_commands = self.source.join(".creft").join("commands");
        copy_dir_recursive(&host_commands, &dest_commands).map_err(SandboxError::Mirror)?;
        Ok(())
    }

    /// `git init` the source dir with a deterministic identity.
    ///
    /// Skills that gate on git state (worktree detection, branch checks) see a
    /// realistic environment. Failure is non-fatal — git is not a hard
    /// dependency of the test framework.
    // Called from tests; not yet called from the production binary path.
    #[allow(dead_code)]
    pub(crate) fn git_init_source(&self) {
        let _ = std::process::Command::new("git")
            .args(["-C", &self.source.to_string_lossy(), "init", "--quiet"])
            .env("GIT_AUTHOR_NAME", "creft-test")
            .env("GIT_AUTHOR_EMAIL", "test@creft.local")
            .env("GIT_COMMITTER_NAME", "creft-test")
            .env("GIT_COMMITTER_EMAIL", "test@creft.local")
            .status();
    }

    /// Write every file in `given` into the sandbox.
    ///
    /// Paths and text content must already be placeholder-expanded by the
    /// caller — `materialise` writes bytes to disk without further
    /// interpretation. Parent directories are created as needed.
    pub(crate) fn materialise(&self, given: &Given) -> Result<(), SandboxError> {
        for (path, content) in &given.files {
            let dest = PathBuf::from(path);

            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent).map_err(|e| SandboxError::Materialise {
                    path: dest.clone(),
                    source: e,
                })?;
            }

            let bytes: Vec<u8> = match content {
                FileContent::Text(s) => s.as_bytes().to_vec(),
                FileContent::Json(val) => serde_json::to_string_pretty(val)
                    .expect("serde_json::Value is always serialisable")
                    .into_bytes(),
            };

            std::fs::write(&dest, &bytes).map_err(|e| SandboxError::Materialise {
                path: dest.clone(),
                source: e,
            })?;
        }
        Ok(())
    }

    /// Build the environment for a child `creft` invocation.
    ///
    /// Returns only the keys needed to run `creft` + interpreter tooling
    /// reliably. All other parent env vars are stripped — `CREFT_HOME`,
    /// user-specific state, agent identity, secrets.
    ///
    /// `parent_env` is the parent process's environment as a slice of
    /// `(name, value)` pairs. Pass `std::env::vars().collect::<Vec<_>>()` in
    /// production and a hand-crafted slice in tests.
    ///
    /// `scenario_env` is the scenario's `when.env` (already placeholder-
    /// expanded by the caller). It overrides any parent value at the same key.
    ///
    /// The returned vec contains:
    /// - `PATH` from the parent (so child processes find executables);
    /// - `HOME = {sandbox}/home`;
    /// - `LANG`, `LC_ALL`, `TERM` from the parent, when set;
    /// - `CREFT_PROJECT_ROOT = {sandbox}/source`;
    /// - everything from `scenario_env` (overrides the above at equal keys).
    pub(crate) fn env_for_child(
        &self,
        parent_env: &[(String, String)],
        scenario_env: &[(String, String)],
    ) -> Vec<(String, String)> {
        // Allowed keys forwarded from the parent environment.
        const FORWARDED: &[&str] = &["PATH", "LANG", "LC_ALL", "TERM"];

        let mut env: Vec<(String, String)> = Vec::new();

        // Forward the allowlisted parent vars.
        for (k, v) in parent_env {
            if FORWARDED.contains(&k.as_str()) {
                env.push((k.clone(), v.clone()));
            }
        }

        // Set the sandbox-specific vars (may override a forwarded value if
        // scenario_env carries the same key, which is resolved below).
        env.push(("HOME".to_owned(), self.home.to_string_lossy().into_owned()));
        env.push((
            "CREFT_PROJECT_ROOT".to_owned(),
            self.source.to_string_lossy().into_owned(),
        ));

        // Apply scenario overrides. Keys already present are replaced; new
        // keys are appended.
        for (k, v) in scenario_env {
            if let Some(entry) = env.iter_mut().find(|(ek, _)| ek == k) {
                entry.1 = v.clone();
            } else {
                env.push((k.clone(), v.clone()));
            }
        }

        env
    }

    /// Root of the sandbox temp directory.
    pub(crate) fn root(&self) -> &Path {
        self.tempdir.path()
    }

    /// `{sandbox}/source` — the project root the child process sees.
    pub(crate) fn source(&self) -> &Path {
        &self.source
    }

    /// `{sandbox}/home` — `HOME` for the child process.
    // Called from tests; not yet called from the production binary path.
    #[allow(dead_code)]
    pub(crate) fn home(&self) -> &Path {
        &self.home
    }

    /// View this sandbox as a [`Paths`] reference borrow for placeholder expansion.
    pub(crate) fn paths(&self) -> Paths<'_> {
        Paths {
            sandbox: self.root(),
            source: &self.source,
            home: &self.home,
        }
    }

    /// Run `sh -c <command>` against this sandbox.
    ///
    /// The child process sees the sandbox env (`HOME = {sandbox}/home`,
    /// `CREFT_PROJECT_ROOT = {sandbox}/source`, plus the parent allowlist from
    /// [`env_for_child`]) and a working directory of [`source`]. All other
    /// parent env vars are stripped — secrets, user-specific state, agent
    /// identity.
    ///
    /// `command` must already be placeholder-expanded. `expand_scenario` walks
    /// `before.shell` and `after.shell` strings before any spawn happens.
    ///
    /// Returns the child's [`ExitStatus`] for the caller to interpret. The
    /// caller decides whether a non-zero status is a setup error or
    /// fire-and-forget cleanup.
    ///
    /// [`env_for_child`]: Self::env_for_child
    /// [`source`]: Self::source
    /// [`ExitStatus`]: std::process::ExitStatus
    pub(crate) fn spawn_shell(
        &self,
        command: &str,
        parent_env: &[(String, String)],
    ) -> std::io::Result<std::process::ExitStatus> {
        std::process::Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(&self.source)
            .env_clear()
            .envs(self.env_for_child(parent_env, &[]))
            .status()
    }
}

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Recursively copy everything under `src` into `dst`, creating `dst` and any
/// intermediate directories as needed. Symlinks are skipped.
fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;

    let mut entries: Vec<_> = std::fs::read_dir(src)?.collect::<std::io::Result<_>>()?;
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let src_path = entry.path();
        let file_type = entry.file_type()?;
        // Skip symlinks to avoid loops.
        if file_type.is_symlink() {
            continue;
        }
        let dst_path = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else if file_type.is_file() {
            std::fs::copy(&src_path, &dst_path)?;
        }
    }

    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    // ── Directory layout ──────────────────────────────────────────────────────

    #[test]
    fn new_sandbox_creates_source_and_home_under_root() {
        let sb = Sandbox::new().expect("sandbox creation");

        assert!(sb.root().exists(), "root exists");
        assert!(sb.source().exists(), "source exists");
        assert!(sb.home().exists(), "home exists");

        // Both subdirs are direct children of root.
        assert_eq!(sb.source().parent().unwrap(), sb.root());
        assert_eq!(sb.home().parent().unwrap(), sb.root());
    }

    // ── materialise ───────────────────────────────────────────────────────────
    //
    // materialise writes pre-expanded paths and content to disk. Placeholder
    // expansion is the caller's responsibility (done by expand::expand_scenario
    // before materialise is invoked in production). Tests therefore pass
    // pre-resolved absolute paths.

    #[test]
    fn materialise_text_file_written_to_pre_expanded_path() {
        let sb = Sandbox::new().expect("sandbox");
        let dest_path = sb.source().join("foo.txt").to_string_lossy().into_owned();
        let given = Given {
            files: vec![(dest_path.clone(), FileContent::Text("hi".to_owned()))],
        };
        sb.materialise(&given).expect("materialise");

        assert!(sb.source().join("foo.txt").exists());
        assert_eq!(std::fs::read_to_string(&dest_path).unwrap(), "hi");
    }

    #[test]
    fn materialise_json_file_produces_pretty_printed_output() {
        let sb = Sandbox::new().expect("sandbox");
        let val = serde_json::json!({"key": "value", "n": 42});
        let dest_path = sb.source().join("data.json").to_string_lossy().into_owned();
        let given = Given {
            files: vec![(dest_path.clone(), FileContent::Json(val.clone()))],
        };
        sb.materialise(&given).expect("materialise");

        let written = std::fs::read_to_string(&dest_path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&written).unwrap();
        assert_eq!(parsed, val);
        // Pretty-printed means multi-line output.
        assert!(written.contains('\n'), "JSON file is pretty-printed");
    }

    #[test]
    fn materialise_creates_parent_directories() {
        let sb = Sandbox::new().expect("sandbox");
        let dest_path = sb
            .source()
            .join("deep")
            .join("nested")
            .join("dir")
            .join("file.txt")
            .to_string_lossy()
            .into_owned();
        let given = Given {
            files: vec![(dest_path, FileContent::Text("content".to_owned()))],
        };
        sb.materialise(&given).expect("materialise");

        let dest = sb
            .source()
            .join("deep")
            .join("nested")
            .join("dir")
            .join("file.txt");
        assert!(dest.exists());
    }

    #[test]
    fn materialise_writes_text_content_verbatim() {
        // Expansion is the caller's job; materialise writes the bytes as given.
        let sb = Sandbox::new().expect("sandbox");
        let dest_path = sb.source().join("out.txt").to_string_lossy().into_owned();
        let content = "already expanded: /tmp/sb/source".to_owned();
        let given = Given {
            files: vec![(dest_path.clone(), FileContent::Text(content.clone()))],
        };
        sb.materialise(&given).expect("materialise");

        assert_eq!(std::fs::read_to_string(&dest_path).unwrap(), content);
    }

    // ── env_for_child ─────────────────────────────────────────────────────────

    #[test]
    fn env_for_child_strips_creft_home() {
        let sb = Sandbox::new().expect("sandbox");
        let parent: Vec<(String, String)> = vec![
            ("PATH".to_owned(), "/usr/bin".to_owned()),
            ("CREFT_HOME".to_owned(), "/home/user/.creft".to_owned()),
        ];
        let child_env = sb.env_for_child(&parent, &[]);

        let keys: Vec<&str> = child_env.iter().map(|(k, _)| k.as_str()).collect();
        assert!(!keys.contains(&"CREFT_HOME"), "CREFT_HOME must be stripped");
    }

    #[test]
    fn env_for_child_propagates_lang_when_set() {
        let sb = Sandbox::new().expect("sandbox");
        let parent = vec![
            ("PATH".to_owned(), "/usr/bin".to_owned()),
            ("LANG".to_owned(), "en_US.UTF-8".to_owned()),
        ];
        let child_env = sb.env_for_child(&parent, &[]);

        let lang = child_env
            .iter()
            .find(|(k, _)| k == "LANG")
            .map(|(_, v)| v.as_str());
        assert_eq!(lang, Some("en_US.UTF-8"));
    }

    #[test]
    fn env_for_child_omits_lang_when_unset() {
        let sb = Sandbox::new().expect("sandbox");
        let parent = vec![("PATH".to_owned(), "/usr/bin".to_owned())];
        let child_env = sb.env_for_child(&parent, &[]);

        let has_lang = child_env.iter().any(|(k, _)| k == "LANG");
        assert!(!has_lang, "LANG should not be emitted when parent lacks it");
    }

    #[test]
    fn env_for_child_sets_home_and_project_root() {
        let sb = Sandbox::new().expect("sandbox");
        let parent = vec![("PATH".to_owned(), "/usr/bin".to_owned())];
        let child_env = sb.env_for_child(&parent, &[]);

        let home_val = child_env
            .iter()
            .find(|(k, _)| k == "HOME")
            .map(|(_, v)| PathBuf::from(v));
        assert_eq!(home_val.as_deref(), Some(sb.home()));

        let root_val = child_env
            .iter()
            .find(|(k, _)| k == "CREFT_PROJECT_ROOT")
            .map(|(_, v)| PathBuf::from(v));
        assert_eq!(root_val.as_deref(), Some(sb.source()));
    }

    #[test]
    fn env_for_child_scenario_env_overrides_parent() {
        let sb = Sandbox::new().expect("sandbox");
        let parent = vec![
            ("PATH".to_owned(), "/usr/bin".to_owned()),
            ("LANG".to_owned(), "en_US.UTF-8".to_owned()),
        ];
        let scenario_env = vec![
            ("LANG".to_owned(), "C".to_owned()),
            ("MY_VAR".to_owned(), "hello".to_owned()),
        ];
        let child_env = sb.env_for_child(&parent, &scenario_env);

        let lang = child_env
            .iter()
            .find(|(k, _)| k == "LANG")
            .map(|(_, v)| v.as_str());
        assert_eq!(lang, Some("C"), "scenario LANG overrides parent");

        let my_var = child_env
            .iter()
            .find(|(k, _)| k == "MY_VAR")
            .map(|(_, v)| v.as_str());
        assert_eq!(my_var, Some("hello"));
    }

    #[test]
    fn env_for_child_strips_unknown_parent_vars() {
        let sb = Sandbox::new().expect("sandbox");
        let parent = vec![
            ("PATH".to_owned(), "/usr/bin".to_owned()),
            ("OPENAI_API_KEY".to_owned(), "secret".to_owned()),
            ("AWS_PROFILE".to_owned(), "prod".to_owned()),
        ];
        let child_env = sb.env_for_child(&parent, &[]);

        let keys: Vec<&str> = child_env.iter().map(|(k, _)| k.as_str()).collect();
        assert!(!keys.contains(&"OPENAI_API_KEY"));
        assert!(!keys.contains(&"AWS_PROFILE"));
        assert!(keys.contains(&"PATH"));
    }

    // ── spawn_shell ───────────────────────────────────────────────────────────

    #[cfg(unix)]
    fn parent_env_with_path() -> Vec<(String, String)> {
        // Provide PATH so child processes can find executables like `sh`,
        // `printf`, `command`, etc. Other parent vars are stripped by the
        // allowlist inside env_for_child.
        vec![(
            "PATH".to_owned(),
            std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".to_owned()),
        )]
    }

    #[cfg(unix)]
    #[test]
    fn spawn_shell_runs_with_sandbox_home() {
        use pretty_assertions::assert_eq;

        let sb = Sandbox::new().expect("sandbox");
        let parent_env = parent_env_with_path();

        // Write $HOME to a file; then read it back and compare to sandbox home.
        sb.spawn_shell(r#"printf '%s' "$HOME" > home.out"#, &parent_env)
            .expect("spawn_shell")
            .success()
            .then_some(())
            .expect("command must succeed");

        let content = std::fs::read_to_string(sb.source().join("home.out")).expect("home.out");
        assert_eq!(content, sb.home().to_string_lossy().as_ref());
    }

    #[cfg(unix)]
    #[test]
    fn spawn_shell_runs_in_source_directory() {
        use pretty_assertions::assert_eq;

        let sb = Sandbox::new().expect("sandbox");
        let parent_env = parent_env_with_path();

        sb.spawn_shell("pwd > pwd.out", &parent_env)
            .expect("spawn_shell")
            .success()
            .then_some(())
            .expect("command must succeed");

        let content = std::fs::read_to_string(sb.source().join("pwd.out"))
            .expect("pwd.out")
            .trim()
            .to_owned();
        let expected = sb
            .source()
            .canonicalize()
            .unwrap_or_else(|_| sb.source().to_path_buf())
            .to_string_lossy()
            .into_owned();
        assert_eq!(content, expected);
    }

    #[cfg(unix)]
    #[test]
    fn spawn_shell_strips_unknown_parent_vars() {
        use pretty_assertions::assert_eq;

        let sb = Sandbox::new().expect("sandbox");
        let mut parent_env = parent_env_with_path();
        parent_env.push(("OPERATOR_SECRET".to_owned(), "value".to_owned()));

        // ${OPERATOR_SECRET-MISSING} expands to MISSING when the var is unset.
        sb.spawn_shell(
            r#"printf '%s' "${OPERATOR_SECRET-MISSING}" > secret.out"#,
            &parent_env,
        )
        .expect("spawn_shell")
        .success()
        .then_some(())
        .expect("command must succeed");

        let content = std::fs::read_to_string(sb.source().join("secret.out")).expect("secret.out");
        assert_eq!(content, "MISSING");
    }

    #[cfg(unix)]
    #[test]
    fn spawn_shell_propagates_exit_status() {
        let sb = Sandbox::new().expect("sandbox");
        let parent_env = parent_env_with_path();

        let status = sb.spawn_shell("exit 7", &parent_env).expect("spawn_shell");
        assert_eq!(status.code(), Some(7));
    }

    #[cfg(unix)]
    #[test]
    fn spawn_shell_propagates_path_from_parent() {
        let sb = Sandbox::new().expect("sandbox");
        let parent_env = parent_env_with_path();

        // `command -v sh` exits 0 when `sh` is found on PATH; exits non-zero
        // when PATH is unset or empty. Use `command -v` (POSIX) over `which`.
        sb.spawn_shell("command -v sh > path.out", &parent_env)
            .expect("spawn_shell")
            .success()
            .then_some(())
            .expect("command -v sh must succeed when PATH is forwarded");

        let content = std::fs::read_to_string(sb.source().join("path.out")).expect("path.out");
        assert!(!content.trim().is_empty(), "path.out must be non-empty");
    }

    // ── mirror_project_skills ─────────────────────────────────────────────────

    #[test]
    fn mirror_project_skills_copies_skill_files() {
        // Build a host project tree in a temp dir.
        let host_tmp = TempDir::new_in(std::env::temp_dir()).expect("host tmp");
        let commands_dir = host_tmp.path().join(".creft").join("commands");
        std::fs::create_dir_all(&commands_dir).unwrap();
        std::fs::write(commands_dir.join("setup.md"), "# setup skill").unwrap();

        let sb = Sandbox::new().expect("sandbox");
        sb.mirror_project_skills(Some(host_tmp.path()))
            .expect("mirror");

        let mirrored = sb.source().join(".creft").join("commands").join("setup.md");
        assert!(mirrored.exists(), "mirrored skill file exists");
        assert_eq!(std::fs::read_to_string(&mirrored).unwrap(), "# setup skill");
    }

    #[test]
    fn mirror_project_skills_none_is_noop() {
        let sb = Sandbox::new().expect("sandbox");
        sb.mirror_project_skills(None)
            .expect("noop should not fail");
        // No .creft directory should exist in source.
        assert!(!sb.source().join(".creft").exists());
    }

    // ── keep_on_drop / set_keep ───────────────────────────────────────────────

    #[test]
    fn drop_removes_dir_by_default() {
        let sb = Sandbox::new().expect("sandbox");
        let root = sb.root().to_owned();
        assert!(root.exists());
        drop(sb);
        assert!(!root.exists(), "sandbox dir must be removed on drop");
    }

    #[test]
    fn set_keep_true_preserves_dir_across_drop() {
        let mut sb = Sandbox::new().expect("sandbox");
        let root = sb.root().to_owned();
        sb.set_keep(true);
        drop(sb);
        assert!(
            root.exists(),
            "sandbox dir must survive drop when keep=true"
        );
        // Clean up manually.
        std::fs::remove_dir_all(&root).ok();
    }
}
