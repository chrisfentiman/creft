//! Implementation of `creft update [--check]`.

use std::path::Path;

use crate::error::CreftError;
use crate::install_method::is_homebrew_install;
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

    // Homebrew-managed binaries must be updated via Homebrew, not the install
    // script. Check before any network call.
    if is_homebrew_install(exe) {
        return Err(CreftError::Setup(
            "creft was installed via Homebrew. Run 'brew upgrade creft' to update.".into(),
        ));
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

    use super::*;

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
