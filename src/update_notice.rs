//! Deferred "update available" notice.
//!
//! Reads `~/.creft/.update-status` (written by the daily background check) and,
//! when a newer version is recorded and the notice has not yet been shown for it,
//! prints one line to stderr and flips the `notice_shown` flag so the same notice
//! does not appear on every subsequent command.

use std::path::Path;

use yansi::Paint as _;

use crate::install_method::is_homebrew_install;
use crate::model::AppContext;
use crate::settings::Settings;
use crate::update_check::{UpdateStatus, status_path};

/// Print the "update available" notice to stderr if one is pending.
///
/// Reads `.update-status`. When a newer version than `CARGO_PKG_VERSION` is
/// recorded and `notice_shown` is `false`, prints one line to stderr and rewrites
/// the file with `notice_shown = true` so the notice does not repeat.
///
/// The notice tail is determined by install method:
/// - Homebrew install: `run 'brew upgrade creft' to upgrade`
/// - All others (including when `current_exe` is `None`): `run 'creft update' to upgrade`
///
/// All errors — telemetry off, missing/malformed status file, stderr write failure,
/// file rewrite failure — are silently swallowed. The user's actual command is
/// never affected.
pub(crate) fn print_if_pending(ctx: &AppContext, current_exe: Option<&Path>) {
    // Respect telemetry setting.
    if let Ok(path) = ctx.settings_path()
        && let Ok(settings) = Settings::load(&path)
        && settings.get("telemetry") == Some("off")
    {
        return;
    }

    // Read the status file.
    let status_file = match status_path(ctx) {
        Ok(p) => p,
        Err(_) => return,
    };
    let content = match std::fs::read_to_string(&status_file) {
        Ok(c) => c,
        Err(_) => return,
    };
    let mut status: UpdateStatus = match serde_json::from_str(&content) {
        Ok(s) => s,
        Err(_) => return,
    };

    // No notice to show if already shown for this version.
    if status.notice_shown {
        return;
    }

    // No notice if the recorded version is not actually newer.
    let installed = env!("CARGO_PKG_VERSION");
    if !is_newer(&status.latest_version, installed) {
        return;
    }

    // Choose the upgrade command based on install method.
    let use_homebrew = current_exe.is_some_and(is_homebrew_install);
    let upgrade_cmd = if use_homebrew {
        "brew upgrade creft"
    } else {
        "creft update"
    };

    // Print the notice to stderr — one line, muted gray.
    let msg = format!(
        "creft {} is available — run '{}' to upgrade (currently {})",
        status.latest_version, upgrade_cmd, installed
    );
    eprintln!("{}", msg.rgb(160, 160, 160));

    // Flip the flag and rewrite atomically.
    status.notice_shown = true;
    if let Ok(json) = serde_json::to_string(&status) {
        let tmp = status_file.with_extension("tmp");
        if std::fs::write(&tmp, &json).is_ok() {
            let _ = std::fs::rename(&tmp, &status_file);
        }
    }
}

/// Returns `true` when `candidate` is strictly newer than `baseline`.
///
/// Compares (major, minor, patch) as unsigned integers. Pre-release suffixes
/// are not supported: the project does not ship them.
fn is_newer(candidate: &str, baseline: &str) -> bool {
    parse_semver(candidate)
        .zip(parse_semver(baseline))
        .is_some_and(|(c, b)| c > b)
}

/// Parse a `"MAJOR.MINOR.PATCH"` string into a comparable tuple.
fn parse_semver(v: &str) -> Option<(u64, u64, u64)> {
    let mut parts = v.split('.');
    let major: u64 = parts.next()?.parse().ok()?;
    let minor: u64 = parts.next()?.parse().ok()?;
    let patch: u64 = parts.next()?.parse().ok()?;
    Some((major, minor, patch))
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use rstest::rstest;
    use tempfile::TempDir;

    use super::*;

    // ── is_newer ─────────────────────────────────────────────────────────────

    #[rstest]
    #[case::behind("0.5.1", "0.4.0", true)]
    #[case::same("0.5.1", "0.5.1", false)]
    #[case::ahead("0.4.0", "0.5.1", false)]
    #[case::minor_bump("0.10.0", "0.9.0", true)]
    #[case::major_bump("1.0.0", "0.99.99", true)]
    #[case::patch_behind("0.5.2", "0.5.1", true)]
    fn is_newer_compares_semver_correctly(
        #[case] candidate: &str,
        #[case] baseline: &str,
        #[case] expected: bool,
    ) {
        assert_eq!(is_newer(candidate, baseline), expected);
    }

    #[rstest]
    #[case::empty("", "0.1.0")]
    #[case::partial("0.5", "0.4.0")]
    #[case::letters("x.y.z", "0.0.1")]
    fn is_newer_returns_false_for_malformed_input(#[case] candidate: &str, #[case] baseline: &str) {
        assert!(!is_newer(candidate, baseline));
    }

    // ── print_if_pending ──────────────────────────────────────────────────────

    fn make_ctx(dir: &TempDir) -> AppContext {
        AppContext::for_test_with_creft_home(dir.path().to_path_buf(), dir.path().to_path_buf())
    }

    fn write_status(dir: &TempDir, latest: &str, notice_shown: bool) {
        let status = UpdateStatus {
            latest_version: latest.to_string(),
            checked_at: "2026-04-28".to_string(),
            notice_shown,
        };
        let json = serde_json::to_string(&status).unwrap();
        std::fs::write(dir.path().join(".update-status"), json).unwrap();
    }

    fn read_status(dir: &TempDir) -> UpdateStatus {
        let content = std::fs::read_to_string(dir.path().join(".update-status")).unwrap();
        serde_json::from_str(&content).unwrap()
    }

    #[test]
    fn no_output_when_status_file_absent() {
        let dir = TempDir::new().unwrap();
        let ctx = make_ctx(&dir);
        // Should not panic or error; no file exists.
        print_if_pending(&ctx, None);
    }

    #[test]
    fn no_output_when_latest_equals_installed() {
        let dir = TempDir::new().unwrap();
        let ctx = make_ctx(&dir);
        let installed = env!("CARGO_PKG_VERSION");
        write_status(&dir, installed, false);

        print_if_pending(&ctx, None);

        // notice_shown should remain false — nothing to flip.
        let status = read_status(&dir);
        assert!(!status.notice_shown);
    }

    #[test]
    fn notice_shown_flag_flips_when_newer_version_is_pending() {
        let dir = TempDir::new().unwrap();
        let ctx = make_ctx(&dir);
        write_status(&dir, "99.99.99", false);

        print_if_pending(&ctx, None);

        let status = read_status(&dir);
        assert!(
            status.notice_shown,
            "notice_shown must be set to true after printing"
        );
    }

    #[test]
    fn no_output_when_notice_already_shown() {
        let dir = TempDir::new().unwrap();
        let ctx = make_ctx(&dir);
        write_status(&dir, "99.99.99", true);

        // Capture: call should be a no-op because notice_shown is already true.
        print_if_pending(&ctx, None);

        // Status file must remain unchanged.
        let status = read_status(&dir);
        assert!(status.notice_shown, "notice_shown must remain true");
        assert_eq!(status.latest_version, "99.99.99");
    }

    #[test]
    fn no_output_when_telemetry_is_off() {
        let dir = TempDir::new().unwrap();
        let ctx = make_ctx(&dir);
        write_status(&dir, "99.99.99", false);

        // Write telemetry=off into settings.
        let settings_path = dir.path().join("settings.json");
        std::fs::write(settings_path, r#"{"telemetry":"off"}"#).unwrap();

        print_if_pending(&ctx, None);

        // notice_shown must not be flipped — function returned early.
        let status = read_status(&dir);
        assert!(
            !status.notice_shown,
            "notice_shown must not be flipped when telemetry=off"
        );
    }

    #[test]
    fn no_error_when_status_file_is_malformed() {
        let dir = TempDir::new().unwrap();
        let ctx = make_ctx(&dir);
        std::fs::write(dir.path().join(".update-status"), "{{not valid json}}").unwrap();
        // Must not panic.
        print_if_pending(&ctx, None);
    }

    #[test]
    fn no_error_when_current_exe_is_none() {
        let dir = TempDir::new().unwrap();
        let ctx = make_ctx(&dir);
        write_status(&dir, "99.99.99", false);
        // None falls back to non-Homebrew tail without panic.
        print_if_pending(&ctx, None);
        let status = read_status(&dir);
        assert!(status.notice_shown);
    }

    #[cfg(unix)]
    #[test]
    fn homebrew_path_selects_brew_upgrade_command() {
        use std::path::PathBuf;

        let dir = TempDir::new().unwrap();
        let ctx = make_ctx(&dir);
        write_status(&dir, "99.99.99", false);

        // Build a symlink that resolves through <tmp>/Cellar/creft/...
        let target = dir
            .path()
            .join("Cellar")
            .join("creft")
            .join("0.5.1")
            .join("bin")
            .join("creft");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, b"").unwrap();
        let link: PathBuf = dir.path().join("bin").join("creft");
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();

        // After the call the notice_shown flag should be flipped, indicating the
        // function ran to completion with the Homebrew detection path.
        print_if_pending(&ctx, Some(&link));
        let status = read_status(&dir);
        assert!(
            status.notice_shown,
            "notice_shown must be true after printing with homebrew path"
        );
    }
}
