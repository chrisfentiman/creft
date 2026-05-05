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
    is_reserved, namespace_exists, namespace_exists_in_scope, pin_ctx_to_root, resolve_command,
    resolve_in_scope,
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
/// Walks nearest local root first, then farther local roots, then global.
/// Removes the alias from the first scope+root that contains it. One
/// invocation removes from one location — if multiple roots define the same
/// `from`, subsequent invocations remove the next occurrence in chain order.
/// Returns `AliasNotFound` when no location contains the alias.
pub fn cmd_alias_remove(ctx: &AppContext, from: &str) -> Result<(), CreftError> {
    let from_segments: Vec<String> = from.split_whitespace().map(str::to_string).collect();
    if from_segments.is_empty() {
        return Err(CreftError::MissingArg("<from>".into()));
    }

    // Walk every local root nearest-first.
    for root in ctx.local_roots() {
        let pinned = pin_ctx_to_root(ctx, root);
        let mut file = load_for_scope(&pinned, Scope::Local)?;
        if let Some(idx) = file.aliases.iter().position(|a| a.from == from_segments) {
            file.aliases.remove(idx);
            save_for_scope(&pinned, Scope::Local, &file)?;
            eprintln!(
                "removed: {} [{}]",
                from_segments.join(" "),
                render_local_tag(ctx, root)
            );
            return Ok(());
        }
    }

    // Fall through to global.
    let mut file = load_for_scope(ctx, Scope::Global)?;
    if let Some(idx) = file.aliases.iter().position(|a| a.from == from_segments) {
        file.aliases.remove(idx);
        save_for_scope(ctx, Scope::Global, &file)?;
        eprintln!("removed: {} [global]", from_segments.join(" "));
        return Ok(());
    }

    Err(CreftError::AliasNotFound(from_segments.join(" ")))
}

/// `creft alias list`
///
/// Prints all aliases sorted by `from` (lexicographic on the joined string),
/// with scope tags, to stdout. Prints "no aliases defined" when all scopes
/// are empty. A parse failure in any scope propagates as
/// `CreftError::Frontmatter` (exits 1), giving the user the file path to fix.
///
/// When multiple local roots exist, each entry is tagged with its root path
/// relative to CWD (e.g., `[local: .creft]`). Single-root projects use the
/// legacy `[local]` tag for backward-compatible output.
pub fn cmd_alias_list(ctx: &AppContext) -> Result<(), CreftError> {
    // Collect (from_str, to_str, tag_string) for all entries.
    let mut entries: Vec<(String, String, String)> = Vec::new();

    let global = load_for_scope(ctx, Scope::Global)?;
    for a in &global.aliases {
        entries.push((a.from.join(" "), a.to.join(" "), "global".to_owned()));
    }

    for root in ctx.local_roots() {
        let pinned = pin_ctx_to_root(ctx, root);
        let file = load_for_scope(&pinned, Scope::Local)?;
        let tag = render_local_tag(ctx, root);
        for a in &file.aliases {
            entries.push((a.from.join(" "), a.to.join(" "), tag.clone()));
        }
    }

    if entries.is_empty() {
        println!("no aliases defined");
        return Ok(());
    }

    // Stable sort by the joined from string (case-sensitive, lexicographic).
    entries.sort_by(|a, b| a.0.cmp(&b.0));

    for (from_key, to_val, tag) in &entries {
        println!("{} \u{2192} {} [{}]", from_key, to_val, tag);
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

/// Render the tag for a local-root alias entry.
///
/// In single-root projects (`ctx.local_roots().len() <= 1`), returns `"local"`
/// for backward-compatible output. In multi-root projects, returns
/// `"local: <relative-path>"` where `<relative-path>` is `root` printed
/// relative to `ctx.cwd`, falling back to the absolute path when the relative
/// form would traverse too many parent segments.
fn render_local_tag(ctx: &AppContext, root: &std::path::Path) -> String {
    if ctx.local_roots().len() <= 1 {
        return "local".to_owned();
    }
    // Try to produce a clean relative path from CWD to the root.
    match root.strip_prefix(&ctx.cwd) {
        Ok(rel) => format!("local: {}", rel.display()),
        Err(_) => {
            // Walk back: count how many parent segments are needed.
            let rel = pathdiff_simple(&ctx.cwd, root);
            format!("local: {}", rel)
        }
    }
}

/// Produce a simple relative path from `from_dir` to `to`. Uses `..` segments.
/// Falls back to the absolute path when the relative form would traverse more
/// than one level upward, as specified: readability degrades beyond a single
/// `..` component.
fn pathdiff_simple(from_dir: &std::path::Path, to: &std::path::Path) -> String {
    // Find the longest common prefix.
    let from_parts: Vec<_> = from_dir.components().collect();
    let to_parts: Vec<_> = to.components().collect();
    let common = from_parts
        .iter()
        .zip(to_parts.iter())
        .take_while(|(a, b)| a == b)
        .count();
    let up = from_parts.len() - common;
    if up > 1 {
        return to.to_string_lossy().into_owned();
    }
    let mut rel = std::path::PathBuf::new();
    for _ in 0..up {
        rel.push("..");
    }
    for part in &to_parts[common..] {
        rel.push(part);
    }
    rel.to_string_lossy().into_owned()
}

/// Resolve which scope to write the alias into, based on where `to` lives.
///
/// Tries each local root nearest-first, then global. When the target exists in
/// any local root, the write goes to the nearest root (matching `default_write_scope`).
/// Errors with `AliasTargetNotFound` if no scope contains the target.
fn resolve_target_scope(ctx: &AppContext, to: &[String]) -> Result<Scope, CreftError> {
    // Check each local root nearest-first.
    for root in ctx.local_roots() {
        let pinned = pin_ctx_to_root(ctx, root);
        if target_exists_in_scope(&pinned, to, Scope::Local)? {
            // Write goes to the nearest root regardless of which root found it.
            return Ok(Scope::Local);
        }
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
        .sort_by_key(|e| std::cmp::Reverse(e.from.len()));
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

    // ── render_local_tag (Stage 3) ────────────────────────────────────────────

    #[test]
    fn render_local_tag_single_root_returns_legacy_form() {
        let base = tempfile::tempdir().unwrap();
        let home_tmp = tempfile::tempdir().unwrap();
        let creft_dir = base.path().join(".creft");
        std::fs::create_dir_all(&creft_dir).unwrap();
        let ctx = crate::model::AppContext::for_test(
            home_tmp.path().to_path_buf(),
            base.path().to_path_buf(),
        );
        // Single root → legacy "local" tag.
        let tag = render_local_tag(&ctx, &creft_dir);
        assert_eq!(tag, "local", "single root must use legacy 'local' tag");
    }

    #[test]
    fn render_local_tag_multi_root_returns_relative_path() {
        let base = tempfile::tempdir().unwrap();
        let project_dir = base.path().join("project");
        let sub_dir = project_dir.join("sub");
        let home_tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(project_dir.join(".creft")).unwrap();
        std::fs::create_dir_all(sub_dir.join(".creft")).unwrap();
        let ctx =
            crate::model::AppContext::for_test(home_tmp.path().to_path_buf(), sub_dir.clone());
        assert_eq!(
            ctx.local_roots().len(),
            2,
            "test setup: expect 2 local roots"
        );
        // For the nearest root (.creft inside sub_dir), tag should include "local: ".
        let nearest = ctx.local_roots()[0].clone();
        let tag = render_local_tag(&ctx, &nearest);
        assert!(
            tag.starts_with("local: "),
            "multi-root tag must start with 'local: '; got: {tag}"
        );
    }

    // ── cmd_alias_remove chain walk (Stage 3) ─────────────────────────────────

    #[test]
    fn cmd_alias_remove_nearest_first_removes_from_nearest_root() {
        let base = tempfile::tempdir().unwrap();
        let project_dir = base.path().join("project");
        let sub_dir = project_dir.join("sub");
        let home_tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(project_dir.join(".creft")).unwrap();
        std::fs::create_dir_all(sub_dir.join(".creft")).unwrap();
        let ctx =
            crate::model::AppContext::for_test(home_tmp.path().to_path_buf(), sub_dir.clone());
        assert_eq!(
            ctx.local_roots().len(),
            2,
            "test setup: expect 2 local roots"
        );

        // Write `bl → backlog` in both local roots.
        let alias_content = b"- from: bl\n  to: backlog\n";
        for root in ctx.local_roots() {
            std::fs::write(root.join("aliases.yaml"), alias_content).unwrap();
        }

        // First removal removes from the nearest root.
        cmd_alias_remove(&ctx, "bl").expect("first removal must succeed");

        // The nearest root's aliases.yaml must no longer contain `bl`.
        let nearest = &ctx.local_roots()[0];
        let nearest_content =
            std::fs::read_to_string(nearest.join("aliases.yaml")).unwrap_or_default();
        assert!(
            !nearest_content.contains("from: bl"),
            "nearest root's alias.yaml must not contain 'bl' after first removal; got:\n{nearest_content}"
        );

        // The farthest root's aliases.yaml must still contain `bl`.
        let farthest = &ctx.local_roots()[1];
        let farthest_content =
            std::fs::read_to_string(farthest.join("aliases.yaml")).unwrap_or_default();
        assert!(
            farthest_content.contains("bl"),
            "farthest root's alias.yaml must still contain 'bl'; got:\n{farthest_content}"
        );
    }

    #[test]
    fn cmd_alias_remove_not_found_returns_alias_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let home_tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".creft")).unwrap();
        let ctx = crate::model::AppContext::for_test(
            home_tmp.path().to_path_buf(),
            dir.path().to_path_buf(),
        );
        let err = cmd_alias_remove(&ctx, "nonexistent").expect_err("missing alias must error");
        assert!(matches!(err, CreftError::AliasNotFound(_)));
    }
}
