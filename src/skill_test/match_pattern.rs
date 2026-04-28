//! Name matching for `creft skills test`.
//!
//! Both the SKILL positional and the SCENARIO positional (and its `--filter`
//! flag form) accept a pattern. The pattern's shape is decided by its content
//! and the [`MatchKind`] passed to [`compile`]:
//!
//! - If the pattern contains no `*` or `?`:
//!   - [`MatchKind::Exact`]: the entire name must equal the pattern.
//!   - [`MatchKind::Substring`]: any name containing the pattern matches.
//! - If the pattern contains `*` or `?` (regardless of kind), it is an
//!   anchored fnmatch glob: `*` matches any run of characters (including
//!   zero), `?` matches exactly one character, and every other byte matches
//!   itself.
//!
//! The metacharacters `*` and `?` are reserved and cannot be escaped. A
//! literal `*` or `?` in a name can be matched by using a substring pattern
//! (a pattern without any metacharacters that contains the surrounding text).
//!
//! The two shapes are encoded as a single compiled [`regex::Regex`], so the
//! per-name match path is allocation-free.

use regex::Regex;

// ── Public types ──────────────────────────────────────────────────────────────

/// Controls how a plain-text pattern (no `*` or `?`) is interpreted.
///
/// Glob patterns are anchored fnmatch regardless of this setting — the kind
/// only affects patterns that contain no metacharacters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MatchKind {
    /// Plain text matches a name only when the entire name equals the pattern.
    ///
    /// Used for SKILL basenames — closed identifiers where partial match is
    /// surprising (e.g. `creft skills test ask` should not discover
    /// `task.test.yaml`).
    Exact,
    /// Plain text matches a name when the name contains the pattern as a
    /// substring.
    ///
    /// Used for SCENARIO names — narrative strings where partial match is
    /// the common case (e.g. `--filter fresh` matches `fresh-install`).
    Substring,
}

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
/// `kind` controls how a plain-text pattern (no `*` or `?`) is interpreted:
/// - [`MatchKind::Exact`] anchors the match at both ends (`^<escaped>$`), so
///   the entire name must equal the pattern. Used for SKILL basenames.
/// - [`MatchKind::Substring`] applies no anchors, so any name containing the
///   pattern as a substring matches. Used for SCENARIO names and `--filter`.
///
/// When the pattern contains `*` or `?`, `kind` is ignored — the pattern is
/// always treated as an fnmatch glob anchored at both ends: `*` matches any
/// run of characters (including zero), `?` matches exactly one character, and
/// every other character matches itself literally.
///
/// `pattern` should be non-empty. An empty plain-text `Exact` pattern compiles
/// to `^$` and matches no real basename. An empty `Substring` pattern matches
/// every name. The CLI rejects empty patterns at its boundary; callers should
/// not rely on either behavior.
pub(crate) fn compile(pattern: &str, kind: MatchKind) -> Result<Matcher, MatchPatternError> {
    let has_glob = pattern.contains(['*', '?']);

    let regex_src = if has_glob {
        // Glob shape: walk character by character, escape literal segments,
        // and replace metacharacters with their regex equivalents. Anchored
        // at both ends so `merge*` does not match `pre-merge-foo`.
        let mut body = String::with_capacity(pattern.len() * 2);
        for ch in pattern.chars() {
            match ch {
                '*' => body.push_str(".*"),
                '?' => body.push('.'),
                other => {
                    // Regex-escape each literal character so that `.`, `+`,
                    // `(`, etc. in names match literally, not as metacharacters.
                    let escaped = regex::escape(&other.to_string());
                    body.push_str(&escaped);
                }
            }
        }
        format!("^{body}$")
    } else {
        // Plain-text shape: escape the whole pattern, then anchor or not
        // depending on kind.
        let escaped = regex::escape(pattern);
        match kind {
            MatchKind::Exact => format!("^{escaped}$"),
            MatchKind::Substring => escaped,
        }
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
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::*;

    fn exact(pattern: &str) -> Matcher {
        compile(pattern, MatchKind::Exact).expect("compile must succeed for well-formed pattern")
    }

    fn sub(pattern: &str) -> Matcher {
        compile(pattern, MatchKind::Substring)
            .expect("compile must succeed for well-formed pattern")
    }

    // ── MatchKind::Exact — plain text ─────────────────────────────────────────

    #[test]
    fn exact_plain_text_matches_whole_name_only() {
        let matcher = exact("setup");
        assert!(matcher.matches("setup"), "identical name matches");
        assert!(
            !matcher.matches("setup-extra"),
            "trailing suffix must not match"
        );
        assert!(
            !matcher.matches("my-setup"),
            "leading prefix must not match"
        );
        assert!(!matcher.matches("asetup"), "embedded must not match");
        assert!(!matcher.matches(""), "empty name must not match");
    }

    #[test]
    fn exact_plain_text_ask_does_not_match_task() {
        // The reported symptom: `creft skills test ask` must not discover task.test.yaml.
        let matcher = exact("ask");
        assert!(matcher.matches("ask"), "basename 'ask' matches");
        assert!(
            !matcher.matches("task"),
            "'task' contains 'ask' but must not match"
        );
        assert!(
            !matcher.matches("mask"),
            "'mask' contains 'ask' but must not match"
        );
    }

    #[test]
    fn exact_plain_text_treats_regex_metacharacters_literally() {
        let matcher = exact("foo.bar");
        assert!(matcher.matches("foo.bar"), "literal dot matches");
        assert!(!matcher.matches("fooXbar"), "dot is not a regex wildcard");
    }

    // ── MatchKind::Exact — glob shape (kind ignored) ──────────────────────────

    #[test]
    fn exact_glob_star_matches_prefix_names() {
        let matcher = exact("setup*");
        assert!(matcher.matches("setup"), "exact basename matches");
        assert!(
            matcher.matches("setup-extra"),
            "trailing suffix matches via star"
        );
        assert!(!matcher.matches("my-setup"), "anchored at start");
    }

    #[test]
    fn exact_glob_star_is_anchored_at_both_ends() {
        let matcher = exact("merge*");
        assert!(matcher.matches("merge"), "empty suffix");
        assert!(matcher.matches("merge-clean"), "hyphen suffix");
        assert!(
            !matcher.matches("pre-merge"),
            "anchored: no suffix-position match"
        );
        assert!(!matcher.matches("pre-merge-foo"), "anchored: no mid-match");
    }

    // ── MatchKind::Substring — plain text ─────────────────────────────────────

    #[test]
    fn substring_plain_text_matches_anywhere_in_name() {
        let matcher = sub("fresh");
        assert!(matcher.matches("fresh install"), "prefix");
        assert!(matcher.matches("re-fresh"), "suffix");
        assert!(matcher.matches("fresh"), "exact");
        assert!(matcher.matches("a-fresh-start"), "infix");
    }

    #[test]
    fn substring_plain_text_does_not_match_when_absent() {
        let matcher = sub("xyz");
        assert!(!matcher.matches("fresh install"));
        assert!(!matcher.matches("merge-clean"));
        assert!(!matcher.matches(""));
    }

    #[test]
    fn substring_plain_text_treats_regex_metacharacters_literally() {
        let dot = sub("foo.bar");
        assert!(dot.matches("foo.bar"), "literal dot matches");
        assert!(
            !dot.matches("fooXbar"),
            "dot does not act as regex wildcard"
        );

        let plus = sub("a+b");
        assert!(plus.matches("a+b"), "literal plus matches");
        assert!(!plus.matches("ab"), "plus does not mean one-or-more");

        let parens = sub("(test)");
        assert!(parens.matches("(test)"), "parens match literally");
    }

    // ── Glob shape — shared behaviour across both kinds ───────────────────────

    #[test]
    fn glob_question_matches_single_char() {
        let matcher = sub("a?b");
        assert!(matcher.matches("axb"), "one char between a and b");
        assert!(!matcher.matches("ab"), "zero chars — does not match");
        assert!(!matcher.matches("axxb"), "two chars — does not match");
        assert!(matcher.matches("a.b"), "dot is a valid single char");
    }

    #[test]
    fn glob_dot_treated_literally_not_as_regex_wildcard() {
        let matcher = sub("foo.*");
        assert!(matcher.matches("foo.bar"), "literal dot in glob");
        assert!(!matcher.matches("foobar"), "no dot — does not match");
        assert!(!matcher.matches("foo-bar"), "hyphen — does not match");
    }

    #[test]
    fn glob_star_in_middle_matches_arbitrary_interior() {
        let matcher = sub("pre*fix");
        assert!(matcher.matches("prefix"), "zero chars in middle");
        assert!(matcher.matches("pre-fix"), "one char in middle");
        assert!(
            matcher.matches("pre-long-middle-fix"),
            "many chars in middle"
        );
        assert!(!matcher.matches("pre"), "must end with 'fix'");
    }

    #[test]
    fn glob_double_star_acts_as_star() {
        let matcher = sub("a**b");
        assert!(matcher.matches("ab"), "zero chars for each star");
        assert!(matcher.matches("axb"), "one char");
        assert!(matcher.matches("axyb"), "two chars");
    }

    // ── Parametrized: kind × shape cross-product ──────────────────────────────

    /// Each case is `(pattern, kind, name, expected_match)`.
    #[rstest]
    // Exact plain-text: whole-name only
    #[case::exact_plain_matches_self("setup", MatchKind::Exact, "setup", true)]
    #[case::exact_plain_rejects_suffix("setup", MatchKind::Exact, "setup-extra", false)]
    #[case::exact_plain_rejects_prefix("setup", MatchKind::Exact, "my-setup", false)]
    // Substring plain-text: anywhere in name
    #[case::sub_plain_matches_self("setup", MatchKind::Substring, "setup", true)]
    #[case::sub_plain_matches_infix("setup", MatchKind::Substring, "my-setup-task", true)]
    #[case::sub_plain_matches_suffix("setup", MatchKind::Substring, "setup-extra", true)]
    // Exact glob: anchored fnmatch (kind does not change glob behaviour)
    #[case::exact_glob_matches_prefix("setup*", MatchKind::Exact, "setup-extra", true)]
    #[case::exact_glob_rejects_non_prefix("setup*", MatchKind::Exact, "my-setup", false)]
    // Substring glob: anchored fnmatch (same as Exact for globs)
    #[case::sub_glob_matches_prefix("setup*", MatchKind::Substring, "setup-extra", true)]
    #[case::sub_glob_rejects_non_prefix("setup*", MatchKind::Substring, "my-setup", false)]
    fn kind_shape_matrix(
        #[case] pattern: &str,
        #[case] kind: MatchKind,
        #[case] name: &str,
        #[case] expected: bool,
    ) {
        let matcher = compile(pattern, kind).expect("valid pattern");
        assert_eq!(
            matcher.matches(name),
            expected,
            "compile({pattern:?}, {kind:?}).matches({name:?})"
        );
    }

    // ── Regression: exact name is always matched by an equal pattern ──────────

    #[test]
    fn exact_name_is_matched_by_equal_pattern_in_both_kinds() {
        for name in ["fresh-install", "setup", "merge-clean"] {
            assert!(
                exact(name).matches(name),
                "Exact: pattern equal to name must match"
            );
            assert!(
                sub(name).matches(name),
                "Substring: pattern equal to name must match"
            );
        }
    }
}
