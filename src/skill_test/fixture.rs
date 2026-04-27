//! `.test.yaml` fixture format, parser, and discovery walk.
//!
//! A fixture file is a YAML document whose top-level value is a list of
//! scenario mappings. Each scenario describes the initial filesystem state
//! ([`Given`]), the `creft` invocation ([`When`]), and the expected outcomes
//! ([`Then`]).
//!
//! Placeholders (`{sandbox}`, `{source}`, `{home}`) are stored unexpanded in
//! the parsed types. Expansion happens at scenario execution time so the same
//! `Scenario` value could be re-run in a different sandbox.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use yaml_rust2::{Yaml, YamlLoader};

// ── Error ─────────────────────────────────────────────────────────────────────

/// Errors that can occur while loading or walking fixture files.
#[derive(Debug, thiserror::Error)]
pub(crate) enum FixtureError {
    /// An I/O error reading a fixture file or walking the skill tree.
    #[error("read fixture {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },

    /// The YAML in a fixture file could not be parsed.
    #[error("parse fixture {path}: {message}")]
    Parse { path: PathBuf, message: String },

    /// A required field was absent or null in a scenario mapping.
    #[error("scenario {scenario} in {path}: {field} is required")]
    MissingField {
        path: PathBuf,
        scenario: String,
        field: &'static str,
    },

    /// An unrecognised top-level key was found in a scenario mapping.
    #[error("scenario {scenario} in {path}: unknown key '{key}'")]
    UnknownKey {
        path: PathBuf,
        scenario: String,
        key: String,
    },
}

// ── Public types ──────────────────────────────────────────────────────────────

/// A single test scenario parsed from a `*.test.yaml` file.
///
/// All path and string fields contain unexpanded `{sandbox}`, `{source}`, and
/// `{home}` placeholders. Expansion happens at scenario execution time, not at
/// parse time, so the same `Scenario` could be re-run in a different sandbox
/// without re-parsing.
#[derive(Debug, Clone)]
pub(crate) struct Scenario {
    /// Human-readable name, required, used in test output.
    pub name: String,
    /// The file this scenario came from.
    pub source_file: PathBuf,
    /// Zero-based position of this scenario in the YAML list.
    // Used by tests to verify parse ordering; not read by the binary path.
    #[allow(dead_code)]
    pub source_index: usize,
    /// Human-readable context rendered under `--detail`. Never asserted on.
    pub notes: Option<String>,
    pub given: Given,
    /// Optional shell command run after `given.files` are materialised, before `when`.
    /// Failure aborts the scenario.
    pub before: Option<ShellHook>,
    pub when: When,
    pub then: Then,
    /// Optional shell command always run after the scenario (even on failure).
    pub after: Option<ShellHook>,
}

/// Seed filesystem state for a scenario.
#[derive(Debug, Clone, Default)]
pub(crate) struct Given {
    /// Files to write before the scenario runs. Order is preserved (insertion order
    /// from the YAML mapping). Paths and text content may contain placeholders.
    pub files: Vec<(String, FileContent)>,
}

/// Content of a seed file written into the sandbox.
#[derive(Debug, Clone)]
pub(crate) enum FileContent {
    /// Written verbatim (including any newline from YAML block scalars).
    Text(String),
    /// A YAML mapping or sequence serialised as `serde_json::to_string_pretty`.
    Json(serde_json::Value),
}

/// The `creft` invocation for a scenario.
#[derive(Debug, Clone)]
pub(crate) struct When {
    /// Full command-line as a list of strings, e.g. `["creft", "setup", "--flag"]`.
    pub argv: Vec<String>,
    /// Optional stdin payload.
    pub stdin: Option<StdinPayload>,
    /// Extra environment variables injected into the child process.
    pub env: Vec<(String, String)>,
    /// Per-scenario timeout override, in whole seconds.
    /// `None` means "use the runner's default timeout".
    pub timeout_seconds: Option<u64>,
}

/// Stdin content for a scenario.
#[derive(Debug, Clone)]
pub(crate) enum StdinPayload {
    /// Written verbatim to the child's stdin.
    Text(String),
    /// Serialised as compact JSON before being written to stdin.
    Json(serde_json::Value),
}

/// Expected outcomes for a scenario.
#[derive(Debug, Clone)]
pub(crate) struct Then {
    /// Expected exit code. Defaults to `0` when the fixture omits `then.exit_code`.
    pub exit_code: i32,
    /// Strings that must appear in stdout (substring checks).
    pub stdout_contains: Vec<String>,
    /// Strings that must appear in stderr (substring checks).
    pub stderr_contains: Vec<String>,
    /// When set, stdout must parse as JSON and the given value must be a subset of it.
    pub stdout_json: Option<serde_json::Value>,
    /// Per-path file assertions.
    pub files: Vec<(String, FileAssertion)>,
    /// Paths that must not exist in the sandbox after the scenario.
    pub files_absent: Vec<String>,
    /// Coverage expectations checked against the runtime coverage trace.
    pub coverage: Option<CoverageExpectation>,
}

impl Default for Then {
    /// Empty `then` block: assert exit 0, no output assertions, no file
    /// assertions, no coverage. Matches a fixture with `then: {}`.
    fn default() -> Self {
        Self {
            exit_code: 0,
            stdout_contains: Vec::new(),
            stderr_contains: Vec::new(),
            stdout_json: None,
            files: Vec::new(),
            files_absent: Vec::new(),
            coverage: None,
        }
    }
}

/// Assertion applied to a file in the sandbox after the scenario runs.
#[derive(Debug, Clone)]
pub(crate) enum FileAssertion {
    /// File content must exactly equal this string.
    Equals(String),
    /// File content must contain this substring.
    Contains(String),
    /// File content must match this regular expression.
    Regex(String),
    /// File content must parse as JSON and equal this value.
    JsonEquals(serde_json::Value),
    /// File content must parse as JSON and the given value must be a subset of it.
    JsonSubset(serde_json::Value),
}

/// Coverage expectations checked against the runtime coverage trace.
#[derive(Debug, Clone, Default)]
pub(crate) struct CoverageExpectation {
    /// Block indices that must have executed at least once.
    pub blocks: Vec<usize>,
    /// Minimum primitive counts per block index, then per primitive name.
    pub primitives: BTreeMap<usize, BTreeMap<String, u32>>,
}

/// A shell command in a `before` or `after` hook.
#[derive(Debug, Clone)]
pub(crate) struct ShellHook {
    /// The shell command string, passed to `sh -c`.
    pub shell: String,
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Load every scenario in a single `*.test.yaml` file.
///
/// Returns `Err` on I/O failure, YAML syntax errors, or schema violations
/// (missing required fields, unknown keys). Each error names the file and the
/// scenario (by name when available, by index otherwise).
pub(crate) fn load_file(path: &Path) -> Result<Vec<Scenario>, FixtureError> {
    let content = std::fs::read_to_string(path).map_err(|e| FixtureError::Io {
        path: path.to_owned(),
        source: e,
    })?;

    let docs = YamlLoader::load_from_str(&content).map_err(|e| FixtureError::Parse {
        path: path.to_owned(),
        message: e.to_string(),
    })?;

    // An empty file or a file with no documents is valid — it contains no scenarios.
    let doc = match docs.into_iter().next() {
        Some(d) => d,
        None => return Ok(Vec::new()),
    };

    let list = match &doc {
        Yaml::Array(arr) => arr,
        Yaml::Null => return Ok(Vec::new()),
        _ => {
            return Err(FixtureError::Parse {
                path: path.to_owned(),
                message: "top-level value must be a YAML list of scenarios".to_owned(),
            });
        }
    };

    let mut scenarios = Vec::with_capacity(list.len());
    for (index, item) in list.iter().enumerate() {
        let scenario = parse_scenario(path, index, item)?;
        scenarios.push(scenario);
    }

    Ok(scenarios)
}

/// Walk a skill-tree root and return every `*.test.yaml` path in lexicographic order.
///
/// `root` must be a skill-tree directory — the `.creft/commands/` directory of the
/// local root, or a sub-tree of it. Do not point this at the project root: it would
/// walk `target/`, `.git/`, `workbench/`, and vendored crates, none of which contain
/// fixtures by convention.
///
/// When `skill_filter` is `Some(name)`, only paths whose basename is exactly
/// `<name>.test.yaml` are returned. The filter is applied during the walk, before
/// any file is opened, so a parse error in an unrelated fixture cannot fail a
/// focused-skill run.
pub(crate) fn discover(
    root: &Path,
    skill_filter: Option<&str>,
) -> Result<Vec<PathBuf>, FixtureError> {
    let mut found = Vec::new();
    collect_fixtures(root, skill_filter, &mut found)?;
    found.sort();
    Ok(found)
}

// ── Internal walk ─────────────────────────────────────────────────────────────

/// Recursively collect `*.test.yaml` files under `dir`, skipping symlinks.
fn collect_fixtures(
    dir: &Path,
    skill_filter: Option<&str>,
    out: &mut Vec<PathBuf>,
) -> Result<(), FixtureError> {
    let raw_entries = std::fs::read_dir(dir).map_err(|e| FixtureError::Io {
        path: dir.to_owned(),
        source: e,
    })?;

    let mut entries = Vec::new();
    for entry_result in raw_entries {
        let entry = entry_result.map_err(|e| FixtureError::Io {
            path: dir.to_owned(),
            source: e,
        })?;
        entries.push(entry);
    }

    // Visit entries in lexicographic order so recursion is deterministic even on
    // filesystems whose read_dir ordering varies (HFS+, ext4, tmpfs).
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let entry_path = entry.path();
        // Skip symlinks to avoid loops and because no creft use case traverses them.
        let file_type = entry.file_type().map_err(|e| FixtureError::Io {
            path: entry_path.clone(),
            source: e,
        })?;
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            collect_fixtures(&entry_path, skill_filter, out)?;
        } else if file_type.is_file() && is_fixture_match(&entry_path, skill_filter) {
            out.push(entry_path);
        }
    }

    Ok(())
}

/// Whether `path` is a `*.test.yaml` file that passes the optional skill filter.
fn is_fixture_match(path: &Path, skill_filter: Option<&str>) -> bool {
    let name = match path.file_name().and_then(|n| n.to_str()) {
        Some(n) => n,
        None => return false,
    };

    if !name.ends_with(".test.yaml") {
        return false;
    }

    match skill_filter {
        None => true,
        Some(filter) => {
            // The expected basename is "<filter>.test.yaml".
            name == format!("{filter}.test.yaml")
        }
    }
}

// ── Scenario parser ───────────────────────────────────────────────────────────

/// Parse one scenario from a YAML mapping node.
fn parse_scenario(path: &Path, index: usize, node: &Yaml) -> Result<Scenario, FixtureError> {
    let map = match node.as_hash() {
        Some(m) => m,
        None => {
            return Err(FixtureError::Parse {
                path: path.to_owned(),
                message: format!("scenario at index {index} is not a mapping"),
            });
        }
    };

    // Collect the scenario name first for use in error messages.
    let name = match map.get(&yaml_str("name")) {
        Some(Yaml::String(s)) => s.clone(),
        Some(Yaml::Null) | None => {
            return Err(FixtureError::MissingField {
                path: path.to_owned(),
                scenario: format!("(index {index})"),
                field: "name",
            });
        }
        Some(_) => {
            return Err(FixtureError::Parse {
                path: path.to_owned(),
                message: format!("scenario at index {index}: 'name' must be a string"),
            });
        }
    };

    // Validate that no unexpected top-level keys exist.
    const KNOWN_KEYS: &[&str] = &["name", "notes", "given", "before", "when", "then", "after"];
    for (key, _) in map.iter() {
        if let Yaml::String(k) = key
            && !KNOWN_KEYS.contains(&k.as_str())
        {
            return Err(FixtureError::UnknownKey {
                path: path.to_owned(),
                scenario: name.clone(),
                key: k.clone(),
            });
        }
    }

    let notes = extract_optional_string(map, "notes");

    let given = match map.get(&yaml_str("given")) {
        Some(Yaml::Hash(h)) => parse_given(path, &name, h)?,
        Some(Yaml::Null) | None => Given::default(),
        Some(_) => {
            return Err(FixtureError::Parse {
                path: path.to_owned(),
                message: format!("scenario '{name}': 'given' must be a mapping"),
            });
        }
    };

    let before = match map.get(&yaml_str("before")) {
        Some(Yaml::Hash(h)) => Some(parse_shell_hook(path, &name, "before", h)?),
        Some(Yaml::Null) | None => None,
        Some(_) => {
            return Err(FixtureError::Parse {
                path: path.to_owned(),
                message: format!("scenario '{name}': 'before' must be a mapping"),
            });
        }
    };

    let when = match map.get(&yaml_str("when")) {
        Some(Yaml::Hash(h)) => parse_when(path, &name, h)?,
        Some(Yaml::Null) | None => {
            return Err(FixtureError::MissingField {
                path: path.to_owned(),
                scenario: name.clone(),
                field: "when",
            });
        }
        Some(_) => {
            return Err(FixtureError::Parse {
                path: path.to_owned(),
                message: format!("scenario '{name}': 'when' must be a mapping"),
            });
        }
    };

    let then = match map.get(&yaml_str("then")) {
        Some(Yaml::Hash(h)) => parse_then(path, &name, h)?,
        Some(Yaml::Null) | None => Then::default(),
        Some(_) => {
            return Err(FixtureError::Parse {
                path: path.to_owned(),
                message: format!("scenario '{name}': 'then' must be a mapping"),
            });
        }
    };

    let after = match map.get(&yaml_str("after")) {
        Some(Yaml::Hash(h)) => Some(parse_shell_hook(path, &name, "after", h)?),
        Some(Yaml::Null) | None => None,
        Some(_) => {
            return Err(FixtureError::Parse {
                path: path.to_owned(),
                message: format!("scenario '{name}': 'after' must be a mapping"),
            });
        }
    };

    Ok(Scenario {
        name,
        source_file: path.to_owned(),
        source_index: index,
        notes,
        given,
        before,
        when,
        then,
        after,
    })
}

// ── Section parsers ───────────────────────────────────────────────────────────

fn parse_given(
    path: &Path,
    scenario: &str,
    map: &yaml_rust2::yaml::Hash,
) -> Result<Given, FixtureError> {
    // Validate known keys under `given`.
    const KNOWN: &[&str] = &["files"];
    for (key, _) in map.iter() {
        if let Yaml::String(k) = key
            && !KNOWN.contains(&k.as_str())
        {
            return Err(FixtureError::UnknownKey {
                path: path.to_owned(),
                scenario: scenario.to_owned(),
                key: format!("given.{k}"),
            });
        }
    }

    let files = match map.get(&yaml_str("files")) {
        Some(Yaml::Hash(h)) => {
            let mut result = Vec::with_capacity(h.len());
            for (k, v) in h.iter() {
                let file_path = match k {
                    Yaml::String(s) => s.clone(),
                    _ => {
                        return Err(FixtureError::Parse {
                            path: path.to_owned(),
                            message: format!(
                                "scenario '{scenario}': given.files keys must be strings"
                            ),
                        });
                    }
                };
                let content = yaml_to_file_content(path, scenario, "given.files", v)?;
                result.push((file_path, content));
            }
            result
        }
        Some(Yaml::Null) | None => Vec::new(),
        Some(_) => {
            return Err(FixtureError::Parse {
                path: path.to_owned(),
                message: format!("scenario '{scenario}': 'given.files' must be a mapping"),
            });
        }
    };

    Ok(Given { files })
}

fn parse_when(
    path: &Path,
    scenario: &str,
    map: &yaml_rust2::yaml::Hash,
) -> Result<When, FixtureError> {
    // Validate known keys under `when`.
    const KNOWN: &[&str] = &["argv", "stdin", "env", "timeout_seconds"];
    for (key, _) in map.iter() {
        if let Yaml::String(k) = key
            && !KNOWN.contains(&k.as_str())
        {
            return Err(FixtureError::UnknownKey {
                path: path.to_owned(),
                scenario: scenario.to_owned(),
                key: format!("when.{k}"),
            });
        }
    }

    // `argv` is required.
    let argv = match map.get(&yaml_str("argv")) {
        Some(Yaml::Array(arr)) => {
            let mut result = Vec::with_capacity(arr.len());
            for item in arr.iter() {
                match item {
                    Yaml::String(s) => result.push(s.clone()),
                    _ => {
                        return Err(FixtureError::Parse {
                            path: path.to_owned(),
                            message: format!(
                                "scenario '{scenario}': when.argv elements must be strings"
                            ),
                        });
                    }
                }
            }
            result
        }
        Some(Yaml::Null) | None => {
            return Err(FixtureError::MissingField {
                path: path.to_owned(),
                scenario: scenario.to_owned(),
                field: "when.argv",
            });
        }
        Some(_) => {
            return Err(FixtureError::Parse {
                path: path.to_owned(),
                message: format!("scenario '{scenario}': 'when.argv' must be a list"),
            });
        }
    };

    let stdin = match map.get(&yaml_str("stdin")) {
        Some(Yaml::String(s)) => Some(StdinPayload::Text(s.clone())),
        Some(Yaml::Null) | None => None,
        Some(Yaml::Hash(_) | Yaml::Array(_)) => {
            // Object/list → serialise as compact JSON before writing to stdin.
            let other = map
                .get(&yaml_str("stdin"))
                .expect("key confirmed present in the arms above");
            let json_val = yaml_to_json_value(path, scenario, "when.stdin", other)?;
            Some(StdinPayload::Json(json_val))
        }
        Some(_) => {
            // Bare scalars (integer, boolean, float) are not valid stdin values.
            // Use a string or an object/list.
            return Err(FixtureError::Parse {
                path: path.to_owned(),
                message: format!(
                    "scenario '{scenario}': 'when.stdin' must be a string, mapping, or list"
                ),
            });
        }
    };

    let env = match map.get(&yaml_str("env")) {
        Some(Yaml::Hash(h)) => {
            let mut result = Vec::with_capacity(h.len());
            for (k, v) in h.iter() {
                let key = match k {
                    Yaml::String(s) => s.clone(),
                    _ => {
                        return Err(FixtureError::Parse {
                            path: path.to_owned(),
                            message: format!(
                                "scenario '{scenario}': when.env keys must be strings"
                            ),
                        });
                    }
                };
                let val = match v {
                    Yaml::String(s) => s.clone(),
                    _ => {
                        return Err(FixtureError::Parse {
                            path: path.to_owned(),
                            message: format!(
                                "scenario '{scenario}': when.env values must be strings"
                            ),
                        });
                    }
                };
                result.push((key, val));
            }
            result
        }
        Some(Yaml::Null) | None => Vec::new(),
        Some(_) => {
            return Err(FixtureError::Parse {
                path: path.to_owned(),
                message: format!("scenario '{scenario}': 'when.env' must be a mapping"),
            });
        }
    };

    let timeout_seconds = match map.get(&yaml_str("timeout_seconds")) {
        Some(Yaml::Integer(n)) => {
            if *n < 0 {
                return Err(FixtureError::Parse {
                    path: path.to_owned(),
                    message: format!(
                        "scenario '{scenario}': when.timeout_seconds must be a non-negative integer, got {n}"
                    ),
                });
            }
            Some(*n as u64)
        }
        Some(Yaml::Null) | None => None,
        Some(other) => {
            return Err(FixtureError::Parse {
                path: path.to_owned(),
                message: format!(
                    "scenario '{scenario}': when.timeout_seconds must be an integer, got {other:?}"
                ),
            });
        }
    };

    Ok(When {
        argv,
        stdin,
        env,
        timeout_seconds,
    })
}

fn parse_then(
    path: &Path,
    scenario: &str,
    map: &yaml_rust2::yaml::Hash,
) -> Result<Then, FixtureError> {
    // Validate known keys under `then`.
    const KNOWN: &[&str] = &[
        "exit_code",
        "stdout_contains",
        "stderr_contains",
        "stdout_json",
        "files",
        "files_absent",
        "coverage",
    ];
    for (key, _) in map.iter() {
        if let Yaml::String(k) = key
            && !KNOWN.contains(&k.as_str())
        {
            return Err(FixtureError::UnknownKey {
                path: path.to_owned(),
                scenario: scenario.to_owned(),
                key: format!("then.{k}"),
            });
        }
    }

    let exit_code = match map.get(&yaml_str("exit_code")) {
        Some(Yaml::Integer(n)) => *n as i32,
        Some(Yaml::Null) | None => 0,
        Some(other) => {
            return Err(FixtureError::Parse {
                path: path.to_owned(),
                message: format!(
                    "scenario '{scenario}': then.exit_code must be an integer, got {other:?}"
                ),
            });
        }
    };

    let stdout_contains = extract_string_list(path, scenario, "then.stdout_contains", map)?;
    let stderr_contains = extract_string_list(path, scenario, "then.stderr_contains", map)?;

    let stdout_json = match map.get(&yaml_str("stdout_json")) {
        Some(Yaml::Null) | None => None,
        Some(v) => Some(yaml_to_json_value(path, scenario, "then.stdout_json", v)?),
    };

    let files = match map.get(&yaml_str("files")) {
        Some(Yaml::Hash(h)) => {
            let mut result = Vec::with_capacity(h.len());
            for (k, v) in h.iter() {
                let file_path = match k {
                    Yaml::String(s) => s.clone(),
                    _ => {
                        return Err(FixtureError::Parse {
                            path: path.to_owned(),
                            message: format!(
                                "scenario '{scenario}': then.files keys must be strings"
                            ),
                        });
                    }
                };
                let assertion = parse_file_assertion(path, scenario, &file_path, v)?;
                result.push((file_path, assertion));
            }
            result
        }
        Some(Yaml::Null) | None => Vec::new(),
        Some(_) => {
            return Err(FixtureError::Parse {
                path: path.to_owned(),
                message: format!("scenario '{scenario}': 'then.files' must be a mapping"),
            });
        }
    };

    let files_absent = match map.get(&yaml_str("files_absent")) {
        Some(Yaml::Array(arr)) => {
            let mut result = Vec::with_capacity(arr.len());
            for item in arr.iter() {
                match item {
                    Yaml::String(s) => result.push(s.clone()),
                    _ => {
                        return Err(FixtureError::Parse {
                            path: path.to_owned(),
                            message: format!(
                                "scenario '{scenario}': then.files_absent elements must be strings"
                            ),
                        });
                    }
                }
            }
            result
        }
        Some(Yaml::Null) | None => Vec::new(),
        Some(_) => {
            return Err(FixtureError::Parse {
                path: path.to_owned(),
                message: format!("scenario '{scenario}': 'then.files_absent' must be a list"),
            });
        }
    };

    let coverage = match map.get(&yaml_str("coverage")) {
        Some(Yaml::Hash(h)) => Some(parse_coverage(path, scenario, h)?),
        Some(Yaml::Null) | None => None,
        Some(_) => {
            return Err(FixtureError::Parse {
                path: path.to_owned(),
                message: format!("scenario '{scenario}': 'then.coverage' must be a mapping"),
            });
        }
    };

    Ok(Then {
        exit_code,
        stdout_contains,
        stderr_contains,
        stdout_json,
        files,
        files_absent,
        coverage,
    })
}

fn parse_file_assertion(
    path: &Path,
    scenario: &str,
    file_path: &str,
    node: &Yaml,
) -> Result<FileAssertion, FixtureError> {
    let map = match node.as_hash() {
        Some(m) => m,
        None => {
            return Err(FixtureError::Parse {
                path: path.to_owned(),
                message: format!(
                    "scenario '{scenario}': then.files['{file_path}'] must be a mapping"
                ),
            });
        }
    };

    // Exactly one assertion key must be present.
    let assertion_keys: Vec<_> = ["equals", "contains", "regex", "json_equals", "json_subset"]
        .iter()
        .filter(|k| map.contains_key(&yaml_str(k)))
        .collect();

    if assertion_keys.is_empty() {
        return Err(FixtureError::Parse {
            path: path.to_owned(),
            message: format!(
                "scenario '{scenario}': then.files['{file_path}'] must have one of: equals, contains, regex, json_equals, json_subset"
            ),
        });
    }
    if assertion_keys.len() > 1 {
        return Err(FixtureError::Parse {
            path: path.to_owned(),
            message: format!(
                "scenario '{scenario}': then.files['{file_path}'] has multiple assertion keys; use exactly one"
            ),
        });
    }

    // Reject unknown keys in the file assertion mapping.
    const ASSERTION_KEYS: &[&str] = &["equals", "contains", "regex", "json_equals", "json_subset"];
    for (key, _) in map.iter() {
        if let Yaml::String(k) = key
            && !ASSERTION_KEYS.contains(&k.as_str())
        {
            return Err(FixtureError::UnknownKey {
                path: path.to_owned(),
                scenario: scenario.to_owned(),
                key: format!("then.files['{file_path}'].{k}"),
            });
        }
    }

    match *assertion_keys[0] {
        "equals" => {
            let s = require_string_field(
                path,
                scenario,
                &format!("then.files['{file_path}'].equals"),
                map,
                "equals",
            )?;
            Ok(FileAssertion::Equals(s))
        }
        "contains" => {
            let s = require_string_field(
                path,
                scenario,
                &format!("then.files['{file_path}'].contains"),
                map,
                "contains",
            )?;
            Ok(FileAssertion::Contains(s))
        }
        "regex" => {
            let s = require_string_field(
                path,
                scenario,
                &format!("then.files['{file_path}'].regex"),
                map,
                "regex",
            )?;
            Ok(FileAssertion::Regex(s))
        }
        "json_equals" => {
            let v = yaml_to_json_value(
                path,
                scenario,
                &format!("then.files['{file_path}'].json_equals"),
                map.get(&yaml_str("json_equals"))
                    .expect("key presence confirmed by assertion_keys filter above"),
            )?;
            Ok(FileAssertion::JsonEquals(v))
        }
        "json_subset" => {
            let v = yaml_to_json_value(
                path,
                scenario,
                &format!("then.files['{file_path}'].json_subset"),
                map.get(&yaml_str("json_subset"))
                    .expect("key presence confirmed by assertion_keys filter above"),
            )?;
            Ok(FileAssertion::JsonSubset(v))
        }
        _ => unreachable!("assertion key was validated above"),
    }
}

fn parse_coverage(
    path: &Path,
    scenario: &str,
    map: &yaml_rust2::yaml::Hash,
) -> Result<CoverageExpectation, FixtureError> {
    // Validate known keys under `coverage`.
    const KNOWN: &[&str] = &["blocks", "primitives"];
    for (key, _) in map.iter() {
        if let Yaml::String(k) = key
            && !KNOWN.contains(&k.as_str())
        {
            return Err(FixtureError::UnknownKey {
                path: path.to_owned(),
                scenario: scenario.to_owned(),
                key: format!("then.coverage.{k}"),
            });
        }
    }

    let blocks = match map.get(&yaml_str("blocks")) {
        Some(Yaml::Array(arr)) => {
            let mut result = Vec::with_capacity(arr.len());
            for item in arr.iter() {
                match item {
                    Yaml::Integer(n) if *n >= 0 => result.push(*n as usize),
                    _ => {
                        return Err(FixtureError::Parse {
                            path: path.to_owned(),
                            message: format!(
                                "scenario '{scenario}': then.coverage.blocks elements must be non-negative integers"
                            ),
                        });
                    }
                }
            }
            result
        }
        Some(Yaml::Null) | None => Vec::new(),
        Some(_) => {
            return Err(FixtureError::Parse {
                path: path.to_owned(),
                message: format!("scenario '{scenario}': 'then.coverage.blocks' must be a list"),
            });
        }
    };

    let primitives = match map.get(&yaml_str("primitives")) {
        Some(Yaml::Hash(h)) => {
            let mut result = BTreeMap::new();
            for (block_key, counts_node) in h.iter() {
                let block_idx = match block_key {
                    Yaml::Integer(n) if *n >= 0 => *n as usize,
                    _ => {
                        return Err(FixtureError::Parse {
                            path: path.to_owned(),
                            message: format!(
                                "scenario '{scenario}': then.coverage.primitives keys must be non-negative integers"
                            ),
                        });
                    }
                };
                let counts = parse_primitive_counts(path, scenario, block_idx, counts_node)?;
                result.insert(block_idx, counts);
            }
            result
        }
        Some(Yaml::Null) | None => BTreeMap::new(),
        Some(_) => {
            return Err(FixtureError::Parse {
                path: path.to_owned(),
                message: format!(
                    "scenario '{scenario}': 'then.coverage.primitives' must be a mapping"
                ),
            });
        }
    };

    Ok(CoverageExpectation { blocks, primitives })
}

fn parse_primitive_counts(
    path: &Path,
    scenario: &str,
    block_idx: usize,
    node: &Yaml,
) -> Result<BTreeMap<String, u32>, FixtureError> {
    let map = match node.as_hash() {
        Some(m) => m,
        None => {
            return Err(FixtureError::Parse {
                path: path.to_owned(),
                message: format!(
                    "scenario '{scenario}': then.coverage.primitives[{block_idx}] must be a mapping"
                ),
            });
        }
    };

    let mut result = BTreeMap::new();
    for (k, v) in map.iter() {
        let prim_name = match k {
            Yaml::String(s) => s.clone(),
            _ => {
                return Err(FixtureError::Parse {
                    path: path.to_owned(),
                    message: format!(
                        "scenario '{scenario}': primitive names in then.coverage.primitives[{block_idx}] must be strings"
                    ),
                });
            }
        };
        let count = match v {
            Yaml::Integer(n) if *n >= 0 => *n as u32,
            _ => {
                return Err(FixtureError::Parse {
                    path: path.to_owned(),
                    message: format!(
                        "scenario '{scenario}': then.coverage.primitives[{block_idx}]['{prim_name}'] must be a non-negative integer"
                    ),
                });
            }
        };
        result.insert(prim_name, count);
    }

    Ok(result)
}

fn parse_shell_hook(
    path: &Path,
    scenario: &str,
    section: &str,
    map: &yaml_rust2::yaml::Hash,
) -> Result<ShellHook, FixtureError> {
    const KNOWN: &[&str] = &["shell"];
    for (key, _) in map.iter() {
        if let Yaml::String(k) = key
            && !KNOWN.contains(&k.as_str())
        {
            return Err(FixtureError::UnknownKey {
                path: path.to_owned(),
                scenario: scenario.to_owned(),
                key: format!("{section}.{k}"),
            });
        }
    }

    let shell = require_string_field(path, scenario, section, map, "shell")?;
    Ok(ShellHook { shell })
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Construct a `Yaml::String` key for hash lookups.
fn yaml_str(s: &str) -> Yaml {
    Yaml::String(s.to_owned())
}

/// Extract an optional string field from a YAML mapping, returning `None` when
/// the key is absent or null.
fn extract_optional_string(map: &yaml_rust2::yaml::Hash, field: &str) -> Option<String> {
    match map.get(&yaml_str(field)) {
        Some(Yaml::String(s)) => Some(s.clone()),
        _ => None,
    }
}

/// Extract a required string field, returning `FixtureError::Parse` when the
/// field is absent, null, or not a string.
fn require_string_field(
    path: &Path,
    scenario: &str,
    context: &str,
    map: &yaml_rust2::yaml::Hash,
    field: &str,
) -> Result<String, FixtureError> {
    match map.get(&yaml_str(field)) {
        Some(Yaml::String(s)) => Ok(s.clone()),
        Some(Yaml::Null) | None => Err(FixtureError::Parse {
            path: path.to_owned(),
            message: format!("scenario '{scenario}': '{context}.{field}' is required"),
        }),
        Some(_) => Err(FixtureError::Parse {
            path: path.to_owned(),
            message: format!("scenario '{scenario}': '{context}.{field}' must be a string"),
        }),
    }
}

/// Extract a list of strings from a YAML mapping field, returning an empty vec
/// when absent or null.
fn extract_string_list(
    path: &Path,
    scenario: &str,
    field_label: &str,
    map: &yaml_rust2::yaml::Hash,
) -> Result<Vec<String>, FixtureError> {
    // The key is the last segment of the dot-path label, e.g. "stdout_contains"
    // from "then.stdout_contains".
    let key = field_label.rsplit('.').next().unwrap_or(field_label);
    match map.get(&yaml_str(key)) {
        Some(Yaml::Array(arr)) => {
            let mut result = Vec::with_capacity(arr.len());
            for item in arr.iter() {
                match item {
                    Yaml::String(s) => result.push(s.clone()),
                    _ => {
                        return Err(FixtureError::Parse {
                            path: path.to_owned(),
                            message: format!(
                                "scenario '{scenario}': '{field_label}' elements must be strings"
                            ),
                        });
                    }
                }
            }
            Ok(result)
        }
        Some(Yaml::Null) | None => Ok(Vec::new()),
        Some(_) => Err(FixtureError::Parse {
            path: path.to_owned(),
            message: format!("scenario '{scenario}': '{field_label}' must be a list"),
        }),
    }
}

/// Convert a `Yaml` node to a `FileContent` value.
///
/// Strings become `FileContent::Text`; maps and arrays become `FileContent::Json`.
fn yaml_to_file_content(
    path: &Path,
    scenario: &str,
    field: &str,
    node: &Yaml,
) -> Result<FileContent, FixtureError> {
    match node {
        Yaml::String(s) => Ok(FileContent::Text(s.clone())),
        Yaml::Null => Ok(FileContent::Text(String::new())),
        other => {
            let v = yaml_to_json_value(path, scenario, field, other)?;
            Ok(FileContent::Json(v))
        }
    }
}

/// Recursively convert a `Yaml` node to a `serde_json::Value`.
///
/// `Yaml::BadValue` and unrepresentable types produce `FixtureError::Parse`.
fn yaml_to_json_value(
    path: &Path,
    scenario: &str,
    field: &str,
    node: &Yaml,
) -> Result<serde_json::Value, FixtureError> {
    match node {
        Yaml::Null => Ok(serde_json::Value::Null),
        Yaml::Boolean(b) => Ok(serde_json::Value::Bool(*b)),
        Yaml::Integer(n) => Ok(serde_json::Value::Number((*n).into())),
        Yaml::Real(s) => {
            let f: f64 = s.parse().map_err(|_| FixtureError::Parse {
                path: path.to_owned(),
                message: format!(
                    "scenario '{scenario}': '{field}' contains a float that cannot be represented as JSON: {s}"
                ),
            })?;
            let n = serde_json::Number::from_f64(f).ok_or_else(|| FixtureError::Parse {
                path: path.to_owned(),
                message: format!(
                    "scenario '{scenario}': '{field}' contains a non-finite float: {s}"
                ),
            })?;
            Ok(serde_json::Value::Number(n))
        }
        Yaml::String(s) => Ok(serde_json::Value::String(s.clone())),
        Yaml::Array(arr) => {
            let items: Result<Vec<_>, _> = arr
                .iter()
                .map(|item| yaml_to_json_value(path, scenario, field, item))
                .collect();
            Ok(serde_json::Value::Array(items?))
        }
        Yaml::Hash(h) => {
            let mut obj = serde_json::Map::new();
            for (k, v) in h.iter() {
                let key = match k {
                    Yaml::String(s) => s.clone(),
                    _ => {
                        return Err(FixtureError::Parse {
                            path: path.to_owned(),
                            message: format!(
                                "scenario '{scenario}': '{field}' mapping keys must be strings"
                            ),
                        });
                    }
                };
                let val = yaml_to_json_value(path, scenario, field, v)?;
                obj.insert(key, val);
            }
            Ok(serde_json::Value::Object(obj))
        }
        Yaml::BadValue | Yaml::Alias(_) => Err(FixtureError::Parse {
            path: path.to_owned(),
            message: format!("scenario '{scenario}': '{field}' contains an unsupported YAML value"),
        }),
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use rstest::rstest;
    use std::io::Write;

    // ── Helpers ───────────────────────────────────────────────────────────────

    /// Write `content` to a temporary file and return the path. The `NamedTempFile`
    /// must be kept alive by the caller for the duration of the test.
    fn write_temp(content: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::Builder::new()
            .suffix(".test.yaml")
            .tempfile()
            .expect("tempfile");
        f.write_all(content.as_bytes()).expect("write");
        f
    }

    fn load(content: &str) -> Result<Vec<Scenario>, FixtureError> {
        let f = write_temp(content);
        load_file(f.path())
    }

    // ── Round-trip: every field type ─────────────────────────────────────────

    #[test]
    fn full_scenario_roundtrip_retains_all_fields() {
        let yaml = r#"
- name: fresh install populates the target directory
  notes: "A multi-line\nnote about this scenario."
  given:
    files:
      "{source}/.claude/rules/example.md": |
        # Example rule
        body
      "{source}/manifest.json":
        key: value
        nested:
          - a
          - b
  before:
    shell: "echo before"
  when:
    argv: [creft, setup, --flag]
    stdin: "some input"
    env:
      EXTRA: "1"
    timeout_seconds: 30
  then:
    exit_code: 0
    stdout_contains:
      - "installed"
    stderr_contains: []
    files:
      "{home}/.claude/rules/example.md":
        contains: "Example rule"
      "{home}/settings.json":
        json_subset:
          hooks: []
    files_absent:
      - "{home}/.claude/rules/missing.md"
    coverage:
      blocks: [0]
      primitives:
        0:
          print: 1
  after:
    shell: "rm -rf {sandbox}/scratch"
"#;
        let scenarios = load(yaml).expect("parse should succeed");
        assert_eq!(scenarios.len(), 1);

        let s = &scenarios[0];
        assert_eq!(s.name, "fresh install populates the target directory");
        assert_eq!(s.source_index, 0);
        assert!(s.notes.as_deref().unwrap().contains("multi-line"));

        // given.files
        assert_eq!(s.given.files.len(), 2);
        let (path0, content0) = &s.given.files[0];
        assert_eq!(path0, "{source}/.claude/rules/example.md");
        assert!(matches!(content0, FileContent::Text(_)));
        let (path1, content1) = &s.given.files[1];
        assert_eq!(path1, "{source}/manifest.json");
        assert!(matches!(content1, FileContent::Json(_)));

        // before
        assert_eq!(s.before.as_ref().unwrap().shell, "echo before");

        // when
        assert_eq!(s.when.argv, vec!["creft", "setup", "--flag"]);
        assert!(matches!(s.when.stdin, Some(StdinPayload::Text(_))));
        assert_eq!(s.when.env, vec![("EXTRA".to_owned(), "1".to_owned())]);
        assert_eq!(s.when.timeout_seconds, Some(30));

        // then
        assert_eq!(s.then.exit_code, 0);
        assert_eq!(s.then.stdout_contains, vec!["installed"]);
        assert!(s.then.stderr_contains.is_empty());
        assert_eq!(s.then.files.len(), 2);
        assert_eq!(s.then.files_absent, vec!["{home}/.claude/rules/missing.md"]);

        // coverage
        let cov = s.then.coverage.as_ref().expect("coverage present");
        assert_eq!(cov.blocks, vec![0usize]);
        assert_eq!(cov.primitives[&0]["print"], 1u32);

        // after
        assert_eq!(s.after.as_ref().unwrap().shell, "rm -rf {sandbox}/scratch");
    }

    // ── Multiple scenarios ────────────────────────────────────────────────────

    #[test]
    fn multiple_scenarios_parsed_in_declaration_order() {
        let yaml = r#"
- name: first
  when:
    argv: [creft, one]
  then:
    exit_code: 0

- name: second
  when:
    argv: [creft, two]
  then:
    exit_code: 1

- name: third
  when:
    argv: [creft, three]
"#;
        let scenarios = load(yaml).expect("parse");
        assert_eq!(scenarios.len(), 3);
        assert_eq!(scenarios[0].name, "first");
        assert_eq!(scenarios[1].name, "second");
        assert_eq!(scenarios[2].name, "third");
        assert_eq!(scenarios[0].source_index, 0);
        assert_eq!(scenarios[1].source_index, 1);
        assert_eq!(scenarios[2].source_index, 2);

        assert_eq!(scenarios[0].then.exit_code, 0);
        assert_eq!(scenarios[1].then.exit_code, 1);
        // exit_code defaults to 0 when absent
        assert_eq!(scenarios[2].then.exit_code, 0);
    }

    // ── Empty file ────────────────────────────────────────────────────────────

    #[test]
    fn empty_file_returns_empty_vec() {
        let scenarios = load("").expect("parse");
        assert!(scenarios.is_empty());
    }

    #[test]
    fn null_document_returns_empty_vec() {
        let scenarios = load("~").expect("parse");
        assert!(scenarios.is_empty());
    }

    // ── Missing required fields ───────────────────────────────────────────────

    #[test]
    fn missing_name_returns_missing_field_error() {
        let yaml = r#"
- when:
    argv: [creft, foo]
"#;
        let err = load(yaml).unwrap_err();
        assert!(
            matches!(err, FixtureError::MissingField { field, .. } if field == "name"),
            "expected MissingField(name), got: {err}"
        );
    }

    #[test]
    fn missing_when_returns_missing_field_error() {
        let yaml = r#"
- name: no-when
"#;
        let err = load(yaml).unwrap_err();
        assert!(
            matches!(err, FixtureError::MissingField { field, .. } if field == "when"),
            "expected MissingField(when), got: {err}"
        );
    }

    #[test]
    fn missing_when_argv_returns_missing_field_error() {
        let yaml = r#"
- name: no-argv
  when:
    stdin: "hello"
"#;
        let err = load(yaml).unwrap_err();
        assert!(
            matches!(err, FixtureError::MissingField { field, .. } if field == "when.argv"),
            "expected MissingField(when.argv), got: {err}"
        );
    }

    // ── Unknown key ───────────────────────────────────────────────────────────

    #[test]
    fn unknown_top_level_key_returns_unknown_key_error() {
        let yaml = r#"
- name: bad-key
  typo_field: true
  when:
    argv: [creft, foo]
"#;
        let err = load(yaml).unwrap_err();
        assert!(
            matches!(err, FixtureError::UnknownKey { ref key, .. } if key == "typo_field"),
            "expected UnknownKey(typo_field), got: {err}"
        );
    }

    #[test]
    fn unknown_then_key_returns_unknown_key_error() {
        let yaml = r#"
- name: bad-then-key
  when:
    argv: [creft, foo]
  then:
    exit_code: 0
    unknown_assertion: true
"#;
        let err = load(yaml).unwrap_err();
        assert!(
            matches!(err, FixtureError::UnknownKey { ref key, .. } if key == "then.unknown_assertion"),
            "expected UnknownKey(then.unknown_assertion), got: {err}"
        );
    }

    // ── notes field ───────────────────────────────────────────────────────────

    #[test]
    fn notes_field_preserved_verbatim_including_newlines() {
        let yaml = "- name: noted\n  notes: |\n    Line one.\n    Line two.\n  when:\n    argv: [creft, foo]\n";
        let scenarios = load(yaml).expect("parse");
        let notes = scenarios[0].notes.as_deref().unwrap();
        assert!(notes.contains("Line one."));
        assert!(notes.contains("Line two."));
    }

    // ── timeout_seconds ───────────────────────────────────────────────────────

    #[test]
    fn timeout_seconds_parsed_as_u64() {
        let yaml = r#"
- name: with-timeout
  when:
    argv: [creft, foo]
    timeout_seconds: 30
"#;
        let scenarios = load(yaml).expect("parse");
        assert_eq!(scenarios[0].when.timeout_seconds, Some(30));
    }

    #[test]
    fn negative_timeout_returns_parse_error() {
        let yaml = r#"
- name: bad-timeout
  when:
    argv: [creft, foo]
    timeout_seconds: -5
"#;
        let err = load(yaml).unwrap_err();
        assert!(
            matches!(err, FixtureError::Parse { .. }),
            "expected Parse error for negative timeout, got: {err}"
        );
    }

    #[rstest]
    #[case::float_timeout("3.14")]
    #[case::string_timeout("\"thirty\"")]
    fn non_integer_timeout_returns_parse_error(#[case] val: &str) {
        let yaml =
            format!("- name: bad\n  when:\n    argv: [creft, foo]\n    timeout_seconds: {val}\n");
        let err = load(&yaml).unwrap_err();
        assert!(
            matches!(err, FixtureError::Parse { .. }),
            "expected Parse error for timeout value {val}, got: {err}"
        );
    }

    // ── coverage with non-creft argv parses without error ────────────────────

    #[test]
    fn coverage_with_non_creft_argv_parses_successfully() {
        let yaml = r#"
- name: coverage-any-binary
  when:
    argv: [ls, -la]
  then:
    coverage:
      blocks: [0]
"#;
        let scenarios = load(yaml).expect("coverage with non-creft argv must parse");
        let cov = scenarios[0].then.coverage.as_ref().unwrap();
        assert_eq!(cov.blocks, vec![0usize]);
    }

    // ── FileContent::Json roundtrip ───────────────────────────────────────────

    #[test]
    fn given_files_map_value_becomes_file_content_json() {
        let yaml = r#"
- name: json-seed
  given:
    files:
      "config.json":
        enabled: true
        count: 42
        items:
          - a
          - b
  when:
    argv: [creft, foo]
"#;
        let scenarios = load(yaml).expect("parse");
        let (_, content) = &scenarios[0].given.files[0];
        match content {
            FileContent::Json(v) => {
                assert_eq!(v["enabled"], serde_json::Value::Bool(true));
                assert_eq!(v["count"], serde_json::Value::Number(42.into()));
                let items = v["items"].as_array().unwrap();
                assert_eq!(items[0], serde_json::Value::String("a".to_owned()));
            }
            FileContent::Text(_) => panic!("expected FileContent::Json"),
        }
    }

    // ── stdin: JSON object ────────────────────────────────────────────────────

    #[test]
    fn stdin_json_object_parsed_as_stdin_payload_json() {
        let yaml = r#"
- name: json-stdin
  when:
    argv: [creft, foo]
    stdin:
      key: value
"#;
        let scenarios = load(yaml).expect("parse");
        match &scenarios[0].when.stdin {
            Some(StdinPayload::Json(v)) => {
                assert_eq!(v["key"], serde_json::Value::String("value".to_owned()));
            }
            other => panic!("expected StdinPayload::Json, got {other:?}"),
        }
    }

    // ── discover ──────────────────────────────────────────────────────────────

    fn build_skill_tree(entries: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        for (rel_path, content) in entries {
            let full = dir.path().join(rel_path);
            std::fs::create_dir_all(full.parent().unwrap()).expect("mkdir");
            std::fs::write(&full, content).expect("write");
        }
        dir
    }

    #[test]
    fn discover_returns_test_yaml_files_in_lexicographic_order() {
        let tree = build_skill_tree(&[
            ("foo.test.yaml", ""),
            ("bar/baz.test.yaml", ""),
            ("unrelated.md", ""),
            ("foo.md", ""),
        ]);

        let found = discover(tree.path(), None).expect("discover");
        let names: Vec<_> = found
            .iter()
            .map(|p| {
                p.strip_prefix(tree.path())
                    .unwrap()
                    .to_string_lossy()
                    .to_string()
            })
            .collect();
        assert_eq!(names, vec!["bar/baz.test.yaml", "foo.test.yaml"]);
    }

    #[rstest]
    #[case::top_level("foo", "foo.test.yaml", true)]
    #[case::nested("baz", "bar/baz.test.yaml", true)]
    #[case::no_match("missing", "", false)]
    fn discover_with_skill_filter(
        #[case] filter: &str,
        #[case] expected_suffix: &str,
        #[case] should_find: bool,
    ) {
        let tree = build_skill_tree(&[
            ("foo.test.yaml", ""),
            ("bar/baz.test.yaml", ""),
            ("unrelated.md", ""),
        ]);

        let found = discover(tree.path(), Some(filter)).expect("discover");
        if should_find {
            assert_eq!(found.len(), 1);
            assert!(
                found[0].ends_with(expected_suffix),
                "expected suffix {expected_suffix:?}, got {:?}",
                found[0]
            );
        } else {
            assert!(
                found.is_empty(),
                "expected no matches for filter {filter:?}"
            );
        }
    }

    #[test]
    fn discover_filter_does_not_surface_errors_from_unfiltered_files() {
        // The unrelated.test.yaml would fail to parse (invalid YAML), but since
        // the filter skips it before opening, no error should surface.
        let tree = build_skill_tree(&[
            (
                "foo.test.yaml",
                "- name: ok\n  when:\n    argv: [creft, foo]\n",
            ),
            ("unrelated.test.yaml", "not: valid: yaml: at all:::::"),
        ]);

        // With filter=Some("foo"), the invalid file is never opened.
        let found = discover(tree.path(), Some("foo")).expect("discover must not error");
        assert_eq!(found.len(), 1);
        assert!(found[0].ends_with("foo.test.yaml"));
    }

    #[test]
    fn discover_filter_foo_returns_both_top_level_and_nested_foo() {
        let tree = build_skill_tree(&[
            ("foo.test.yaml", ""),
            ("bar/foo.test.yaml", ""),
            ("bar/baz.test.yaml", ""),
        ]);

        let found = discover(tree.path(), Some("foo")).expect("discover");
        assert_eq!(found.len(), 2);
        let names: Vec<_> = found
            .iter()
            .map(|p| {
                p.strip_prefix(tree.path())
                    .unwrap()
                    .to_string_lossy()
                    .to_string()
            })
            .collect();
        // Lexicographic order: bar/foo.test.yaml < foo.test.yaml
        assert_eq!(names, vec!["bar/foo.test.yaml", "foo.test.yaml"]);
    }

    // ── I/O error on missing path ─────────────────────────────────────────────

    #[test]
    fn load_file_missing_path_returns_io_error() {
        let err = load_file(Path::new("/nonexistent/path/to/fixture.test.yaml")).unwrap_err();
        assert!(
            matches!(err, FixtureError::Io { .. }),
            "expected Io error, got: {err}"
        );
    }

    // ── Minimal scenario (only required fields) ───────────────────────────────

    #[test]
    fn minimal_scenario_with_only_required_fields_parses() {
        let yaml = "- name: minimal\n  when:\n    argv: [creft, foo]\n";
        let scenarios = load(yaml).expect("parse");
        assert_eq!(scenarios.len(), 1);
        let s = &scenarios[0];
        assert_eq!(s.name, "minimal");
        assert!(s.notes.is_none());
        assert!(s.given.files.is_empty());
        assert!(s.before.is_none());
        assert!(s.after.is_none());
        assert_eq!(s.when.argv, vec!["creft", "foo"]);
        assert!(s.when.stdin.is_none());
        assert!(s.when.env.is_empty());
        assert!(s.when.timeout_seconds.is_none());
        assert_eq!(s.then.exit_code, 0);
        assert!(s.then.stdout_contains.is_empty());
        assert!(s.then.coverage.is_none());
    }

    // ── Invalid YAML syntax ───────────────────────────────────────────────────

    #[test]
    fn invalid_yaml_syntax_returns_parse_error() {
        let err = load("key: [unclosed").unwrap_err();
        assert!(
            matches!(err, FixtureError::Parse { .. }),
            "expected Parse error for invalid YAML, got: {err}"
        );
    }

    // ── Top-level value must be a list ────────────────────────────────────────

    #[test]
    fn top_level_mapping_not_list_returns_parse_error() {
        let err = load("name: not-a-list\nwhen:\n  argv: [creft]").unwrap_err();
        assert!(
            matches!(err, FixtureError::Parse { ref message, .. } if message.contains("list")),
            "expected Parse error mentioning 'list', got: {err}"
        );
    }

    // ── Scenario element not a mapping ────────────────────────────────────────

    #[test]
    fn scenario_that_is_not_a_mapping_returns_parse_error() {
        let err = load("- just a string\n").unwrap_err();
        assert!(
            matches!(err, FixtureError::Parse { .. }),
            "expected Parse error for non-mapping scenario, got: {err}"
        );
    }

    // ── Name field type errors ────────────────────────────────────────────────

    #[test]
    fn integer_name_returns_parse_error() {
        let err = load("- name: 42\n  when:\n    argv: [creft, foo]\n").unwrap_err();
        assert!(
            matches!(err, FixtureError::Parse { .. }),
            "expected Parse error for integer name, got: {err}"
        );
    }

    // ── Section type errors ───────────────────────────────────────────────────

    #[rstest]
    #[case::given_not_mapping(
        "- name: test\n  given: string_value\n  when:\n    argv: [creft, foo]\n"
    )]
    #[case::before_not_mapping(
        "- name: test\n  before: string_value\n  when:\n    argv: [creft, foo]\n"
    )]
    #[case::when_not_mapping("- name: test\n  when: not_a_mapping\n")]
    #[case::then_not_mapping(
        "- name: test\n  when:\n    argv: [creft, foo]\n  then: string_value\n"
    )]
    #[case::after_not_mapping(
        "- name: test\n  when:\n    argv: [creft, foo]\n  after: string_value\n"
    )]
    fn section_as_non_mapping_returns_parse_error(#[case] yaml: &str) {
        let err = load(yaml).unwrap_err();
        assert!(
            matches!(err, FixtureError::Parse { .. }),
            "expected Parse error for non-mapping section, got: {err}"
        );
    }

    // ── given.files error paths ───────────────────────────────────────────────

    #[test]
    fn given_files_unknown_key_returns_unknown_key_error() {
        let yaml = "- name: t\n  given:\n    typo: true\n  when:\n    argv: [creft, foo]\n";
        let err = load(yaml).unwrap_err();
        assert!(
            matches!(err, FixtureError::UnknownKey { ref key, .. } if key.contains("given.")),
            "expected UnknownKey with 'given.' prefix, got: {err}"
        );
    }

    #[test]
    fn given_files_not_mapping_returns_parse_error() {
        let yaml = "- name: t\n  given:\n    files: [a, b]\n  when:\n    argv: [creft, foo]\n";
        let err = load(yaml).unwrap_err();
        assert!(
            matches!(err, FixtureError::Parse { .. }),
            "expected Parse error for files as list, got: {err}"
        );
    }

    // ── when.env error paths ──────────────────────────────────────────────────

    #[test]
    fn when_env_not_mapping_returns_parse_error() {
        let yaml = "- name: t\n  when:\n    argv: [creft, foo]\n    env: [a, b]\n";
        let err = load(yaml).unwrap_err();
        assert!(
            matches!(err, FixtureError::Parse { .. }),
            "expected Parse error for env as list, got: {err}"
        );
    }

    #[test]
    fn when_env_value_not_string_returns_parse_error() {
        let yaml = "- name: t\n  when:\n    argv: [creft, foo]\n    env:\n      KEY: 42\n";
        let err = load(yaml).unwrap_err();
        assert!(
            matches!(err, FixtureError::Parse { .. }),
            "expected Parse error for non-string env value, got: {err}"
        );
    }

    #[test]
    fn when_unknown_key_returns_unknown_key_error() {
        let yaml = "- name: t\n  when:\n    argv: [creft, foo]\n    unknown_when_key: true\n";
        let err = load(yaml).unwrap_err();
        assert!(
            matches!(err, FixtureError::UnknownKey { ref key, .. } if key.contains("when.")),
            "expected UnknownKey with 'when.' prefix, got: {err}"
        );
    }

    // ── when.argv element not a string ────────────────────────────────────────

    #[test]
    fn when_argv_element_not_string_returns_parse_error() {
        let yaml = "- name: t\n  when:\n    argv: [creft, 42]\n";
        let err = load(yaml).unwrap_err();
        assert!(
            matches!(err, FixtureError::Parse { .. }),
            "expected Parse error for non-string argv element, got: {err}"
        );
    }

    // ── then.exit_code wrong type ─────────────────────────────────────────────

    #[test]
    fn then_exit_code_as_string_returns_parse_error() {
        let yaml = "- name: t\n  when:\n    argv: [creft, foo]\n  then:\n    exit_code: \"zero\"\n";
        let err = load(yaml).unwrap_err();
        assert!(
            matches!(err, FixtureError::Parse { .. }),
            "expected Parse error for string exit_code, got: {err}"
        );
    }

    // ── then.stdout_contains element not a string ─────────────────────────────

    #[test]
    fn then_stdout_contains_not_list_returns_parse_error() {
        let yaml = "- name: t\n  when:\n    argv: [creft, foo]\n  then:\n    stdout_contains: \"plain string\"\n";
        let err = load(yaml).unwrap_err();
        assert!(
            matches!(err, FixtureError::Parse { .. }),
            "expected Parse error for stdout_contains as string, got: {err}"
        );
    }

    // ── then.files_absent element not a string ────────────────────────────────

    #[test]
    fn then_files_absent_element_not_string_returns_parse_error() {
        let yaml = "- name: t\n  when:\n    argv: [creft, foo]\n  then:\n    files_absent: [42]\n";
        let err = load(yaml).unwrap_err();
        assert!(
            matches!(err, FixtureError::Parse { .. }),
            "expected Parse error for non-string files_absent element, got: {err}"
        );
    }

    // ── then.files_absent not a list ──────────────────────────────────────────

    #[test]
    fn then_files_absent_not_list_returns_parse_error() {
        let yaml =
            "- name: t\n  when:\n    argv: [creft, foo]\n  then:\n    files_absent: \"path\"\n";
        let err = load(yaml).unwrap_err();
        assert!(
            matches!(err, FixtureError::Parse { .. }),
            "expected Parse error for files_absent as string, got: {err}"
        );
    }

    // ── then.files assertion type errors ─────────────────────────────────────

    #[test]
    fn then_files_value_not_mapping_returns_parse_error() {
        let yaml = "- name: t\n  when:\n    argv: [creft, foo]\n  then:\n    files:\n      path.txt: \"flat string\"\n";
        let err = load(yaml).unwrap_err();
        assert!(
            matches!(err, FixtureError::Parse { .. }),
            "expected Parse error for file assertion as plain string, got: {err}"
        );
    }

    #[test]
    fn then_files_no_assertion_key_returns_parse_error() {
        // The assertion-key check runs before the unknown-key check, so a mapping
        // with no recognised assertion key produces a Parse error naming the valid
        // keys, not an UnknownKey error.
        let yaml = "- name: t\n  when:\n    argv: [creft, foo]\n  then:\n    files:\n      path.txt:\n        unexpected_key: value\n";
        let err = load(yaml).unwrap_err();
        assert!(
            matches!(err, FixtureError::Parse { .. }),
            "expected Parse error for missing assertion key, got: {err}"
        );
    }

    #[test]
    fn then_files_multiple_assertion_keys_returns_parse_error() {
        let yaml = "- name: t\n  when:\n    argv: [creft, foo]\n  then:\n    files:\n      path.txt:\n        equals: foo\n        contains: bar\n";
        let err = load(yaml).unwrap_err();
        assert!(
            matches!(err, FixtureError::Parse { .. }),
            "expected Parse error for multiple assertion keys, got: {err}"
        );
    }

    #[test]
    fn then_files_assertion_all_types_parse() {
        let yaml = r#"
- name: file-assertions
  when:
    argv: [creft, foo]
  then:
    files:
      "a.txt":
        equals: "exact content"
      "b.txt":
        regex: "^pattern"
      "c.json":
        json_equals:
          key: value
      "d.json":
        json_subset:
          key: value
"#;
        let scenarios = load(yaml).expect("parse");
        assert_eq!(scenarios[0].then.files.len(), 4);
        assert!(matches!(
            scenarios[0].then.files[0].1,
            FileAssertion::Equals(_)
        ));
        assert!(matches!(
            scenarios[0].then.files[1].1,
            FileAssertion::Regex(_)
        ));
        assert!(matches!(
            scenarios[0].then.files[2].1,
            FileAssertion::JsonEquals(_)
        ));
        assert!(matches!(
            scenarios[0].then.files[3].1,
            FileAssertion::JsonSubset(_)
        ));
    }

    // ── then.coverage error paths ─────────────────────────────────────────────

    #[test]
    fn coverage_not_mapping_returns_parse_error() {
        let yaml = "- name: t\n  when:\n    argv: [creft, foo]\n  then:\n    coverage: string\n";
        let err = load(yaml).unwrap_err();
        assert!(
            matches!(err, FixtureError::Parse { .. }),
            "expected Parse error for coverage as string, got: {err}"
        );
    }

    #[test]
    fn coverage_blocks_not_list_returns_parse_error() {
        let yaml =
            "- name: t\n  when:\n    argv: [creft, foo]\n  then:\n    coverage:\n      blocks: 0\n";
        let err = load(yaml).unwrap_err();
        assert!(
            matches!(err, FixtureError::Parse { .. }),
            "expected Parse error for blocks as integer, got: {err}"
        );
    }

    #[test]
    fn coverage_blocks_negative_element_returns_parse_error() {
        let yaml = "- name: t\n  when:\n    argv: [creft, foo]\n  then:\n    coverage:\n      blocks: [-1]\n";
        let err = load(yaml).unwrap_err();
        assert!(
            matches!(err, FixtureError::Parse { .. }),
            "expected Parse error for negative block index, got: {err}"
        );
    }

    #[test]
    fn coverage_primitives_not_mapping_returns_parse_error() {
        let yaml = "- name: t\n  when:\n    argv: [creft, foo]\n  then:\n    coverage:\n      primitives: [a, b]\n";
        let err = load(yaml).unwrap_err();
        assert!(
            matches!(err, FixtureError::Parse { .. }),
            "expected Parse error for primitives as list, got: {err}"
        );
    }

    #[test]
    fn coverage_unknown_key_returns_unknown_key_error() {
        let yaml = "- name: t\n  when:\n    argv: [creft, foo]\n  then:\n    coverage:\n      typo: true\n";
        let err = load(yaml).unwrap_err();
        assert!(
            matches!(err, FixtureError::UnknownKey { ref key, .. } if key.contains("coverage")),
            "expected UnknownKey with 'coverage' prefix, got: {err}"
        );
    }

    // ── before/after shell hook ───────────────────────────────────────────────

    #[test]
    fn before_shell_hook_parses() {
        let yaml =
            "- name: t\n  before:\n    shell: \"setup cmd\"\n  when:\n    argv: [creft, foo]\n";
        let scenarios = load(yaml).expect("parse");
        assert_eq!(scenarios[0].before.as_ref().unwrap().shell, "setup cmd");
    }

    #[test]
    fn after_shell_hook_parses() {
        let yaml =
            "- name: t\n  after:\n    shell: \"teardown cmd\"\n  when:\n    argv: [creft, foo]\n";
        let scenarios = load(yaml).expect("parse");
        assert_eq!(scenarios[0].after.as_ref().unwrap().shell, "teardown cmd");
    }

    #[test]
    fn shell_hook_unknown_key_returns_unknown_key_error() {
        let yaml = "- name: t\n  after:\n    shell: cmd\n    typo: true\n  when:\n    argv: [creft, foo]\n";
        let err = load(yaml).unwrap_err();
        assert!(
            matches!(err, FixtureError::UnknownKey { ref key, .. } if key.contains("after.")),
            "expected UnknownKey with 'after.' prefix, got: {err}"
        );
    }

    // ── yaml_to_json_value: float values ─────────────────────────────────────

    #[test]
    fn given_files_json_value_with_float_parses() {
        let yaml = "- name: t\n  given:\n    files:\n      config.json:\n        score: 2.5\n  when:\n    argv: [creft, foo]\n";
        let scenarios = load(yaml).expect("parse");
        let (_, content) = &scenarios[0].given.files[0];
        match content {
            FileContent::Json(v) => {
                let score = v["score"].as_f64().expect("score is f64");
                assert!((score - 2.5).abs() < 1e-6);
            }
            FileContent::Text(_) => panic!("expected Json content"),
        }
    }

    // ── stdin: bare scalar values rejected ───────────────────────────────────

    #[rstest]
    #[case::integer("42")]
    #[case::boolean("true")]
    #[case::float("1.5")]
    fn stdin_bare_scalar_returns_parse_error(#[case] val: &str) {
        let yaml = format!("- name: t\n  when:\n    argv: [creft, foo]\n    stdin: {val}\n");
        let err = load(&yaml).unwrap_err();
        assert!(
            matches!(err, FixtureError::Parse { .. }),
            "expected Parse error for stdin bare scalar {val}, got: {err}"
        );
    }

    // ── stdin: list value ─────────────────────────────────────────────────────

    #[test]
    fn stdin_list_parsed_as_stdin_payload_json() {
        let yaml = "- name: t\n  when:\n    argv: [creft, foo]\n    stdin:\n      - a\n      - b\n";
        let scenarios = load(yaml).expect("parse");
        match &scenarios[0].when.stdin {
            Some(StdinPayload::Json(v)) => {
                assert!(v.is_array());
                assert_eq!(v.as_array().unwrap().len(), 2);
            }
            other => panic!("expected StdinPayload::Json array, got {other:?}"),
        }
    }

    // ── stdout_json parses as serde_json value ────────────────────────────────

    #[test]
    fn then_stdout_json_parsed() {
        let yaml = "- name: t\n  when:\n    argv: [creft, foo]\n  then:\n    stdout_json:\n      result: ok\n";
        let scenarios = load(yaml).expect("parse");
        let v = scenarios[0].then.stdout_json.as_ref().unwrap();
        assert_eq!(v["result"], serde_json::Value::String("ok".to_owned()));
    }

    // ── then.files key not a string ───────────────────────────────────────────

    #[test]
    fn then_files_not_mapping_returns_parse_error() {
        let yaml = "- name: t\n  when:\n    argv: [creft, foo]\n  then:\n    files: [a, b]\n";
        let err = load(yaml).unwrap_err();
        assert!(
            matches!(err, FixtureError::Parse { .. }),
            "expected Parse error for files as list, got: {err}"
        );
    }
}
