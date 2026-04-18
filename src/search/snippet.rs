//! Content snippet extraction and rendering for documentation search results.
//!
//! After the XOR filter identifies candidate documents, this module scans their
//! text line-by-line to find lines containing the user's query terms, groups
//! those lines with surrounding context into contiguous snippets, and renders
//! the results for terminal display.

// Stage 2 wires callers into these types and functions; suppress dead_code
// warnings until that stage lands.
#![allow(dead_code)]

use yansi::Paint;

/// A contiguous block of lines from a document that contains at least one
/// query match. Lines include surrounding context.
pub(crate) struct Snippet {
    /// The lines in this snippet, in document order.
    pub lines: Vec<SnippetLine>,
}

/// A single line within a snippet.
pub(crate) struct SnippetLine {
    /// The line content (no trailing newline).
    pub text: String,
    /// Whether this line contains a query term match.
    pub is_match: bool,
}

/// A search result paired with its extracted snippets, ready for rendering.
pub(crate) struct SnippetResult {
    pub name: String,
    pub namespace: String,
    pub description: String,
    pub snippets: Vec<Snippet>,
}

/// Extract snippets from document text for the given query terms.
///
/// Splits `text` into lines, finds lines where any term from `query_terms`
/// appears (case-insensitive substring match), adds `context` lines above and
/// below each match, merges overlapping regions, and returns the resulting
/// snippets.
///
/// `query_terms` are the raw words from the user's query (split on whitespace),
/// not hashed tokens. Matching is case-insensitive substring: query term "exit"
/// matches lines containing "exit", "creft_exit", "EXIT".
///
/// Returns an empty vec when no lines match (handles XOR filter false positives)
/// or when `query_terms` is empty.
pub(crate) fn extract_snippets(text: &str, query_terms: &[&str], context: usize) -> Vec<Snippet> {
    if query_terms.is_empty() {
        return Vec::new();
    }

    let lower_terms: Vec<String> = query_terms
        .iter()
        .map(|t| t.to_lowercase())
        .collect();

    let lines: Vec<&str> = text.lines().collect();
    let line_count = lines.len();

    // Collect indices of all matching lines.
    let match_indices: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| {
            let lower_line = line.to_lowercase();
            lower_terms.iter().any(|term| lower_line.contains(term.as_str()))
        })
        .map(|(i, _)| i)
        .collect();

    if match_indices.is_empty() {
        return Vec::new();
    }

    // Expand each match index to a [start, end) range with context, then merge
    // overlapping or adjacent ranges into contiguous spans.
    let ranges = merge_ranges(
        match_indices.iter().map(|&i| {
            let start = i.saturating_sub(context);
            let end = (i + context + 1).min(line_count);
            (start, end)
        }),
    );

    // Build one Snippet per merged range.
    ranges
        .into_iter()
        .map(|(start, end)| {
            let snippet_lines = lines[start..end]
                .iter()
                .enumerate()
                .map(|(offset, &line)| {
                    let abs_idx = start + offset;
                    let is_match = match_indices.contains(&abs_idx);
                    SnippetLine {
                        text: line.to_owned(),
                        is_match,
                    }
                })
                .collect();
            Snippet { lines: snippet_lines }
        })
        .collect()
}

/// Render search results with content snippets to a string.
///
/// For each result with non-empty snippets:
/// - Prints a header: bold document name (prefixed by namespace when
///   `show_namespace` is true)
/// - Prints the description on the next line, dimmed
/// - Prints each snippet's lines, with matching lines shown fully and context
///   lines dimmed
/// - Separates non-adjacent snippets within a document with `  ...`
/// - Separates documents with a blank line
///
/// Returns `None` if no results have snippets (all XOR filter false positives).
pub(crate) fn render_snippet_results(
    results: &[SnippetResult],
    show_namespace: bool,
) -> Option<String> {
    let with_snippets: Vec<&SnippetResult> =
        results.iter().filter(|r| !r.snippets.is_empty()).collect();

    if with_snippets.is_empty() {
        return None;
    }

    let mut out = String::new();
    let mut first_doc = true;

    for result in with_snippets {
        if !first_doc {
            out.push('\n');
        }
        first_doc = false;

        // Namespace header for cross-source searches.
        if show_namespace && !result.namespace.is_empty() {
            out.push_str(&format!("[{}]\n", result.namespace.as_str().bold()));
        }

        // Document name and description.
        out.push_str(&format!("{}\n", result.name.as_str().bold()));
        out.push_str(&format!("{}\n", result.description.as_str().dim()));

        // Snippets separated by "  ..." when non-adjacent.
        for (i, snippet) in result.snippets.iter().enumerate() {
            if i > 0 {
                out.push_str("  ...\n");
            }
            for line in &snippet.lines {
                if line.is_match {
                    out.push_str(&line.text);
                } else {
                    out.push_str(&line.text.as_str().dim().to_string());
                }
                out.push('\n');
            }
        }
    }

    Some(out)
}

// ── internal helpers ──────────────────────────────────────────────────────────

/// Merge an iterator of `[start, end)` ranges into a minimal set of
/// non-overlapping, non-adjacent ranges, preserving document order.
///
/// Two ranges merge when the second starts at or before the first ends
/// (i.e., they overlap or are immediately adjacent).
fn merge_ranges(ranges: impl Iterator<Item = (usize, usize)>) -> Vec<(usize, usize)> {
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (start, end) in ranges {
        match merged.last_mut() {
            Some(last) if start <= last.1 => {
                // Overlapping or adjacent — extend the current range.
                last.1 = last.1.max(end);
            }
            _ => merged.push((start, end)),
        }
    }
    merged
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use pretty_assertions::{assert_eq, assert_ne};
    use rstest::rstest;
    use yansi::Paint;

    use super::*;

    // Helper: collect matching line texts from all snippets.
    fn match_texts(snippets: &[Snippet]) -> Vec<&str> {
        snippets
            .iter()
            .flat_map(|s| s.lines.iter())
            .filter(|l| l.is_match)
            .map(|l| l.text.as_str())
            .collect()
    }

    // Helper: total line count across all snippets.
    fn total_lines(snippets: &[Snippet]) -> usize {
        snippets.iter().map(|s| s.lines.len()).sum()
    }

    // ── extract_snippets ──────────────────────────────────────────────────────

    #[test]
    fn single_match_returns_one_snippet_with_match_line() {
        let text = "alpha\nbeta\ngamma\ndelta\nepsilon\n";
        let snippets = extract_snippets(text, &["gamma"], 0);
        assert_eq!(snippets.len(), 1);
        assert_eq!(snippets[0].lines.len(), 1);
        assert!(snippets[0].lines[0].is_match);
        assert_eq!(snippets[0].lines[0].text, "gamma");
    }

    #[test]
    fn single_match_with_context_includes_surrounding_lines() {
        let text = "alpha\nbeta\ngamma\ndelta\nepsilon\n";
        // "gamma" is at index 2; context=1 should include beta and delta.
        let snippets = extract_snippets(text, &["gamma"], 1);
        assert_eq!(snippets.len(), 1);
        assert_eq!(snippets[0].lines.len(), 3);
        let texts: Vec<&str> = snippets[0].lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts, vec!["beta", "gamma", "delta"]);
        assert!(!snippets[0].lines[0].is_match);
        assert!(snippets[0].lines[1].is_match);
        assert!(!snippets[0].lines[2].is_match);
    }

    #[test]
    fn two_nearby_matches_within_context_merge_into_one_snippet() {
        // Lines 0..4: a b c d e; "b" at 1 and "d" at 3 are each within context=2 of the other.
        let text = "a\nb\nc\nd\ne\n";
        let snippets = extract_snippets(text, &["b", "d"], 2);
        assert_eq!(snippets.len(), 1, "nearby matches must merge into one snippet");
    }

    #[test]
    fn two_distant_matches_produce_two_snippets() {
        let text = "match_one\nfiller1\nfiller2\nfiller3\nfiller4\nfiller5\nmatch_two\n";
        // context=0: no lines overlap; indices 0 and 6 are far apart.
        let snippets = extract_snippets(text, &["match_one", "match_two"], 0);
        assert_eq!(snippets.len(), 2, "distant matches must produce separate snippets");
        assert_eq!(match_texts(&snippets), vec!["match_one", "match_two"]);
    }

    #[test]
    fn no_matches_returns_empty_vec() {
        let text = "alpha\nbeta\ngamma\n";
        let snippets = extract_snippets(text, &["zzz_not_present"], 2);
        assert!(snippets.is_empty(), "XOR false positive must produce no snippets");
    }

    #[test]
    fn context_at_start_of_document_does_not_underflow() {
        // Match is the first line; context should not try to go before index 0.
        let text = "match_line\nsecond\nthird\n";
        let snippets = extract_snippets(text, &["match_line"], 2);
        assert_eq!(snippets.len(), 1);
        // Should start at 0, not panic.
        assert_eq!(snippets[0].lines[0].text, "match_line");
    }

    #[test]
    fn context_at_end_of_document_does_not_overflow() {
        // Match is the last line; context should not go past the end.
        let text = "first\nsecond\nmatch_line\n";
        let snippets = extract_snippets(text, &["match_line"], 2);
        assert_eq!(snippets.len(), 1);
        let last = snippets[0].lines.last().unwrap();
        assert_eq!(last.text, "match_line");
    }

    #[test]
    fn matching_is_case_insensitive() {
        let text = "The EXIT code is zero.\n";
        let snippets = extract_snippets(text, &["exit"], 0);
        assert_eq!(snippets.len(), 1, "case-insensitive match must find 'EXIT'");
    }

    #[test]
    fn matching_is_substring() {
        let text = "call creft_exit to stop.\n";
        let snippets = extract_snippets(text, &["exit"], 0);
        assert_eq!(
            snippets.len(),
            1,
            "substring match must find 'exit' inside 'creft_exit'"
        );
    }

    #[test]
    fn empty_query_terms_returns_empty_vec() {
        let text = "alpha\nbeta\ngamma\n";
        let snippets = extract_snippets(text, &[], 2);
        assert!(
            snippets.is_empty(),
            "empty query terms must not match every line"
        );
    }

    #[test]
    fn empty_text_returns_empty_vec() {
        let snippets = extract_snippets("", &["exit"], 2);
        assert!(snippets.is_empty());
    }

    // Parametrize context window sizes to verify line counts are always correct.
    #[rstest]
    #[case::context_zero(0, 1)]
    #[case::context_one(1, 3)]
    #[case::context_two(2, 5)]
    fn context_window_size_controls_line_count(#[case] context: usize, #[case] expected: usize) {
        // 10-line document, match in the middle (line 5).
        let text = "a\nb\nc\nd\ne\nmatch_here\ng\nh\ni\nj\n";
        let snippets = extract_snippets(text, &["match_here"], context);
        assert_eq!(snippets.len(), 1);
        assert_eq!(
            total_lines(&snippets),
            expected,
            "context={context} must produce {expected} lines"
        );
    }

    // ── render_snippet_results ────────────────────────────────────────────────

    fn make_result(name: &str, namespace: &str, description: &str, text: &str) -> SnippetResult {
        let snippets = extract_snippets(text, &["match"], 1);
        SnippetResult {
            name: name.to_owned(),
            namespace: namespace.to_owned(),
            description: description.to_owned(),
            snippets,
        }
    }

    #[test]
    fn render_returns_none_when_no_results_have_snippets() {
        let results = [SnippetResult {
            name: "no-match".to_owned(),
            namespace: "ns".to_owned(),
            description: "desc".to_owned(),
            snippets: vec![],
        }];
        let output = render_snippet_results(&results, false);
        assert!(output.is_none(), "all false positives must produce None");
    }

    #[test]
    fn render_returns_none_for_empty_results_slice() {
        let output = render_snippet_results(&[], false);
        assert!(output.is_none());
    }

    #[test]
    fn render_includes_name_and_description() {
        yansi::disable();
        let result = make_result("my-skill", "ns", "Does a thing", "this is a match line\n");
        let output = render_snippet_results(&[result], false).unwrap();
        yansi::enable();
        assert!(output.contains("my-skill"), "output must contain name");
        assert!(output.contains("Does a thing"), "output must contain description");
    }

    #[test]
    fn render_with_show_namespace_true_includes_namespace_header() {
        yansi::disable();
        let result = make_result("deploy rollback", "deploy", "Roll back", "rollback match line\n");
        let output = render_snippet_results(&[result], true).unwrap();
        yansi::enable();
        assert!(
            output.contains("[deploy]"),
            "cross-source render must include namespace header"
        );
    }

    #[test]
    fn render_with_show_namespace_false_omits_namespace_header() {
        yansi::disable();
        let result = make_result("deploy rollback", "deploy", "Roll back", "rollback match line\n");
        let output = render_snippet_results(&[result], false).unwrap();
        yansi::enable();
        assert!(
            !output.contains("[deploy]"),
            "per-command render must not include namespace header"
        );
    }

    #[test]
    fn render_separates_non_adjacent_snippets_with_ellipsis() {
        yansi::disable();
        // Two distant matches in the same document → two snippets → ellipsis separator.
        let text =
            "match_one\nfiller1\nfiller2\nfiller3\nfiller4\nfiller5\nfiller6\nmatch_two\n";
        let snippets = extract_snippets(text, &["match_one", "match_two"], 0);
        assert_eq!(snippets.len(), 2, "must have two separate snippets to test separator");
        let result = SnippetResult {
            name: "doc".to_owned(),
            namespace: "ns".to_owned(),
            description: "desc".to_owned(),
            snippets,
        };
        let output = render_snippet_results(&[result], false).unwrap();
        yansi::enable();
        assert!(
            output.contains("  ..."),
            "non-adjacent snippets must be separated by '  ...'"
        );
    }

    #[test]
    fn render_skips_false_positive_results_but_shows_real_matches() {
        yansi::disable();
        let false_positive = SnippetResult {
            name: "false-pos".to_owned(),
            namespace: "ns".to_owned(),
            description: "no snippets".to_owned(),
            snippets: vec![],
        };
        let real_match = make_result("real", "ns", "has content", "a match line here\n");
        let output = render_snippet_results(&[false_positive, real_match], false).unwrap();
        yansi::enable();
        assert!(!output.contains("false-pos"), "false positive must be excluded from output");
        assert!(output.contains("real"), "real match must be in output");
    }

    #[test]
    fn render_produces_different_output_with_and_without_ansi() {
        // With ANSI enabled the output differs from plain text (has escape codes).
        yansi::enable();
        let result = make_result("doc", "ns", "description", "a match here\n");
        let with_ansi = render_snippet_results(&[result], false).unwrap();

        yansi::disable();
        let result2 = make_result("doc", "ns", "description", "a match here\n");
        let without_ansi = render_snippet_results(&[result2], false).unwrap();
        yansi::enable();

        assert_ne!(
            with_ansi, without_ansi,
            "ANSI-enabled output must differ from plain output"
        );
        assert!(with_ansi.contains('\x1b'), "ANSI output must contain escape sequences");
    }

    #[test]
    fn render_multiple_docs_separated_by_blank_line() {
        yansi::disable();
        let r1 = make_result("doc-one", "ns", "first", "match here for one\n");
        let r2 = make_result("doc-two", "ns", "second", "match here for two\n");
        let output = render_snippet_results(&[r1, r2], false).unwrap();
        yansi::enable();
        // A blank line separates the two documents.
        assert!(
            output.contains("\n\n"),
            "multiple documents must be separated by a blank line"
        );
        assert!(output.contains("doc-one"));
        assert!(output.contains("doc-two"));
    }
}
