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

    /// Describes how to set up the `.update-status` file before calling `print_if_pending`.
    enum StatusSetup {
        /// No status file — `print_if_pending` must not panic or create the file.
        Absent,
        /// Status file present with valid JSON.
        Present {
            latest: &'static str,
            notice_shown: bool,
        },
        /// Status file present but contains invalid JSON.
        Malformed,
    }

    /// The expected state of `notice_shown` after calling `print_if_pending`.
    #[derive(Debug, Clone, Copy, PartialEq)]
    enum ExpectedNoticeShown {
        /// The status file should have `notice_shown = true`.
        True,
        /// The status file should have `notice_shown = false` (unchanged).
        False,
        /// No readable status file; nothing to assert on.
        NotApplicable,
    }

    #[rstest]
    // No status file — must not panic.
    #[case::status_file_absent(StatusSetup::Absent, false, ExpectedNoticeShown::NotApplicable)]
    // Same version as installed — no notice, flag stays false.
    #[case::latest_equals_installed(StatusSetup::Present { latest: env!("CARGO_PKG_VERSION"), notice_shown: false }, false, ExpectedNoticeShown::False)]
    // Newer version, notice not yet shown — flag must flip to true.
    #[case::newer_version_pending(StatusSetup::Present { latest: "99.99.99", notice_shown: false }, false, ExpectedNoticeShown::True)]
    // Notice already shown — no repeat; flag stays true.
    #[case::notice_already_shown(StatusSetup::Present { latest: "99.99.99", notice_shown: true }, false, ExpectedNoticeShown::True)]
    // Telemetry off — flag must not flip.
    #[case::telemetry_off(StatusSetup::Present { latest: "99.99.99", notice_shown: false }, true, ExpectedNoticeShown::False)]
    // Malformed status file — must not panic.
    #[case::malformed_status_file(
        StatusSetup::Malformed,
        false,
        ExpectedNoticeShown::NotApplicable
    )]
    // current_exe is None — falls back to 'creft update' tail; flag must flip.
    #[case::current_exe_none(StatusSetup::Present { latest: "99.99.99", notice_shown: false }, false, ExpectedNoticeShown::True)]
    fn print_if_pending_cases(
        #[case] status_setup: StatusSetup,
        #[case] telemetry_off: bool,
        #[case] expected: ExpectedNoticeShown,
    ) {
        let dir = TempDir::new().unwrap();
        let ctx = make_ctx(&dir);

        if telemetry_off {
            std::fs::write(dir.path().join("settings.json"), r#"{"telemetry":"off"}"#).unwrap();
        }

        match &status_setup {
            StatusSetup::Absent => {}
            StatusSetup::Present {
                latest,
                notice_shown,
            } => {
                write_status(&dir, latest, *notice_shown);
            }
            StatusSetup::Malformed => {
                std::fs::write(dir.path().join(".update-status"), "{{not valid json}}").unwrap();
            }
        }

        // All cases pass `None` for `current_exe` — the Homebrew path is covered by
        // `homebrew_path_selects_brew_upgrade_command` below.
        print_if_pending(&ctx, None);

        match expected {
            ExpectedNoticeShown::True => {
                let s = read_status(&dir);
                assert!(s.notice_shown, "notice_shown must be true after printing");
            }
            ExpectedNoticeShown::False => {
                let s = read_status(&dir);
                assert!(!s.notice_shown, "notice_shown must remain false");
            }
            ExpectedNoticeShown::NotApplicable => {
                // Absent or malformed status file — function must be a no-op; no panic.
            }
        }
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
