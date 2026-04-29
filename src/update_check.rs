//! Shared HTTP path and daily-check subsystem for creft version resolution.
//!
//! This module has two surfaces:
//!
//! - **HTTP helpers** (`fetch_latest`, `endpoint`, `user_agent`, `LatestResponse`):
//!   shared by the synchronous `cmd::update` command and the background check child.
//! - **Daily-check machinery** (`maybe_fire_daily`, `cmd_check`, `UpdateStatus`):
//!   dispatched from `dispatch()` and the hidden `_creft check` arm respectively.
//!
//! Keeping everything in one module avoids duplicating the HTTP client setup and
//! lets the date/file helpers be tested without exposing them beyond this crate.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::error::CreftError;
use crate::model::AppContext;

/// JSON payload shape returned by `https://creft.run/latest`.
///
/// `tarball_url` and `checksum_url` may be absent when the worker could not
/// derive platform-specific URLs (no User-Agent, malformed User-Agent, or
/// unsupported platform). The synchronous updater handles this by letting the
/// install script perform its own platform detection; these fields are
/// preserved in the struct so future callers can use them without a schema
/// change.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct LatestResponse {
    pub version: String,
    /// The full release tag, e.g. `"creft-v0.5.1"`. Reserved for callers
    /// that need the exact tag name (e.g. constructing asset URLs directly).
    #[allow(dead_code)]
    pub tag: String,
    /// Platform-specific tarball URL. Empty when the worker could not derive
    /// a target triple from the request's User-Agent.
    #[serde(default)]
    #[allow(dead_code)]
    pub tarball_url: String,
    /// SHA-256 checksum file URL. Empty when `tarball_url` is empty.
    #[serde(default)]
    #[allow(dead_code)]
    pub checksum_url: String,
}

/// Return the endpoint URL for the latest-version GET.
///
/// Reads `CREFT_UPDATE_ENDPOINT` when set; falls back to
/// `https://creft.run/latest`. The env var is a development and integration-test
/// seam — tests bind a `TcpListener` to `127.0.0.1:0` and export the listener
/// address via this var so the HTTP path can be exercised end-to-end without
/// hitting the production endpoint.
///
/// The env var is honored in both debug and release builds; there is no
/// `cfg!(debug_assertions)` gate. Anyone with env-var rights on the user's
/// machine can already replace the binary, intercept DNS, or rewrite the
/// config, so gating the seam behind a build flag adds complexity without
/// security benefit.
pub(crate) fn endpoint() -> String {
    std::env::var("CREFT_UPDATE_ENDPOINT").unwrap_or_else(|_| "https://creft.run/latest".into())
}

/// Build the canonical User-Agent string: `creft/<version> (<os>; <arch>)`.
///
/// Uses `CARGO_PKG_VERSION` (baked in at build time), `os_string()` (which
/// maps Rust's `"macos"` to the install-script convention `"darwin"`), and
/// `std::env::consts::ARCH`.
pub(crate) fn user_agent() -> String {
    format!(
        "creft/{} ({}; {})",
        env!("CARGO_PKG_VERSION"),
        os_string(),
        std::env::consts::ARCH
    )
}

/// Map `std::env::consts::OS` to the install-script OS naming convention.
///
/// Rust reports `"macos"` for Apple platforms; the install script
/// (`scripts/install.sh`) reports `"darwin"` (from `uname -s`). The
/// Cloudflare Worker's `parseUserAgent` regex and `targetTriple` mapper
/// are written against the install-script convention. Without this bridge,
/// the Analytics Engine `os` axis would record `"macos"` for binary requests
/// and `"darwin"` for install-script requests from the same machine.
///
/// All non-macOS values pass through unchanged.
pub(crate) fn os_string() -> &'static str {
    match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    }
}

/// Fetch the latest version from the configured endpoint with the given timeout.
///
/// This is the single HTTP path used by both the interactive command and the
/// background check child — keeping all agent construction, header setting, and
/// error mapping here ensures both callers stay in sync when the wire format
/// changes.
///
/// # Errors
///
/// - `CreftError::Network` — any ureq transport error, HTTP status code outside
///   2xx, or JSON parse failure.
fn fetch_latest_with_timeout(timeout_secs: u64) -> Result<LatestResponse, CreftError> {
    let agent = ureq::Agent::new_with_config(
        ureq::config::Config::builder()
            .timeout_global(Some(Duration::from_secs(timeout_secs)))
            .build(),
    );

    let url = endpoint();
    let ua = user_agent();

    let mut response = agent
        .get(&url)
        .header("user-agent", &ua)
        .call()
        .map_err(|e| match e {
            ureq::Error::StatusCode(code) => {
                CreftError::Network(format!("{url} returned HTTP {code}"))
            }
            other => CreftError::Network(other.to_string()),
        })?;

    let body = response
        .body_mut()
        .read_to_string()
        .map_err(|e| CreftError::Network(format!("failed to read response from {url}: {e}")))?;

    serde_json::from_str::<LatestResponse>(&body)
        .map_err(|e| CreftError::Network(format!("malformed response from {url}: {e}")))
}

/// Fetch the latest version from the configured endpoint.
///
/// Uses a 5-second global timeout, appropriate for interactive commands where
/// the user is waiting. The background check child uses a shorter timeout via
/// [`fetch_latest_with_timeout`] directly.
///
/// # Errors
///
/// - `CreftError::Network` — any ureq transport error, HTTP status code outside
///   2xx, or JSON parse failure.
pub(crate) fn fetch_latest() -> Result<LatestResponse, CreftError> {
    fetch_latest_with_timeout(5)
}

// ── Persisted update status ────────────────────────────────────────────────

/// On-disk shape of `~/.creft/.update-status`.
///
/// Written by `cmd_check` after a successful fetch. Read by
/// `update_notice::print_if_pending` to decide whether to surface a notice.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct UpdateStatus {
    /// Latest version reported by the worker at `checked_at`.
    pub latest_version: String,
    /// UTC date the check ran (`YYYY-MM-DD`).
    pub checked_at: String,
    /// Flipped to `true` after the notice has been shown for this `latest_version`.
    /// Reset to `false` whenever a check records a newer version.
    pub notice_shown: bool,
}

// ── Path helpers ───────────────────────────────────────────────────────────

/// Path to the per-user check timestamp: `~/.creft/.last-check`.
///
/// Uses `resolve_root(Scope::Global)` so `CREFT_HOME` redirects bookkeeping
/// into the isolated test directory when running under the integration-test
/// harness.
fn last_check_path(ctx: &AppContext) -> Result<PathBuf, CreftError> {
    Ok(ctx
        .resolve_root(crate::model::Scope::Global)?
        .join(".last-check"))
}

/// Path to the per-user update status: `~/.creft/.update-status`.
///
/// Uses `resolve_root(Scope::Global)` for the same reason as `last_check_path`.
pub(crate) fn status_path(ctx: &AppContext) -> Result<PathBuf, CreftError> {
    Ok(ctx
        .resolve_root(crate::model::Scope::Global)?
        .join(".update-status"))
}

// ── Date computation (no chrono / time dependency) ─────────────────────────

/// Today's UTC date as `YYYY-MM-DD`.
///
/// Derives the date from `SystemTime::now()` without any external dependency.
fn today_utc() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    date_from_epoch_seconds(secs)
}

/// Convert Unix epoch seconds to `YYYY-MM-DD` (UTC).
///
/// Uses the Howard Hinnant date algorithm, a pure-arithmetic conversion that
/// maps days-since-epoch to (year, month, day) without lookup tables or libc.
///
/// Reference: <https://howardhinnant.github.io/date_algorithms.html>
///
/// Exposed as `pub(crate)` so tests can pin it against known epoch values.
pub(crate) fn date_from_epoch_seconds(secs: u64) -> String {
    // Days since 1970-01-01.
    let z = (secs / 86400) as i64 + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{:04}-{:02}-{:02}", y, m, d)
}

// ── Last-check file helpers ────────────────────────────────────────────────

/// Read `.last-check`. Returns `None` if the file is missing, empty, or unreadable.
fn read_last_check(path: &Path) -> Option<String> {
    let s = std::fs::read_to_string(path).ok()?;
    let trimmed = s.trim().to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

/// Write `today` to `.last-check`, creating parent directories as needed.
fn write_last_check(path: &Path, today: &str) -> Result<(), CreftError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, today)?;
    Ok(())
}

// ── Daily-check dispatch hook ──────────────────────────────────────────────

/// Returns `true` when the daily background check and deferred notice should
/// be suppressed because of the CI environment.
///
/// The policy is:
///
/// - If `$CI` is truthy (`"true"` or `"1"`, trimmed, lowercase) AND
/// - `$CREFT_FORCE_CHECK` is *not* truthy (same parsing),
///
/// then return `true`. Otherwise return `false`.
///
/// `$CREFT_FORCE_CHECK` is an internal test seam used by the project's own
/// integration tests, which run under `cargo nextest` on GitHub Actions
/// (where the runner sets `CI=true`). Tests that exercise the daily-check
/// or deferred-notice path opt in by setting `CREFT_FORCE_CHECK=1` on the
/// spawned command. Tests that verify the CI guard itself do *not* set the
/// override and exercise the real CI path.
///
/// Used by [`maybe_fire_daily`] and by [`crate::update_notice::print_if_pending`].
pub(crate) fn ci_skip_active() -> bool {
    is_ci() && !is_force_check()
}

/// Read `$CI` and return `true` for truthy values (`"true"`, `"1"`, trimmed).
fn is_ci() -> bool {
    matches!(
        std::env::var("CI").ok().as_deref().map(str::trim),
        Some("true" | "1")
    )
}

/// Read `$CREFT_FORCE_CHECK` and return `true` for truthy values (same parsing).
fn is_force_check() -> bool {
    matches!(
        std::env::var("CREFT_FORCE_CHECK")
            .ok()
            .as_deref()
            .map(str::trim),
        Some("true" | "1")
    )
}

/// Check whether to fire the daily background check and, if so, spawn it.
///
/// Fires at most once per UTC day per install. Returns immediately — the
/// spawned child runs to completion in the background; the parent never waits.
///
/// Precondition chain (first failure is a silent no-op):
///
/// 1. Welcome marker must exist — the user has seen the telemetry disclosure.
/// 2. `$CI` must not be truthy (unless overridden by `$CREFT_FORCE_CHECK`) —
///    suppresses the check in CI environments to avoid bot traffic.
/// 3. `telemetry` must not be `"off"`.
/// 4. `.last-check` must not already equal today's UTC date.
/// 5. Write today's date to `.last-check` (records the attempt before spawning).
/// 6. `CREFT_HOME` must not be set — suppresses the spawn in test environments.
/// 7. `std::env::current_exe()` must succeed.
/// 8. Spawn `creft _creft check` with all stdio handles redirected to `/dev/null`.
pub(crate) fn maybe_fire_daily(ctx: &AppContext) {
    // Step 1: welcome marker guard.
    //
    // Using resolve_root(Scope::Global) rather than global_root() so that
    // CREFT_HOME (set in integration-test fixtures) redirects the marker lookup
    // to the isolated test directory.
    let marker_path = match ctx.resolve_root(crate::model::Scope::Global) {
        Ok(root) => root.join(crate::cmd::welcome::WELCOME_MARKER_FILENAME),
        Err(_) => return,
    };
    if !marker_path.exists() {
        return;
    }

    // Step 2: CI guard — skip before writing .last-check so CI produces zero
    // side effects on ~/.creft/ (important for self-hosted runners that mount
    // the developer's $HOME).
    if ci_skip_active() {
        return;
    }

    // Step 3: telemetry setting guard.
    if let Ok(path) = ctx.settings_path()
        && let Ok(settings) = crate::settings::Settings::load(&path)
        && settings.get("telemetry") == Some("off")
    {
        return;
    }

    // Step 4: daily debounce.
    let today = today_utc();
    let check_path = match last_check_path(ctx) {
        Ok(p) => p,
        Err(_) => return,
    };
    if read_last_check(&check_path).as_deref() == Some(today.as_str()) {
        return;
    }

    // Step 5: write today's date — records the attempt regardless of spawn outcome.
    if write_last_check(&check_path, &today).is_err() {
        return;
    }

    // Step 6: CREFT_HOME guard — suppress spawn in test environments.
    if ctx.creft_home.is_some() {
        return;
    }

    // Step 7: resolve the current executable.
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(_) => return,
    };

    // Step 8: fire-and-forget. The `let _ =` discards the spawn Result on purpose:
    // a failed spawn means no child runs, and the user's command proceeds normally.
    let _ = std::process::Command::new(&exe)
        .args(["_creft", "check"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

// ── Background check entry point ───────────────────────────────────────────

/// Hidden `_creft check` entry point.
///
/// Fetches the latest version and writes the result to `~/.creft/.update-status`.
///
/// Always returns `Ok(())`. Every internal error path — network failure,
/// status-path resolution failure, JSON serialization failure, write failure —
/// is silently swallowed. The fire-and-forget contract is "the child either
/// records a result or does not"; a non-zero exit code would surface internal
/// error state to consumers that can do nothing useful with it.
pub(crate) fn cmd_check(ctx: &AppContext) -> Result<(), CreftError> {
    // The background child uses a 3-second timeout — shorter than the interactive
    // path (5s) to match the validate.rs convention and bound the child's lifetime.
    let result: Result<(), ()> = (|| {
        let latest = fetch_latest_with_timeout(3).map_err(|_| ())?;
        let path = status_path(ctx).map_err(|_| ())?;
        write_status_atomic(&path, &latest.version).map_err(|_| ())?;
        Ok(())
    })();
    let _ = result;
    Ok(())
}

/// Write a new `UpdateStatus` to `path` using a temp-file + rename pattern.
///
/// The rename is atomic on all POSIX systems, so `print_if_pending` never
/// reads a partial file. The temp file is created in the same directory as
/// `path` so the rename stays on the same filesystem.
fn write_status_atomic(path: &Path, latest_version: &str) -> Result<(), CreftError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let status = UpdateStatus {
        latest_version: latest_version.to_string(),
        checked_at: today_utc(),
        notice_shown: false,
    };
    let json =
        serde_json::to_string(&status).map_err(|e| CreftError::Serialization(e.to_string()))?;

    // Write to a temp file in the same directory, then rename.
    let tmp_path = path.with_extension("tmp");
    std::fs::write(&tmp_path, &json)?;
    std::fs::rename(&tmp_path, path)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use rstest::rstest;
    use serial_test::serial;

    use super::*;

    // ── endpoint() ───────────────────────────────────────────────────────────

    #[test]
    fn endpoint_returns_default_when_env_var_unset() {
        // SAFETY: single-threaded test context; no other thread reads this var.
        unsafe { std::env::remove_var("CREFT_UPDATE_ENDPOINT") };
        assert_eq!(endpoint(), "https://creft.run/latest");
    }

    #[test]
    fn endpoint_returns_env_var_when_set() {
        // SAFETY: single-threaded test context; no other thread reads this var.
        unsafe { std::env::set_var("CREFT_UPDATE_ENDPOINT", "http://127.0.0.1:9999/latest") };
        let result = endpoint();
        // SAFETY: single-threaded test context.
        unsafe { std::env::remove_var("CREFT_UPDATE_ENDPOINT") };
        assert_eq!(result, "http://127.0.0.1:9999/latest");
    }

    // ── os_string() ──────────────────────────────────────────────────────────

    #[cfg(target_os = "macos")]
    #[test]
    fn os_string_maps_macos_to_darwin() {
        assert_eq!(os_string(), "darwin");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn os_string_passes_linux_through() {
        assert_eq!(os_string(), "linux");
    }

    // ── user_agent() ─────────────────────────────────────────────────────────

    #[test]
    fn user_agent_matches_expected_format() {
        let ua = user_agent();
        // Must start with "creft/" and contain the version from Cargo.toml.
        assert!(
            ua.starts_with("creft/"),
            "user_agent must start with 'creft/': {ua:?}"
        );
        assert!(
            ua.contains(env!("CARGO_PKG_VERSION")),
            "user_agent must include CARGO_PKG_VERSION: {ua:?}"
        );
        // Must contain parenthesized (os; arch) section.
        assert!(
            ua.contains('(') && ua.contains(')') && ua.contains(';'),
            "user_agent must contain '(<os>; <arch>)': {ua:?}"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn user_agent_uses_darwin_on_macos() {
        let ua = user_agent();
        assert!(
            ua.contains("darwin"),
            "user_agent on macOS must contain 'darwin', not 'macos': {ua:?}"
        );
        assert!(
            !ua.contains("macos"),
            "user_agent on macOS must not contain 'macos': {ua:?}"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn user_agent_uses_linux_on_linux() {
        let ua = user_agent();
        assert!(
            ua.contains("linux"),
            "user_agent on Linux must contain 'linux': {ua:?}"
        );
    }

    // ── LatestResponse deserialization ───────────────────────────────────────

    #[test]
    fn latest_response_deserializes_full_payload() {
        let json = r#"{
            "version": "0.5.1",
            "tag": "creft-v0.5.1",
            "tarball_url": "https://example.com/creft.tar.gz",
            "checksum_url": "https://example.com/creft.tar.gz.sha256"
        }"#;
        let resp: LatestResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.version, "0.5.1");
        assert_eq!(resp.tag, "creft-v0.5.1");
        assert_eq!(resp.tarball_url, "https://example.com/creft.tar.gz");
        assert_eq!(resp.checksum_url, "https://example.com/creft.tar.gz.sha256");
    }

    #[test]
    fn latest_response_defaults_optional_fields_to_empty_string() {
        // Worker omits platform fields when UA does not parse.
        let json = r#"{"version": "0.5.1", "tag": "creft-v0.5.1"}"#;
        let resp: LatestResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.version, "0.5.1");
        assert_eq!(resp.tag, "creft-v0.5.1");
        assert_eq!(
            resp.tarball_url, "",
            "missing tarball_url must default to empty string"
        );
        assert_eq!(
            resp.checksum_url, "",
            "missing checksum_url must default to empty string"
        );
    }

    // ── os_string() exhaustive mapping ───────────────────────────────────────

    #[rstest]
    #[case::freebsd("freebsd", "freebsd")]
    fn os_string_passthrough(#[case] input: &str, #[case] expected: &str) {
        // We can't call os_string() for non-host platforms, but we can verify
        // the match logic directly.
        let result = match input {
            "macos" => "darwin",
            other => other,
        };
        assert_eq!(result, expected);
    }

    // ── date_from_epoch_seconds ───────────────────────────────────────────────

    #[rstest]
    #[case::epoch(0, "1970-01-01")]
    #[case::y2k(946684800, "2000-01-01")]
    #[case::y2025_jan01(1735689600, "2025-01-01")]
    #[case::y2026_dec31(1798761599, "2026-12-31")]
    #[case::before_midnight(86400 - 1, "1970-01-01")]
    #[case::after_midnight(86400, "1970-01-02")]
    fn date_from_epoch_seconds_produces_correct_date(#[case] secs: u64, #[case] expected: &str) {
        assert_eq!(date_from_epoch_seconds(secs), expected);
    }

    // ── today_utc ─────────────────────────────────────────────────────────────

    #[test]
    fn today_utc_matches_date_format() {
        let today = today_utc();
        assert!(
            regex::Regex::new(r"^\d{4}-\d{2}-\d{2}$")
                .unwrap()
                .is_match(&today),
            "today_utc must match YYYY-MM-DD: {today:?}"
        );
    }

    // ── read_last_check / write_last_check ────────────────────────────────────

    /// `None` means do not pre-create the file; `Some(content)` writes `content`.
    #[rstest]
    #[case::missing(None)]
    #[case::empty(Some(""))]
    #[case::whitespace(Some("   \n"))]
    fn read_last_check_returns_none_for_blank_or_absent(#[case] content: Option<&str>) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".last-check");
        if let Some(c) = content {
            std::fs::write(&path, c).unwrap();
        }
        assert!(read_last_check(&path).is_none());
    }

    #[test]
    fn read_last_check_returns_none_for_non_date_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".last-check");
        std::fs::write(&path, "not a date").unwrap();
        // Non-date content is returned as-is; the comparison in maybe_fire_daily
        // will simply not match today's date.
        assert_eq!(read_last_check(&path), Some("not a date".into()));
    }

    #[test]
    fn write_last_check_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".last-check");
        write_last_check(&path, "2026-04-28").unwrap();
        assert_eq!(read_last_check(&path), Some("2026-04-28".into()));
    }

    #[test]
    fn write_last_check_creates_parent_directories() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("dirs").join(".last-check");
        write_last_check(&path, "2026-04-28").unwrap();
        assert!(path.exists());
    }

    // ── Shared helpers for maybe_fire_daily tests ─────────────────────────────

    fn make_creft_home_ctx(dir: &tempfile::TempDir) -> AppContext {
        AppContext::for_test_with_creft_home(dir.path().to_path_buf(), dir.path().to_path_buf())
    }

    fn write_welcome_marker(dir: &tempfile::TempDir) {
        let marker = dir
            .path()
            .join(crate::cmd::welcome::WELCOME_MARKER_FILENAME);
        std::fs::write(marker, "").unwrap();
    }

    fn write_telemetry_off(dir: &tempfile::TempDir) {
        // settings.json in the CREFT_HOME root (since creft_home overrides global).
        let path = dir.path().join("settings.json");
        std::fs::write(path, r#"{"telemetry":"off"}"#).unwrap();
    }

    // ── is_ci ─────────────────────────────────────────────────────────────────

    #[rstest]
    #[case::ci_true("true", true)]
    #[case::ci_one("1", true)]
    #[case::ci_false("false", false)]
    #[case::ci_zero("0", false)]
    #[case::ci_empty("", false)]
    #[case::ci_uppercase_true("True", false)]
    #[case::ci_all_caps("TRUE", false)]
    #[serial(env_ci_skip)]
    fn is_ci_parses_truthy_values(#[case] value: &str, #[case] expected: bool) {
        let prior = std::env::var("CI").ok();
        // SAFETY: single-threaded test context; no other thread reads this var.
        unsafe { std::env::set_var("CI", value) };
        let result = is_ci();
        match prior {
            Some(v) => unsafe { std::env::set_var("CI", v) },
            None => unsafe { std::env::remove_var("CI") },
        }
        assert_eq!(
            result, expected,
            "is_ci() with CI={value:?} must be {expected}"
        );
    }

    #[test]
    #[serial(env_ci_skip)]
    fn is_ci_returns_false_when_unset() {
        let prior = std::env::var("CI").ok();
        // SAFETY: single-threaded test context; no other thread reads this var.
        unsafe { std::env::remove_var("CI") };
        let result = is_ci();
        match prior {
            Some(v) => unsafe { std::env::set_var("CI", v) },
            None => unsafe { std::env::remove_var("CI") },
        }
        assert!(!result, "is_ci() must be false when CI is unset");
    }

    #[test]
    #[serial(env_ci_skip)]
    fn is_ci_ignores_vendor_specific_vars() {
        // Only $CI should trigger the guard; vendor-specific vars like
        // GITHUB_ACTIONS or BUILDKITE must not.
        let prior_ci = std::env::var("CI").ok();
        let prior_ga = std::env::var("GITHUB_ACTIONS").ok();
        // SAFETY: single-threaded test context; no other thread reads this var.
        unsafe { std::env::remove_var("CI") };
        unsafe { std::env::set_var("GITHUB_ACTIONS", "true") };
        let result = is_ci();
        match prior_ci {
            Some(v) => unsafe { std::env::set_var("CI", v) },
            None => unsafe { std::env::remove_var("CI") },
        }
        match prior_ga {
            Some(v) => unsafe { std::env::set_var("GITHUB_ACTIONS", v) },
            None => unsafe { std::env::remove_var("GITHUB_ACTIONS") },
        }
        assert!(!result, "is_ci() must not fire on GITHUB_ACTIONS alone");
    }

    // ── is_force_check ────────────────────────────────────────────────────────

    #[rstest]
    #[case::force_true("true", true)]
    #[case::force_one("1", true)]
    #[case::force_false("false", false)]
    #[case::force_empty("", false)]
    #[serial(env_force_check)]
    fn is_force_check_parses_truthy_values(#[case] value: &str, #[case] expected: bool) {
        let prior = std::env::var("CREFT_FORCE_CHECK").ok();
        // SAFETY: single-threaded test context; no other thread reads this var.
        unsafe { std::env::set_var("CREFT_FORCE_CHECK", value) };
        let result = is_force_check();
        match prior {
            Some(v) => unsafe { std::env::set_var("CREFT_FORCE_CHECK", v) },
            None => unsafe { std::env::remove_var("CREFT_FORCE_CHECK") },
        }
        assert_eq!(
            result, expected,
            "is_force_check() with CREFT_FORCE_CHECK={value:?} must be {expected}"
        );
    }

    #[test]
    #[serial(env_force_check)]
    fn is_force_check_returns_false_when_unset() {
        let prior = std::env::var("CREFT_FORCE_CHECK").ok();
        // SAFETY: single-threaded test context; no other thread reads this var.
        unsafe { std::env::remove_var("CREFT_FORCE_CHECK") };
        let result = is_force_check();
        match prior {
            Some(v) => unsafe { std::env::set_var("CREFT_FORCE_CHECK", v) },
            None => unsafe { std::env::remove_var("CREFT_FORCE_CHECK") },
        }
        assert!(
            !result,
            "is_force_check() must be false when CREFT_FORCE_CHECK is unset"
        );
    }

    // ── ci_skip_active truth table ────────────────────────────────────────────

    #[rstest]
    // CI=unset, FORCE=unset → false
    #[case::both_unset(None, None, false)]
    // CI=true, FORCE=unset → true (CI guard fires)
    #[case::ci_true_no_force(Some("true"), None, true)]
    // CI=1, FORCE=unset → true
    #[case::ci_one_no_force(Some("1"), None, true)]
    // CI=false, FORCE=unset → false (escape hatch honored)
    #[case::ci_false_no_force(Some("false"), None, false)]
    // CI=true, FORCE=1 → false (override suppresses guard)
    #[case::ci_true_force_one(Some("true"), Some("1"), false)]
    // CI=true, FORCE=true → false
    #[case::ci_true_force_true(Some("true"), Some("true"), false)]
    // CI=true, FORCE=0 → true (non-truthy override does not disable guard)
    #[case::ci_true_force_zero(Some("true"), Some("0"), true)]
    // CI=true, FORCE=false → true
    #[case::ci_true_force_false(Some("true"), Some("false"), true)]
    // CI=true, FORCE="" → true (empty override does not disable guard)
    #[case::ci_true_force_empty(Some("true"), Some(""), true)]
    // CI=unset, FORCE=1 → false (force-check without CI is a no-op)
    #[case::ci_unset_force_one(None, Some("1"), false)]
    #[serial(env_ci_skip)]
    fn ci_skip_active_truth_table(
        #[case] ci: Option<&str>,
        #[case] force: Option<&str>,
        #[case] expected: bool,
    ) {
        let prior_ci = std::env::var("CI").ok();
        let prior_force = std::env::var("CREFT_FORCE_CHECK").ok();
        // SAFETY: single-threaded test context; no other thread reads these vars.
        match ci {
            Some(v) => unsafe { std::env::set_var("CI", v) },
            None => unsafe { std::env::remove_var("CI") },
        }
        match force {
            Some(v) => unsafe { std::env::set_var("CREFT_FORCE_CHECK", v) },
            None => unsafe { std::env::remove_var("CREFT_FORCE_CHECK") },
        }
        let result = ci_skip_active();
        match prior_ci {
            Some(v) => unsafe { std::env::set_var("CI", v) },
            None => unsafe { std::env::remove_var("CI") },
        }
        match prior_force {
            Some(v) => unsafe { std::env::set_var("CREFT_FORCE_CHECK", v) },
            None => unsafe { std::env::remove_var("CREFT_FORCE_CHECK") },
        }
        assert_eq!(
            result, expected,
            "ci_skip_active() with CI={ci:?}, CREFT_FORCE_CHECK={force:?} must be {expected}"
        );
    }

    // ── maybe_fire_daily precondition behavior ────────────────────────────────
    //
    // All cases set CREFT_HOME (via for_test_with_creft_home) which suppresses
    // the spawn but allows the .last-check write to be observable.
    //
    // Every case except CiTrue sets CREFT_FORCE_CHECK=1 to neutralize the CI
    // guard — this test runs under cargo nextest on GitHub Actions where
    // CI=true is set by the runner, and we want each arm to test its own
    // precondition rather than the CI guard.

    /// Setup actions applied before calling `maybe_fire_daily` in each case.
    enum DailyCheckSetup {
        /// No marker, no settings — welcome guard fires, .last-check must not be written.
        WelcomeAbsent,
        /// Marker present, CI=true, no CREFT_FORCE_CHECK — CI guard fires, .last-check must not be written.
        CiTrue,
        /// Marker present, telemetry=off — telemetry guard fires, .last-check must not be written.
        TelemetryOff,
        /// Marker present, .last-check already contains today — debounce fires, file must not change.
        AlreadyCheckedToday,
        /// Marker present, .last-check has a stale date — debounce passes, file must be updated.
        StaleLastCheck,
        /// Marker present, no .last-check file — first-run path, file must be created with today.
        LastCheckAbsent,
    }

    #[rstest]
    #[case::welcome_absent(DailyCheckSetup::WelcomeAbsent)]
    #[case::ci_true(DailyCheckSetup::CiTrue)]
    #[case::telemetry_off(DailyCheckSetup::TelemetryOff)]
    #[case::already_checked_today(DailyCheckSetup::AlreadyCheckedToday)]
    #[case::stale_last_check(DailyCheckSetup::StaleLastCheck)]
    #[case::last_check_absent(DailyCheckSetup::LastCheckAbsent)]
    #[serial(env_ci_skip)]
    fn maybe_fire_daily_precondition_chain(#[case] setup: DailyCheckSetup) {
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_creft_home_ctx(&dir);
        let check_path = dir.path().join(".last-check");

        // Save both env vars for restore.
        let prior_ci = std::env::var("CI").ok();
        let prior_force = std::env::var("CREFT_FORCE_CHECK").ok();

        // Default: neutralize the CI guard so each arm tests its own precondition.
        // The CiTrue arm overrides this to exercise the real CI path.
        let (set_ci, set_force) = match &setup {
            DailyCheckSetup::CiTrue => (Some("true"), None),
            _ => (None, Some("1")),
        };
        // SAFETY: single-threaded test context; no other thread reads these vars.
        match set_ci {
            Some(v) => unsafe { std::env::set_var("CI", v) },
            None => unsafe { std::env::remove_var("CI") },
        }
        match set_force {
            Some(v) => unsafe { std::env::set_var("CREFT_FORCE_CHECK", v) },
            None => unsafe { std::env::remove_var("CREFT_FORCE_CHECK") },
        }

        match &setup {
            DailyCheckSetup::WelcomeAbsent => {
                // No marker — function must short-circuit before writing .last-check.
            }
            DailyCheckSetup::CiTrue => {
                write_welcome_marker(&dir);
                // CI=true is set above; CREFT_FORCE_CHECK is removed above.
            }
            DailyCheckSetup::TelemetryOff => {
                write_welcome_marker(&dir);
                write_telemetry_off(&dir);
            }
            DailyCheckSetup::AlreadyCheckedToday => {
                write_welcome_marker(&dir);
                let today = today_utc();
                std::fs::write(&check_path, &today).unwrap();
            }
            DailyCheckSetup::StaleLastCheck => {
                write_welcome_marker(&dir);
                std::fs::write(&check_path, "2000-01-01").unwrap();
            }
            DailyCheckSetup::LastCheckAbsent => {
                write_welcome_marker(&dir);
            }
        }

        // For the debounce case, record mtime before the call.
        let mtime_before = if matches!(setup, DailyCheckSetup::AlreadyCheckedToday) {
            Some(std::fs::metadata(&check_path).unwrap().modified().unwrap())
        } else {
            None
        };

        maybe_fire_daily(&ctx);

        // Restore env vars.
        match prior_ci {
            Some(v) => unsafe { std::env::set_var("CI", v) },
            None => unsafe { std::env::remove_var("CI") },
        }
        match prior_force {
            Some(v) => unsafe { std::env::set_var("CREFT_FORCE_CHECK", v) },
            None => unsafe { std::env::remove_var("CREFT_FORCE_CHECK") },
        }

        match setup {
            DailyCheckSetup::WelcomeAbsent => {
                assert!(
                    !check_path.exists(),
                    ".last-check must not be created when welcome marker is absent"
                );
            }
            DailyCheckSetup::CiTrue => {
                assert!(
                    !check_path.exists(),
                    ".last-check must not be created when CI=true (CI guard fires before write)"
                );
            }
            DailyCheckSetup::TelemetryOff => {
                assert!(
                    !check_path.exists(),
                    ".last-check must not be created when telemetry=off"
                );
            }
            DailyCheckSetup::AlreadyCheckedToday => {
                let mtime_after = std::fs::metadata(&check_path).unwrap().modified().unwrap();
                assert_eq!(
                    mtime_before.unwrap(),
                    mtime_after,
                    ".last-check must not be rewritten when already checked today"
                );
            }
            DailyCheckSetup::StaleLastCheck | DailyCheckSetup::LastCheckAbsent => {
                let written = read_last_check(&check_path);
                let today = today_utc();
                assert_eq!(
                    written,
                    Some(today.clone()),
                    ".last-check must contain today's date; got {written:?}, expected {today:?}"
                );
            }
        }
    }

    // ── write_status_atomic ───────────────────────────────────────────────────

    #[test]
    fn write_status_atomic_creates_valid_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".update-status");
        write_status_atomic(&path, "1.2.3").unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        let status: UpdateStatus = serde_json::from_str(&content).unwrap();
        assert_eq!(status.latest_version, "1.2.3");
        assert!(!status.notice_shown);
        assert!(
            regex::Regex::new(r"^\d{4}-\d{2}-\d{2}$")
                .unwrap()
                .is_match(&status.checked_at),
            "checked_at must match YYYY-MM-DD: {:?}",
            status.checked_at
        );
    }

    #[test]
    fn write_status_atomic_creates_parent_directories() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("subdir").join(".update-status");
        write_status_atomic(&path, "0.1.0").unwrap();
        assert!(path.exists());
    }

    #[test]
    fn write_status_atomic_leaves_no_tmp_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".update-status");
        write_status_atomic(&path, "0.1.0").unwrap();
        let tmp = path.with_extension("tmp");
        assert!(!tmp.exists(), "temp file must be renamed away: {tmp:?}");
    }
}
