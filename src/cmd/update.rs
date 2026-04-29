//! Implementation of `creft update [--check]`.

use std::path::Path;

use crate::error::CreftError;
use crate::install_method::{self, InstallMethod};
use crate::model::AppContext;
use crate::update_check::{LatestResponse, fetch_latest};

/// Version comparison result.
#[derive(Debug, PartialEq, Eq)]
enum Status {
    UpToDate,
    Behind,
    Ahead,
}

/// Run `creft update [--check]`.
///
/// `current_exe` is the canonicalized path of the running binary, computed once
/// by `dispatch()` and passed in so this function is testable without
/// intercepting `std::env::current_exe`. When `None`, the binary path could not
/// be resolved and an error is returned immediately.
///
/// When `check` is `true`, resolves the latest version and prints a status line
/// without modifying the binary.
pub(crate) fn cmd_update(
    _ctx: &AppContext,
    current_exe: Option<&Path>,
    check: bool,
) -> Result<(), CreftError> {
    let exe =
        current_exe.ok_or_else(|| CreftError::Setup("running binary path unresolved".into()))?;

    // Package-manager-managed binaries must be updated through their package
    // manager, not the install script. Check before any network call.
    // cmd_update is intentionally not CI-gated: the user typed the command,
    // so the same exemption that applies to the telemetry=off setting applies here.
    match install_method::detect(exe) {
        method @ (InstallMethod::Homebrew | InstallMethod::Cargo) => {
            // refusal_message() returns Some(_) for both Homebrew and Cargo;
            // the match arm constrains the variant, so the expect will never panic.
            let msg = method
                .refusal_message()
                .expect("Homebrew and Cargo always carry a refusal message");
            return Err(CreftError::Setup(msg.into()));
        }
        InstallMethod::InstallScript => {} // fall through to install-script flow
    }

    let latest = fetch_latest()?;
    let installed = env!("CARGO_PKG_VERSION");

    let status = compare_versions(installed, &latest.version)?;

    if check {
        let status_str = match status {
            Status::UpToDate => "up-to-date",
            Status::Behind => "behind",
            Status::Ahead => "ahead",
        };
        println!(
            "latest: {}, installed: {installed}, status: {status_str}",
            latest.version
        );
        return Ok(());
    }

    match status {
        Status::UpToDate => {
            println!("creft is up to date ({installed})");
        }
        Status::Ahead => {
            println!("creft is ahead of the published release ({installed})");
        }
        Status::Behind => {
            run_install_script(exe, &latest)?;
            println!("creft updated: {installed} -> {}", latest.version);
        }
    }

    Ok(())
}

/// Shell out to the install script for the given version.
fn run_install_script(exe: &Path, latest: &LatestResponse) -> Result<(), CreftError> {
    let install_dir = exe
        .parent()
        .ok_or_else(|| CreftError::Setup("running binary has no parent directory".into()))?;

    let url = format!("https://creft.run/v{}", latest.version);
    let cmd = format!("curl --proto '=https' --tlsv1.2 -fsSL {url} | sh");

    let status = std::process::Command::new("sh")
        .arg("-c")
        .arg(&cmd)
        .env("CREFT_INSTALL_DIR", install_dir)
        .status()
        .map_err(CreftError::Io)?;

    if !status.success() {
        let code = status.code().unwrap_or(1);
        return Err(CreftError::InstallScriptFailed { code });
    }

    Ok(())
}

/// Compare version strings on `(major, minor, patch)`.
///
/// Pre-release suffixes are not supported; the project does not ship them.
/// Returns an error when either version string cannot be parsed as three
/// dot-separated unsigned integers.
fn compare_versions(installed: &str, latest: &str) -> Result<Status, CreftError> {
    let parse = |s: &str| -> Result<(u64, u64, u64), CreftError> {
        let parts: Vec<&str> = s.split('.').collect();
        if parts.len() != 3 {
            return Err(CreftError::Setup(format!(
                "malformed version string: {s:?}"
            )));
        }
        let major = parts[0]
            .parse::<u64>()
            .map_err(|_| CreftError::Setup(format!("malformed version string: {s:?}")))?;
        let minor = parts[1]
            .parse::<u64>()
            .map_err(|_| CreftError::Setup(format!("malformed version string: {s:?}")))?;
        let patch = parts[2]
            .parse::<u64>()
            .map_err(|_| CreftError::Setup(format!("malformed version string: {s:?}")))?;
        Ok((major, minor, patch))
    };

    let inst = parse(installed)?;
    let lat = parse(latest)?;

    Ok(match inst.cmp(&lat) {
        std::cmp::Ordering::Equal => Status::UpToDate,
        std::cmp::Ordering::Less => Status::Behind,
        std::cmp::Ordering::Greater => Status::Ahead,
    })
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use rstest::rstest;
    use serial_test::serial;

    use super::*;

    // ── cmd_update refusal arms ───────────────────────────────────────────────

    /// cmd_update refuses with a Homebrew redirect when the binary is managed
    /// by Homebrew. No HTTP call is attempted (the refusal short-circuits before
    /// fetch_latest is reached).
    #[cfg(unix)]
    #[test]
    fn cmd_update_refuses_homebrew_with_redirect_message() {
        use tempfile::TempDir;

        let dir = TempDir::new().unwrap();
        // Build a path that canonicalizes through <tmp>/Cellar/creft/<ver>/bin/creft.
        let target = dir
            .path()
            .join("Cellar")
            .join("creft")
            .join("0.5.1")
            .join("bin")
            .join("creft");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, b"").unwrap();
        let link = dir.path().join("bin").join("creft");
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let ctx =
            crate::model::AppContext::for_test(dir.path().to_path_buf(), dir.path().to_path_buf());
        let result = cmd_update(&ctx, Some(&link), false);

        let err = result.expect_err("cmd_update must refuse for Homebrew installs");
        let msg = err.to_string();
        assert!(
            msg.contains("brew upgrade creft"),
            "error must contain 'brew upgrade creft'; got: {msg:?}"
        );
    }

    /// cmd_update refuses with a Cargo redirect when the binary lives in
    /// $CARGO_HOME/bin/. No HTTP call is attempted.
    #[cfg(unix)]
    #[test]
    #[serial(env_cargo_home)]
    fn cmd_update_refuses_cargo_with_redirect_message() {
        use tempfile::TempDir;

        let dir = TempDir::new().unwrap();
        let cargo_bin = dir.path().join(".cargo").join("bin");
        std::fs::create_dir_all(&cargo_bin).unwrap();
        let real = cargo_bin.join("creft");
        std::fs::write(&real, b"").unwrap();
        // Symlink elsewhere so canonicalize resolves to cargo_bin.
        let link = dir.path().join("shim").join("creft");
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let prior = std::env::var("CARGO_HOME").ok();
        // SAFETY: single-threaded test context; no other thread reads this var.
        unsafe { std::env::set_var("CARGO_HOME", dir.path().join(".cargo")) };
        let ctx =
            crate::model::AppContext::for_test(dir.path().to_path_buf(), dir.path().to_path_buf());
        let result = cmd_update(&ctx, Some(&link), false);
        match prior {
            Some(v) => unsafe { std::env::set_var("CARGO_HOME", v) },
            None => unsafe { std::env::remove_var("CARGO_HOME") },
        }

        let err = result.expect_err("cmd_update must refuse for Cargo installs");
        let msg = err.to_string();
        assert!(
            msg.contains("cargo install creft"),
            "error must contain 'cargo install creft'; got: {msg:?}"
        );
    }

    /// cmd_update proceeds past the refusal check for an install-script binary
    /// and falls through to the network call. Using --check with a connection-
    /// refused endpoint proves the code reached fetch_latest, not the refusal branch.
    #[cfg(unix)]
    #[test]
    #[serial(env_cargo_home)]
    fn cmd_update_proceeds_past_refusal_check_for_install_script() {
        use std::net::TcpListener;
        use tempfile::TempDir;

        let dir = TempDir::new().unwrap();
        // Binary lives outside both Homebrew and Cargo prefixes.
        let elsewhere = dir.path().join("elsewhere").join("creft");
        std::fs::create_dir_all(elsewhere.parent().unwrap()).unwrap();
        std::fs::write(&elsewhere, b"").unwrap();

        // Ensure CARGO_HOME/bin/ does not accidentally contain this path.
        let prior = std::env::var("CARGO_HOME").ok();
        // SAFETY: single-threaded test context; no other thread reads this var.
        unsafe { std::env::set_var("CARGO_HOME", dir.path().join(".no-cargo")) };

        // Bind a listener, get the port, then drop it — nothing listening.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let endpoint = format!("http://127.0.0.1:{port}/latest");
        // SAFETY: single-threaded test context.
        let prior_ep = std::env::var("CREFT_UPDATE_ENDPOINT").ok();
        unsafe { std::env::set_var("CREFT_UPDATE_ENDPOINT", &endpoint) };

        let ctx =
            crate::model::AppContext::for_test(dir.path().to_path_buf(), dir.path().to_path_buf());
        let result = cmd_update(&ctx, Some(&elsewhere), true);

        match prior {
            Some(v) => unsafe { std::env::set_var("CARGO_HOME", v) },
            None => unsafe { std::env::remove_var("CARGO_HOME") },
        }
        match prior_ep {
            Some(v) => unsafe { std::env::set_var("CREFT_UPDATE_ENDPOINT", v) },
            None => unsafe { std::env::remove_var("CREFT_UPDATE_ENDPOINT") },
        }

        // The call must have proceeded past the refusal check and attempted the
        // network call, failing with a network error (connection refused).
        let err = result
            .expect_err("cmd_update must fail with a network error for install-script binary");
        let msg = err.to_string();
        assert!(
            msg.contains("network error"),
            "error must be a network error (connection refused); got: {msg:?}"
        );
    }

    // ── compare_versions() ───────────────────────────────────────────────────

    #[rstest]
    #[case::behind("0.4.0", "0.5.1", Ok(Status::Behind))]
    #[case::up_to_date("0.5.1", "0.5.1", Ok(Status::UpToDate))]
    #[case::ahead("1.0.0", "0.9.9", Ok(Status::Ahead))]
    #[case::multi_digit_minor("0.10.0", "0.9.0", Ok(Status::Ahead))]
    #[case::malformed_installed("0.5", "0.5.1", Err(()))]
    #[case::malformed_latest("0.5.1", "x.y.z", Err(()))]
    #[case::empty_installed("", "0.5.1", Err(()))]
    #[case::empty_latest("0.5.1", "", Err(()))]
    fn compare_versions_cases(
        #[case] installed: &str,
        #[case] latest: &str,
        #[case] expected: Result<Status, ()>,
    ) {
        let result = compare_versions(installed, latest);
        match expected {
            Ok(status) => assert_eq!(result.unwrap(), status),
            Err(()) => assert!(
                result.is_err(),
                "expected error for {installed:?} vs {latest:?}"
            ),
        }
    }
}
