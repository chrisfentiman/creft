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
