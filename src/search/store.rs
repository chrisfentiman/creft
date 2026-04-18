use std::path::PathBuf;

use crate::error::CreftError;
use crate::help::{self, BuiltinHelp};
use crate::model::{AppContext, Scope, SkillSource};
use crate::namespace::skill_namespace;
use crate::store as skill_store;

use super::index::SearchIndex;

/// Compute the index file path for a namespace within a scope.
///
/// - Namespace `"deploy"` in global scope → `~/.creft/indexes/deploy.idx`
/// - Empty namespace (root skills) → `<scope_root>/indexes/_root.idx`
/// - Plugin `"acme"` namespace `"deploy"` → `<scope_root>/indexes/acme.deploy.idx`
///
/// The `plugin_prefix` parameter carries the plugin name when the namespace
/// belongs to a plugin. Pass `None` for owned and package skills.
pub(crate) fn index_path(
    ctx: &AppContext,
    namespace: &str,
    scope: Scope,
    plugin_prefix: Option<&str>,
) -> Result<PathBuf, CreftError> {
    let dir = ctx.index_dir_for(scope)?;
    let filename = match (plugin_prefix, namespace) {
        (Some(plugin), ns) if !ns.is_empty() => format!("{}.{}.idx", plugin, ns),
        (Some(plugin), _) => format!("{}.idx", plugin),
        (None, ns) if !ns.is_empty() => format!("{}.idx", ns),
        (None, _) => "_root.idx".to_owned(),
    };
    Ok(dir.join(filename))
}

/// Rebuild the search index for a single namespace within a scope.
///
/// Lists all owned skills in the given scope whose name falls in the namespace,
/// tokenizes each skill's prose (frontmatter description + body text with
/// code blocks stripped), builds a `SearchIndex`, and writes it atomically
/// to disk.
///
/// The `namespace` parameter is the top-level namespace token (e.g., `"deploy"`
/// for skills like `"deploy rollback"`). An empty string addresses root-level
/// skills (e.g., `"hello"`).
///
/// For indexing skills from all sources (owned, package, plugin), use
/// `rebuild_all_indexes` instead.
pub(crate) fn rebuild_namespace_index(
    ctx: &AppContext,
    namespace: &str,
    scope: Scope,
) -> Result<(), CreftError> {
    let all = skill_store::list_all_in(ctx, scope)?;

    let namespace_skills: Vec<_> = all
        .into_iter()
        .filter(|def| skill_namespace(&def.name) == namespace)
        .collect();

    let mut documents: Vec<(String, String, String)> = Vec::new();

    for def in &namespace_skills {
        let path = skill_store::name_to_path_in(ctx, &def.name, scope)?;
        let raw = match std::fs::read_to_string(&path) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("warning: could not read {}: {}", path.display(), e);
                continue;
            }
        };
        let body_text = extract_indexable_text(&raw, &def.description);
        documents.push((def.name.clone(), def.description.clone(), body_text));
    }

    let refs: Vec<(&str, &str, &str)> = documents
        .iter()
        .map(|(n, d, t)| (n.as_str(), d.as_str(), t.as_str()))
        .collect();
    let index = SearchIndex::build(&refs);

    write_index(ctx, namespace, scope, None, &index)
}

/// Load a search index from disk.
///
/// Returns `None` if the file does not exist or is corrupt. The caller decides
/// whether to rebuild; this function never triggers a rebuild automatically.
#[allow(dead_code)]
pub(crate) fn load_index(
    ctx: &AppContext,
    namespace: &str,
    scope: Scope,
) -> Result<Option<SearchIndex>, CreftError> {
    let path = index_path(ctx, namespace, scope, None)?;
    if !path.exists() {
        return Ok(None);
    }
    let bytes = std::fs::read(&path)?;
    Ok(SearchIndex::from_bytes(&bytes))
}

/// Rebuild all indexes across all scopes, including built-in command docs.
///
/// Iterates all skills from all sources (owned, package, and plugin) using
/// `list_all_with_source`, groups them by `(namespace, plugin_prefix, scope)`,
/// and writes one index file per group. Plugin skills are indexed under their
/// plugin-prefixed path (e.g., `acme.deploy.idx`). Also rebuilds the built-in
/// command index (`_builtin.idx`).
pub(crate) fn rebuild_all_indexes(ctx: &AppContext) -> Result<(), CreftError> {
    use std::collections::HashMap;

    use crate::model::CommandDef;

    // Groups skills for indexing. Key: (namespace, Option<plugin_name>, scope).
    type IndexGroup = HashMap<(String, Option<String>, Scope), Vec<(CommandDef, SkillSource)>>;

    let mut groups: IndexGroup = HashMap::new();

    let all = skill_store::list_all_with_source(ctx)?;

    for (def, source) in all {
        let ns = skill_namespace(&def.name).to_owned();
        let key = match &source {
            SkillSource::Plugin(plugin_name) => (ns, Some(plugin_name.clone()), Scope::Global),
            SkillSource::Owned(scope) => (ns, None, *scope),
            SkillSource::Package(_, scope) => (ns, None, *scope),
        };
        groups.entry(key).or_default().push((def, source));
    }

    for ((ns, plugin_prefix, scope), skills) in &groups {
        let mut documents: Vec<(String, String, String)> = Vec::new();

        for (def, source) in skills {
            let raw = match skill_store::read_raw_from(ctx, &def.name, source) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("warning: could not read '{}': {}", def.name, e);
                    continue;
                }
            };
            let body_text = extract_indexable_text(&raw, &def.description);
            documents.push((def.name.clone(), def.description.clone(), body_text));
        }

        let refs: Vec<(&str, &str, &str)> = documents
            .iter()
            .map(|(n, d, t)| (n.as_str(), d.as_str(), t.as_str()))
            .collect();
        let index = SearchIndex::build(&refs);

        let plugin_prefix_ref = plugin_prefix.as_deref();
        if let Err(e) = write_index(ctx, ns, *scope, plugin_prefix_ref, &index) {
            eprintln!(
                "warning: could not write index for namespace '{}': {}",
                ns, e
            );
        }
    }

    rebuild_builtin_index(ctx)?;

    Ok(())
}

/// Rebuild the built-in command docs index.
///
/// Iterates all `BuiltinHelp` variants, strips code blocks from each
/// command's full docs text, and builds a `SearchIndex` written to
/// `<global_root>/indexes/_builtin.idx`.
///
/// Each entry's name is the built-in command's CLI name (e.g., `"add"`,
/// `"plugin install"`). The description is the first non-empty line of
/// the command's docs text.
pub(crate) fn rebuild_builtin_index(ctx: &AppContext) -> Result<(), CreftError> {
    let mut documents: Vec<(String, String, String)> = Vec::new();

    for &variant in BuiltinHelp::all_variants() {
        let cli_name = variant.cli_name();
        let docs_text = help::render_docs(variant);
        let stripped = strip_code_blocks_plain(&docs_text);
        let description = first_nonempty_line(&stripped)
            .unwrap_or(cli_name)
            .to_owned();
        documents.push((cli_name.to_owned(), description, stripped));
    }

    let refs: Vec<(&str, &str, &str)> = documents
        .iter()
        .map(|(n, d, t)| (n.as_str(), d.as_str(), t.as_str()))
        .collect();
    let index = SearchIndex::build(&refs);

    let dir = ctx.index_dir_for(Scope::Global)?;
    std::fs::create_dir_all(&dir)?;

    let path = dir.join("_builtin.idx");
    write_index_bytes(&path, &index.to_bytes())
}

// ── internal helpers ──────────────────────────────────────────────────────────

/// Write an index to disk for a namespace, creating the directory if needed.
///
/// Uses a write-then-rename strategy so readers never see a partial file.
fn write_index(
    ctx: &AppContext,
    namespace: &str,
    scope: Scope,
    plugin_prefix: Option<&str>,
    index: &SearchIndex,
) -> Result<(), CreftError> {
    let path = index_path(ctx, namespace, scope, plugin_prefix)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    write_index_bytes(&path, &index.to_bytes())
}

/// Write bytes to a path atomically via a temp file and rename.
fn write_index_bytes(path: &std::path::Path, bytes: &[u8]) -> Result<(), CreftError> {
    let dir = path.parent().unwrap_or(path);
    // Write to a temp file in the same directory, then atomically rename.
    let tmp_path = dir.join(format!(
        ".tmp-{}.idx",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0)
    ));
    std::fs::write(&tmp_path, bytes)?;
    std::fs::rename(&tmp_path, path)?;
    Ok(())
}

/// Extract indexable text from a raw skill file.
///
/// Returns the frontmatter description plus the body with executable code
/// blocks stripped (plain text, no ANSI). The description is prepended so
/// that filtering on the description field also matches search terms.
fn extract_indexable_text(raw_content: &str, description: &str) -> String {
    // Split on the closing frontmatter delimiter `---`.
    // The raw content starts with `---\n<yaml>\n---\n<body>`.
    let body = if let Some(after_open) = raw_content.strip_prefix("---") {
        // Find the second `---` delimiter.
        if let Some(close_pos) = after_open.find("\n---") {
            after_open[close_pos + 4..].to_owned()
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    let stripped_body = strip_code_blocks_plain(&body);
    if description.is_empty() {
        stripped_body
    } else {
        format!("{}\n{}", description, stripped_body)
    }
}

/// Strip fenced code blocks from text, returning plain text without ANSI.
///
/// Executable code blocks (bash, python, etc.) are dropped entirely.
/// `docs` block content is preserved (fence delimiters dropped).
/// Lines outside fences are passed through unchanged.
/// No ANSI bold is applied to header lines.
pub(crate) fn strip_code_blocks_plain(text: &str) -> String {
    let mut out = String::new();
    let mut in_fence = false;
    let mut fence_backtick_count = 0usize;
    let mut fence_is_docs = false;

    for line in text.lines() {
        let trimmed = line.trim_start();

        if !in_fence {
            if trimmed.starts_with("```") {
                let count = trimmed.chars().take_while(|c| *c == '`').count();
                if count >= 3 {
                    let lang = trimmed[count..].trim();
                    in_fence = true;
                    fence_backtick_count = count;
                    fence_is_docs = lang == "docs";
                    continue;
                }
            }
            // Emit the line as-is (no ANSI).
            out.push_str(line);
            out.push('\n');
        } else {
            let closing = "`".repeat(fence_backtick_count);
            if trimmed.starts_with(closing.as_str())
                && trimmed[fence_backtick_count..].trim().is_empty()
            {
                in_fence = false;
                fence_backtick_count = 0;
                fence_is_docs = false;
                continue;
            }
            if fence_is_docs {
                out.push_str(line);
                out.push('\n');
            }
        }
    }

    out
}

/// Return the first non-empty, non-whitespace line from a text.
fn first_nonempty_line(text: &str) -> Option<&str> {
    text.lines().find(|l| !l.trim().is_empty())
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn make_ctx() -> (AppContext, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = AppContext::for_test_with_creft_home(
            tmp.path().to_path_buf(),
            tmp.path().to_path_buf(),
        );
        (ctx, tmp)
    }

    fn write_skill(ctx: &AppContext, name: &str, description: &str, body: &str) {
        let content = format!("---\nname: {name}\ndescription: {description}\n---\n\n{body}");
        skill_store::save(ctx, &content, false, Scope::Global).unwrap();
    }

    // ── index_path ────────────────────────────────────────────────────────────

    #[test]
    fn index_path_named_namespace_returns_dotted_idx() {
        let (ctx, _tmp) = make_ctx();
        let path = index_path(&ctx, "deploy", Scope::Global, None).unwrap();
        assert!(path.ends_with("indexes/deploy.idx"));
    }

    #[test]
    fn index_path_root_namespace_returns_root_idx() {
        let (ctx, _tmp) = make_ctx();
        let path = index_path(&ctx, "", Scope::Global, None).unwrap();
        assert!(path.ends_with("indexes/_root.idx"));
    }

    #[test]
    fn index_path_with_plugin_prefix_returns_dotted_idx() {
        let (ctx, _tmp) = make_ctx();
        let path = index_path(&ctx, "deploy", Scope::Global, Some("acme")).unwrap();
        assert!(path.ends_with("indexes/acme.deploy.idx"));
    }

    // ── load_index ────────────────────────────────────────────────────────────

    #[test]
    fn load_index_returns_none_for_nonexistent_file() {
        let (ctx, _tmp) = make_ctx();
        let result = load_index(&ctx, "nonexistent", Scope::Global).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn load_index_returns_none_for_corrupt_bytes() {
        let (ctx, _tmp) = make_ctx();
        let dir = ctx.index_dir_for(Scope::Global).unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("corrupt.idx");
        std::fs::write(&path, b"\xFF\xFF\xFF\xFF\xFF").unwrap();
        let result = load_index(&ctx, "corrupt", Scope::Global).unwrap();
        assert!(result.is_none());
    }

    // ── rebuild_namespace_index ───────────────────────────────────────────────

    #[test]
    fn rebuild_namespace_index_creates_index_file_on_disk() {
        let (ctx, _tmp) = make_ctx();
        write_skill(
            &ctx,
            "deploy rollback",
            "Roll back a deployment",
            "rollback procedure template\n",
        );

        rebuild_namespace_index(&ctx, "deploy", Scope::Global).unwrap();

        let path = index_path(&ctx, "deploy", Scope::Global, None).unwrap();
        assert!(path.exists(), "index file must be created after rebuild");
    }

    #[test]
    fn rebuild_namespace_index_is_queryable_after_write() {
        let (ctx, _tmp) = make_ctx();
        write_skill(
            &ctx,
            "deploy rollback",
            "Roll back a deployment",
            "rollback procedure template\n",
        );
        write_skill(
            &ctx,
            "deploy push",
            "Push a build",
            "push artifact to environment\n",
        );

        rebuild_namespace_index(&ctx, "deploy", Scope::Global).unwrap();

        let index = load_index(&ctx, "deploy", Scope::Global)
            .unwrap()
            .expect("index must exist");
        assert_eq!(index.len(), 2);
        let results = index.search("rollback");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "deploy rollback");
    }

    #[test]
    fn rebuild_namespace_index_excludes_skills_from_other_namespaces() {
        let (ctx, _tmp) = make_ctx();
        write_skill(&ctx, "deploy rollback", "Roll back", "deploy content\n");
        write_skill(&ctx, "test run", "Run tests", "test content\n");

        rebuild_namespace_index(&ctx, "deploy", Scope::Global).unwrap();

        let index = load_index(&ctx, "deploy", Scope::Global)
            .unwrap()
            .expect("index must exist");
        assert_eq!(
            index.len(),
            1,
            "only deploy namespace skills in deploy index"
        );
    }

    // ── rebuild_all_indexes ───────────────────────────────────────────────────

    #[test]
    fn rebuild_all_indexes_creates_index_for_each_namespace() {
        let (ctx, _tmp) = make_ctx();
        write_skill(&ctx, "deploy rollback", "Roll back", "deploy content\n");
        write_skill(&ctx, "test run", "Run tests", "test content\n");

        rebuild_all_indexes(&ctx).unwrap();

        let deploy_idx = load_index(&ctx, "deploy", Scope::Global).unwrap();
        let test_idx = load_index(&ctx, "test", Scope::Global).unwrap();
        assert!(deploy_idx.is_some(), "deploy index must be created");
        assert!(test_idx.is_some(), "test index must be created");
    }

    #[test]
    fn rebuild_all_indexes_creates_builtin_idx() {
        let (ctx, _tmp) = make_ctx();

        rebuild_all_indexes(&ctx).unwrap();

        let dir = ctx.index_dir_for(Scope::Global).unwrap();
        assert!(
            dir.join("_builtin.idx").exists(),
            "_builtin.idx must be created by rebuild_all_indexes"
        );
    }

    // ── rebuild_builtin_index ─────────────────────────────────────────────────

    #[test]
    fn rebuild_builtin_index_contains_entries_for_all_variants() {
        let (ctx, _tmp) = make_ctx();
        rebuild_builtin_index(&ctx).unwrap();

        let dir = ctx.index_dir_for(Scope::Global).unwrap();
        let bytes = std::fs::read(dir.join("_builtin.idx")).unwrap();
        let index = SearchIndex::from_bytes(&bytes).expect("_builtin.idx must deserialize");

        // The index must have one entry per BuiltinHelp variant.
        assert_eq!(
            index.len(),
            BuiltinHelp::all_variants().len(),
            "builtin index must have one entry per variant"
        );
    }

    #[test]
    fn rebuild_builtin_index_contains_add_entry() {
        let (ctx, _tmp) = make_ctx();
        rebuild_builtin_index(&ctx).unwrap();

        let dir = ctx.index_dir_for(Scope::Global).unwrap();
        let bytes = std::fs::read(dir.join("_builtin.idx")).unwrap();
        let index = SearchIndex::from_bytes(&bytes).unwrap();

        let names: Vec<&str> = index.search("").iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"add"), "builtin index must contain 'add'");
    }

    #[test]
    fn rebuild_builtin_index_is_searchable_for_builtin_term() {
        let (ctx, _tmp) = make_ctx();
        rebuild_builtin_index(&ctx).unwrap();

        let dir = ctx.index_dir_for(Scope::Global).unwrap();
        let bytes = std::fs::read(dir.join("_builtin.idx")).unwrap();
        let index = SearchIndex::from_bytes(&bytes).unwrap();

        // "frontmatter" appears in ADD_LONG_ABOUT — searching for it should return "add".
        let results = index.search("frontmatter");
        assert!(
            results.iter().any(|e| e.name == "add"),
            "searching 'frontmatter' in builtin index must return 'add'"
        );
    }

    #[test]
    fn rebuild_builtin_index_returns_empty_for_unknown_term() {
        let (ctx, _tmp) = make_ctx();
        rebuild_builtin_index(&ctx).unwrap();

        let dir = ctx.index_dir_for(Scope::Global).unwrap();
        let bytes = std::fs::read(dir.join("_builtin.idx")).unwrap();
        let index = SearchIndex::from_bytes(&bytes).unwrap();

        let results = index.search("zzz_term_that_cannot_appear_in_any_builtin");
        assert!(results.is_empty(), "unknown term must return no results");
    }

    #[test]
    fn builtin_index_round_trips_via_load_index() {
        let (ctx, _tmp) = make_ctx();
        rebuild_builtin_index(&ctx).unwrap();

        // load_index doesn't directly handle "_builtin" — load it manually to
        // verify the SearchIndex format matches what load_index produces.
        let dir = ctx.index_dir_for(Scope::Global).unwrap();
        let bytes = std::fs::read(dir.join("_builtin.idx")).unwrap();
        let index = SearchIndex::from_bytes(&bytes).expect("round-trip must succeed");

        let results = index.search("plugin");
        assert!(
            !results.is_empty(),
            "searching 'plugin' must return at least one builtin"
        );
    }

    // ── remove_in index rebuild ───────────────────────────────────────────────

    #[test]
    fn remove_in_rebuilds_namespace_index_without_removed_skill() {
        let (ctx, _tmp) = make_ctx();
        write_skill(
            &ctx,
            "deploy rollback",
            "Roll back a deployment",
            "rollback procedure steps\n",
        );
        write_skill(
            &ctx,
            "deploy push",
            "Push a build to an environment",
            "push artifact to environment\n",
        );

        // Explicitly rebuild so the index reflects both skills.
        rebuild_namespace_index(&ctx, "deploy", Scope::Global).unwrap();

        let index_before = load_index(&ctx, "deploy", Scope::Global)
            .unwrap()
            .expect("index must exist before remove");
        assert_eq!(
            index_before.len(),
            2,
            "index must contain both skills before remove"
        );

        // Remove one skill — the remove_in implementation rebuilds the index.
        skill_store::remove_in(&ctx, "deploy rollback", Scope::Global).unwrap();

        let index_after = load_index(&ctx, "deploy", Scope::Global)
            .unwrap()
            .expect("index must still exist after remove");
        assert_eq!(
            index_after.len(),
            1,
            "index must contain only the remaining skill after remove"
        );
        let names: Vec<&str> = index_after
            .search("")
            .iter()
            .map(|e| e.name.as_str())
            .collect();
        assert!(
            names.contains(&"deploy push"),
            "remaining skill 'deploy push' must be in the index"
        );
        assert!(
            !names.contains(&"deploy rollback"),
            "removed skill 'deploy rollback' must not be in the index"
        );
    }

    // ── strip_code_blocks_plain ───────────────────────────────────────────────

    #[test]
    fn strip_code_blocks_plain_removes_bash_blocks() {
        let text = "Some prose.\n\n```bash\necho hello\n```\n\nMore prose.\n";
        let result = strip_code_blocks_plain(text);
        assert!(!result.contains("echo hello"));
        assert!(result.contains("Some prose."));
        assert!(result.contains("More prose."));
    }

    #[test]
    fn strip_code_blocks_plain_preserves_docs_block_content() {
        let text = "Prose.\n\n```docs\nThis is docs content.\n```\n";
        let result = strip_code_blocks_plain(text);
        assert!(result.contains("This is docs content."));
        assert!(!result.contains("```"));
    }

    #[test]
    fn strip_code_blocks_plain_emits_no_ansi_for_headers() {
        let text = "# My Header\n\nSome text.\n";
        let result = strip_code_blocks_plain(text);
        // No ANSI escape sequences.
        assert!(!result.contains('\x1b'));
        assert!(result.contains("# My Header"));
    }
}
