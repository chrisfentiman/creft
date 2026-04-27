//! Placeholder expansion for `{sandbox}`, `{source}`, and `{home}` references.
//!
//! Placeholders appear in fixture paths and string content. They are expanded
//! at scenario execution time, not parse time, so the same parsed [`Scenario`]
//! can be re-used in different sandboxes.
//!
//! Unknown placeholders (e.g. a future `{cache}`) are left as literal text.
//! This keeps older `creft` versions forward-compatible with fixtures that
//! reference placeholders that do not yet exist.
//!
//! [`Scenario`]: crate::skill_test::fixture::Scenario

use std::path::Path;

/// Paths that back the three recognised placeholders.
pub(crate) struct Paths<'a> {
    /// Root of the sandbox temp directory.
    pub sandbox: &'a Path,
    /// `{sandbox}/source` — the project root the child process sees.
    pub source: &'a Path,
    /// `{sandbox}/home` — `HOME` for the child process.
    pub home: &'a Path,
}

/// Expand `{sandbox}`, `{source}`, and `{home}` references in `s`.
///
/// Replacements are applied in longest-match order (`{source}` before
/// `{sandbox}`) so that a string containing `{source}` does not first have
/// `{sandbox}` substituted, producing a garbled intermediate.
///
/// Unknown placeholders are left as literal text.
pub(crate) fn expand(s: &str, paths: &Paths<'_>) -> String {
    let sandbox = paths.sandbox.to_string_lossy();
    let source = paths.source.to_string_lossy();
    let home = paths.home.to_string_lossy();

    // Replace longest tokens first to avoid partial substitutions.
    // {source} contains the string "source", which does not overlap with
    // {sandbox} or {home}, so order matters only for safety, not correctness
    // in practice. Explicit longest-first ordering makes the guarantee clear.
    s.replace("{source}", &source)
        .replace("{home}", &home)
        .replace("{sandbox}", &sandbox)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::*;

    struct TestPaths {
        sandbox: PathBuf,
        source: PathBuf,
        home: PathBuf,
    }

    impl TestPaths {
        fn new() -> Self {
            Self {
                sandbox: PathBuf::from("/tmp/sb"),
                source: PathBuf::from("/tmp/sb/source"),
                home: PathBuf::from("/tmp/sb/home"),
            }
        }

        fn as_paths(&self) -> Paths<'_> {
            Paths {
                sandbox: &self.sandbox,
                source: &self.source,
                home: &self.home,
            }
        }
    }

    #[rstest]
    #[case::source_placeholder("{source}/foo.txt", "/tmp/sb/source/foo.txt")]
    #[case::home_placeholder("{home}/.creft", "/tmp/sb/home/.creft")]
    #[case::sandbox_placeholder("{sandbox}/scratch", "/tmp/sb/scratch")]
    fn placeholder_expanded(#[case] input: &str, #[case] expected: &str) {
        let tp = TestPaths::new();
        let p = tp.as_paths();
        assert_eq!(expand(input, &p), expected);
    }

    #[test]
    fn multiple_placeholders_in_one_string() {
        let tp = TestPaths::new();
        let p = tp.as_paths();
        let result = expand("cp {source}/a.txt {home}/b.txt && rm {sandbox}/tmp", &p);
        assert_eq!(
            result,
            "cp /tmp/sb/source/a.txt /tmp/sb/home/b.txt && rm /tmp/sb/tmp"
        );
    }

    #[test]
    fn unknown_placeholder_left_as_literal() {
        let tp = TestPaths::new();
        let p = tp.as_paths();
        assert_eq!(expand("{cache}/foo", &p), "{cache}/foo");
    }

    #[test]
    fn no_placeholders_returns_input_unchanged() {
        let tp = TestPaths::new();
        let p = tp.as_paths();
        assert_eq!(expand("plain string", &p), "plain string");
    }

    #[test]
    fn source_does_not_expand_sandbox_prefix() {
        // {source} must NOT be corrupted by a prior {sandbox} expansion:
        // {source} → /tmp/sb/source, not /tmp/sb/sb/source
        let tp = TestPaths::new();
        let p = tp.as_paths();
        let result = expand("{source}/x and {sandbox}/y", &p);
        assert_eq!(result, "/tmp/sb/source/x and /tmp/sb/y");
    }
}
