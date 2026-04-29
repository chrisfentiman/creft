//! Integration tests for `creft settings` and `creft settings show`.

mod helpers;

use helpers::{creft_env, creft_with};
use predicates::prelude::*;
use pretty_assertions::assert_eq;

/// `creft settings` with no settings file prints one line per known key
/// in the default format.
#[test]
fn settings_show_prints_known_keys_with_defaults_when_no_file_exists() {
    let dir = creft_env();
    creft_with(&dir)
        .args(["settings"])
        .assert()
        .success()
        .stdout(predicate::str::contains("shell = (default:"))
        .stderr(predicate::str::is_empty());
}

/// `creft settings show` is the same as bare `creft settings`.
#[test]
fn settings_show_subcommand_matches_bare_settings() {
    let dir = creft_env();

    let bare = creft_with(&dir).args(["settings"]).output().unwrap();
    let explicit = creft_with(&dir)
        .args(["settings", "show"])
        .output()
        .unwrap();

    assert!(bare.status.success());
    assert!(explicit.status.success());
    assert_eq!(bare.stdout, explicit.stdout);
}

/// After `creft settings set shell zsh`, `creft settings` prints `shell = zsh`.
#[test]
fn settings_show_prints_configured_value_for_set_key() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["settings", "set", "shell", "zsh"])
        .assert()
        .success();

    creft_with(&dir)
        .args(["settings"])
        .assert()
        .success()
        .stdout(predicate::str::contains("shell = zsh"))
        // The shell line must show the configured value, not a default placeholder.
        .stdout(predicate::str::contains("shell = (default:").not());
}

/// `creft settings` never prints "no settings configured".
#[test]
fn settings_show_does_not_print_no_settings_configured() {
    let dir = creft_env();
    creft_with(&dir)
        .args(["settings"])
        .assert()
        .success()
        .stdout(predicate::str::contains("no settings configured").not());
}
