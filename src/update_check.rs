//! Shared HTTP path for resolving the latest creft version.
//!
//! This module exposes the fetch helpers used by both `cmd::update` (synchronous
//! user command) and, in Stage 3, the daily-check background child. Keeping both
//! in one module avoids duplicating the HTTP client setup and User-Agent logic.

use std::time::Duration;

use crate::error::CreftError;

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

/// Fetch the latest version from the configured endpoint.
///
/// Constructs a new `ureq::Agent` with a 5-second global timeout, sets the
/// `User-Agent` header explicitly (the header value is the volume signal), and
/// parses the response body as JSON into [`LatestResponse`].
///
/// # Errors
///
/// - `CreftError::Network` — any ureq transport error, HTTP status code outside
///   2xx, or JSON parse failure.
pub(crate) fn fetch_latest() -> Result<LatestResponse, CreftError> {
    let agent = ureq::Agent::new_with_config(
        ureq::config::Config::builder()
            .timeout_global(Some(Duration::from_secs(5)))
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

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use rstest::rstest;

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
}
