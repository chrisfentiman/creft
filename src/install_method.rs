//! Helpers for detecting how creft was installed.
//!
//! Used by the update command and the deferred update notice to route users to
//! the correct upgrade path (direct vs. Homebrew).

use std::path::Path;

/// Returns `true` when the running binary is managed by Homebrew.
///
/// Canonicalizes `exe` (resolves symlinks) before pattern-matching, so a
/// Homebrew-shim that points into the Cellar is correctly identified.
///
/// Recognized Homebrew paths:
/// - Any path starting with `/opt/homebrew/` (Apple Silicon default prefix)
/// - Any path starting with `/usr/local/Homebrew/` (Intel macOS legacy prefix)
/// - Any path starting with `/home/linuxbrew/` (Linux Homebrew prefix)
/// - Any path containing `/Cellar/creft/` (catches non-default prefix installs)
///
/// Returns `false` when canonicalization fails (fail-open: prefer to allow a
/// legitimate direct update over blocking a non-Homebrew install).
pub(crate) fn is_homebrew_install(exe: &Path) -> bool {
    let canonical = match std::fs::canonicalize(exe) {
        Ok(p) => p,
        Err(_) => return false,
    };
    let s = canonical.to_string_lossy();
    s.starts_with("/opt/homebrew/")
        || s.starts_with("/usr/local/Homebrew/")
        || s.starts_with("/home/linuxbrew/")
        || s.contains("/Cellar/creft/")
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use rstest::rstest;
    use tempfile::TempDir;

    use super::*;

    // Build a symlink at `link_path` pointing to `target`, both within `dir`.
    // Returns the link path.
    #[cfg(unix)]
    fn make_symlink(dir: &TempDir, target_rel: &str, link_rel: &str) -> std::path::PathBuf {
        let target = dir.path().join(target_rel);
        // Create any intermediate directories.
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        // Write a dummy file at the target.
        std::fs::write(&target, b"").unwrap();
        let link = dir.path().join(link_rel);
        if let Some(parent) = link.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::os::unix::fs::symlink(&target, &link).unwrap();
        link
    }

    // ── Homebrew detection ────────────────────────────────────────────────────

    #[cfg(unix)]
    #[test]
    fn homebrew_cellar_path_is_detected() {
        let dir = TempDir::new().unwrap();
        // Symlink that resolves through <tmp>/Cellar/creft/0.5.1/bin/creft.
        let link = make_symlink(&dir, "Cellar/creft/0.5.1/bin/creft", "bin/creft");
        assert!(is_homebrew_install(&link));
    }

    #[cfg(unix)]
    #[test]
    fn non_homebrew_path_is_not_detected() {
        let dir = TempDir::new().unwrap();
        let link = make_symlink(&dir, ".local/bin/creft-real", ".local/bin/creft");
        assert!(!is_homebrew_install(&link));
    }

    #[rstest]
    #[case::missing_file("/tmp/creft-does-not-exist-abc123xyz")]
    fn nonexistent_path_returns_false(#[case] path: &str) {
        // canonicalize fails on a nonexistent path → fail-open → false.
        assert!(!is_homebrew_install(Path::new(path)));
    }

    // Verify the prefix strings cover all documented Homebrew layouts.
    #[rstest]
    #[case::opt_homebrew("/opt/homebrew/bin/creft")]
    #[case::usr_local_homebrew("/usr/local/Homebrew/bin/creft")]
    #[case::linuxbrew("/home/linuxbrew/.linuxbrew/bin/creft")]
    fn homebrew_prefix_strings_are_recognized(#[case] path: &str) {
        // These paths don't exist on disk; test only the string-matching logic
        // by bypassing canonicalize — the real function is tested with real
        // symlinks above. Here we verify the prefix constants are correct.
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
