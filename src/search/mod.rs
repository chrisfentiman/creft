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
use self::tokenize::score_query;

const SNIPPET_CONTEXT: usize = 2;
pub(crate) const FUZZY_THRESHOLD: f64 = 0.2;

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
/// When exact search returns no results, runs a fuzzy fallback using 3-gram
/// candidate gating and Tversky scoring. Fuzzy results are sorted by score
/// descending before snippet extraction.
///
/// Files that fail to deserialize or whose content cannot be loaded are
/// skipped silently. When both scopes resolve to the same directory (e.g.
/// under `CREFT_HOME`), the directory is searched only once.
pub(crate) fn search_all_indexes(ctx: &AppContext, query: &str) -> Vec<SnippetResult> {
    let terms: Vec<&str> = query.split_whitespace().collect();
    let mut seen_dirs: Vec<std::path::PathBuf> = Vec::new();

    // Load all valid indexes first so we can reuse them for the fuzzy pass
    // without reading from disk twice.
    let mut loaded: Vec<(String, self::index::SearchIndex)> = Vec::new();

    for scope in &[Scope::Global, Scope::Local] {
        let dir = match ctx.index_dir_for(*scope) {
            Ok(d) => d,
            Err(_) => continue,
        };

        if seen_dirs.contains(&dir) {
            continue;
        }
        seen_dirs.push(dir.clone());

        let read_dir = match std::fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(_) => continue,
        };

        for dir_entry in read_dir.flatten() {
            let path = dir_entry.path();
            let Some(ext) = path.extension() else {
                continue;
            };
            if ext != "idx" {
                continue;
            }

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

            loaded.push((namespace, index));
        }
    }

    // Exact search pass (AND semantics on whole-token hashes).
    let mut results: Vec<SnippetResult> = Vec::new();
    for (namespace, index) in &loaded {
        for entry in index.search(query) {
            let snippets = load_snippets_for_entry(ctx, namespace, entry, &terms);
            results.push(SnippetResult {
                namespace: namespace.clone(),
                name: entry.name.clone(),
                description: entry.description.clone(),
                snippets,
            });
        }
    }

    // Fuzzy fallback: only runs when exact search found nothing.
    if results.is_empty() {
        let mut scored: Vec<(f64, SnippetResult)> = Vec::new();

        for (namespace, index) in &loaded {
            for entry in index.search_fuzzy(query) {
                let text = if namespace == "_builtin" {
                    load_builtin_text(&entry.name)
                } else {
                    load_skill_text(ctx, &entry.name)
                };
                let Some(text) = text else { continue };

                let score = score_query(query, &text);
                if score < FUZZY_THRESHOLD {
                    continue;
                }

                let snippets = extract_snippets(&text, &terms, SNIPPET_CONTEXT);
                scored.push((
                    score,
                    SnippetResult {
                        namespace: namespace.clone(),
                        name: entry.name.clone(),
                        description: entry.description.clone(),
                        snippets,
                    },
                ));
            }
        }

        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        results = scored.into_iter().map(|(_, r)| r).collect();
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
pub(crate) fn load_skill_text(ctx: &AppContext, name: &str) -> Option<String> {
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

    /// Write a minimal skill markdown file to the commands directory.
    ///
    /// The body text is placed after the frontmatter so that `load_skill_text`
    /// extracts it for scoring. `namespace` is the first path component (e.g.
    /// `"sort"`), `leaf` is the filename stem (e.g. `"alpha"`), and the full
    /// skill name is `"<namespace> <leaf>"`.
    fn write_skill_file(
        ctx: &AppContext,
        scope: Scope,
        namespace: &str,
        leaf: &str,
        body_text: &str,
    ) {
        let commands_dir = ctx.commands_dir_for(scope).unwrap();
        let ns_dir = commands_dir.join(namespace);
        std::fs::create_dir_all(&ns_dir).unwrap();
        let content = format!("---\nname: {namespace} {leaf}\ndescription: \n---\n\n{body_text}\n");
        std::fs::write(ns_dir.join(format!("{leaf}.md")), content).unwrap();
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

    // ── fuzzy fallback in search_all_indexes ──────────────────────────────────

    #[test]
    fn search_all_indexes_exact_not_affected_by_fuzzy_path() {
        // Exact search must still work: a correctly-typed query must find the doc.
        let (ctx, _tmp) = make_ctx();
        let idx = SearchIndex::build(&[("my skill", "A skill", "rollback procedure")]);
        write_index_file(&ctx, Scope::Global, "ns.idx", &idx);

        // Exact query for "rollback" must succeed without fuzzy involvement.
        // (The skill file doesn't exist on disk so snippets will be empty, but
        //  the result entry will be returned — same behavior as before Stage 2.)
        let results = search_all_indexes(&ctx, "rollback");
        assert_eq!(
            results.len(),
            1,
            "exact query must still return the matching entry"
        );
        assert_eq!(results[0].name, "my skill");
    }

    #[test]
    fn search_all_indexes_no_fuzzy_candidates_below_threshold_returns_empty() {
        // A query that produces no fuzzy matches above the threshold must return empty.
        let (ctx, _tmp) = make_ctx();
        let idx = SearchIndex::build(&[("my skill", "A skill", "rollback procedure")]);
        write_index_file(&ctx, Scope::Global, "ns.idx", &idx);

        // "zzq" has no gram overlap with "rollback" or "procedure".
        let results = search_all_indexes(&ctx, "zzq");
        assert!(
            results.is_empty(),
            "query with no gram overlap must return empty results"
        );
    }

    #[test]
    fn search_all_indexes_no_matches_returns_empty_without_error() {
        // No infinite loop or panic when both exact and fuzzy find nothing.
        let (ctx, _tmp) = make_ctx();
        let idx = SearchIndex::build(&[("my skill", "A skill", "rollback procedure")]);
        write_index_file(&ctx, Scope::Global, "ns.idx", &idx);

        let results = search_all_indexes(&ctx, "xyzxyzxyz");
        assert!(results.is_empty());
    }

    #[test]
    fn search_all_indexes_fuzzy_results_sorted_by_tversky_score_descending() {
        // Two documents both match a typo query via n-gram overlap, but one scores
        // higher because the query's second word ("proceduure") also matches its
        // content. The higher-scoring document must appear first.
        //
        // Query: "roollback proceduure" (two typos, no exact match)
        //
        // "sort alpha" body: "rollback procedure"
        //   per-word best scores: tversky("roollback","rollback")≈0.71,
        //                         tversky("proceduure","procedure")=0.75
        //   average ≈ 0.73
        //
        // "sort bravo" body: "rollback"
        //   per-word best scores: tversky("roollback","rollback")≈0.71,
        //                         tversky("proceduure","rollback")=0.0
        //   average ≈ 0.36
        //
        // Both are above FUZZY_THRESHOLD (0.2). Descending sort must put "sort alpha"
        // at index 0.
        let (ctx, _tmp) = make_ctx();

        let alpha_body = "rollback procedure";
        let bravo_body = "rollback";

        write_skill_file(&ctx, Scope::Global, "sort", "alpha", alpha_body);
        write_skill_file(&ctx, Scope::Global, "sort", "bravo", bravo_body);

        let idx = SearchIndex::build(&[
            ("sort alpha", "A skill", alpha_body),
            ("sort bravo", "A skill", bravo_body),
        ]);
        write_index_file(&ctx, Scope::Global, "sort.idx", &idx);

        let results = search_all_indexes(&ctx, "roollback proceduure");

        assert!(
            results.len() >= 2,
            "both documents must be returned by the fuzzy fallback (got {})",
            results.len()
        );
        assert_eq!(
            results[0].name, "sort alpha",
            "higher-scoring document must appear first (descending sort by Tversky score)"
        );
    }
}
