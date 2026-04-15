//! Verifies that removed command names are rejected rather than silently treated
//! as skill invocations. Each case guards against a future regression where a
//! backward-compat path is accidentally re-added.

mod helpers;

use helpers::{creft_env, creft_with};
use rstest::rstest;

/// Old command aliases and namespaces that were removed in the v0.3.0 CLI
/// restructure must not be accepted as built-in commands. Each should fail with
/// a non-zero exit code.
#[rstest]
#[case::cmd_add(&["cmd", "add"])]
#[case::rm(&["rm", "hello"])]
#[case::cat(&["cat", "hello"])]
#[case::plugins_list(&["plugins", "list"])]
fn old_command_names_are_rejected(#[case] args: &[&str]) {
    let dir = creft_env();
    creft_with(&dir).args(args).assert().failure();
}
