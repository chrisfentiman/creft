//! Content snippet extraction and rendering for documentation search results.
//!
//! After the XOR filter identifies candidate documents, this module scans their
//! text line-by-line to find lines containing the user's query terms, groups
//! those lines with surrounding context into contiguous snippets, and renders
//! the results for terminal display.

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

/// Extract snippets from document text using fuzzy-matched document words.
///
/// On the fuzzy search path, the original query terms are misspelled and will
/// not appear as substrings of any document line. This function accepts
/// `matched_words` — the document-side tokens that scored highest against each
/// query word during Tversky scoring — and uses those for substring matching
/// instead.
///
/// When `matched_words` is empty (all query words scored 0.0), falls back to
/// `query_terms` so the caller never silently drops results.
///
/// All other behaviour (context lines, merge, empty-input) is identical to
/// [`extract_snippets`].
pub(crate) fn extract_snippets_fuzzy(
    text: &str,
    query_terms: &[&str],
    matched_words: &[String],
    context: usize,
) -> Vec<Snippet> {
    if matched_words.is_empty() {
        // No fuzzy matches scored > 0.0 — use the original terms as a best-effort.
        extract_snippets(text, query_terms, context)
    } else {
        let words_as_strs: Vec<&str> = matched_words.iter().map(String::as_str).collect();
        extract_snippets(text, &words_as_strs, context)
    }
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

    let lower_terms: Vec<String> = query_terms.iter().map(|t| t.to_lowercase()).collect();

    let lines: Vec<&str> = text.lines().collect();
    let line_count = lines.len();

    // Collect indices of all matching lines.
    let match_indices: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| {
            let lower_line = line.to_lowercase();
            lower_terms
                .iter()
                .any(|term| lower_line.contains(term.as_str()))
        })
        .map(|(i, _)| i)
        .collect();

    if match_indices.is_empty() {
        return Vec::new();
    }

    // Expand each match index to a [start, end) range with context, then merge
    // overlapping or adjacent ranges into contiguous spans.
    let ranges = merge_ranges(match_indices.iter().map(|&i| {
        let start = i.saturating_sub(context);
        let end = (i + context + 1).min(line_count);
        (start, end)
    }));

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
            Snippet {
                lines: snippet_lines,
            }
        })
        .collect()
}

/// Render search results with content snippets to a string.
///
/// For each result with non-empty snippets:
/// - Prints a header: bold document name (prefixed by namespace when
///   `show_namespace` is true)
/// - Prints the description on the next line, dimmed
/// - Prints each snippet's lines, with matching lines shown fully (query terms
///   wrapped in ANSI bold) and context lines dimmed
/// - Separates non-adjacent snippets within a document with `  ...`
/// - Separates documents with a blank line
///
/// Returns `None` if no results have snippets (all XOR filter false positives).
pub(crate) fn render_snippet_results(
    results: &[SnippetResult],
    query_terms: &[&str],
    show_namespace: bool,
) -> Option<String> {
    let with_snippets: Vec<&SnippetResult> =
        results.iter().filter(|r| !r.snippets.is_empty()).collect();

    if with_snippets.is_empty() {
        return None;
    }

    let lower_terms: Vec<String> = query_terms.iter().map(|t| t.to_lowercase()).collect();

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
                    out.push_str(&highlight_terms(&line.text, &lower_terms));
                } else {
                    out.push_str(&line.text.as_str().dim().to_string());
                }
                out.push('\n');
            }
        }
    }

    Some(out)
}

/// Wrap each occurrence of any query term in the line with ANSI bold, leaving
/// the rest of the text unstyled.
///
/// Matching is case-insensitive: the original casing from the line is preserved
/// in the output, but the bold spans are placed at the positions where a
/// lowercased query term matches the lowercased line.
fn highlight_terms(line: &str, lower_terms: &[String]) -> String {
    if lower_terms.is_empty() {
        return line.to_owned();
    }

    let lower_line = line.to_lowercase();

    // Collect [start, end) byte spans for every term match in the line.
    let mut spans: Vec<(usize, usize)> = Vec::new();
    for term in lower_terms {
        if term.is_empty() {
            continue;
        }
        let mut search_start = 0;
        while let Some(pos) = lower_line[search_start..].find(term.as_str()) {
            let abs_start = search_start + pos;
            let abs_end = abs_start + term.len();
            spans.push((abs_start, abs_end));
            search_start = abs_end;
        }
    }

    if spans.is_empty() {
        return line.to_owned();
    }

    // Sort and merge overlapping spans so bold regions don't nest or overlap.
    spans.sort_unstable_by_key(|&(s, _)| s);
    let merged = merge_ranges(spans.into_iter());

    // Build the output by alternating plain and bold segments.
    let mut result = String::with_capacity(line.len() + merged.len() * 16);
    let mut cursor = 0;
    for (start, end) in merged {
        if cursor < start {
            result.push_str(&line[cursor..start]);
        }
        result.push_str(&line[start..end].bold().to_string());
        cursor = end;
    }
    if cursor < line.len() {
        result.push_str(&line[cursor..]);
    }
    result
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
        assert_eq!(
            snippets.len(),
            1,
            "nearby matches must merge into one snippet"
        );
    }

    #[test]
    fn two_distant_matches_produce_two_snippets() {
        let text = "match_one\nfiller1\nfiller2\nfiller3\nfiller4\nfiller5\nmatch_two\n";
        // context=0: no lines overlap; indices 0 and 6 are far apart.
        let snippets = extract_snippets(text, &["match_one", "match_two"], 0);
        assert_eq!(
            snippets.len(),
            2,
            "distant matches must produce separate snippets"
        );
        assert_eq!(match_texts(&snippets), vec!["match_one", "match_two"]);
    }

    #[test]
    fn no_matches_returns_empty_vec() {
        let text = "alpha\nbeta\ngamma\n";
        let snippets = extract_snippets(text, &["zzz_not_present"], 2);
        assert!(
            snippets.is_empty(),
            "XOR false positive must produce no snippets"
        );
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
        let output = render_snippet_results(&results, &[], false);
        assert!(output.is_none(), "all false positives must produce None");
    }

    #[test]
    fn render_returns_none_for_empty_results_slice() {
        let output = render_snippet_results(&[], &[], false);
        assert!(output.is_none());
    }

    #[test]
    fn render_includes_name_and_description() {
        yansi::disable();
        let result = make_result("my-skill", "ns", "Does a thing", "this is a match line\n");
        let output = render_snippet_results(&[result], &["match"], false).unwrap();
        yansi::enable();
        assert!(output.contains("my-skill"), "output must contain name");
        assert!(
            output.contains("Does a thing"),
            "output must contain description"
        );
    }

    #[test]
    fn render_with_show_namespace_true_includes_namespace_header() {
        yansi::disable();
        let result = make_result(
            "deploy rollback",
            "deploy",
            "Roll back",
            "rollback match line\n",
        );
        let output = render_snippet_results(&[result], &["match"], true).unwrap();
        yansi::enable();
        assert!(
            output.contains("[deploy]"),
            "cross-source render must include namespace header"
        );
    }

    #[test]
    fn render_with_show_namespace_false_omits_namespace_header() {
        yansi::disable();
        let result = make_result(
            "deploy rollback",
            "deploy",
            "Roll back",
            "rollback match line\n",
        );
        let output = render_snippet_results(&[result], &["match"], false).unwrap();
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
        let text = "match_one\nfiller1\nfiller2\nfiller3\nfiller4\nfiller5\nfiller6\nmatch_two\n";
        let snippets = extract_snippets(text, &["match_one", "match_two"], 0);
        assert_eq!(
            snippets.len(),
            2,
            "must have two separate snippets to test separator"
        );
        let result = SnippetResult {
            name: "doc".to_owned(),
            namespace: "ns".to_owned(),
            description: "desc".to_owned(),
            snippets,
        };
        let output = render_snippet_results(&[result], &["match_one", "match_two"], false).unwrap();
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
        let output =
            render_snippet_results(&[false_positive, real_match], &["match"], false).unwrap();
        yansi::enable();
        assert!(
            !output.contains("false-pos"),
            "false positive must be excluded from output"
        );
        assert!(output.contains("real"), "real match must be in output");
    }

    #[test]
    fn render_produces_different_output_with_and_without_ansi() {
        // With ANSI enabled the output differs from plain text (has escape codes).
        yansi::enable();
        let result = make_result("doc", "ns", "description", "a match here\n");
        let with_ansi = render_snippet_results(&[result], &["match"], false).unwrap();

        yansi::disable();
        let result2 = make_result("doc", "ns", "description", "a match here\n");
        let without_ansi = render_snippet_results(&[result2], &["match"], false).unwrap();
        yansi::enable();

        assert_ne!(
            with_ansi, without_ansi,
            "ANSI-enabled output must differ from plain output"
        );
        assert!(
            with_ansi.contains('\x1b'),
            "ANSI output must contain escape sequences"
        );
    }

    #[test]
    fn render_multiple_docs_separated_by_blank_line() {
        yansi::disable();
        let r1 = make_result("doc-one", "ns", "first", "match here for one\n");
        let r2 = make_result("doc-two", "ns", "second", "match here for two\n");
        let output = render_snippet_results(&[r1, r2], &["match"], false).unwrap();
        yansi::enable();
        // A blank line separates the two documents.
        assert!(
            output.contains("\n\n"),
            "multiple documents must be separated by a blank line"
        );
        assert!(output.contains("doc-one"));
        assert!(output.contains("doc-two"));
    }

    #[test]
    fn render_highlights_query_terms_in_matching_lines() {
        // Without ANSI: query term appears as plain text in the output.
        yansi::disable();
        let snippets = extract_snippets("call creft_exit to stop\n", &["exit"], 0);
        let result = SnippetResult {
            name: "doc".to_owned(),
            namespace: "ns".to_owned(),
            description: "desc".to_owned(),
            snippets,
        };
        let plain = render_snippet_results(&[result], &["exit"], false).unwrap();
        yansi::enable();
        assert!(
            plain.contains("call creft_exit to stop"),
            "without ANSI the matching line text must appear verbatim"
        );

        // With ANSI: the output contains escape sequences specifically around "exit".
        yansi::enable();
        let snippets2 = extract_snippets("call creft_exit to stop\n", &["exit"], 0);
        let result2 = SnippetResult {
            name: "doc".to_owned(),
            namespace: "ns".to_owned(),
            description: "desc".to_owned(),
            snippets: snippets2,
        };
        let styled = render_snippet_results(&[result2], &["exit"], false).unwrap();
        yansi::enable();
        assert!(
            styled.contains('\x1b'),
            "ANSI output must contain escape sequences wrapping the matched term"
        );
        // The styled output must not contain the bare word "exit" surrounded by
        // plain text on both sides — the bold wrap must be present.
        assert!(
            !styled.contains("creft_exit to"),
            "the term 'exit' inside 'creft_exit' must be wrapped in bold, breaking the plain sequence"
        );
    }

    // ── extract_snippets_fuzzy ────────────────────────────────────────────────

    #[test]
    fn fuzzy_snippets_use_matched_words_not_query_terms() {
        // "ecit" is a typo for "exit" — it is not a substring of any line in the doc.
        // extract_snippets with &["ecit"] would return nothing.
        // extract_snippets_fuzzy with matched_words=["exit"] must find the line.
        let text = "call the exit code when done\n";
        let query_terms = &["ecit"];
        let matched_words = vec!["exit".to_owned()];

        let direct_snippets = extract_snippets(text, query_terms, 0);
        assert!(
            direct_snippets.is_empty(),
            "sanity: 'ecit' is not a substring of any line — direct extract must return nothing"
        );

        let fuzzy_snippets = extract_snippets_fuzzy(text, query_terms, &matched_words, 0);
        assert_eq!(
            fuzzy_snippets.len(),
            1,
            "fuzzy extract must find the line via matched word 'exit'"
        );
        assert!(
            fuzzy_snippets[0].lines[0].is_match,
            "the found line must be marked as a match"
        );
        assert_eq!(
            fuzzy_snippets[0].lines[0].text,
            "call the exit code when done"
        );
    }

    #[test]
    fn fuzzy_snippets_fallback_to_query_terms_when_no_matched_words() {
        // When matched_words is empty (all query words scored 0.0), fall back to
        // the original query terms so callers don't silently drop results.
        let text = "the heredoc template guide\n";
        let snippets = extract_snippets_fuzzy(text, &["heredoc"], &[], 0);
        assert_eq!(
            snippets.len(),
            1,
            "empty matched_words must fall back to query_terms for matching"
        );
    }

    #[test]
    fn fuzzy_snippets_no_match_returns_empty() {
        // Matched words that don't appear in the document produce no snippets.
        let text = "alpha beta gamma\n";
        let snippets = extract_snippets_fuzzy(text, &["zzz"], &["qqq".to_owned()], 0);
        assert!(
            snippets.is_empty(),
            "matched words absent from document must produce no snippets"
        );
    }

    #[test]
    fn fuzzy_snippets_context_lines_work_same_as_extract_snippets() {
        // Context behavior is unchanged — the fuzzy variant delegates to extract_snippets.
        let text = "alpha\nbeta\nexit\ndelta\nepsilon\n";
        let matched_words = vec!["exit".to_owned()];
        let snippets = extract_snippets_fuzzy(text, &["ecit"], &matched_words, 1);
        assert_eq!(snippets.len(), 1);
        assert_eq!(
            snippets[0].lines.len(),
            3,
            "context=1 must include one line above and below"
        );
        let texts: Vec<&str> = snippets[0].lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts, vec!["beta", "exit", "delta"]);
    }
}
