//! Integration tests for namespace alias rewrite at the dispatcher.
//!
//! These tests verify that `aliases.yaml` files are loaded and applied before
//! dispatch so that `creft <alias> [args...]` resolves to the aliased skill.
//! They also pin the documented limitation: built-in subcommand arguments (e.g.
//! `creft help <alias>`) are NOT rewritten.

mod helpers;

use helpers::{TwoScopeEnv, creft_two_scope};
use predicates::prelude::*;

// ── helpers ───────────────────────────────────────────────────────────────────

/// Write `aliases.yaml` for the global scope in `env`.
fn write_global_aliases(env: &TwoScopeEnv, content: &str) {
    let path = env.home_dir.path().join(".creft").join("aliases.yaml");
    std::fs::write(path, content).unwrap();
}

/// Write `aliases.yaml` for the local scope in `env`.
fn write_local_aliases(env: &TwoScopeEnv, content: &str) {
    let path = env.project_dir.path().join(".creft").join("aliases.yaml");
    std::fs::write(path, content).unwrap();
}

/// Remove the local `aliases.yaml` from `env` if it exists.
fn remove_local_aliases(env: &TwoScopeEnv) {
    let path = env.project_dir.path().join(".creft").join("aliases.yaml");
    let _ = std::fs::remove_file(path);
}

/// Add a skill to the global scope in `env`.
fn add_global_skill(env: &TwoScopeEnv, name: &str, body: &str) {
    let content =
        format!("---\nname: {name}\ndescription: {name} skill\n---\n\n```bash\n{body}\n```\n");
    let path = env
        .home_dir
        .path()
        .join(".creft")
        .join("commands")
        .join(format!("{name}.md"));
    std::fs::write(path, content).unwrap();
}

/// Add a skill to the local scope in `env`.
fn add_local_skill(env: &TwoScopeEnv, name: &str, body: &str) {
    let content =
        format!("---\nname: {name}\ndescription: {name} skill\n---\n\n```bash\n{body}\n```\n");
    let path = env
        .project_dir
        .path()
        .join(".creft")
        .join("commands")
        .join(format!("{name}.md"));
    std::fs::write(path, content).unwrap();
}

// ── rewrite dispatches the aliased skill ──────────────────────────────────────

/// A global alias `bl → backlog` causes `creft bl` to run the `backlog` skill.
#[test]
fn alias_rewrites_global_skill_invocation() {
    let env = TwoScopeEnv::new();
    add_global_skill(&env, "backlog", "echo backlog-ran");
    write_global_aliases(&env, "- from: bl\n  to: backlog\n");

    creft_two_scope(&env)
        .args(["bl"])
        .assert()
        .success()
        .stdout(predicate::str::contains("backlog-ran"));
}

/// Local alias `bl → tasks` overrides the global `bl → backlog`.
#[test]
fn local_alias_overrides_global_alias() {
    let env = TwoScopeEnv::new();
    add_global_skill(&env, "backlog", "echo backlog-ran");
    add_local_skill(&env, "tasks", "echo tasks-ran");
    write_global_aliases(&env, "- from: bl\n  to: backlog\n");
    write_local_aliases(&env, "- from: bl\n  to: tasks\n");

    creft_two_scope(&env)
        .args(["bl"])
        .assert()
        .success()
        .stdout(predicate::str::contains("tasks-ran"));
}

/// Removing the local alias restores the global alias without restarting.
#[test]
fn removing_local_alias_restores_global_alias() {
    let env = TwoScopeEnv::new();
    add_global_skill(&env, "backlog", "echo backlog-ran");
    add_local_skill(&env, "tasks", "echo tasks-ran");
    write_global_aliases(&env, "- from: bl\n  to: backlog\n");
    write_local_aliases(&env, "- from: bl\n  to: tasks\n");

    // Local alias active: tasks-ran.
    creft_two_scope(&env)
        .args(["bl"])
        .assert()
        .success()
        .stdout(predicate::str::contains("tasks-ran"));

    // Remove local alias: global alias takes effect on next invocation.
    remove_local_aliases(&env);

    creft_two_scope(&env)
        .args(["bl"])
        .assert()
        .success()
        .stdout(predicate::str::contains("backlog-ran"));
}

// ── --help flag is rewritten along with the alias ─────────────────────────────

/// `creft bl --help` (where `bl → backlog`) shows help for `backlog`, not `bl`.
///
/// The arg vector `["bl", "--help"]` matches the alias at index 0, rewrites to
/// `["backlog", "--help"]`, and cli::parse takes the existing --help path for
/// the canonical name. Help output reflects the canonical name.
#[test]
fn alias_followed_by_help_flag_shows_canonical_help() {
    let env = TwoScopeEnv::new();
    add_global_skill(&env, "backlog", "echo backlog-ran");
    write_global_aliases(&env, "- from: bl\n  to: backlog\n");

    creft_two_scope(&env)
        .args(["bl", "--help"])
        .assert()
        .success()
        // The rewrite produces ["backlog", "--help"]; cli::parse then renders the
        // canonical name in the Usage line. A regression that surfaced the alias
        // name ("bl") instead of the canonical name ("backlog") in the Usage line
        // would be caught here.
        .stdout(predicate::str::contains("Usage: creft backlog"));
}

// ── creft help <alias> does NOT rewrite ──────────────────────────────────────

/// `creft help bl` (with `bl → backlog`) shows the root listing, not backlog's
/// help. The arg vector is `["help", "bl"]`; the prefix match starts at index 0,
/// so the alias whose `from` is `["bl"]` does not fire. This is the documented
/// limitation: aliases shorten direct invocations, not built-in arguments.
#[test]
fn creft_help_alias_shows_root_listing_not_alias_target() {
    let env = TwoScopeEnv::new();
    add_global_skill(&env, "backlog", "echo backlog-ran");
    write_global_aliases(&env, "- from: bl\n  to: backlog\n");

    // When "bl" doesn't resolve as a skill or namespace, handle_help falls
    // through to the root listing. The root listing renders
    // "Usage: creft <command> [ARGS] [OPTIONS]" (src/cmd/skill.rs:244).
    // A hypothetical rewrite-also-applied-to-help path would instead render
    // "Usage: creft backlog" — these strings are mutually exclusive, so
    // testing for the root-listing string and the absence of the skill-help
    // string discriminates the two paths precisely.
    creft_two_scope(&env)
        .args(["help", "bl"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Usage: creft <command>"))
        .stdout(predicate::str::contains("Usage: creft backlog").not());
}

// ── malformed aliases.yaml emits warning but does not break dispatch ──────────

/// When `aliases.yaml` is malformed, a warning is emitted to stderr and the
/// skill still runs. A broken alias file must not prevent unrelated commands.
#[test]
fn malformed_aliases_yaml_emits_warning_and_skill_still_runs() {
    let env = TwoScopeEnv::new();
    add_global_skill(&env, "backlog", "echo backlog-ran");
    write_global_aliases(&env, "not: a: list:\n");

    creft_two_scope(&env)
        .args(["backlog"])
        .assert()
        .success()
        .stdout(predicate::str::contains("backlog-ran"))
        .stderr(predicate::str::contains("warning: ignoring aliases"));
}

// ── no aliases.yaml files behave identically to empty alias maps ──────────────

/// When neither global nor local `aliases.yaml` exist, dispatch is a no-op
/// rewrite: the skill is still found and executed.
#[test]
fn no_alias_file_dispatches_skill_by_canonical_name() {
    let env = TwoScopeEnv::new();
    add_global_skill(&env, "backlog", "echo backlog-ran");
    // No aliases.yaml written — files do not exist.

    creft_two_scope(&env)
        .args(["backlog"])
        .assert()
        .success()
        .stdout(predicate::str::contains("backlog-ran"));
}

/// When no alias files exist and a non-existent command is given, the error
/// is the ordinary "command not found" path — no panic, no alias warning.
#[test]
fn no_alias_file_produces_no_warning_on_missing_command() {
    let env = TwoScopeEnv::new();

    creft_two_scope(&env)
        .args(["nonexistent-skill"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("warning: ignoring aliases").not());
}

/// Running a skill when no `aliases.yaml` exists must not create the file in
/// either scope. Silent file creation in `~/.creft/` is the kind of side-effect
/// users notice in `git status` and report as unexpected pollution.
#[test]
fn running_skill_without_alias_file_does_not_create_aliases_yaml() {
    let env = TwoScopeEnv::new();
    add_global_skill(&env, "backlog", "echo backlog-ran");
    // Deliberately write no aliases.yaml in either scope.

    creft_two_scope(&env)
        .args(["backlog"])
        .assert()
        .success()
        .stdout(predicate::str::contains("backlog-ran"));

    let global_aliases = env.home_dir.path().join(".creft").join("aliases.yaml");
    let local_aliases = env.project_dir.path().join(".creft").join("aliases.yaml");

    assert!(
        !global_aliases.exists(),
        "dispatch must not create global aliases.yaml when it did not exist"
    );
    assert!(
        !local_aliases.exists(),
        "dispatch must not create local aliases.yaml when it did not exist"
    );
}

// ── creft alias add ───────────────────────────────────────────────────────────

/// `creft alias add bl backlog` writes to the global scope when the target
/// skill lives in the global scope. Stderr includes the scope tag `[global]`.
#[test]
fn alias_add_global_skill_writes_global_scope() {
    let env = TwoScopeEnv::new();
    add_global_skill(&env, "backlog", "echo backlog-ran");

    creft_two_scope(&env)
        .args(["alias", "add", "bl", "backlog"])
        .assert()
        .success()
        .stderr(predicate::str::contains("added: bl → backlog [global]"));

    // The alias file must exist in the global scope.
    let global_aliases = env.home_dir.path().join(".creft").join("aliases.yaml");
    assert!(
        global_aliases.exists(),
        "aliases.yaml must be created in the global scope"
    );
    // The local scope must remain untouched.
    let local_aliases = env.project_dir.path().join(".creft").join("aliases.yaml");
    assert!(
        !local_aliases.exists(),
        "aliases.yaml must not be created in the local scope"
    );
}

/// After `creft alias add bl backlog`, running `creft bl` dispatches to the
/// `backlog` skill.
#[test]
fn alias_add_then_invocation_dispatches_to_target() {
    let env = TwoScopeEnv::new();
    add_global_skill(&env, "backlog", "echo backlog-ran");

    creft_two_scope(&env)
        .args(["alias", "add", "bl", "backlog"])
        .assert()
        .success();

    creft_two_scope(&env)
        .args(["bl"])
        .assert()
        .success()
        .stdout(predicate::str::contains("backlog-ran"));
}

/// `creft alias add bl tasks` writes to the local scope when the target skill
/// lives in the local scope. Stderr includes `[local]`.
#[test]
fn alias_add_local_skill_writes_local_scope() {
    let env = TwoScopeEnv::new();
    add_local_skill(&env, "tasks", "echo tasks-ran");

    creft_two_scope(&env)
        .args(["alias", "add", "bl", "tasks"])
        .assert()
        .success()
        .stderr(predicate::str::contains("added: bl → tasks [local]"));

    // Local scope gets the alias; global scope must be untouched.
    let local_aliases = env.project_dir.path().join(".creft").join("aliases.yaml");
    assert!(
        local_aliases.exists(),
        "aliases.yaml must be created in the local scope"
    );
    let global_aliases = env.home_dir.path().join(".creft").join("aliases.yaml");
    assert!(
        !global_aliases.exists(),
        "aliases.yaml must not be created in the global scope"
    );
}

/// Multi-segment `from` (e.g. `my new`) is accepted as a single quoted argument.
#[test]
fn alias_add_multi_segment_from_quoted() {
    let env = TwoScopeEnv::new();
    add_global_skill(&env, "backlog", "echo backlog-ran");

    creft_two_scope(&env)
        .args(["alias", "add", "my new", "backlog"])
        .assert()
        .success()
        .stderr(predicate::str::contains("added: my new → backlog [global]"));
}

/// Updating an existing alias (same `from`) replaces it in-place rather than
/// appending a duplicate.
#[test]
fn alias_add_update_existing_alias_replaces_in_place() {
    let env = TwoScopeEnv::new();
    add_global_skill(&env, "backlog", "echo backlog-ran");
    add_global_skill(&env, "tasks", "echo tasks-ran");

    creft_two_scope(&env)
        .args(["alias", "add", "bl", "backlog"])
        .assert()
        .success();

    // Re-add with a different target — must replace, not duplicate.
    creft_two_scope(&env)
        .args(["alias", "add", "bl", "tasks"])
        .assert()
        .success()
        .stderr(predicate::str::contains("added: bl → tasks [global]"));

    // Running `creft bl` must dispatch to `tasks`, not `backlog`.
    creft_two_scope(&env)
        .args(["bl"])
        .assert()
        .success()
        .stdout(predicate::str::contains("tasks-ran"));
}

// ── creft alias list ──────────────────────────────────────────────────────────

/// When no aliases are defined, `creft alias list` prints "no aliases defined".
#[test]
fn alias_list_empty_prints_no_aliases_defined() {
    let env = TwoScopeEnv::new();

    creft_two_scope(&env)
        .args(["alias", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("no aliases defined"));
}

/// `creft alias list` shows all aliases sorted by `from` (lexicographic),
/// with scope tags, to stdout.
#[test]
fn alias_list_shows_sorted_aliases_with_scope_tags() {
    let env = TwoScopeEnv::new();
    add_global_skill(&env, "backlog", "echo backlog-ran");
    add_local_skill(&env, "tasks", "echo tasks-ran");
    // Write aliases.yaml files directly to bypass the add command.
    write_global_aliases(&env, "- from: zz\n  to: backlog\n");
    write_local_aliases(&env, "- from: aa\n  to: tasks\n");

    let out = creft_two_scope(&env)
        .args(["alias", "list"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(out).unwrap();

    // Both entries must appear.
    assert!(
        text.contains("aa → tasks [local]"),
        "local alias must appear with [local] tag; got:\n{text}"
    );
    assert!(
        text.contains("zz → backlog [global]"),
        "global alias must appear with [global] tag; got:\n{text}"
    );
    // 'aa' must come before 'zz' (lexicographic sort).
    let aa_pos = text.find("aa →").unwrap();
    let zz_pos = text.find("zz →").unwrap();
    assert!(
        aa_pos < zz_pos,
        "aliases must be sorted lexicographically by from; got:\n{text}"
    );
}

// ── creft alias remove ────────────────────────────────────────────────────────

/// `creft alias remove bl` removes the local alias first when both scopes
/// define the same `from`.
#[test]
fn alias_remove_local_first_when_both_scopes_have_same_from() {
    let env = TwoScopeEnv::new();
    add_global_skill(&env, "backlog", "echo backlog-ran");
    add_local_skill(&env, "tasks", "echo tasks-ran");
    write_global_aliases(&env, "- from: bl\n  to: backlog\n");
    write_local_aliases(&env, "- from: bl\n  to: tasks\n");

    // First remove: takes the local alias.
    creft_two_scope(&env)
        .args(["alias", "remove", "bl"])
        .assert()
        .success()
        .stderr(predicate::str::contains("removed: bl [local]"));

    // After removing local, the global alias is now active.
    creft_two_scope(&env)
        .args(["bl"])
        .assert()
        .success()
        .stdout(predicate::str::contains("backlog-ran"));
}

/// A second `creft alias remove bl` after the local alias is gone removes the
/// global alias.
#[test]
fn alias_remove_second_call_removes_global_alias() {
    let env = TwoScopeEnv::new();
    add_global_skill(&env, "backlog", "echo backlog-ran");
    add_local_skill(&env, "tasks", "echo tasks-ran");
    write_global_aliases(&env, "- from: bl\n  to: backlog\n");
    write_local_aliases(&env, "- from: bl\n  to: tasks\n");

    // Remove local first.
    creft_two_scope(&env)
        .args(["alias", "remove", "bl"])
        .assert()
        .success();

    // Remove global next.
    creft_two_scope(&env)
        .args(["alias", "remove", "bl"])
        .assert()
        .success()
        .stderr(predicate::str::contains("removed: bl [global]"));

    // No alias remains — `creft bl` must fail with command-not-found.
    creft_two_scope(&env)
        .args(["bl"])
        .assert()
        .failure();
}

/// A third `creft alias remove bl` when neither scope has it exits 2 with a
/// not-found message.
#[test]
fn alias_remove_not_found_exits_2() {
    let env = TwoScopeEnv::new();

    creft_two_scope(&env)
        .args(["alias", "remove", "bl"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("bl"));
}

// ── conflict rejection ────────────────────────────────────────────────────────

/// Aliasing a built-in command name is rejected with exit code 3.
#[test]
fn alias_add_rejects_builtin_from() {
    let env = TwoScopeEnv::new();
    add_global_skill(&env, "backlog", "echo backlog-ran");

    creft_two_scope(&env)
        .args(["alias", "add", "list", "backlog"])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("built-in command"));
}

/// Aliasing an existing skill name is rejected with exit code 3.
#[test]
fn alias_add_rejects_existing_skill_from() {
    let env = TwoScopeEnv::new();
    add_global_skill(&env, "backlog", "echo backlog-ran");

    // `backlog` is already a skill; aliasing it must fail.
    creft_two_scope(&env)
        .args(["alias", "add", "backlog", "backlog"])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("skill"));
}

/// Aliasing a name whose target doesn't exist is rejected with exit code 2.
#[test]
fn alias_add_rejects_nonexistent_target() {
    let env = TwoScopeEnv::new();

    creft_two_scope(&env)
        .args(["alias", "add", "bl", "nonexistent-skill"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("nonexistent-skill"));
}

/// A `from` value with an invalid path token (e.g. a slash) is rejected with
/// exit code 3.
#[test]
fn alias_add_rejects_invalid_from_segment() {
    let env = TwoScopeEnv::new();
    add_global_skill(&env, "backlog", "echo backlog-ran");

    creft_two_scope(&env)
        .args(["alias", "add", "my/bad", "backlog"])
        .assert()
        .code(3);
}

/// Adding an alias that would create a cycle is rejected with exit code 3.
///
/// The setup: `fb` is a real skill. `aliases.yaml` already contains both
/// `fa → fb` and `fb → fa` (written directly, bypassing the CLI target
/// check that would normally reject `fb → fa` because `fa` is not a skill).
///
/// Calling `creft alias add fa fb`:
///   - `fa` is not a skill/builtin/namespace → conflict check passes.
///   - `fb` is a real skill → target check passes.
///   - Post-write view: `fa → fb` (replaced), `fb → fa`.
///   - Cycle walker starts at `fb` (new.to). Finds `fb → fa`. Lands on `fa`
///     which equals `new.from`. Cycle detected → exit 3.
#[test]
fn alias_add_rejects_cycle() {
    let env = TwoScopeEnv::new();
    add_global_skill(&env, "fb", "echo fb");
    // Write both hops directly: fa → fb AND fb → fa.
    // fb → fa bypasses add-time target validation (fa is not a real skill).
    write_global_aliases(&env, "- from: fa\n  to: fb\n- from: fb\n  to: fa\n");

    // alias add fa fb: conflict check passes (fa is not a skill), target check
    // passes (fb is a real skill), then post-write cycle detection fires.
    creft_two_scope(&env)
        .args(["alias", "add", "fa", "fb"])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("cycle"));
}

// ── argument parsing edge cases ───────────────────────────────────────────────

/// `creft alias remove my new` (two unquoted tokens) causes the parser to
/// reject "new" as an unexpected argument. Exit code 2 (CliParse).
#[test]
fn alias_remove_unquoted_two_tokens_exits_2() {
    let env = TwoScopeEnv::new();

    creft_two_scope(&env)
        .args(["alias", "remove", "my", "new"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("unexpected argument: new"));
}

/// `creft alias remove "my new"` (quoted, single argument) succeeds when the
/// alias exists.
#[test]
fn alias_remove_quoted_two_segment_from_succeeds() {
    let env = TwoScopeEnv::new();
    add_global_skill(&env, "backlog", "echo backlog-ran");
    write_global_aliases(&env, "- from: my new\n  to: backlog\n");

    creft_two_scope(&env)
        .args(["alias", "remove", "my new"])
        .assert()
        .success()
        .stderr(predicate::str::contains("removed: my new [global]"));
}

// ── --docs flag ───────────────────────────────────────────────────────────────

/// `creft alias --docs` prints non-empty documentation to stdout.
#[test]
fn alias_docs_prints_nonempty_output() {
    let env = TwoScopeEnv::new();

    let out = creft_two_scope(&env)
        .args(["alias", "--docs"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert!(
        !out.is_empty(),
        "`creft alias --docs` must print documentation to stdout"
    );
}

/// `creft alias add --docs` prints non-empty documentation to stdout.
#[test]
fn alias_add_docs_prints_nonempty_output() {
    let env = TwoScopeEnv::new();

    let out = creft_two_scope(&env)
        .args(["alias", "add", "--docs"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert!(
        !out.is_empty(),
        "`creft alias add --docs` must print documentation to stdout"
    );
}

// ── malformed aliases.yaml ────────────────────────────────────────────────────

/// A malformed `aliases.yaml` in the global scope causes `creft alias list`
/// to exit 1 and include the file path in the error message.
#[test]
fn alias_list_malformed_aliases_yaml_exits_1_with_path() {
    let env = TwoScopeEnv::new();
    write_global_aliases(&env, "not: a: list:\n");

    creft_two_scope(&env)
        .args(["alias", "list"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("aliases.yaml"));
}
