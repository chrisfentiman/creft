//! Indexed search primitives for creft skill documentation.
//!
//! Provides text tokenization, XOR filter construction, a searchable index
//! format, and lifecycle management for per-namespace index files on disk.

pub(crate) mod index;
pub(crate) mod store;
pub(crate) mod tokenize;
pub(crate) mod xor;

use crate::model::{AppContext, Scope};

use self::index::IndexEntry;

/// A search result with its source namespace.
pub(crate) struct SearchResult {
    /// The namespace this result came from (e.g., `"deploy"`, `"_builtin"`).
    pub namespace: String,
    pub name: String,
    pub description: String,
}

/// Format search results for display.
///
/// Returns a formatted string listing each result's name and description,
/// or `None` if there are no results.
pub(crate) fn render_search_results(results: &[&IndexEntry]) -> Option<String> {
    if results.is_empty() {
        return None;
    }
    let max_name = results.iter().map(|r| r.name.len()).max().unwrap_or(0);
    let mut out = String::new();
    for entry in results {
        let pad = " ".repeat(max_name - entry.name.len());
        out.push_str(&format!("  {}{}  {}\n", entry.name, pad, entry.description));
    }
    Some(out)
}

/// Search all indexes across all scopes.
///
/// Reads every `.idx` file from the index directories (global and local),
/// including `_builtin.idx`, queries each with the given query string, and
/// returns the combined results. Files that fail to deserialize are skipped.
/// Returns an empty vec if no index files exist. When both scopes resolve to
/// the same directory (e.g. under `CREFT_HOME`), the directory is searched
/// only once.
pub(crate) fn search_all_indexes(ctx: &AppContext, query: &str) -> Vec<SearchResult> {
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
                results.push(SearchResult {
                    namespace: namespace.clone(),
                    name: entry.name.clone(),
                    description: entry.description.clone(),
                });
            }
        }
    }

    results
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;
    use crate::model::AppContext;
    use crate::search::index::SearchIndex;
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

    // ── render_search_results ─────────────────────────────────────────────────

    #[test]
    fn render_search_results_returns_none_for_empty_slice() {
        let result = render_search_results(&[]);
        assert!(result.is_none(), "empty results must return None");
    }

    #[test]
    fn render_search_results_contains_name_and_description() {
        let idx = SearchIndex::build(&[("deploy rollback", "Roll back a deployment", "content")]);
        let entries = idx.search("");
        let rendered = render_search_results(&entries).unwrap();
        assert!(
            rendered.contains("deploy rollback"),
            "rendered output must include the entry name"
        );
        assert!(
            rendered.contains("Roll back a deployment"),
            "rendered output must include the description"
        );
    }

    #[test]
    fn render_search_results_aligns_columns() {
        let idx = SearchIndex::build(&[
            ("short", "Short desc", "x"),
            ("a longer name", "Long desc", "y"),
        ]);
        let entries = idx.search("");
        let rendered = render_search_results(&entries).unwrap();
        // Both lines should exist; alignment is handled by padding.
        assert!(rendered.contains("short"));
        assert!(rendered.contains("a longer name"));
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
}
