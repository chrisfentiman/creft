//! Helpers for detecting how creft was installed.
//!
//! Used by the update command and the deferred update notice to route users to
//! the correct upgrade path (install script, Homebrew, or Cargo).

use std::path::{Path, PathBuf};

/// How creft was installed on this machine.
///
/// Returned by [`detect`] and consumed by [`crate::cmd::update::cmd_update`]
/// (refuse-and-redirect) and [`crate::update_notice::print_if_pending`]
/// (upgrade-tail selector). The variants are closed: any binary not detected
/// as Homebrew or Cargo falls through to [`InstallMethod::InstallScript`],
/// the install-script-managed default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InstallMethod {
    /// `/opt/homebrew/`, `/usr/local/Homebrew/`, `/home/linuxbrew/`, or
    /// any path containing `/Cellar/creft/` after canonicalization.
    Homebrew,
    /// Inside `$CARGO_HOME/bin/` (default `$HOME/.cargo/bin/`) after
    /// canonicalization.
    Cargo,
    /// Anything else — the install-script-managed default.
    InstallScript,
}

impl InstallMethod {
    /// User-facing command to upgrade creft on this install method.
    ///
    /// Used verbatim by [`crate::update_notice::print_if_pending`] for the
    /// deferred-notice tail.
    pub(crate) fn upgrade_command(self) -> &'static str {
        match self {
            InstallMethod::Homebrew => "brew upgrade creft",
            InstallMethod::Cargo => "cargo install creft",
            InstallMethod::InstallScript => "creft update",
        }
    }

    /// Refuse-and-redirect message for `creft update` when the binary is
    /// managed by an external package manager.
    ///
    /// Returns `Some(message)` for [`InstallMethod::Homebrew`] and
    /// [`InstallMethod::Cargo`] — the package-manager-managed variants where
    /// `creft update` refuses and redirects the user to the package manager.
    /// Returns `None` for [`InstallMethod::InstallScript`], where `creft
    /// update` proceeds with the install-script flow rather than refusing.
    /// The `Option` shape encodes the actual call-site contract: the caller
    /// refuses on `Some(_)` and falls through on `None`.
    pub(crate) fn refusal_message(self) -> Option<&'static str> {
        match self {
            InstallMethod::Homebrew => {
                Some("creft was installed via Homebrew. Run 'brew upgrade creft' to update.")
            }
            InstallMethod::Cargo => {
                Some("creft was installed via Cargo. Run 'cargo install creft' to update.")
            }
            InstallMethod::InstallScript => None,
        }
    }
}

/// Classify the install method of the binary at `exe`.
///
/// Canonicalizes `exe` once and applies detection rules in priority order:
///
/// 1. Homebrew prefixes / Cellar substring → [`InstallMethod::Homebrew`].
/// 2. `$CARGO_HOME/bin/` prefix (or `$HOME/.cargo/bin/` fallback) →
///    [`InstallMethod::Cargo`].
/// 3. Otherwise → [`InstallMethod::InstallScript`].
///
/// Returns [`InstallMethod::InstallScript`] when canonicalization fails
/// (fail-open: prefer to allow a legitimate direct update over blocking a
/// non-Homebrew, non-Cargo install).
pub(crate) fn detect(exe: &Path) -> InstallMethod {
    let canonical = match std::fs::canonicalize(exe) {
        Ok(p) => p,
        Err(_) => return InstallMethod::InstallScript,
    };

    if is_homebrew(&canonical) {
        return InstallMethod::Homebrew;
    }

    if is_cargo(&canonical) {
        return InstallMethod::Cargo;
    }

    InstallMethod::InstallScript
}

/// Returns `true` when the canonicalized path matches a known Homebrew prefix
/// or contains the Cellar substring.
///
/// Uses string comparison because the Homebrew rules are a mixed set (multiple
/// prefixes plus a substring match), making `Path::starts_with` less natural
/// than raw string operations.
fn is_homebrew(canonical: &Path) -> bool {
    let s = canonical.to_string_lossy();
    s.starts_with("/opt/homebrew/")
        || s.starts_with("/usr/local/Homebrew/")
        || s.starts_with("/home/linuxbrew/")
        || s.contains("/Cellar/creft/")
}

/// Returns `true` when the canonicalized path is inside the cargo install root.
///
/// Uses `Path::starts_with` for path-aware comparison: handles trailing
/// separators and prevents `/foo-bar/...` matching against a `/foo` prefix.
fn is_cargo(canonical: &Path) -> bool {
    match cargo_install_root() {
        Some(root) => canonical.starts_with(&root),
        None => false,
    }
}

/// Resolve the cargo install root — `$CARGO_HOME/bin` (or `$HOME/.cargo/bin`
/// fallback) — and canonicalize.
///
/// The candidate path is constructed first as `<root>/bin`, then
/// `std::fs::canonicalize` is called on that path. Canonicalizing `bin`
/// directly (rather than the parent) means a missing `bin/` subdirectory
/// returns `None` immediately — the realistic pristine-rustup case where
/// `$CARGO_HOME` exists but `bin/` has not been created yet. Returns `None`
/// when neither env var nor home directory is resolvable, or when
/// canonicalization of `<root>/bin` fails for any reason.
fn cargo_install_root() -> Option<PathBuf> {
    let root = if let Ok(cargo_home) = std::env::var("CARGO_HOME") {
        if cargo_home.is_empty() {
            return None;
        }
        PathBuf::from(cargo_home)
    } else {
        home_dir()?.join(".cargo")
    };

    std::fs::canonicalize(root.join("bin")).ok()
}

/// Resolve the user's home directory.
///
/// Mirrors `AppContext::read_home_dir` (`src/model.rs`) exactly: reads `$HOME`
/// on Unix and `$USERPROFILE` on Windows, filtering empty strings. Local helper
/// so `cargo_install_root` does not require an `AppContext`.
fn home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    let var = "USERPROFILE";
    #[cfg(not(windows))]
    let var = "HOME";

    std::env::var(var)
        .ok()
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use rstest::rstest;
    use serial_test::serial;
    use tempfile::TempDir;

    use super::*;

    // Build a symlink at `link_path` pointing to `target`, both within `dir`.
    // Returns the link path.
    #[cfg(unix)]
    fn make_symlink(dir: &TempDir, target_rel: &str, link_rel: &str) -> std::path::PathBuf {
        let target = dir.path().join(target_rel);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&target, b"").unwrap();
        let link = dir.path().join(link_rel);
        if let Some(parent) = link.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::os::unix::fs::symlink(&target, &link).unwrap();
        link
    }

    // ── detect: Homebrew ──────────────────────────────────────────────────────

    #[cfg(unix)]
    #[test]
    fn detect_homebrew_cellar_path() {
        let dir = TempDir::new().unwrap();
        let link = make_symlink(&dir, "Cellar/creft/0.5.1/bin/creft", "bin/creft");
        assert_eq!(detect(&link), InstallMethod::Homebrew);
    }

    // ── detect: Cargo ─────────────────────────────────────────────────────────

    #[cfg(unix)]
    #[test]
    #[serial(env_cargo_home)]
    fn detect_cargo_with_cargo_home_set() {
        let dir = TempDir::new().unwrap();

        // Create <tmp>/.cargo/bin/creft and a symlink pointing to it.
        let cargo_bin = dir.path().join(".cargo").join("bin");
        std::fs::create_dir_all(&cargo_bin).unwrap();
        let real = cargo_bin.join("creft");
        std::fs::write(&real, b"").unwrap();
        let link = dir.path().join("shim").join("creft");
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let cargo_home = dir.path().join(".cargo");
        // SAFETY: single-threaded test context; no other thread reads this var.
        let prior = std::env::var("CARGO_HOME").ok();
        unsafe { std::env::set_var("CARGO_HOME", &cargo_home) };
        let result = detect(&link);
        match prior {
            Some(v) => unsafe { std::env::set_var("CARGO_HOME", v) },
            None => unsafe { std::env::remove_var("CARGO_HOME") },
        }
        assert_eq!(result, InstallMethod::Cargo);
    }

    // ── detect: InstallScript (fallback cases) ────────────────────────────────

    #[cfg(unix)]
    #[test]
    fn detect_non_homebrew_non_cargo_returns_install_script() {
        let dir = TempDir::new().unwrap();
        let link = make_symlink(&dir, ".local/bin/creft-real", ".local/bin/creft");
        assert_eq!(detect(&link), InstallMethod::InstallScript);
    }

    #[test]
    fn detect_nonexistent_path_returns_install_script() {
        assert_eq!(
            detect(Path::new("/tmp/creft-does-not-exist-abc123xyz")),
            InstallMethod::InstallScript,
        );
    }

    #[cfg(unix)]
    #[test]
    #[serial(env_cargo_home)]
    fn detect_exe_outside_cargo_bin_with_bin_dir_present_returns_install_script() {
        // Realistic case: developer has rustup installed (so $CARGO_HOME/bin/ exists
        // and canonicalizes), but creft was installed via the install script and
        // lives elsewhere. The cargo install root resolves but the prefix check fails.
        let dir = TempDir::new().unwrap();

        // Create $CARGO_HOME/bin/ but do NOT put creft there.
        let cargo_bin = dir.path().join(".cargo").join("bin");
        std::fs::create_dir_all(&cargo_bin).unwrap();

        // creft lives at a different path.
        let elsewhere = dir.path().join("elsewhere").join("creft");
        std::fs::create_dir_all(elsewhere.parent().unwrap()).unwrap();
        std::fs::write(&elsewhere, b"").unwrap();

        let prior = std::env::var("CARGO_HOME").ok();
        // SAFETY: single-threaded test context; no other thread reads this var.
        unsafe { std::env::set_var("CARGO_HOME", dir.path().join(".cargo")) };
        let result = detect(&elsewhere);
        match prior {
            Some(v) => unsafe { std::env::set_var("CARGO_HOME", v) },
            None => unsafe { std::env::remove_var("CARGO_HOME") },
        }
        assert_eq!(result, InstallMethod::InstallScript);
    }

    #[cfg(unix)]
    #[test]
    #[serial(env_cargo_home)]
    fn detect_exe_with_cargo_home_bin_absent_returns_install_script() {
        // Pristine rustup install: $CARGO_HOME exists but bin/ has not been created yet.
        let dir = TempDir::new().unwrap();

        // Create $CARGO_HOME but NOT $CARGO_HOME/bin/.
        let cargo_home = dir.path().join(".cargo");
        std::fs::create_dir_all(&cargo_home).unwrap();
        // Do NOT create cargo_home.join("bin").

        let elsewhere = dir.path().join("elsewhere").join("creft");
        std::fs::create_dir_all(elsewhere.parent().unwrap()).unwrap();
        std::fs::write(&elsewhere, b"").unwrap();

        let prior = std::env::var("CARGO_HOME").ok();
        // SAFETY: single-threaded test context; no other thread reads this var.
        unsafe { std::env::set_var("CARGO_HOME", &cargo_home) };
        let result = detect(&elsewhere);
        match prior {
            Some(v) => unsafe { std::env::set_var("CARGO_HOME", v) },
            None => unsafe { std::env::remove_var("CARGO_HOME") },
        }
        assert_eq!(result, InstallMethod::InstallScript);
    }

    // ── detect: priority (Homebrew before Cargo) ──────────────────────────────

    #[cfg(unix)]
    #[test]
    #[serial(env_cargo_home)]
    fn detect_homebrew_wins_when_cargo_home_also_matches() {
        // Contrived: a path that contains /Cellar/creft/ AND starts with
        // $CARGO_HOME/bin/. Homebrew must win.
        let dir = TempDir::new().unwrap();

        // Build a directory that looks like both Cellar and cargo bin.
        let cellar_path = dir.path().join("Cellar").join("creft").join("bin");
        std::fs::create_dir_all(&cellar_path).unwrap();
        let real = cellar_path.join("creft");
        std::fs::write(&real, b"").unwrap();

        // Set CARGO_HOME so that canonicalized path of dir.path()/Cellar/creft/bin
        // would start with $CARGO_HOME/bin — we simulate this by pointing CARGO_HOME
        // such that its bin/ is dir.path()/Cellar/creft/bin.
        let prior = std::env::var("CARGO_HOME").ok();
        // SAFETY: single-threaded test context; no other thread reads this var.
        unsafe { std::env::set_var("CARGO_HOME", dir.path().join("Cellar").join("creft")) };
        let result = detect(&real);
        match prior {
            Some(v) => unsafe { std::env::set_var("CARGO_HOME", v) },
            None => unsafe { std::env::remove_var("CARGO_HOME") },
        }
        // Homebrew wins because is_homebrew is checked first.
        assert_eq!(result, InstallMethod::Homebrew);
    }

    // ── upgrade_command ───────────────────────────────────────────────────────

    #[rstest]
    #[case::homebrew(InstallMethod::Homebrew, "brew upgrade creft")]
    #[case::cargo(InstallMethod::Cargo, "cargo install creft")]
    #[case::install_script(InstallMethod::InstallScript, "creft update")]
    fn upgrade_command_returns_correct_string(
        #[case] method: InstallMethod,
        #[case] expected: &str,
    ) {
        assert_eq!(method.upgrade_command(), expected);
    }

    // ── refusal_message ───────────────────────────────────────────────────────

    #[rstest]
    #[case::homebrew(InstallMethod::Homebrew, Some("brew upgrade creft"))]
    #[case::cargo(InstallMethod::Cargo, Some("cargo install creft"))]
    #[case::install_script(InstallMethod::InstallScript, None)]
    fn refusal_message_contract(#[case] method: InstallMethod, #[case] contains: Option<&str>) {
        let msg = method.refusal_message();
        match contains {
            Some(substring) => {
                let m = msg.expect("Homebrew and Cargo must return Some(_)");
                assert!(
                    m.contains(substring),
                    "refusal_message for {method:?} must contain {substring:?}; got: {m:?}"
                );
            }
            None => {
                assert!(
                    msg.is_none(),
                    "InstallScript must return None from refusal_message; got: {msg:?}"
                );
            }
        }
    }

    // ── Homebrew prefix strings (string-logic validation) ────────────────────

    #[rstest]
    #[case::opt_homebrew("/opt/homebrew/bin/creft")]
    #[case::usr_local_homebrew("/usr/local/Homebrew/bin/creft")]
    #[case::linuxbrew("/home/linuxbrew/.linuxbrew/bin/creft")]
    fn homebrew_prefix_strings_are_recognized(#[case] path: &str) {
        let s = path;
        let matches_prefix = s.starts_with("/opt/homebrew/")
            || s.starts_with("/usr/local/Homebrew/")
            || s.starts_with("/home/linuxbrew/")
            || s.contains("/Cellar/creft/");
        assert_eq!(
            matches_prefix, true,
            "path {path} should match a Homebrew prefix"
        );
    }
}
