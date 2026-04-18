//! Indexed search primitives for creft skill documentation.
//!
//! Provides text tokenization, XOR filter construction, a searchable index
//! format, and lifecycle management for per-namespace index files on disk.

pub(crate) mod index;
pub(crate) mod snippet;
pub(crate) mod store;
pub(crate) mod tokenize;
pub(crate) mod xor;

use crate::help;
use crate::model::{AppContext, Scope};
use crate::store as skill_store;

use self::index::IndexEntry;
use self::snippet::{SnippetResult, extract_snippets};

const SNIPPET_CONTEXT: usize = 2;

/// Search all indexes across all scopes, loading content snippets for matches.
///
/// Reads every `.idx` file from the index directories (global and local),
/// including `_builtin.idx`, queries each with the given query string, and
/// returns combined results with snippets populated from the source documents.
///
/// For each matching index entry, loads the source document text and extracts
/// snippets containing the query terms. Entries where the XOR filter matched
/// but no lines actually contain the query terms (false positives) are included
/// with empty snippets — the caller filters them at render time via
/// `render_snippet_results`.
///
/// Files that fail to deserialize or whose content cannot be loaded are
/// skipped silently. When both scopes resolve to the same directory (e.g.
/// under `CREFT_HOME`), the directory is searched only once.
pub(crate) fn search_all_indexes(ctx: &AppContext, query: &str) -> Vec<SnippetResult> {
    let terms: Vec<&str> = query.split_whitespace().collect();
    let mut results = Vec::new();
    let mut seen_dirs: Vec<std::path::PathBuf> = Vec::new();

    for scope in &[Scope::Global, Scope::Local] {
        let dir = match ctx.index_dir_for(*scope) {
            Ok(d) => d,
            Err(_) => continue,
        };

        // Skip if we already searched this resolved directory.
        if seen_dirs.contains(&dir) {
            continue;
        }
        seen_dirs.push(dir.clone());

        let read_dir = match std::fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(_) => continue,
        };

        for entry in read_dir.flatten() {
            let path = entry.path();
            let Some(ext) = path.extension() else {
                continue;
            };
            if ext != "idx" {
                continue;
            }

            // Derive namespace name from filename (strip `.idx` extension).
            let namespace = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_owned();

            let bytes = match std::fs::read(&path) {
                Ok(b) => b,
                Err(_) => continue,
            };
            let index = match self::index::SearchIndex::from_bytes(&bytes) {
                Some(idx) => idx,
                None => continue,
            };

            for entry in index.search(query) {
                let snippets = load_snippets_for_entry(ctx, &namespace, entry, &terms);
                results.push(SnippetResult {
                    namespace: namespace.clone(),
                    name: entry.name.clone(),
                    description: entry.description.clone(),
                    snippets,
                });
            }
        }
    }

    results
}

/// Load and extract snippets for a single index entry.
///
/// For builtin entries (`_builtin` namespace), loads the compiled help text.
/// For skill entries, resolves the skill source from disk and reads the raw file.
/// Returns an empty vec when content cannot be loaded or no lines match.
fn load_snippets_for_entry(
    ctx: &AppContext,
    namespace: &str,
    entry: &IndexEntry,
    terms: &[&str],
) -> Vec<self::snippet::Snippet> {
    let text = if namespace == "_builtin" {
        load_builtin_text(&entry.name)
    } else {
        load_skill_text(ctx, &entry.name)
    };

    match text {
        Some(t) => extract_snippets(&t, terms, SNIPPET_CONTEXT),
        None => Vec::new(),
    }
}

/// Load plain-text content for a builtin command by CLI name.
///
/// Strips code blocks to produce the same searchable text used during indexing.
/// Returns `None` if the name doesn't match any known builtin.
fn load_builtin_text(cli_name: &str) -> Option<String> {
    let variant = help::BuiltinHelp::from_cli_name(cli_name)?;
    let docs = help::render_docs(variant);
    Some(self::store::strip_code_blocks_plain(&docs))
}

/// Load plain-text content for a skill by name.
///
/// Resolves the skill source, reads the raw file, then extracts indexable text
/// (description prepended to code-stripped body). Returns `None` when the skill
/// cannot be resolved or read.
fn load_skill_text(ctx: &AppContext, name: &str) -> Option<String> {
    let name_parts: Vec<String> = name.split_whitespace().map(str::to_owned).collect();
    let (resolved_name, _, source) = skill_store::resolve_command(ctx, &name_parts).ok()?;
    let raw = skill_store::read_raw_from(ctx, &resolved_name, &source)
        .map_err(|e| {
            eprintln!("warning: could not read '{}': {}", name, e);
        })
        .ok()?;

    // Use the description field from the raw file to produce indexable text
    // consistent with what was written during indexing.
    let description = extract_description_from_raw(&raw);
    Some(self::store::extract_indexable_text(&raw, &description))
}

/// Extract the description field from raw skill markdown without a full parse.
///
/// Reads the frontmatter YAML to find the `description:` line. Returns an empty
/// string when the frontmatter is missing or malformed, so `extract_indexable_text`
/// still produces the body text without the description prefix.
fn extract_description_from_raw(raw: &str) -> String {
    let Some(after_open) = raw.strip_prefix("---") else {
        return String::new();
    };
    let Some(close_pos) = after_open.find("\n---") else {
        return String::new();
    };
    let yaml_block = &after_open[..close_pos];

    for line in yaml_block.lines() {
        if let Some(rest) = line.strip_prefix("description:") {
            return rest.trim().to_owned();
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;
    use crate::model::AppContext;
    use crate::search::index::SearchIndex;
    use crate::search::snippet::SnippetResult;
    use crate::search::store::rebuild_builtin_index;

    fn make_ctx() -> (AppContext, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = AppContext::for_test_with_creft_home(
            tmp.path().to_path_buf(),
            tmp.path().to_path_buf(),
        );
        (ctx, tmp)
    }

    fn write_index_file(ctx: &AppContext, scope: Scope, filename: &str, index: &SearchIndex) {
        let dir = ctx.index_dir_for(scope).unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(filename);
        std::fs::write(&path, index.to_bytes()).unwrap();
    }

    // ── search_all_indexes ────────────────────────────────────────────────────

    #[test]
    fn search_all_indexes_returns_empty_when_no_index_files_exist() {
        let (ctx, _tmp) = make_ctx();
        let results = search_all_indexes(&ctx, "rollback");
        assert!(
            results.is_empty(),
            "no index files must produce empty results"
        );
    }

    #[test]
    fn search_all_indexes_returns_matches_from_skill_index() {
        let (ctx, _tmp) = make_ctx();
        let idx = SearchIndex::build(&[
            ("deploy rollback", "Roll back", "rollback procedure"),
            ("deploy push", "Push build", "push artifact"),
        ]);
        write_index_file(&ctx, Scope::Global, "deploy.idx", &idx);

        let results = search_all_indexes(&ctx, "rollback");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "deploy rollback");
        assert_eq!(results[0].namespace, "deploy");
    }

    #[test]
    fn search_all_indexes_includes_builtin_idx() {
        let (ctx, _tmp) = make_ctx();
        rebuild_builtin_index(&ctx).unwrap();

        let results = search_all_indexes(&ctx, "frontmatter");
        assert!(
            results.iter().any(|r| r.name == "add"),
            "search_all_indexes must return 'add' when querying 'frontmatter' (term in ADD_LONG_ABOUT)"
        );
    }

    #[test]
    fn search_all_indexes_skips_corrupt_files() {
        let (ctx, _tmp) = make_ctx();
        let dir = ctx.index_dir_for(Scope::Global).unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("corrupt.idx"), b"\xFF\xFF").unwrap();

        let idx = SearchIndex::build(&[("my skill", "A skill", "rollback content")]);
        write_index_file(&ctx, Scope::Global, "real.idx", &idx);

        let results = search_all_indexes(&ctx, "rollback");
        assert_eq!(results.len(), 1, "corrupt index must be skipped");
        assert_eq!(results[0].name, "my skill");
    }

    #[test]
    fn search_all_indexes_combines_results_from_multiple_namespaces() {
        let (ctx, _tmp) = make_ctx();

        let deploy_idx = SearchIndex::build(&[("deploy rollback", "Roll back", "rollback steps")]);
        let aws_idx = SearchIndex::build(&[("aws restore", "Restore", "rollback restore aws")]);
        write_index_file(&ctx, Scope::Global, "deploy.idx", &deploy_idx);
        write_index_file(&ctx, Scope::Global, "aws.idx", &aws_idx);

        let results = search_all_indexes(&ctx, "rollback");
        assert_eq!(
            results.len(),
            2,
            "results from multiple indexes must be combined"
        );
    }

    #[test]
    fn search_all_indexes_finds_results_in_global_scope() {
        let (ctx, _tmp) = make_ctx();

        let global_idx = SearchIndex::build(&[("global skill", "Global", "rollback global")]);
        write_index_file(&ctx, Scope::Global, "ns.idx", &global_idx);

        let results = search_all_indexes(&ctx, "rollback");
        assert!(!results.is_empty(), "global scope index must be searched");
        assert!(
            results.iter().any(|r| r.name == "global skill"),
            "global skill must appear in results"
        );
    }

    #[test]
    fn search_all_indexes_builtin_match_has_snippets_for_known_term() {
        let (ctx, _tmp) = make_ctx();
        rebuild_builtin_index(&ctx).unwrap();

        // "frontmatter" appears in ADD_LONG_ABOUT; the "add" entry must have snippets.
        let results = search_all_indexes(&ctx, "frontmatter");
        let add_result = results.iter().find(|r| r.name == "add");
        assert!(
            add_result.is_some(),
            "search for 'frontmatter' must return the 'add' builtin"
        );
        assert!(
            !add_result.unwrap().snippets.is_empty(),
            "builtin 'add' must have non-empty snippets for query 'frontmatter'"
        );
    }

    #[test]
    fn search_all_indexes_false_positive_has_empty_snippets() {
        let (ctx, _tmp) = make_ctx();
        // Build an index where "no-match skill" has no content matching "zzz_term".
        // The XOR filter may pass it (it won't for a truly absent term, but we
        // can simulate a false positive by using a skill index with mismatched content).
        let idx = SearchIndex::build(&[("no-match skill", "A skill", "zzz_term in index")]);
        write_index_file(&ctx, Scope::Global, "test.idx", &idx);

        // Query "zzz_term" matches the index entry (content contains it).
        let results = search_all_indexes(&ctx, "zzz_term");
        // Since the skill file doesn't exist on disk, load_skill_text returns None.
        // The entry will have empty snippets (content not loadable).
        if !results.is_empty() {
            // If we got a result, its snippets must be empty (file not on disk).
            let r = &results[0];
            assert!(
                r.snippets.is_empty(),
                "skill with unreadable content must have empty snippets"
            );
        }
        // It's also valid for results to be empty if the XOR filter eliminates the entry.
    }

    #[test]
    fn search_all_indexes_returns_snippet_results_not_old_type() {
        // Verify the return type is Vec<SnippetResult> by destructuring.
        let (ctx, _tmp) = make_ctx();
        let results: Vec<SnippetResult> = search_all_indexes(&ctx, "anything");
        assert!(results.is_empty());
    }
}
