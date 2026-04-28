//! Handlers for the `creft alias` built-in: add, remove, list.
//!
//! Target-scope resolution delegates entirely to `store::resolve_in_scope` and
//! `store::namespace_exists_in_scope` — no filesystem walk lives in this module.
//! `validate_path_token` is intentionally NOT imported; it is enforced through
//! `Alias::new`, which is the only construction path.

use crate::aliases::{Alias, AliasFile, AliasMap, load_for_scope, save_for_scope};
use crate::error::CreftError;
use crate::model::{AppContext, Scope};
use crate::store::{
    is_reserved, namespace_exists, namespace_exists_in_scope, resolve_command, resolve_in_scope,
};

/// `creft alias add <from> <to>`
///
/// Validates segments via `Alias::new`, checks for conflicts (binary name,
/// built-in, existing skill, existing namespace), resolves the scope from
/// the target, detects cycles in the post-write combined map, and saves.
pub fn cmd_alias_add(ctx: &AppContext, from: &str, to: &str) -> Result<(), CreftError> {
    let from_segments: Vec<String> = from.split_whitespace().map(str::to_string).collect();
    let to_segments: Vec<String> = to.split_whitespace().map(str::to_string).collect();

    // Construction via Alias::new enforces non-empty vectors and per-segment
    // validate_path_token (empty, `.`, `..`, `/`, `\` → CreftError::InvalidName,
    // which exits 3). This is the only construction path — no direct struct
    // literal exists in this module.
    let alias = Alias::new(from_segments.clone(), to_segments.clone())?;

    check_conflict(ctx, &from_segments)?;

    let scope = resolve_target_scope(ctx, &to_segments)?;

    // Build the post-write deduplicated combined view, then walk for cycles.
    let mut post_write = AliasMap::load(ctx)?;
    insert_or_replace(&mut post_write, &alias);
    if would_create_cycle(&post_write, &alias) {
        return Err(CreftError::AliasCycle(from_segments.join(" ")));
    }

    // Load the scope file, upsert the alias, and save.
    let mut file = load_for_scope(ctx, scope)?;
    upsert_alias(&mut file, alias);
    save_for_scope(ctx, scope, &file)?;

    eprintln!(
        "added: {} \u{2192} {} [{}]",
        from_segments.join(" "),
        to_segments.join(" "),
        scope_tag(scope)
    );
    Ok(())
}

/// `creft alias remove <from>`
///
/// Searches local then global (or global only under CREFT_HOME) and removes
/// the alias from the first scope that contains it. One invocation removes
/// from one scope — if both scopes have the same `from`, two invocations are
/// required. Returns `AliasNotFound` when neither scope contains the alias.
pub fn cmd_alias_remove(ctx: &AppContext, from: &str) -> Result<(), CreftError> {
    let from_segments: Vec<String> = from.split_whitespace().map(str::to_string).collect();
    if from_segments.is_empty() {
        return Err(CreftError::MissingArg("<from>".into()));
    }

    // Build the search order. Local is skipped under CREFT_HOME so the file
    // is never opened twice with the same path (resolve_root redirects both
    // scopes to the same dir under CREFT_HOME).
    let mut search: Vec<Scope> = Vec::with_capacity(2);
    if ctx.find_local_root().is_some() {
        search.push(Scope::Local);
    }
    search.push(Scope::Global);

    for scope in search {
        let mut file = load_for_scope(ctx, scope)?;
        if let Some(idx) = file.aliases.iter().position(|a| a.from == from_segments) {
            file.aliases.remove(idx);
            save_for_scope(ctx, scope, &file)?;
            eprintln!(
                "removed: {} [{}]",
                from_segments.join(" "),
                scope_tag(scope)
            );
            return Ok(());
        }
    }

    Err(CreftError::AliasNotFound(from_segments.join(" ")))
}

/// `creft alias list`
///
/// Prints all aliases sorted by `from` (lexicographic on the joined string),
/// with scope tags, to stdout. Prints "no aliases defined" when both scopes
/// are empty. A parse failure in either scope propagates as
/// `CreftError::Frontmatter` (exits 1), giving the user the file path to fix.
pub fn cmd_alias_list(ctx: &AppContext) -> Result<(), CreftError> {
    // Load both scopes explicitly so each entry carries its scope tag.
    // Missing files are empty; parse failures propagate immediately.
    let global = load_for_scope(ctx, Scope::Global)?;
    let local = if ctx.find_local_root().is_some() {
        load_for_scope(ctx, Scope::Local)?
    } else {
        crate::aliases::AliasFile::default()
    };

    // Collect (from, to, scope) triples in a single pass. Under CREFT_HOME
    // find_local_root() returns None, so the local branch is skipped and only
    // global entries appear (tagged [global]).
    let mut entries: Vec<(String, String, Scope)> = global
        .aliases
        .iter()
        .map(|a| (a.from.join(" "), a.to.join(" "), Scope::Global))
        .collect();
    for a in &local.aliases {
        entries.push((a.from.join(" "), a.to.join(" "), Scope::Local));
    }

    if entries.is_empty() {
        println!("no aliases defined");
        return Ok(());
    }

    // Stable sort by the joined from string (case-sensitive, lexicographic).
    entries.sort_by(|a, b| a.0.cmp(&b.0));

    for (from_key, to_val, scope) in &entries {
        println!("{} \u{2192} {} [{}]", from_key, to_val, scope_tag(*scope));
    }

    Ok(())
}

// ── Private helpers ───────────────────────────────────────────────────────────

fn scope_tag(scope: Scope) -> &'static str {
    match scope {
        Scope::Local => "local",
        Scope::Global => "global",
    }
}

/// Resolve which scope to write the alias into, based on where `to` lives.
///
/// Tries local first (when a real local root exists), then global. Errors
/// with `AliasTargetNotFound` if neither scope contains the target as a
/// skill, package skill, plugin skill, or namespace prefix.
fn resolve_target_scope(ctx: &AppContext, to: &[String]) -> Result<Scope, CreftError> {
    if ctx.find_local_root().is_some() && target_exists_in_scope(ctx, to, Scope::Local)? {
        return Ok(Scope::Local);
    }
    if target_exists_in_scope(ctx, to, Scope::Global)? {
        return Ok(Scope::Global);
    }
    Err(CreftError::AliasTargetNotFound(to.join(" ")))
}

/// Check whether `to` resolves as a skill or namespace prefix in `scope`.
///
/// Delegates entirely to `store::resolve_in_scope` (which handles owned
/// skills, package skills, and activated plugin skills) and falls through to
/// `store::namespace_exists_in_scope` on `CommandNotFound`. No filesystem
/// walk lives here.
fn target_exists_in_scope(
    ctx: &AppContext,
    to: &[String],
    scope: Scope,
) -> Result<bool, CreftError> {
    match resolve_in_scope(ctx, to, scope) {
        Ok(_) => return Ok(true),
        Err(CreftError::CommandNotFound(_)) => {}
        Err(e) => return Err(e),
    }
    let prefix: Vec<&str> = to.iter().map(String::as_str).collect();
    namespace_exists_in_scope(ctx, &prefix, scope)
}

/// Check whether `from` would conflict with a binary name, built-in, skill, or namespace.
///
/// Returns `Ok(())` on no conflict. Returns `AliasConflict { from, kind }` where
/// `kind` is `"binary name"`, `"built-in command"`, `"skill"`, or `"namespace"`.
fn check_conflict(ctx: &AppContext, from: &[String]) -> Result<(), CreftError> {
    let first = &from[0];

    // The binary name "creft" never appears in args[..]; an alias for it
    // would silently never fire.
    if first == "creft" {
        return Err(CreftError::AliasConflict {
            from: from.join(" "),
            kind: "binary name".into(),
        });
    }

    // The `_creft` prefix is reserved for internal infrastructure. An alias
    // whose from starts with `_creft` would be dead code because the
    // dispatcher guards `_creft` before the rewrite.
    if first == "_creft" || is_reserved(first) {
        return Err(CreftError::AliasConflict {
            from: from.join(" "),
            kind: "built-in command".into(),
        });
    }

    // Check against existing skills (any scope, cross-source).
    let from_str: Vec<&str> = from.iter().map(String::as_str).collect();
    match resolve_command(ctx, from) {
        Ok((_, remaining, _)) if remaining.is_empty() => {
            return Err(CreftError::AliasConflict {
                from: from.join(" "),
                kind: "skill".into(),
            });
        }
        Ok(_) | Err(CreftError::CommandNotFound(_)) => {}
        Err(e) => return Err(e),
    }

    // Check against existing namespaces (any scope, cross-source).
    if namespace_exists(ctx, &from_str)? {
        return Err(CreftError::AliasConflict {
            from: from.join(" "),
            kind: "namespace".into(),
        });
    }

    Ok(())
}

/// Replace an existing alias with the same `from` in `map`, or append.
///
/// Maintains the longest-first order that `AliasMap::load` establishes so the
/// cycle walker's `find` uses the same match semantics as the runtime rewrite's
/// `find_prefix` — longest match first.
fn insert_or_replace(map: &mut AliasMap, new: &Alias) {
    for entry in map.entries_mut() {
        if entry.from == new.from {
            *entry = new.clone();
            // In-place replacement preserves length so the sort is unchanged.
            return;
        }
    }
    // New entry appended at the end; re-sort to restore longest-first order.
    map.push(new.clone());
    map.entries_mut()
        .sort_by(|a, b| b.from.len().cmp(&a.from.len()));
}

/// Upsert `alias` into `file`: replace any existing entry with the same `from`.
fn upsert_alias(file: &mut AliasFile, alias: Alias) {
    for entry in &mut file.aliases {
        if entry.from == alias.from {
            *entry = alias;
            return;
        }
    }
    file.aliases.push(alias);
}

/// Cycle detection over the post-write deduplicated combined view.
///
/// Returns `true` if adding `new` would introduce a cycle. Walks edges
/// starting at `new.to` using the same prefix-match step the runtime rewrite
/// uses. A visit set bounds the walk — a pre-existing disjoint cycle terminates
/// the walk without falsely implicating `new`.
fn would_create_cycle(post_write: &AliasMap, new: &Alias) -> bool {
    let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut current: Vec<String> = new.to.clone();

    loop {
        let current_key = current.join(" ");
        if current_key == new.from.join(" ") {
            // Walked back to the new alias's `from` — cycle detected.
            return true;
        }
        if visited.contains(&current_key) {
            // Pre-existing cycle disjoint from `new` — terminate without
            // implicating `new`.
            return false;
        }
        visited.insert(current_key);

        // Find the next hop using the same prefix-match the runtime uses.
        let matched = post_write.entries().iter().find(|a| {
            current.len() >= a.from.len() && a.from.iter().zip(current.iter()).all(|(f, c)| f == c)
        });

        match matched {
            None => return false,
            Some(next) => {
                let mut next_current = next.to.clone();
                next_current.extend_from_slice(&current[next.from.len()..]);
                current = next_current;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::*;
    use crate::aliases::{Alias, AliasMap};

    fn make_alias(from: &[&str], to: &[&str]) -> Alias {
        Alias::new(
            from.iter().map(|s| s.to_string()).collect(),
            to.iter().map(|s| s.to_string()).collect(),
        )
        .unwrap()
    }

    fn make_map(aliases: &[(&[&str], &[&str])]) -> AliasMap {
        let mut map = AliasMap::default();
        for (from, to) in aliases {
            map.push(make_alias(from, to));
        }
        map
    }

    // ── would_create_cycle ────────────────────────────────────────────────────

    #[test]
    fn cycle_detection_direct_two_cycle() {
        // post-write view: [bl → backlog]; new alias: backlog → bl
        let new = make_alias(&["backlog"], &["bl"]);
        let map = make_map(&[(&["bl"], &["backlog"]), (&["backlog"], &["bl"])]);
        assert!(
            would_create_cycle(&map, &new),
            "backlog → bl with bl → backlog must be detected as a cycle"
        );
    }

    #[test]
    fn cycle_detection_three_cycle() {
        // bl → backlog, backlog → tasks; new: tasks → bl
        let new = make_alias(&["tasks"], &["bl"]);
        let map = make_map(&[
            (&["bl"], &["backlog"]),
            (&["backlog"], &["tasks"]),
            (&["tasks"], &["bl"]),
        ]);
        assert!(
            would_create_cycle(&map, &new),
            "3-cycle bl → backlog → tasks → bl must be detected"
        );
    }

    #[test]
    fn cycle_detection_no_cycle_simple() {
        // bl → backlog; new: backlog → tasks (no tasks → ... entry)
        let new = make_alias(&["backlog"], &["tasks"]);
        let map = make_map(&[(&["bl"], &["backlog"]), (&["backlog"], &["tasks"])]);
        assert!(
            !would_create_cycle(&map, &new),
            "backlog → tasks with no tasks → ... must not be a cycle"
        );
    }

    #[test]
    fn cycle_detection_pre_existing_disjoint_cycle_terminates() {
        // Pre-existing 2-cycle: a → b, b → a (disjoint from new alias c → d).
        // The visit set must prevent infinite looping.
        let new = make_alias(&["c"], &["d"]);
        let map = make_map(&[(&["a"], &["b"]), (&["b"], &["a"]), (&["c"], &["d"])]);
        assert!(
            !would_create_cycle(&map, &new),
            "a pre-existing disjoint cycle must not implicate the new alias"
        );
    }

    #[test]
    fn cycle_detection_local_shadows_global_not_cycle() {
        // If the post-write view has new local alias bl → tasks replacing
        // global bl → backlog, and backlog → bl exists globally, the cycle
        // check must walk bl → tasks (via the new local entry, not the global
        // backlog → bl chain), and find no cycle if tasks has no entry.
        let new = make_alias(&["bl"], &["tasks"]);
        // Post-write: new local entry replaces global bl → backlog. backlog → bl still present.
        let map = make_map(&[
            (&["bl"], &["tasks"]), // replaced (new local)
            (&["backlog"], &["bl"]),
        ]);
        assert!(
            !would_create_cycle(&map, &new),
            "local alias replacing global must break the old cycle path"
        );
    }

    // ── check_conflict (unit tests using a real AppContext + tempdir) ──────────

    #[test]
    fn check_conflict_rejects_creft_binary_name() {
        let dir = tempfile::tempdir().unwrap();
        let ctx =
            crate::model::AppContext::for_test(dir.path().to_path_buf(), dir.path().to_path_buf());
        let from = vec!["creft".to_string()];
        let err = check_conflict(&ctx, &from).expect_err("creft must be rejected");
        let kind = match err {
            CreftError::AliasConflict { kind, .. } => kind,
            other => panic!("expected AliasConflict, got {other:?}"),
        };
        assert_eq!(kind, "binary name");
    }

    #[test]
    fn check_conflict_rejects_creft_internal_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let ctx =
            crate::model::AppContext::for_test(dir.path().to_path_buf(), dir.path().to_path_buf());
        let from = vec!["_creft".to_string(), "x".to_string()];
        let err = check_conflict(&ctx, &from).expect_err("_creft must be rejected");
        let kind = match err {
            CreftError::AliasConflict { kind, .. } => kind,
            other => panic!("expected AliasConflict, got {other:?}"),
        };
        assert_eq!(kind, "built-in command");
    }

    #[rstest]
    #[case::add("add")]
    #[case::list("list")]
    #[case::alias_builtin("alias")]
    fn check_conflict_rejects_reserved_names(#[case] name: &str) {
        let dir = tempfile::tempdir().unwrap();
        let ctx =
            crate::model::AppContext::for_test(dir.path().to_path_buf(), dir.path().to_path_buf());
        let from = vec![name.to_string()];
        let err = check_conflict(&ctx, &from).expect_err("reserved name must be rejected");
        let kind = match err {
            CreftError::AliasConflict { kind, .. } => kind,
            other => panic!("expected AliasConflict for '{name}', got {other:?}"),
        };
        assert_eq!(kind, "built-in command");
    }
}
