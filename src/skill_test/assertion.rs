//! File, stdout, and JSON assertion primitives for scenario checking.
//!
//! Each function takes the parsed [`Then`] block and actual output and returns
//! any failures found. Callers collect all failures and report them together so
//! the author sees every broken assertion in one run, not just the first.

use std::path::Path;

use regex::Regex;

use crate::skill_test::fixture::{FileAssertion, Then};

/// A single failed assertion, carrying enough context to print a useful message.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct AssertionFailure {
    /// Category of assertion: `"exit_code"`, `"stdout_contains"`,
    /// `"stderr_contains"`, `"stdout_json"`, `"file"`, `"file_absent"`.
    pub kind: &'static str,
    /// The expected value as a human-readable string.
    pub expected: String,
    /// The actual value as a human-readable string.
    pub actual: String,
    /// Optional locator — file path for file checks, block index for coverage,
    /// etc. `None` for single-valued assertions like `exit_code`.
    pub locator: Option<String>,
}

// ── Exit code ─────────────────────────────────────────────────────────────────

/// Check the child's exit code against `then.exit_code`.
///
/// Returns `Some(failure)` when the codes differ, `None` on match.
pub(crate) fn check_exit_code(then: &Then, actual: i32) -> Option<AssertionFailure> {
    if actual == then.exit_code {
        return None;
    }
    Some(AssertionFailure {
        kind: "exit_code",
        expected: then.exit_code.to_string(),
        actual: actual.to_string(),
        locator: None,
    })
}

// ── Stdout / stderr containment ───────────────────────────────────────────────

/// Check that every string in `then.stdout_contains` appears in `stdout`.
pub(crate) fn check_stdout_contains(then: &Then, stdout: &str) -> Vec<AssertionFailure> {
    check_contains("stdout_contains", then.stdout_contains.iter(), stdout)
}

/// Check that every string in `then.stderr_contains` appears in `stderr`.
pub(crate) fn check_stderr_contains(then: &Then, stderr: &str) -> Vec<AssertionFailure> {
    check_contains("stderr_contains", then.stderr_contains.iter(), stderr)
}

fn check_contains<'a>(
    kind: &'static str,
    needles: impl Iterator<Item = &'a String>,
    haystack: &str,
) -> Vec<AssertionFailure> {
    needles
        .filter(|needle| !haystack.contains(needle.as_str()))
        .map(|needle| AssertionFailure {
            kind,
            expected: format!("output to contain {:?}", needle),
            actual: truncate(haystack, 200),
            locator: None,
        })
        .collect()
}

// ── Stdout JSON subset ────────────────────────────────────────────────────────

/// Check that stdout parses as JSON and the `then.stdout_json` value is a
/// subset of it.
///
/// Returns `None` when `then.stdout_json` is absent. Returns one failure when
/// stdout does not parse as JSON or the subset check fails.
pub(crate) fn check_stdout_json(then: &Then, stdout: &str) -> Option<AssertionFailure> {
    let expected = then.stdout_json.as_ref()?;

    let actual: serde_json::Value = match serde_json::from_str(stdout.trim()) {
        Ok(v) => v,
        Err(e) => {
            return Some(AssertionFailure {
                kind: "stdout_json",
                expected: format!(
                    "valid JSON: {}",
                    serde_json::to_string_pretty(expected).unwrap_or_default()
                ),
                actual: format!("parse error: {e}"),
                locator: None,
            });
        }
    };

    if json_subset(expected, &actual) {
        return None;
    }

    Some(AssertionFailure {
        kind: "stdout_json",
        expected: serde_json::to_string_pretty(expected).unwrap_or_default(),
        actual: serde_json::to_string_pretty(&actual).unwrap_or_default(),
        locator: None,
    })
}

// ── File assertions ───────────────────────────────────────────────────────────

/// Verify every expected file assertion.
///
/// Paths and assertion values in `then` must already be placeholder-expanded
/// by the caller. This function reads files and compares content directly.
pub(crate) fn check_files(then: &Then) -> Vec<AssertionFailure> {
    let mut failures = Vec::new();

    for (path_str, assertion) in &then.files {
        let path = Path::new(path_str);

        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                failures.push(AssertionFailure {
                    kind: "file",
                    expected: "file to exist and be readable".to_string(),
                    actual: format!("read error: {e}"),
                    locator: Some(path_str.clone()),
                });
                continue;
            }
        };

        if let Some(failure) = check_file_assertion(path_str, &content, assertion) {
            failures.push(failure);
        }
    }

    failures
}

fn check_file_assertion(
    path: &str,
    content: &str,
    assertion: &FileAssertion,
) -> Option<AssertionFailure> {
    match assertion {
        FileAssertion::Equals(expected) => {
            if content == expected {
                return None;
            }
            Some(AssertionFailure {
                kind: "file",
                expected: expected.clone(),
                actual: content.to_owned(),
                locator: Some(path.to_owned()),
            })
        }
        FileAssertion::Contains(needle) => {
            if content.contains(needle.as_str()) {
                return None;
            }
            Some(AssertionFailure {
                kind: "file",
                expected: format!("file to contain {:?}", needle),
                actual: truncate(content, 200),
                locator: Some(path.to_owned()),
            })
        }
        FileAssertion::Regex(pattern) => {
            let re = match Regex::new(pattern) {
                Ok(r) => r,
                Err(e) => {
                    return Some(AssertionFailure {
                        kind: "file",
                        expected: format!("valid regex {:?}", pattern),
                        actual: format!("regex compile error: {e}"),
                        locator: Some(path.to_owned()),
                    });
                }
            };
            if re.is_match(content) {
                return None;
            }
            Some(AssertionFailure {
                kind: "file",
                expected: format!("file to match regex {:?}", pattern),
                actual: truncate(content, 200),
                locator: Some(path.to_owned()),
            })
        }
        FileAssertion::JsonEquals(expected) => {
            let actual: serde_json::Value = match serde_json::from_str(content.trim()) {
                Ok(v) => v,
                Err(e) => {
                    return Some(AssertionFailure {
                        kind: "file",
                        expected: format!(
                            "valid JSON: {}",
                            serde_json::to_string_pretty(expected).unwrap_or_default()
                        ),
                        actual: format!("parse error: {e}"),
                        locator: Some(path.to_owned()),
                    });
                }
            };
            if actual == *expected {
                return None;
            }
            Some(AssertionFailure {
                kind: "file",
                expected: serde_json::to_string_pretty(expected).unwrap_or_default(),
                actual: serde_json::to_string_pretty(&actual).unwrap_or_default(),
                locator: Some(path.to_owned()),
            })
        }
        FileAssertion::JsonSubset(expected) => {
            let actual: serde_json::Value = match serde_json::from_str(content.trim()) {
                Ok(v) => v,
                Err(e) => {
                    return Some(AssertionFailure {
                        kind: "file",
                        expected: format!(
                            "valid JSON superset of: {}",
                            serde_json::to_string_pretty(expected).unwrap_or_default()
                        ),
                        actual: format!("parse error: {e}"),
                        locator: Some(path.to_owned()),
                    });
                }
            };
            if json_subset(expected, &actual) {
                return None;
            }
            Some(AssertionFailure {
                kind: "file",
                expected: serde_json::to_string_pretty(expected).unwrap_or_default(),
                actual: serde_json::to_string_pretty(&actual).unwrap_or_default(),
                locator: Some(path.to_owned()),
            })
        }
    }
}

// ── Absent file assertions ────────────────────────────────────────────────────

/// Verify that every path in `then.files_absent` does not exist after the scenario.
///
/// Paths in `then` must already be placeholder-expanded by the caller.
pub(crate) fn check_files_absent(then: &Then) -> Vec<AssertionFailure> {
    let mut failures = Vec::new();

    for path_str in &then.files_absent {
        let path = Path::new(path_str);

        if path.exists() {
            failures.push(AssertionFailure {
                kind: "file_absent",
                expected: "path to not exist".to_owned(),
                actual: "path exists".to_owned(),
                locator: Some(path_str.clone()),
            });
        }
    }

    failures
}

// ── JSON subset matching ──────────────────────────────────────────────────────

/// Recursive subset match: every key/value in `expected` must appear in `actual`.
///
/// For objects: every key in `expected` must exist in `actual` with a value that
/// is itself a subset match.
///
/// For arrays: every element of `expected` must subset-match at least one element
/// of `actual`. Order is not significant — this matches the semantics the POC
/// used for hook arrays and generalises to any JSON structure.
///
/// For scalar values: the values must be equal.
pub(crate) fn json_subset(expected: &serde_json::Value, actual: &serde_json::Value) -> bool {
    use serde_json::Value;

    match expected {
        Value::Object(exp_map) => {
            let Value::Object(act_map) = actual else {
                return false;
            };
            for (k, exp_val) in exp_map {
                match act_map.get(k) {
                    Some(act_val) => {
                        if !json_subset(exp_val, act_val) {
                            return false;
                        }
                    }
                    None => return false,
                }
            }
            true
        }
        Value::Array(exp_arr) => {
            let Value::Array(act_arr) = actual else {
                return false;
            };
            // Every expected element must subset-match at least one actual element.
            for exp_elem in exp_arr {
                if !act_arr
                    .iter()
                    .any(|act_elem| json_subset(exp_elem, act_elem))
                {
                    return false;
                }
            }
            true
        }
        // Scalars: exact equality.
        _ => expected == actual,
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Truncate a string to at most `max` characters for display in failure messages,
/// appending `"..."` when cut. Character-boundary-safe; never panics on multi-byte input.
fn truncate(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        None => s.to_owned(),
        Some((byte_pos, _)) => format!("{}...", &s[..byte_pos]),
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use crate::skill_test::fixture::{FileAssertion, Then};
    use crate::skill_test::placeholder;
    use crate::skill_test::sandbox::Sandbox;

    use super::*;

    fn empty_then() -> Then {
        Then::default()
    }

    /// Create a sandbox, write `content` to the expanded `path`, and return
    /// the sandbox and the resolved (expanded) path string. Callers use the
    /// returned path string to construct `then.files` entries with pre-expanded
    /// paths, matching what `expand_scenario` would produce at runtime.
    fn sandbox_with_file(path: &str, content: &str) -> (Sandbox, String) {
        let sb = Sandbox::new().expect("sandbox");
        let full_path = placeholder::expand(path, &sb.paths());
        if let Some(parent) = std::path::Path::new(&full_path).parent() {
            std::fs::create_dir_all(parent).ok();
        }
        std::fs::write(&full_path, content).expect("write file");
        (sb, full_path)
    }

    // ── check_exit_code ───────────────────────────────────────────────────────

    #[rstest]
    #[case::match_zero(0, 0, false)]
    #[case::match_nonzero(2, 2, false)]
    #[case::mismatch(0, 2, true)]
    #[case::mismatch_nonzero(1, 2, true)]
    fn exit_code_assertion(#[case] expected: i32, #[case] actual: i32, #[case] should_fail: bool) {
        let mut then = empty_then();
        then.exit_code = expected;
        let result = check_exit_code(&then, actual);
        assert_eq!(result.is_some(), should_fail);
        if let Some(f) = result {
            assert_eq!(f.kind, "exit_code");
            assert_eq!(f.expected, expected.to_string());
            assert_eq!(f.actual, actual.to_string());
        }
    }

    // ── check_stdout_contains ─────────────────────────────────────────────────

    #[test]
    fn stdout_contains_passes_when_needle_present() {
        let mut then = empty_then();
        then.stdout_contains = vec!["hello".to_owned()];
        let failures = check_stdout_contains(&then, "say hello world");
        assert!(failures.is_empty());
    }

    #[test]
    fn stdout_contains_fails_when_needle_absent() {
        let mut then = empty_then();
        then.stdout_contains = vec!["missing".to_owned()];
        let failures = check_stdout_contains(&then, "hello world");
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].kind, "stdout_contains");
    }

    #[test]
    fn stdout_contains_reports_all_missing_needles() {
        let mut then = empty_then();
        then.stdout_contains = vec!["a".to_owned(), "b".to_owned(), "c".to_owned()];
        let failures = check_stdout_contains(&then, "only b is here");
        assert_eq!(failures.len(), 2, "a and c missing");
    }

    // ── check_stderr_contains ─────────────────────────────────────────────────

    #[test]
    fn stderr_contains_passes_when_present() {
        let mut then = empty_then();
        then.stderr_contains = vec!["error: file not found".to_owned()];
        let failures = check_stderr_contains(&then, "error: file not found\n");
        assert!(failures.is_empty());
    }

    // ── check_stdout_json ─────────────────────────────────────────────────────

    #[test]
    fn stdout_json_absent_returns_none() {
        assert!(check_stdout_json(&empty_then(), "anything").is_none());
    }

    #[test]
    fn stdout_json_passes_when_subset_matches() {
        let mut then = empty_then();
        then.stdout_json = Some(serde_json::json!({"status": "ok"}));
        let stdout = r#"{"status":"ok","extra":42}"#;
        assert!(check_stdout_json(&then, stdout).is_none());
    }

    #[test]
    fn stdout_json_fails_when_key_missing() {
        let mut then = empty_then();
        then.stdout_json = Some(serde_json::json!({"missing_key": true}));
        let stdout = r#"{"other":"value"}"#;
        let f = check_stdout_json(&then, stdout).expect("should fail");
        assert_eq!(f.kind, "stdout_json");
    }

    #[test]
    fn stdout_json_fails_on_invalid_json() {
        let mut then = empty_then();
        then.stdout_json = Some(serde_json::json!({}));
        let f = check_stdout_json(&then, "not json").expect("should fail");
        assert!(f.actual.contains("parse error"));
    }

    // ── check_files ───────────────────────────────────────────────────────────

    #[test]
    fn file_equals_passes() {
        let (_sb, full_path) = sandbox_with_file("{source}/f.txt", "exact content");
        let mut then = empty_then();
        then.files = vec![(full_path, FileAssertion::Equals("exact content".to_owned()))];
        assert!(check_files(&then).is_empty());
    }

    #[test]
    fn file_equals_fails_on_mismatch() {
        let (_sb, full_path) = sandbox_with_file("{source}/f.txt", "actual");
        let mut then = empty_then();
        then.files = vec![(full_path, FileAssertion::Equals("expected".to_owned()))];
        let failures = check_files(&then);
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].kind, "file");
    }

    #[test]
    fn file_contains_passes() {
        let (_sb, full_path) = sandbox_with_file("{source}/f.txt", "hello world");
        let mut then = empty_then();
        then.files = vec![(full_path, FileAssertion::Contains("hello".to_owned()))];
        assert!(check_files(&then).is_empty());
    }

    #[test]
    fn file_regex_passes() {
        let (_sb, full_path) = sandbox_with_file("{source}/f.txt", "version: 1.2.3");
        let mut then = empty_then();
        then.files = vec![(
            full_path,
            FileAssertion::Regex(r"version: \d+\.\d+\.\d+".to_owned()),
        )];
        assert!(check_files(&then).is_empty());
    }

    #[test]
    fn file_json_equals_passes() {
        let (_sb, full_path) = sandbox_with_file("{source}/f.json", r#"{"a":1}"#);
        let mut then = empty_then();
        then.files = vec![(
            full_path,
            FileAssertion::JsonEquals(serde_json::json!({"a": 1})),
        )];
        assert!(check_files(&then).is_empty());
    }

    #[test]
    fn file_json_subset_passes_when_superset() {
        let (_sb, full_path) = sandbox_with_file("{source}/f.json", r#"{"a":1,"b":2}"#);
        let mut then = empty_then();
        then.files = vec![(
            full_path,
            FileAssertion::JsonSubset(serde_json::json!({"a": 1})),
        )];
        assert!(check_files(&then).is_empty());
    }

    #[test]
    fn file_json_subset_fails_when_key_missing() {
        let (_sb, full_path) = sandbox_with_file("{source}/f.json", r#"{"a":1}"#);
        let mut then = empty_then();
        then.files = vec![(
            full_path,
            FileAssertion::JsonSubset(serde_json::json!({"b": 2})),
        )];
        let failures = check_files(&then);
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].kind, "file");
    }

    #[test]
    fn file_missing_reports_read_error() {
        let sb = Sandbox::new().expect("sandbox");
        // Use a pre-expanded path pointing to a file that does not exist.
        let nonexistent = placeholder::expand("{source}/nonexistent.txt", &sb.paths());
        let mut then = empty_then();
        then.files = vec![(nonexistent, FileAssertion::Equals("anything".to_owned()))];
        let failures = check_files(&then);
        assert_eq!(failures.len(), 1);
        assert!(failures[0].actual.contains("read error"));
    }

    // ── check_files_absent ────────────────────────────────────────────────────

    #[test]
    fn files_absent_passes_when_not_present() {
        let sb = Sandbox::new().expect("sandbox");
        // Pre-expand the path; the file does not exist, which is the passing case.
        let missing = placeholder::expand("{source}/missing.txt", &sb.paths());
        let mut then = empty_then();
        then.files_absent = vec![missing];
        assert!(check_files_absent(&then).is_empty());
    }

    #[test]
    fn files_absent_fails_when_file_exists() {
        let (_sb, full_path) = sandbox_with_file("{source}/exists.txt", "content");
        let mut then = empty_then();
        then.files_absent = vec![full_path];
        let failures = check_files_absent(&then);
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].kind, "file_absent");
    }

    // ── json_subset ───────────────────────────────────────────────────────────

    #[rstest]
    #[case::scalar_equal(serde_json::json!(42), serde_json::json!(42), true)]
    #[case::scalar_not_equal(serde_json::json!(42), serde_json::json!(43), false)]
    #[case::object_key_present(
        serde_json::json!({"a": 1}),
        serde_json::json!({"a": 1, "b": 2}),
        true
    )]
    #[case::object_key_missing(
        serde_json::json!({"c": 3}),
        serde_json::json!({"a": 1, "b": 2}),
        false
    )]
    #[case::nested_object(
        serde_json::json!({"hooks": {"PreToolUse": [{"matcher": ""}]}}),
        serde_json::json!({"hooks": {"PreToolUse": [{"matcher": "", "extra": true}]}}),
        true
    )]
    #[case::array_order_insensitive(
        serde_json::json!([{"x": 1}]),
        serde_json::json!([{"x": 2}, {"x": 1, "y": 3}]),
        true
    )]
    #[case::array_element_missing(
        serde_json::json!([{"x": 99}]),
        serde_json::json!([{"x": 1}, {"x": 2}]),
        false
    )]
    #[case::type_mismatch_object_vs_array(
        serde_json::json!({"a": 1}),
        serde_json::json!([1, 2, 3]),
        false
    )]
    fn json_subset_contract(
        #[case] expected: serde_json::Value,
        #[case] actual: serde_json::Value,
        #[case] should_match: bool,
    ) {
        assert_eq!(json_subset(&expected, &actual), should_match);
    }
}
