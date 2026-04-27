//! Name matching for `creft skills test`.
//!
//! Both the SKILL positional and the SCENARIO positional (and its `--filter`
//! flag form) accept a pattern. The pattern's shape is decided by its content:
//!
//! - If the pattern contains no `*` or `?`, it is a substring match.
//!   Any name containing the pattern as a substring matches.
//! - If the pattern contains `*` or `?`, it is an anchored fnmatch glob:
//!   `*` matches any run of characters (including zero), `?` matches exactly
//!   one character, and every other byte matches itself.
//!
//! The metacharacters `*` and `?` are reserved and cannot be escaped. A
//! literal `*` or `?` in a name can be matched by using the substring shape
//! (a pattern without any metacharacters that contains the surrounding text).
//!
//! The two shapes are encoded as a single compiled [`regex::Regex`], so the
//! per-name match path is allocation-free.

use regex::Regex;

// ── Public types ──────────────────────────────────────────────────────────────

/// A compiled name pattern. Used for both skill basenames and scenario names.
///
/// Build once per run via [`compile`]; match per-name via [`Matcher::matches`].
pub(crate) struct Matcher(Regex);

/// Errors produced when compiling a pattern.
///
/// The translator is designed so that every non-empty input produces a valid
/// regex, but this error is returned defensively if a bug in the translator
/// produces an invalid regex string.
#[derive(Debug, thiserror::Error)]
pub(crate) enum MatchPatternError {
    #[error("could not compile filter pattern: {0}")]
    InvalidPattern(#[from] regex::Error),
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Compile a user-supplied pattern into a [`Matcher`].
///
/// When the pattern contains no `*` or `?`, it is treated as a substring
/// pattern: any name containing the pattern matches. When the pattern contains
/// `*` or `?`, it is treated as an fnmatch glob anchored at both ends: `*`
/// matches any run of characters (including zero characters), `?` matches
/// exactly one character, and every other character matches itself literally.
///
/// `pattern` should be non-empty. An empty pattern compiles to a
/// substring matcher whose body is empty, meaning every name matches.
/// The parser rejects empty patterns at the CLI boundary; callers should
/// not rely on this behavior.
pub(crate) fn compile(pattern: &str) -> Result<Matcher, MatchPatternError> {
    let has_glob = pattern.contains(['*', '?']);

    let regex_src = if has_glob {
        // Glob shape: split on metacharacters, escape each segment, then
        // rejoin with the regex equivalent of the metacharacter. Anchor at
        // both ends so that `merge*` does not match `pre-merge-foo`.
        let mut body = String::with_capacity(pattern.len() * 2);
        // Walk character by character to preserve the original order of `*`
        // and `?` relative to the literal segments.
        for ch in pattern.chars() {
            match ch {
                '*' => body.push_str(".*"),
                '?' => body.push('.'),
                other => {
                    // Each non-metacharacter is regex-escaped so that dots,
                    // plus signs, parens, etc. in skill/scenario names match
                    // literally rather than as regex metacharacters.
                    let escaped = regex::escape(&other.to_string());
                    body.push_str(&escaped);
                }
            }
        }
        format!("^{body}$")
    } else {
        // Substring shape: the entire pattern is escaped so that regex
        // metacharacters in names (`.`, `+`, `(`, etc.) match literally.
        // No anchors — a match anywhere in the name counts.
        regex::escape(pattern)
    };

    Ok(Matcher(Regex::new(&regex_src)?))
}

impl Matcher {
    /// Returns `true` if `name` matches this pattern.
    pub(crate) fn matches(&self, name: &str) -> bool {
        self.0.is_match(name)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn m(pattern: &str) -> Matcher {
        compile(pattern).expect("compile must succeed for well-formed pattern")
    }

    // ── Substring shape ───────────────────────────────────────────────────────

    #[test]
    fn compile_substring_matches_anywhere_in_name() {
        let matcher = m("fresh");
        assert!(matcher.matches("fresh install"), "prefix");
        assert!(matcher.matches("re-fresh"), "suffix");
        assert!(matcher.matches("fresh"), "exact");
        assert!(matcher.matches("a-fresh-start"), "infix");
    }

    #[test]
    fn compile_substring_does_not_match_when_absent() {
        let matcher = m("xyz");
        assert!(!matcher.matches("fresh install"));
        assert!(!matcher.matches("merge-clean"));
        assert!(!matcher.matches(""));
    }

    #[test]
    fn compile_substring_treats_regex_metacharacters_literally() {
        let dot = m("foo.bar");
        // The dot must match a literal dot, not any character.
        assert!(dot.matches("foo.bar"), "literal dot matches");
        assert!(
            !dot.matches("fooXbar"),
            "dot does not act as regex wildcard"
        );

        let plus = m("a+b");
        assert!(plus.matches("a+b"), "literal plus matches");
        assert!(!plus.matches("ab"), "plus does not mean one-or-more");

        let parens = m("(test)");
        assert!(parens.matches("(test)"), "parens match literally");
    }

    // ── Glob shape ────────────────────────────────────────────────────────────

    #[test]
    fn compile_glob_star_matches_run_of_chars() {
        let matcher = m("merge*");
        assert!(matcher.matches("merge"), "empty suffix");
        assert!(matcher.matches("merge-clean"), "hyphen suffix");
        assert!(matcher.matches("merge anything"), "space suffix");
        assert!(
            !matcher.matches("pre-merge"),
            "anchored: does not match suffix position"
        );
        assert!(!matcher.matches("pre-merge-foo"), "anchored: no mid-match");
    }

    #[test]
    fn compile_glob_star_matches_empty_suffix() {
        let matcher = m("merge*");
        assert!(matcher.matches("merge"), "star matches zero characters");
    }

    #[test]
    fn compile_glob_question_matches_single_char() {
        let matcher = m("a?b");
        assert!(matcher.matches("axb"), "one char between a and b");
        assert!(!matcher.matches("ab"), "zero chars — does not match");
        assert!(!matcher.matches("axxb"), "two chars — does not match");
        assert!(matcher.matches("a.b"), "dot is a valid single char");
    }

    #[test]
    fn compile_glob_anchored_at_both_ends() {
        // Substring shape: "middle" matches anywhere.
        assert!(m("middle").matches("a-middle-b"), "substring is unanchored");
        // Glob shape: "middle*" is anchored at the start.
        assert!(
            !m("middle*").matches("a-middle"),
            "glob is anchored — prefix does not match"
        );
        assert!(
            m("middle*").matches("middle-b"),
            "glob anchored — starts at beginning"
        );
    }

    #[test]
    fn compile_glob_dot_treated_literally_not_as_regex_wildcard() {
        // In glob shape, the dot in "foo.*" must match a literal dot.
        let matcher = m("foo.*");
        assert!(matcher.matches("foo.bar"), "literal dot in glob");
        assert!(!matcher.matches("foobar"), "no dot — does not match");
        assert!(!matcher.matches("foo-bar"), "hyphen — does not match");
    }

    #[test]
    fn compile_glob_star_in_middle_matches_arbitrary_interior() {
        let matcher = m("pre*fix");
        assert!(matcher.matches("prefix"), "zero chars in middle");
        assert!(matcher.matches("pre-fix"), "one char in middle");
        assert!(
            matcher.matches("pre-long-middle-fix"),
            "many chars in middle"
        );
        assert!(!matcher.matches("pre"), "must end with 'fix'");
    }

    #[test]
    fn compile_glob_double_star_acts_as_star() {
        // Two stars together: ".*.*" — matches any suffix after start.
        let matcher = m("a**b");
        assert!(matcher.matches("ab"), "zero chars for each star");
        assert!(matcher.matches("axb"), "one char");
        assert!(matcher.matches("axyb"), "two chars");
    }

    // ── Edge: pattern with no metacharacters is substring ─────────────────────

    #[test]
    fn pattern_with_no_metacharacters_is_substring_not_glob() {
        // Even if the name has glob-like characters, a pattern without * or ?
        // is a substring search.
        let matcher = m("setup");
        assert!(matcher.matches("setup"), "exact");
        assert!(matcher.matches("my-setup-task"), "infix");
    }

    // ── Regression: exact-name match is a subset of substring ─────────────────

    #[test]
    fn exact_name_is_matched_by_substring_pattern() {
        // The existing exact-match behavior (setup == setup) must be a subset
        // of the new pattern shape. A pattern equal to the name always matches.
        assert!(m("fresh-install").matches("fresh-install"));
        assert!(m("setup").matches("setup"));
        assert!(m("merge-clean").matches("merge-clean"));
    }
}
