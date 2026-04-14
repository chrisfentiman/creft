/// Maximum column width for all help output.
pub const MAX_WIDTH: usize = 80;

/// Wrap a paragraph of text to fit within `width` columns.
///
/// Splits on whitespace boundaries. Lines that consist of a single word longer
/// than `width` are emitted as-is (no mid-word breaks). Leading indentation on
/// the first line is preserved; continuation lines receive `indent` spaces of
/// leading whitespace.
///
/// Lines that start with two or more spaces (pre-formatted examples) are emitted
/// unchanged — they are assumed to be intentionally formatted.
///
/// Blank lines are preserved as paragraph separators.
pub fn wrap_text(text: &str, width: usize, indent: usize) -> String {
    let indent_str = " ".repeat(indent);
    let mut out = String::new();

    for line in text.lines() {
        // Blank line: pass through as paragraph separator.
        if line.trim().is_empty() {
            out.push('\n');
            continue;
        }

        // Pre-formatted line (two or more leading spaces): pass through unchanged.
        if line.starts_with("  ") {
            out.push_str(line);
            out.push('\n');
            continue;
        }

        // Markdown headers (starting with `#`): pass through unchanged.
        if line.starts_with('#') {
            out.push_str(line);
            out.push('\n');
            continue;
        }

        // Wrap the line as a prose paragraph.
        let wrapped = wrap_words(line, width, indent, &indent_str);
        out.push_str(&wrapped);
        out.push('\n');
    }

    // Remove trailing newline added above so callers control the terminator.
    if out.ends_with('\n') {
        out.pop();
    }

    out
}

/// Wrap a description that appears after a label+padding column in help output.
///
/// `first_line_budget` is the remaining columns available on the first line
/// (after the label and padding have been written). `continuation_indent` is
/// the column where continuation lines start (typically `2 + label_width + 2`).
///
/// Returns the wrapped text. The first line has no leading whitespace (the
/// caller has already written the label). Continuation lines start with
/// `continuation_indent` spaces.
///
/// When `first_line_budget` is zero the first line is left empty and the full
/// description starts on the next continuation line.
///
/// Example with `first_line_budget = 40` and `continuation_indent = 30`:
/// ```text
/// Force ecosystem (npm, crates, pypi).
///                               Auto-detected from project files if omitted
/// ```
pub fn wrap_description(
    text: &str,
    first_line_budget: usize,
    continuation_indent: usize,
) -> String {
    if text.is_empty() {
        return String::new();
    }

    let cont_str = " ".repeat(continuation_indent);
    let words: Vec<&str> = text.split_whitespace().collect();

    if words.is_empty() {
        return String::new();
    }

    let mut out = String::new();
    let mut current_budget = first_line_budget;
    let mut current_len = 0usize;
    let mut first_line = true;
    let mut line_started = false;

    for word in &words {
        let word_len = word.len();

        if !line_started {
            // First word on this line.
            if current_budget == 0 || (!first_line && word_len > current_budget) {
                // No budget or word exceeds budget: start continuation.
                if first_line {
                    // Push the word onto a new continuation line.
                    out.push('\n');
                    out.push_str(&cont_str);
                    out.push_str(word);
                    current_len = word_len;
                    line_started = true;
                    first_line = false;
                    current_budget = MAX_WIDTH.saturating_sub(continuation_indent);
                } else {
                    out.push('\n');
                    out.push_str(&cont_str);
                    out.push_str(word);
                    current_len = word_len;
                    line_started = true;
                    current_budget = MAX_WIDTH.saturating_sub(continuation_indent);
                }
            } else {
                // Fits: place word directly (first-line has no leading spaces).
                out.push_str(word);
                current_len = word_len;
                line_started = true;
            }
        } else {
            // Subsequent words: check if appending (space + word) still fits.
            let needed = 1 + word_len; // space + word
            if current_len + needed <= current_budget {
                out.push(' ');
                out.push_str(word);
                current_len += needed;
            } else {
                // Wrap to a new continuation line.
                out.push('\n');
                out.push_str(&cont_str);
                out.push_str(word);
                current_len = word_len;
                first_line = false;
                current_budget = MAX_WIDTH.saturating_sub(continuation_indent);
            }
        }
    }

    out
}

/// Core word-wrap algorithm for a single prose line.
///
/// Wraps `text` to `width` columns. The first line may have a different
/// effective budget than continuation lines when the caller passes `indent > 0`.
/// In practice, `wrap_text` passes `indent = 0` so all lines share `width`.
fn wrap_words(text: &str, width: usize, indent: usize, indent_str: &str) -> String {
    let cont_width = width.saturating_sub(indent);
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return String::new();
    }

    let mut out = String::new();
    let mut current_len = 0usize;
    let mut first_on_line = true;
    let mut first_line = true;

    for word in &words {
        let word_len = word.len();
        let budget = if first_line { width } else { cont_width };

        if first_on_line {
            if !first_line {
                out.push_str(indent_str);
            }
            out.push_str(word);
            current_len = if first_line {
                word_len
            } else {
                indent + word_len
            };
            first_on_line = false;
        } else {
            let needed = 1 + word_len;
            if current_len + needed <= budget {
                out.push(' ');
                out.push_str(word);
                current_len += needed;
            } else {
                out.push('\n');
                first_line = false;
                out.push_str(indent_str);
                out.push_str(word);
                current_len = indent + word_len;
            }
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::*;

    // ── wrap_text ────────────────────────────────────────────────────────────

    #[test]
    fn wrap_text_under_width_unchanged() {
        let s = "Short line.";
        assert_eq!(wrap_text(s, 80, 0), s);
    }

    #[test]
    fn wrap_text_at_width_unchanged() {
        // Exactly 80 characters, no wrap.
        let s = "a".repeat(80);
        assert_eq!(wrap_text(&s, 80, 0), s);
    }

    #[test]
    fn wrap_text_over_width_wraps_at_space() {
        // 90-char line must break at a space boundary.
        let s = "word ".repeat(18); // 90 chars
        let result = wrap_text(s.trim(), 80, 0);
        for line in result.lines() {
            assert!(
                line.len() <= 80,
                "all lines must be ≤ 80 chars; got {}: {line:?}",
                line.len()
            );
        }
    }

    #[test]
    fn wrap_text_single_word_over_width_emitted_as_is() {
        let s = "a".repeat(100);
        let result = wrap_text(&s, 80, 0);
        // Single long word: emitted on one line, no break.
        assert_eq!(result.lines().count(), 1);
        assert_eq!(result, s);
    }

    #[test]
    fn wrap_text_blank_lines_preserved_as_paragraph_separators() {
        let s = "First paragraph.\n\nSecond paragraph.";
        let result = wrap_text(s, 80, 0);
        assert!(result.contains("\n\n"), "blank line must be preserved");
    }

    #[test]
    fn wrap_text_preformatted_lines_passed_through() {
        let s = "Prose line.\n  preformatted example\nMore prose.";
        let result = wrap_text(s, 80, 0);
        assert!(
            result.contains("  preformatted example"),
            "pre-formatted line must be passed through unchanged"
        );
    }

    #[test]
    fn wrap_text_markdown_headers_passed_through() {
        let s = "## Section header with many words that would exceed eighty columns if wrapped";
        let result = wrap_text(s, 80, 0);
        assert_eq!(result, s, "markdown headers must not be wrapped");
    }

    #[test]
    fn wrap_text_empty_string_returns_empty() {
        assert_eq!(wrap_text("", 80, 0), "");
    }

    // ── wrap_description ─────────────────────────────────────────────────────

    #[test]
    fn wrap_description_empty_returns_empty() {
        assert_eq!(wrap_description("", 40, 20), "");
    }

    #[test]
    fn wrap_description_fits_on_first_line_unchanged() {
        let s = "Short description";
        assert_eq!(wrap_description(s, 40, 20), s);
    }

    #[test]
    fn wrap_description_wraps_when_over_budget() {
        // budget = 20, continuation_indent = 30
        let s = "Force ecosystem (npm, crates, pypi). Auto-detected from project files if omitted";
        let result = wrap_description(s, 20, 30);
        let lines: Vec<&str> = result.lines().collect();
        assert!(lines.len() > 1, "must wrap to multiple lines");
        for (i, line) in lines.iter().enumerate() {
            if i == 0 {
                assert!(
                    line.len() <= 20,
                    "first line must fit within budget of 20; got {}: {line:?}",
                    line.len()
                );
            } else {
                assert!(
                    line.starts_with(&" ".repeat(30)),
                    "continuation lines must start with 30 spaces; got: {line:?}"
                );
            }
        }
    }

    #[test]
    fn wrap_description_zero_budget_starts_on_continuation() {
        let s = "Some description text";
        let result = wrap_description(s, 0, 10);
        let lines: Vec<&str> = result.lines().collect();
        assert!(
            lines.len() >= 2,
            "zero budget must push description to continuation line"
        );
        // First line is empty (the label position).
        assert_eq!(lines[0], "", "first line must be empty with zero budget");
        // Continuation starts with correct indent.
        assert!(
            lines[1].starts_with("          "), // 10 spaces
            "continuation must start with 10 spaces; got: {:?}",
            lines[1]
        );
    }

    #[test]
    fn wrap_description_continuation_indent_exact() {
        // Verify continuation lines are indented to exactly continuation_indent columns.
        let s = "word1 word2 word3 word4 word5 word6 word7 word8 word9 word10";
        let result = wrap_description(s, 10, 15);
        for (i, line) in result.lines().enumerate() {
            if i > 0 {
                let spaces = line.chars().take_while(|c| *c == ' ').count();
                assert_eq!(
                    spaces, 15,
                    "continuation line {i} must have exactly 15 leading spaces; got {spaces}"
                );
            }
        }
    }

    #[test]
    fn wrap_description_no_trailing_whitespace_on_any_line() {
        let s = "This is a longer description that will definitely need to wrap across multiple lines in the output";
        let result = wrap_description(s, 30, 20);
        for line in result.lines() {
            assert_eq!(
                line,
                line.trim_end(),
                "line must not have trailing whitespace: {line:?}"
            );
        }
    }

    #[rstest]
    #[case::single_word_fits("hello", 20, 10, 1)]
    #[case::two_words_fit("hello world", 20, 10, 1)]
    #[case::wraps_at_budget("hello world extra", 11, 10, 2)]
    fn wrap_description_line_count(
        #[case] input: &str,
        #[case] budget: usize,
        #[case] cont_indent: usize,
        #[case] expected_lines: usize,
    ) {
        let result = wrap_description(input, budget, cont_indent);
        assert_eq!(
            result.lines().count(),
            expected_lines,
            "input {input:?} with budget {budget} must produce {expected_lines} line(s)"
        );
    }
}
