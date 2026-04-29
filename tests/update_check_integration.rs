//! Integration tests for `creft update [--check]` and the shared HTTP path.
//!
//! These tests bind a `TcpListener` to `127.0.0.1:0`, serve a fixture JSON
//! response, and inject the listener address via `CREFT_UPDATE_ENDPOINT` so
//! the binary's HTTP path is exercised end-to-end without hitting the
//! production endpoint.

mod helpers;

use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::thread;

use helpers::{creft_env, creft_with};
use predicates::prelude::*;
use pretty_assertions::assert_eq;

/// Spawn a minimal HTTP/1.1 server in a background thread that accepts one
/// connection, responds with `body`, and closes. Returns the listener address
/// as a `http://127.0.0.1:<port>/latest` URL.
fn spawn_fixture_server(status: u16, body: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind failed");
    let addr = listener.local_addr().expect("local_addr failed");

    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept failed");

        // Drain the request.
        let mut buf = [0u8; 4096];
        let _ = stream.read(&mut buf);

        // Write a minimal HTTP response.
        let response = format!(
            "HTTP/1.1 {status} OK\r\n\
             Content-Type: application/json\r\n\
             Content-Length: {len}\r\n\
             Connection: close\r\n\
             \r\n\
             {body}",
            len = body.len()
        );
        let _ = stream.write_all(response.as_bytes());
    });

    format!("http://{}:{}/latest", addr.ip(), addr.port())
}

// ── creft update --check ──────────────────────────────────────────────────────

/// `creft update --check` against a fixture server prints version status and exits 0.
#[test]
fn update_check_prints_status_line() {
    let body = r#"{"version":"99.99.99","tag":"creft-v99.99.99"}"#;
    let endpoint = spawn_fixture_server(200, body);

    let dir = creft_env();
    creft_with(&dir)
        .args(["update", "--check"])
        .env("CREFT_UPDATE_ENDPOINT", &endpoint)
        .assert()
        .success()
        .stdout(predicate::str::contains("latest: 99.99.99"))
        .stdout(predicate::str::contains("status:"));
}

/// `creft update --check` with the same version reports `up-to-date`.
#[test]
fn update_check_reports_up_to_date_when_versions_match() {
    let installed = env!("CARGO_PKG_VERSION");
    let body = format!(r#"{{"version":"{installed}","tag":"creft-v{installed}"}}"#);
    // Leak the body string so it lives for 'static — the server thread needs it.
    let body: &'static str = Box::leak(body.into_boxed_str());
    let endpoint = spawn_fixture_server(200, body);

    let dir = creft_env();
    creft_with(&dir)
        .args(["update", "--check"])
        .env("CREFT_UPDATE_ENDPOINT", &endpoint)
        .assert()
        .success()
        .stdout(predicate::str::contains("up-to-date"));
}

/// `creft update --check` with a higher server version reports `behind`.
#[test]
fn update_check_reports_behind_when_newer_version_available() {
    let body = r#"{"version":"99.99.99","tag":"creft-v99.99.99"}"#;
    let endpoint = spawn_fixture_server(200, body);

    let dir = creft_env();
    creft_with(&dir)
        .args(["update", "--check"])
        .env("CREFT_UPDATE_ENDPOINT", &endpoint)
        .assert()
        .success()
        .stdout(predicate::str::contains("behind"));
}

/// `creft update --check` with a lower server version reports `ahead`.
#[test]
fn update_check_reports_ahead_when_installed_is_newer() {
    let body = r#"{"version":"0.0.1","tag":"creft-v0.0.1"}"#;
    let endpoint = spawn_fixture_server(200, body);

    let dir = creft_env();
    creft_with(&dir)
        .args(["update", "--check"])
        .env("CREFT_UPDATE_ENDPOINT", &endpoint)
        .assert()
        .success()
        .stdout(predicate::str::contains("ahead"));
}

// ── Network error handling ────────────────────────────────────────────────────

/// A non-success HTTP status yields a network error and exit code 1.
#[test]
fn update_network_error_on_502() {
    let body = "upstream error";
    let endpoint = spawn_fixture_server(502, body);

    let dir = creft_env();
    creft_with(&dir)
        .args(["update", "--check"])
        .env("CREFT_UPDATE_ENDPOINT", &endpoint)
        .assert()
        .failure()
        .stderr(predicate::str::contains("network error"));
}

/// Pointing at a port with no server yields a connection error.
#[test]
fn update_connection_refused_yields_network_error() {
    // Bind a listener, get its port, then drop it — the port is free but
    // nothing is listening on it, so connect will be refused.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);

    let endpoint = format!("http://127.0.0.1:{}/latest", addr.port());
    let dir = creft_env();
    creft_with(&dir)
        .args(["update", "--check"])
        .env("CREFT_UPDATE_ENDPOINT", &endpoint)
        .assert()
        .failure()
        .stderr(predicate::str::contains("network error"));
}

// ── User-Agent header ─────────────────────────────────────────────────────────

/// The HTTP request includes the expected `creft/<v> (<os>; <arch>)` User-Agent.
#[test]
fn update_sends_correct_user_agent() {
    use std::io::{BufRead as _, BufReader};

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind failed");
    let addr = listener.local_addr().expect("local_addr failed");

    let ua_capture = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let ua_clone = ua_capture.clone();

    thread::spawn(move || {
        let (stream, _) = listener.accept().expect("accept failed");
        let mut reader = BufReader::new(stream.try_clone().expect("clone failed"));

        // Read request headers line by line until the blank line.
        let mut ua = String::new();
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                break;
            }
            if line.trim().is_empty() {
                break;
            }
            let lower = line.to_lowercase();
            if lower.starts_with("user-agent:") {
                ua = line.trim().to_string();
            }
        }
        *ua_clone.lock().unwrap() = ua;

        // Write a minimal response.
        let body = r#"{"version":"0.0.1","tag":"creft-v0.0.1"}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let mut writer = reader.into_inner();
        let _ = writer.write_all(response.as_bytes());
    });

    let endpoint = format!("http://{}:{}/latest", addr.ip(), addr.port());
    let dir = creft_env();
    creft_with(&dir)
        .args(["update", "--check"])
        .env("CREFT_UPDATE_ENDPOINT", &endpoint)
        .assert()
        .success();

    let ua = ua_capture.lock().unwrap().clone();
    assert!(
        ua.to_lowercase().contains("creft/"),
        "User-Agent must contain 'creft/': {ua:?}"
    );
    let version = env!("CARGO_PKG_VERSION");
    assert!(
        ua.contains(version),
        "User-Agent must contain CARGO_PKG_VERSION {version:?}: {ua:?}"
    );
    // The OS in the UA must use the install-script convention: "darwin" not "macos".
    #[cfg(target_os = "macos")]
    assert!(
        ua.contains("darwin"),
        "User-Agent on macOS must contain 'darwin': {ua:?}"
    );
}

// ── Reserved name guard ───────────────────────────────────────────────────────

/// `creft add --name update` is rejected because `update` is reserved.
#[test]
fn add_update_is_rejected_as_reserved() {
    let dir = creft_env();
    creft_with(&dir)
        .args(["add", "--name", "update", "--description", "test"])
        .write_stdin("---\nname: update\ndescription: test\n---\n\n```bash\necho hi\n```\n")
        .assert()
        .failure()
        .stderr(predicate::str::contains("reserved"));
}

// ── Settings: telemetry ───────────────────────────────────────────────────────

/// `creft settings set telemetry off` persists and shows in `settings show`.
#[test]
fn settings_telemetry_off_roundtrips() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["settings", "set", "telemetry", "off"])
        .assert()
        .success();

    creft_with(&dir)
        .args(["settings"])
        .assert()
        .success()
        .stdout(predicate::str::contains("telemetry = off"));
}

/// `creft settings set telemetry on` persists and shows in `settings show`.
#[test]
fn settings_telemetry_on_roundtrips() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["settings", "set", "telemetry", "on"])
        .assert()
        .success();

    creft_with(&dir)
        .args(["settings"])
        .assert()
        .success()
        .stdout(predicate::str::contains("telemetry = on"));
}

/// An invalid telemetry value is rejected with a clear error.
#[test]
fn settings_telemetry_invalid_value_is_rejected() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["settings", "set", "telemetry", "yes"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("yes"))
        .stderr(predicate::str::contains("on, off"));
}

/// Default `settings show` includes the telemetry key with its default.
#[test]
fn settings_show_includes_telemetry_default() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["settings"])
        .assert()
        .success()
        .stdout(predicate::str::contains("telemetry"));
}

// ── Version output unchanged ──────────────────────────────────────────────────

/// `creft --version` prints exactly the version string and nothing else.
#[test]
fn version_output_has_no_telemetry_footer() {
    let dir = creft_env();
    let output = creft_with(&dir).arg("--version").output().unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Must start with "creft " and contain the version.
    assert!(
        stdout.trim().starts_with("creft "),
        "version output: {stdout:?}"
    );
    // Must be exactly one line — no disclosure footer.
    assert_eq!(
        stdout.lines().count(),
        1,
        "version must be exactly one line, got: {stdout:?}"
    );
}

// ── Daily check hooks: non-Command arms produce no side effects ───────────────

/// `creft --version`, `creft list --help`, and `creft --docs add` must not
/// write `.last-check` or `.update-status` — the daily-check hooks fire only
/// on the `Parsed::Command(_)` arm.
#[test]
fn non_command_arms_produce_no_daily_check_side_effects() {
    // Provide a welcome marker so the guard inside maybe_fire_daily is satisfied.
    // If the hooks fired on non-Command arms, they would write .last-check.
    let dir = creft_env();

    // Write the welcome marker directly into CREFT_HOME.
    std::fs::write(dir.path().join(".welcome-done"), "0.4.0").unwrap();

    // --version
    creft_with(&dir).arg("--version").assert().success();
    assert!(
        !dir.path().join(".last-check").exists(),
        "--version must not write .last-check"
    );

    // list --help (Parsed::Help(_))
    creft_with(&dir).args(["list", "--help"]).assert().success();
    assert!(
        !dir.path().join(".last-check").exists(),
        "list --help must not write .last-check"
    );

    // --docs add (Parsed::DocsSearchAll(_))
    // This may print "no documentation matches" but must still exit 0 and not write the file.
    let _ = creft_with(&dir).args(["--docs", "add"]).output().unwrap();
    assert!(
        !dir.path().join(".last-check").exists(),
        "--docs add must not write .last-check"
    );
}

// ── _creft check: integration test against a local HTTP listener ──────────────

/// `creft _creft check` performs a GET to `/latest`, writes `.update-status`
/// with `notice_shown: false`, and exits 0.
#[test]
fn creft_check_writes_update_status_on_success() {
    let body = r#"{"version":"99.99.99","tag":"creft-v99.99.99"}"#;
    let endpoint = spawn_fixture_server(200, body);

    let dir = creft_env();
    creft_with(&dir)
        .args(["_creft", "check"])
        .env("CREFT_UPDATE_ENDPOINT", &endpoint)
        .assert()
        .success();

    let status_path = dir.path().join(".update-status");
    assert!(status_path.exists(), ".update-status must be written");

    let content = std::fs::read_to_string(&status_path).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert_eq!(parsed["latest_version"], "99.99.99");
    assert_eq!(parsed["notice_shown"], false);
    assert!(
        parsed["checked_at"].as_str().map_or(false, |s| {
            s.len() == 10 && s.chars().nth(4) == Some('-')
        }),
        "checked_at must be YYYY-MM-DD: {}",
        parsed["checked_at"]
    );
}

/// `creft _creft check` against a 502 exits 0 and does NOT write `.update-status`.
#[test]
fn creft_check_exits_zero_on_network_error() {
    let body = "upstream error";
    let endpoint = spawn_fixture_server(502, body);

    let dir = creft_env();
    creft_with(&dir)
        .args(["_creft", "check"])
        .env("CREFT_UPDATE_ENDPOINT", &endpoint)
        .assert()
        .success(); // always exits 0

    assert!(
        !dir.path().join(".update-status").exists(),
        ".update-status must not be written on HTTP error"
    );
}

// ── Deferred notice: print_if_pending wired through dispatch ─────────────────

/// When `.update-status` records a newer version with `notice_shown: false`,
/// the next interactive command prints one line to stderr.
#[test]
fn dispatch_prints_update_notice_when_pending() {
    let dir = creft_env();

    // Write a status file indicating a newer version is available.
    let status = serde_json::json!({
        "latest_version": "99.99.99",
        "checked_at": "2026-04-28",
        "notice_shown": false
    });
    std::fs::write(
        dir.path().join(".update-status"),
        serde_json::to_string(&status).unwrap(),
    )
    .unwrap();

    // Run any interactive command. --version is NOT a Command arm, so use `list`.
    // The notice goes to stderr; `list` output goes to stdout.
    // We use a minimal CREFT_HOME so `list` returns quickly.
    let output = creft_with(&dir).args(["list"]).output().unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("99.99.99"),
        "stderr must contain the newer version; got: {stderr:?}"
    );
    assert!(
        stderr.contains("creft update") || stderr.contains("brew upgrade"),
        "stderr must contain the upgrade command; got: {stderr:?}"
    );
}

/// Once the notice has been shown (`notice_shown: true`), subsequent commands
/// must not re-print it.
#[test]
fn dispatch_does_not_repeat_notice_after_shown() {
    let dir = creft_env();

    let status = serde_json::json!({
        "latest_version": "99.99.99",
        "checked_at": "2026-04-28",
        "notice_shown": true
    });
    std::fs::write(
        dir.path().join(".update-status"),
        serde_json::to_string(&status).unwrap(),
    )
    .unwrap();

    let output = creft_with(&dir).args(["list"]).output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("99.99.99"),
        "stderr must not contain the version after notice_shown=true; got: {stderr:?}"
    );
}

/// When `telemetry=off`, no notice is printed even if `.update-status` is newer.
#[test]
fn dispatch_no_notice_when_telemetry_off() {
    let dir = creft_env();

    let status = serde_json::json!({
        "latest_version": "99.99.99",
        "checked_at": "2026-04-28",
        "notice_shown": false
    });
    std::fs::write(
        dir.path().join(".update-status"),
        serde_json::to_string(&status).unwrap(),
    )
    .unwrap();

    creft_with(&dir)
        .args(["settings", "set", "telemetry", "off"])
        .assert()
        .success();

    let output = creft_with(&dir).args(["list"]).output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("99.99.99"),
        "no notice should print when telemetry=off; got: {stderr:?}"
    );
}
