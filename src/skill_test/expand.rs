//! Whole-scenario placeholder expansion.
//!
//! [`expand_scenario`] walks every leaf string in a [`Scenario`] and passes it
//! through [`placeholder::expand`], returning a fully-expanded owned clone.
//! Running the walk once at the top of the scenario runner — before any
//! assertion or spawn — makes expansion a structural contract: every leaf is
//! touched, no call site can accidentally bypass it.

use serde_json::Value;

use crate::skill_test::fixture::{
    FileAssertion, FileContent, Given, Scenario, ShellHook, StdinPayload, Then, When,
};
use crate::skill_test::placeholder::{Paths, expand};

/// Walk a [`serde_json::Value`] and return a clone with every string leaf
/// passed through [`expand`].
///
/// Object keys are not expanded — keys are author identifiers, not data.
/// Numbers, booleans, and nulls clone unchanged.
pub(crate) fn expand_json(value: &Value, paths: &Paths<'_>) -> Value {
    match value {
        Value::String(s) => Value::String(expand(s, paths)),
        Value::Array(arr) => Value::Array(arr.iter().map(|v| expand_json(v, paths)).collect()),
        Value::Object(map) => {
            let mut out = serde_json::Map::with_capacity(map.len());
            for (k, v) in map {
                // Keys are identifiers; only values are expanded.
                out.insert(k.clone(), expand_json(v, paths));
            }
            Value::Object(out)
        }
        // Numbers, booleans, and null pass through unchanged.
        other => other.clone(),
    }
}

/// Return a deep clone of `scenario` with every leaf string passed through
/// [`expand`].
///
/// Leaf string positions covered:
/// - `given.files`: every key (path) and `FileContent::Text` value.
/// - `given.files`: every string leaf inside a `FileContent::Json` tree.
/// - `before.shell` and `after.shell` strings.
/// - `when.argv` and `when.env` keys/values.
/// - `when.stdin`: `StdinPayload::Text` whole-string and every string leaf
///   inside a `StdinPayload::Json` tree.
/// - `when.timeout_seconds`: not a string; passes through.
/// - `then.stdout_contains` / `then.stderr_contains` strings.
/// - `then.stdout_json`: every string leaf in the JSON tree.
/// - `then.files`: every key (path) and the `String` payload of `Equals` /
///   `Contains` / `Regex`. The JSON payload of `JsonEquals` / `JsonSubset`
///   is walked leaf-by-leaf.
/// - `then.files_absent`: every entry.
///
/// Non-string fields (`then.exit_code`, `then.coverage`, `name`, `notes`,
/// `source_file`, `source_index`) pass through unchanged. `name` and `notes`
/// are author-facing labels rendered in test output; expanding them would
/// produce different output across sandboxes, making diffing failures harder.
pub(crate) fn expand_scenario(scenario: &Scenario, paths: &Paths<'_>) -> Scenario {
    Scenario {
        name: scenario.name.clone(),
        source_file: scenario.source_file.clone(),
        source_index: scenario.source_index,
        notes: scenario.notes.clone(),
        given: expand_given(&scenario.given, paths),
        before: scenario
            .before
            .as_ref()
            .map(|h| expand_shell_hook(h, paths)),
        when: expand_when(&scenario.when, paths),
        then: expand_then(&scenario.then, paths),
        after: scenario.after.as_ref().map(|h| expand_shell_hook(h, paths)),
    }
}

fn expand_given(given: &Given, paths: &Paths<'_>) -> Given {
    Given {
        files: given
            .files
            .iter()
            .map(|(raw_path, content)| {
                let path = expand(raw_path, paths);
                let content = match content {
                    FileContent::Text(s) => FileContent::Text(expand(s, paths)),
                    FileContent::Json(v) => FileContent::Json(expand_json(v, paths)),
                };
                (path, content)
            })
            .collect(),
    }
}

fn expand_shell_hook(hook: &ShellHook, paths: &Paths<'_>) -> ShellHook {
    ShellHook {
        shell: expand(&hook.shell, paths),
    }
}

fn expand_when(when: &When, paths: &Paths<'_>) -> When {
    When {
        argv: when.argv.iter().map(|s| expand(s, paths)).collect(),
        stdin: when.stdin.as_ref().map(|p| match p {
            StdinPayload::Text(s) => StdinPayload::Text(expand(s, paths)),
            StdinPayload::Json(v) => StdinPayload::Json(expand_json(v, paths)),
        }),
        env: when
            .env
            .iter()
            .map(|(k, v)| (k.clone(), expand(v, paths)))
            .collect(),
        timeout_seconds: when.timeout_seconds,
    }
}

fn expand_then(then: &Then, paths: &Paths<'_>) -> Then {
    Then {
        exit_code: then.exit_code,
        stdout_contains: then
            .stdout_contains
            .iter()
            .map(|s| expand(s, paths))
            .collect(),
        stderr_contains: then
            .stderr_contains
            .iter()
            .map(|s| expand(s, paths))
            .collect(),
        stdout_json: then.stdout_json.as_ref().map(|v| expand_json(v, paths)),
        files: then
            .files
            .iter()
            .map(|(raw_path, assertion)| {
                let path = expand(raw_path, paths);
                let assertion = expand_file_assertion(assertion, paths);
                (path, assertion)
            })
            .collect(),
        files_absent: then.files_absent.iter().map(|s| expand(s, paths)).collect(),
        coverage: then.coverage.clone(),
    }
}

fn expand_file_assertion(assertion: &FileAssertion, paths: &Paths<'_>) -> FileAssertion {
    match assertion {
        FileAssertion::Equals(s) => FileAssertion::Equals(expand(s, paths)),
        FileAssertion::Contains(s) => FileAssertion::Contains(expand(s, paths)),
        FileAssertion::Regex(s) => FileAssertion::Regex(expand(s, paths)),
        FileAssertion::JsonEquals(v) => FileAssertion::JsonEquals(expand_json(v, paths)),
        FileAssertion::JsonSubset(v) => FileAssertion::JsonSubset(expand_json(v, paths)),
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use pretty_assertions::assert_eq;

    use crate::skill_test::fixture::{
        FileAssertion, FileContent, Given, Scenario, ShellHook, StdinPayload, Then, When,
    };
    use crate::skill_test::placeholder::Paths;

    use super::*;

    fn test_paths_owned() -> (PathBuf, PathBuf, PathBuf) {
        (
            PathBuf::from("/tmp/sb"),
            PathBuf::from("/tmp/sb/source"),
            PathBuf::from("/tmp/sb/home"),
        )
    }

    fn test_paths<'a>(sandbox: &'a Path, source: &'a Path, home: &'a Path) -> Paths<'a> {
        Paths {
            sandbox,
            source,
            home,
        }
    }

    // ── expand_json ───────────────────────────────────────────────────────────

    #[test]
    fn expand_json_replaces_string_leaves() {
        let (sb, src, home) = test_paths_owned();
        let paths = test_paths(&sb, &src, &home);

        let input = serde_json::json!({
            "cwd": "{sandbox}/wt",
            "home": "{home}",
            "nested": {
                "path": "{source}/file"
            },
            "arr": ["{sandbox}", "{home}"]
        });
        let result = expand_json(&input, &paths);

        assert_eq!(result["cwd"], "/tmp/sb/wt");
        assert_eq!(result["home"], "/tmp/sb/home");
        assert_eq!(result["nested"]["path"], "/tmp/sb/source/file");
        assert_eq!(result["arr"][0], "/tmp/sb");
        assert_eq!(result["arr"][1], "/tmp/sb/home");
    }

    #[test]
    fn expand_json_leaves_non_strings_unchanged() {
        let (sb, src, home) = test_paths_owned();
        let paths = test_paths(&sb, &src, &home);

        let input = serde_json::json!({
            "count": 42,
            "flag": true,
            "nothing": null
        });
        let result = expand_json(&input, &paths);
        assert_eq!(result, input);
    }

    #[test]
    fn expand_json_does_not_expand_object_keys() {
        let (sb, src, home) = test_paths_owned();
        let paths = test_paths(&sb, &src, &home);

        let input = serde_json::json!({"{sandbox}": "value"});
        let result = expand_json(&input, &paths);
        // Key is unchanged; value would be expanded but has no placeholder here.
        assert!(result.as_object().unwrap().contains_key("{sandbox}"));
    }

    // ── expand_scenario: comprehensive coverage ───────────────────────────────

    #[test]
    fn expand_scenario_touches_every_leaf_position() {
        let (sb, src, home) = test_paths_owned();
        let paths = test_paths(&sb, &src, &home);

        let scenario = Scenario {
            name: "{sandbox}-name".to_owned(),
            source_file: PathBuf::from("test.test.yaml"),
            source_index: 0,
            notes: Some("{sandbox}-notes".to_owned()),
            given: Given {
                files: vec![
                    (
                        "{source}/seed.txt".to_owned(),
                        FileContent::Text("content at {home}".to_owned()),
                    ),
                    (
                        "{source}/data.json".to_owned(),
                        FileContent::Json(serde_json::json!({"dir": "{sandbox}/wt"})),
                    ),
                ],
            },
            before: Some(ShellHook {
                shell: "mkdir {source}/sub".to_owned(),
            }),
            when: When {
                argv: vec!["creft".to_owned(), "{source}/skill".to_owned()],
                stdin: Some(StdinPayload::Text("{home}/.creft".to_owned())),
                env: vec![("MY_VAR".to_owned(), "{sandbox}/tmp".to_owned())],
                timeout_seconds: Some(10),
            },
            then: Then {
                exit_code: 0,
                stdout_contains: vec!["{sandbox}/wt".to_owned()],
                stderr_contains: vec!["{home}/log".to_owned()],
                stdout_json: Some(serde_json::json!({"dir": "{source}/out"})),
                files: vec![(
                    "{source}/out.txt".to_owned(),
                    FileAssertion::Equals("{home}/x".to_owned()),
                )],
                files_absent: vec!["{sandbox}/gone.txt".to_owned()],
                coverage: None,
            },
            after: Some(ShellHook {
                shell: "rm -rf {sandbox}/tmp".to_owned(),
            }),
        };

        let expanded = expand_scenario(&scenario, &paths);

        // name and notes are not expanded.
        assert_eq!(expanded.name, "{sandbox}-name");
        assert_eq!(expanded.notes.as_deref(), Some("{sandbox}-notes"));

        // given.files: path and text content.
        assert_eq!(expanded.given.files[0].0, "/tmp/sb/source/seed.txt");
        assert_eq!(
            expanded.given.files[0].1,
            FileContent::Text("content at /tmp/sb/home".to_owned())
        );
        // given.files: JSON leaf.
        let FileContent::Json(ref jv) = expanded.given.files[1].1 else {
            panic!("expected Json variant");
        };
        assert_eq!(jv["dir"], "/tmp/sb/wt");

        // before.shell.
        assert_eq!(
            expanded.before.as_ref().unwrap().shell,
            "mkdir /tmp/sb/source/sub"
        );

        // when.argv and when.env.
        assert_eq!(expanded.when.argv[1], "/tmp/sb/source/skill");
        assert_eq!(expanded.when.env[0].1, "/tmp/sb/tmp");

        // when.stdin text.
        assert_eq!(
            expanded.when.stdin,
            Some(StdinPayload::Text("/tmp/sb/home/.creft".to_owned()))
        );

        // when.timeout_seconds passes through.
        assert_eq!(expanded.when.timeout_seconds, Some(10));

        // then: stdout_contains, stderr_contains.
        assert_eq!(expanded.then.stdout_contains[0], "/tmp/sb/wt");
        assert_eq!(expanded.then.stderr_contains[0], "/tmp/sb/home/log");

        // then.stdout_json leaf.
        assert_eq!(
            expanded.then.stdout_json.as_ref().unwrap()["dir"],
            "/tmp/sb/source/out"
        );

        // then.files: path and assertion value.
        assert_eq!(expanded.then.files[0].0, "/tmp/sb/source/out.txt");
        assert_eq!(
            expanded.then.files[0].1,
            FileAssertion::Equals("/tmp/sb/home/x".to_owned())
        );

        // then.files_absent.
        assert_eq!(expanded.then.files_absent[0], "/tmp/sb/gone.txt");

        // after.shell.
        assert_eq!(expanded.after.as_ref().unwrap().shell, "rm -rf /tmp/sb/tmp");
    }

    #[test]
    fn expand_scenario_stdin_json_leaves_expanded() {
        let (sb, src, home) = test_paths_owned();
        let paths = test_paths(&sb, &src, &home);

        let scenario = Scenario {
            name: "stdin-json".to_owned(),
            source_file: PathBuf::from("t.test.yaml"),
            source_index: 0,
            notes: None,
            given: Given::default(),
            before: None,
            when: When {
                argv: vec!["cat".to_owned()],
                stdin: Some(StdinPayload::Json(serde_json::json!({
                    "cwd": "{sandbox}/wt",
                    "project": "{home}/p"
                }))),
                env: Vec::new(),
                timeout_seconds: None,
            },
            then: Then::default(),
            after: None,
        };

        let expanded = expand_scenario(&scenario, &paths);
        let StdinPayload::Json(v) = expanded.when.stdin.as_ref().unwrap() else {
            panic!("expected Json stdin");
        };
        assert_eq!(v["cwd"], "/tmp/sb/wt");
        assert_eq!(v["project"], "/tmp/sb/home/p");
    }

    #[test]
    fn expand_scenario_with_no_placeholders_round_trips() {
        let (sb, src, home) = test_paths_owned();
        let paths = test_paths(&sb, &src, &home);

        let scenario = Scenario {
            name: "no-placeholders".to_owned(),
            source_file: PathBuf::from("t.test.yaml"),
            source_index: 0,
            notes: None,
            given: Given {
                files: vec![(
                    "/absolute/path/seed.txt".to_owned(),
                    FileContent::Text("static content".to_owned()),
                )],
            },
            before: None,
            when: When {
                argv: vec!["sh".to_owned(), "-c".to_owned(), "echo hi".to_owned()],
                stdin: None,
                env: Vec::new(),
                timeout_seconds: None,
            },
            then: Then {
                exit_code: 0,
                stdout_contains: vec!["hi".to_owned()],
                stderr_contains: Vec::new(),
                stdout_json: None,
                files: Vec::new(),
                files_absent: Vec::new(),
                coverage: None,
            },
            after: None,
        };

        let expanded = expand_scenario(&scenario, &paths);
        // Everything is identical when there are no placeholders.
        assert_eq!(expanded.name, scenario.name);
        assert_eq!(expanded.given.files[0].0, "/absolute/path/seed.txt");
        assert_eq!(
            expanded.given.files[0].1,
            FileContent::Text("static content".to_owned())
        );
        assert_eq!(expanded.then.stdout_contains[0], "hi");
    }

    #[test]
    fn expand_file_assertion_covers_all_variants() {
        let (sb, src, home) = test_paths_owned();
        let paths = test_paths(&sb, &src, &home);

        assert_eq!(
            expand_file_assertion(&FileAssertion::Equals("{home}/x".to_owned()), &paths),
            FileAssertion::Equals("/tmp/sb/home/x".to_owned())
        );
        assert_eq!(
            expand_file_assertion(&FileAssertion::Contains("{sandbox}/y".to_owned()), &paths),
            FileAssertion::Contains("/tmp/sb/y".to_owned())
        );
        assert_eq!(
            expand_file_assertion(&FileAssertion::Regex("{source}/z.*".to_owned()), &paths),
            FileAssertion::Regex("/tmp/sb/source/z.*".to_owned())
        );
        let je = expand_file_assertion(
            &FileAssertion::JsonEquals(serde_json::json!({"dir": "{sandbox}/wt"})),
            &paths,
        );
        let FileAssertion::JsonEquals(ref v) = je else {
            panic!("expected JsonEquals");
        };
        assert_eq!(v["dir"], "/tmp/sb/wt");
        let js = expand_file_assertion(
            &FileAssertion::JsonSubset(serde_json::json!({"dir": "{home}/p"})),
            &paths,
        );
        let FileAssertion::JsonSubset(ref v) = js else {
            panic!("expected JsonSubset");
        };
        assert_eq!(v["dir"], "/tmp/sb/home/p");
    }
}
