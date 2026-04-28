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

use yaml_rust2::{Yaml, YamlEmitter, YamlLoader};

use crate::skill_test::match_pattern::Matcher;

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
#[derive(Debug, Clone, PartialEq)]
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
#[derive(Debug, Clone, PartialEq)]
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
#[derive(Debug, Clone, PartialEq)]
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
    parse_scenarios_str(&content, path)
}

/// Parse zero-or-more scenarios from a YAML string into the typed view.
///
/// `path_label` is used only in error messages — it is not opened. Pass the
/// real fixture path when validating an existing file's content; pass a
/// synthetic label like `Path::new("<stdin>")` when validating user input.
///
/// Returns `Ok(Vec::new())` for empty input or a YAML null document, matching
/// the behavior of [`load_file`]. Returns `Err` for syntax errors, schema
/// violations, or a non-list top-level value.
pub(crate) fn parse_scenarios_str(
    content: &str,
    path_label: &Path,
) -> Result<Vec<Scenario>, FixtureError> {
    let list = yaml_top_level_list(content, path_label)?;
    let mut scenarios = Vec::with_capacity(list.len());
    for (index, item) in list.iter().enumerate() {
        let scenario = parse_scenario(path_label, index, item)?;
        scenarios.push(scenario);
    }
    Ok(scenarios)
}

/// Parse zero-or-more scenarios from a YAML string into the raw `Yaml::Hash`
/// mapping nodes. Used for byte-fidelity rendering when appending or replacing
/// scenarios in an existing fixture file.
///
/// Returns `Ok(Vec::new())` for empty input or a YAML null document. Returns
/// `Err` for syntax errors or a non-list top-level value. Schema validation is
/// NOT performed here — callers that need schema validation pair this with
/// [`parse_scenarios_str`].
#[allow(dead_code)] // called by cmd_add_test in cmd/skill.rs
pub(crate) fn parse_scenarios_yaml(
    content: &str,
    path_label: &Path,
) -> Result<Vec<Yaml>, FixtureError> {
    Ok(yaml_top_level_list(content, path_label)?.to_vec())
}

/// Find the index of a scenario whose `name` field equals `name`.
///
/// Returns `None` when no scenario matches. Used by the add-test path to
/// detect collisions before writing.
#[allow(dead_code)] // called by cmd_add_test in cmd/skill.rs
pub(crate) fn find_scenario_by_name(scenarios: &[Scenario], name: &str) -> Option<usize> {
    scenarios.iter().position(|s| s.name == name)
}

/// Append a scenario's serialized YAML to an existing fixture file's content.
///
/// The new scenario is rendered as a single list entry (`- name: ...\n  ...`)
/// using the YAML emitter's block style. Existing bytes are preserved verbatim —
/// comments, blank lines, and hand-formatted YAML are not touched. A trailing
/// newline is added to the existing content if absent.
///
/// `existing_content` is `""` when the fixture file does not yet exist.
/// `new_scenario_yaml` is the full YAML for one mapping produced by
/// [`render_scenario_yaml`], without the leading `- ` list marker (this
/// function adds the marker and indentation).
///
/// Returns the bytes to write to the fixture file.
#[allow(dead_code)] // called by cmd_add_test in cmd/skill.rs
pub(crate) fn append_scenario(existing_content: &str, new_scenario_yaml: &str) -> String {
    let mut out = String::new();

    if existing_content.is_empty() {
        // New fixture: emit a list with one entry.
        format_list_entry(&mut out, new_scenario_yaml);
    } else {
        out.push_str(existing_content);
        // Ensure the existing content ends with a newline before appending.
        if !out.ends_with('\n') {
            out.push('\n');
        }
        format_list_entry(&mut out, new_scenario_yaml);
    }

    out
}

/// Replace the scenario at `index` in `existing_content` with `new_scenario`.
///
/// This round-trips the file through `yaml-rust2`'s emitter, which does NOT
/// preserve comments. The `--force` path is the only caller; users are warned
/// in the success message that comments may be lost.
///
/// `index` is the zero-based position in the YAML list — typically obtained
/// from [`find_scenario_by_name`]. `new_scenario` is the raw `Yaml::Hash` for
/// the replacement entry (callers obtain it by parsing the candidate via
/// [`parse_scenarios_yaml`] and taking the first element). Out-of-bounds
/// indices return a `Parse` error (defensive — the CLI never supplies an
/// out-of-bounds index, but the function is robust to misuse).
#[allow(dead_code)] // called by cmd_add_test in cmd/skill.rs
pub(crate) fn replace_scenario(
    existing_content: &str,
    index: usize,
    new_scenario: &Yaml,
    path_label: &Path,
) -> Result<String, FixtureError> {
    let mut nodes = yaml_top_level_list(existing_content, path_label)?.to_vec();

    if index >= nodes.len() {
        return Err(FixtureError::Parse {
            path: path_label.to_owned(),
            message: format!(
                "replace_scenario: index {index} is out of bounds (file has {} scenarios)",
                nodes.len()
            ),
        });
    }

    nodes[index] = new_scenario.clone();

    let list_doc = Yaml::Array(nodes);
    let mut out = String::new();
    let mut emitter = YamlEmitter::new(&mut out);
    emitter.dump(&list_doc).map_err(|e| FixtureError::Parse {
        path: path_label.to_owned(),
        message: format!("YAML emission failed: {e}"),
    })?;

    // YamlEmitter writes "---\n" as a document marker; strip it so the file
    // starts with the list directly.
    let content = out.strip_prefix("---\n").unwrap_or(&out).to_string();

    Ok(content)
}

/// Remove the scenario at `index` from `existing_content` and return the new
/// file bytes.
///
/// Comments, blank lines, and hand-formatted bytes outside the target entry's
/// byte range are preserved verbatim. The entry's range is defined as the
/// half-open byte interval `[start, end)` where:
///
/// - `start` is the byte offset of the line containing the entry's leading
///   `-` indicator.
/// - `end` is the byte offset of the line containing the next entry's `-`
///   indicator, or the file length if `index` is the last entry.
///
/// Inter-entry comments and blank lines attach to the entry above them. Any
/// comment or blank line whose byte offset is `>= start` and `< end` is part
/// of the target entry's range and is removed with it — including a comment
/// line that visually appears immediately above the next entry's `-` line.
/// Comments and blank lines before the first entry's `-` line are file-level
/// (offset `< start_0`) and survive every removal.
///
/// `index` is the zero-based position obtained from
/// [`find_scenario_by_name`]. Out-of-bounds indices return a
/// [`FixtureError::Parse`] error (defensive; the CLI does not produce one).
///
/// The implementation locates entry boundaries by direct byte scan over
/// `existing_content.as_bytes()`. `yaml-rust2` is used upstream by the caller
/// for schema validation (via [`parse_scenarios_str`]) and is used here only
/// to reject flow-style top-level sequences before the byte scan begins. The
/// byte scan itself never consults `yaml-rust2`'s `Marker` indices, sidestepping
/// the inconsistency where the scanner's index field advances per-char in most
/// paths but per-byte inside literal/folded block scalars.
///
/// The fidelity contract matches [`append_scenario`]; [`replace_scenario`]
/// still round-trips through the YAML emitter and is tracked separately for
/// evolution to the same byte-fidelity contract these two primitives share.
#[allow(dead_code)] // called by cmd_remove_test in cmd/skill.rs
pub(crate) fn remove_scenario_at(
    existing_content: &str,
    index: usize,
    path_label: &Path,
) -> Result<String, FixtureError> {
    let bytes = existing_content.as_bytes();

    // Step 0 — pre-scan rejections.
    //
    // Walk lines and classify "significant" lines: lines whose first
    // non-whitespace byte is neither `b'#'` (comment) nor a line-end byte
    // (blank). Both rejected constructs are checked in a single pass.
    let mut first_significant_seen = false;
    let mut line_start = 0usize;

    loop {
        if line_start >= bytes.len() {
            break;
        }
        // Find the end of this line.
        let line_end = bytes[line_start..]
            .iter()
            .position(|&b| b == b'\n')
            .map(|p| line_start + p + 1)
            .unwrap_or(bytes.len());

        let line = &bytes[line_start..line_end];

        // Skip leading whitespace to find the first significant byte.
        let first_nonws = line.iter().position(|&b| !b.is_ascii_whitespace());

        if let Some(pos) = first_nonws {
            let first_byte = line[pos];
            // Skip comment lines and blank lines (no non-whitespace byte is blank).
            if first_byte != b'#' {
                // This is a significant line.
                if !first_significant_seen {
                    // Check for flow-style: first significant byte is `[`.
                    if first_byte == b'[' {
                        return Err(FixtureError::Parse {
                            path: path_label.to_owned(),
                            message:
                                "remove_scenario_at: flow-style top-level sequences are not supported"
                                    .to_owned(),
                        });
                    }
                    first_significant_seen = true;
                }

                // Check for YAML document-start marker `---`. The YAML 1.2
                // grammar places the document-start marker at column 0 (zero
                // leading whitespace). Lines with `---` content inside block
                // scalars always appear at deeper indent than the parent key,
                // so they have `pos > 0` and are not document-start markers.
                // Only check lines that begin at column 0 (`pos == 0`).
                if pos == 0 {
                    let trimmed = line[pos..]
                        .iter()
                        .rposition(|&b| !b.is_ascii_whitespace())
                        .map(|end| &line[pos..=pos + end])
                        .unwrap_or(&line[pos..]);
                    if trimmed == b"---" {
                        return Err(FixtureError::Parse {
                            path: path_label.to_owned(),
                            message: "remove_scenario_at: YAML document-start marker '---' is not supported in test fixtures".to_owned(),
                        });
                    }
                }
            }
        }

        line_start = line_end;
    }

    // Step 1 — collect top-level entry line-start byte offsets.
    //
    // A "top-level entry line" satisfies all of:
    //   (1) First non-whitespace byte is `b'-'`.
    //   (2) The byte immediately following that `-` is ASCII space, `\n`,
    //       `\r`, or the line is the last byte of the file.
    //   (3) Leading-whitespace count equals the base-indent column, which is
    //       fixed by the first line satisfying (1) and (2).
    let entries = collect_top_level_entry_offsets(existing_content);

    // Step 2 — bounds check.
    if index >= entries.len() {
        return Err(FixtureError::Parse {
            path: path_label.to_owned(),
            message: "remove_scenario_at: index out of bounds".to_owned(),
        });
    }

    // Step 3 — compute byte range.
    let start = entries[index];
    let end = entries
        .get(index + 1)
        .copied()
        .unwrap_or(existing_content.len());

    // Step 4 — slice.
    //
    // Both `start` and `end` are line-start byte offsets: either 0 or the
    // byte immediately after a `\n`. `\n` is a single-byte UTF-8 codepoint,
    // so every line-start is a valid UTF-8 boundary.
    let result = format!("{}{}", &existing_content[..start], &existing_content[end..]);

    Ok(result)
}

/// Collect the byte offsets of every top-level block-sequence entry indicator
/// in `content`.
///
/// Returns a `Vec<usize>` of line-start byte offsets, one per top-level entry,
/// in document order. This helper is exposed as a named function so tests can
/// assert its output directly without going through [`remove_scenario_at`]'s
/// full path (including Step 0 rejections).
///
/// Entry indicator predicate (all three clauses must hold):
///
/// 1. First non-whitespace byte is `b'-'`.
/// 2. The byte immediately following that `-` is: ASCII space (`b' '`),
///    line-feed (`b'\n'`), carriage-return (`b'\r'`), or there is no
///    following byte on this line (the `-` is the last byte of the file or
///    immediately before the end of the line). A `-` followed by any other
///    byte is a plain scalar, not an entry indicator.
/// 3. Leading-whitespace count equals the base-indent column, fixed by the
///    first line satisfying clauses (1) and (2).
pub(crate) fn collect_top_level_entry_offsets(content: &str) -> Vec<usize> {
    let bytes = content.as_bytes();
    let mut entries: Vec<usize> = Vec::new();
    let mut base_indent: Option<usize> = None;
    let mut line_start = 0usize;

    loop {
        if line_start >= bytes.len() {
            break;
        }

        // Find end of line.
        let line_end = bytes[line_start..]
            .iter()
            .position(|&b| b == b'\n')
            .map(|p| line_start + p + 1)
            .unwrap_or(bytes.len());

        let line = &bytes[line_start..line_end];

        // Count leading whitespace.
        let indent = line
            .iter()
            .take_while(|&&b| b == b' ' || b == b'\t')
            .count();

        if indent < line.len() {
            let first_byte = line[indent];

            if first_byte == b'-' {
                // Clause (2): what follows the `-`?
                let after_dash = line.get(indent + 1).copied();
                let is_entry = match after_dash {
                    None => true,        // `-` is the last byte of the file
                    Some(b' ') => true,  // `- value` or `- ` followed by content
                    Some(b'\n') => true, // `-\n`
                    Some(b'\r') => true, // `-\r\n`
                    _ => false,          // `-foo` is a plain scalar, not an indicator
                };

                if is_entry {
                    // Clause (3): match or establish base indent.
                    match base_indent {
                        None => {
                            base_indent = Some(indent);
                            entries.push(line_start);
                        }
                        Some(base) if indent == base => {
                            entries.push(line_start);
                        }
                        _ => {} // nested entry at deeper indent — skip
                    }
                }
            }
            // Lines whose first non-whitespace byte is anything else are
            // not entry indicators (comments, continuation lines, etc.).
        }

        line_start = line_end;
    }

    entries
}

/// Render a single scenario mapping node as a YAML string in block style.
///
/// The result is suitable for passing to [`append_scenario`]. It does NOT
/// include the leading `- ` list marker — `append_scenario` adds the framing.
/// The document marker (`---\n`) emitted by `YamlEmitter` is stripped.
#[allow(dead_code)] // called by cmd_add_test in cmd/skill.rs
pub(crate) fn render_scenario_yaml(node: &Yaml) -> String {
    let mut out = String::new();
    let mut emitter = YamlEmitter::new(&mut out);
    // Emit the node directly; on error produce an empty string (defensive — a
    // well-formed Yaml::Hash always emits cleanly).
    let _ = emitter.dump(node);
    // Strip the leading "---\n" document marker.
    out.strip_prefix("---\n").unwrap_or(&out).to_string()
}

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Parse `content` and return a reference to its top-level YAML array.
///
/// Returns an owned `Vec<Yaml>` because the parsed document is not `'static`
/// but callers need to work with the elements beyond the lifetime of the
/// temporary document vec.
fn yaml_top_level_list(content: &str, path_label: &Path) -> Result<Vec<Yaml>, FixtureError> {
    let docs = YamlLoader::load_from_str(content).map_err(|e| FixtureError::Parse {
        path: path_label.to_owned(),
        message: e.to_string(),
    })?;

    let doc = match docs.into_iter().next() {
        Some(d) => d,
        None => return Ok(Vec::new()),
    };

    match doc {
        Yaml::Array(arr) => Ok(arr),
        Yaml::Null => Ok(Vec::new()),
        _ => Err(FixtureError::Parse {
            path: path_label.to_owned(),
            message: "top-level value must be a YAML list of scenarios".to_owned(),
        }),
    }
}

/// Format a YAML mapping string as a list entry, writing into `out`.
///
/// The first line gets `- ` prefix; subsequent non-empty lines get `  `
/// (two-space) indentation to match the block-list convention used throughout
/// the project's fixture files.
#[allow(dead_code)] // called by append_scenario
fn format_list_entry(out: &mut String, scenario_yaml: &str) {
    let mut first = true;
    for line in scenario_yaml.lines() {
        if first {
            out.push_str("- ");
            out.push_str(line);
            out.push('\n');
            first = false;
        } else if line.is_empty() {
            // Preserve blank lines without spurious leading spaces.
            out.push('\n');
        } else {
            out.push_str("  ");
            out.push_str(line);
            out.push('\n');
        }
    }
    // If the input ended without a trailing newline, ensure the entry ends
    // with one so subsequent appends don't run together.
    if !scenario_yaml.ends_with('\n') && !first {
        out.push('\n');
    }
}

/// Walk a skill-tree root and return every `*.test.yaml` path in lexicographic order.
///
/// `root` must be a skill-tree directory — the `.creft/commands/` directory of the
/// local root, or a sub-tree of it. Do not point this at the project root: it would
/// walk `target/`, `.git/`, `workbench/`, and vendored crates, none of which contain
/// fixtures by convention.
///
/// When `skill_filter` is `Some(matcher)`, only paths whose basename stem (the
/// filename with `.test.yaml` stripped) matches the pattern are returned. The
/// filter is applied during the walk, before any file is opened, so a parse
/// error in an unrelated fixture cannot fail a focused-skill run.
pub(crate) fn discover(
    root: &Path,
    skill_filter: Option<&Matcher>,
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
    skill_filter: Option<&Matcher>,
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
///
/// When `skill_filter` is `None`, every `*.test.yaml` file matches. When a
/// matcher is supplied, the basename stem (the filename with `.test.yaml`
/// stripped) is tested against it. This allows both exact names (`setup`)
/// and glob patterns (`merge*`) to work uniformly at the discovery boundary.
fn is_fixture_match(path: &Path, skill_filter: Option<&Matcher>) -> bool {
    let name = match path.file_name().and_then(|n| n.to_str()) {
        Some(n) => n,
        None => return false,
    };

    let stem = match name.strip_suffix(".test.yaml") {
        Some(s) => s,
        None => return false,
    };

    match skill_filter {
        None => true,
        Some(matcher) => matcher.matches(stem),
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
    use crate::skill_test::match_pattern::{self, MatchKind};
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

    /// Compile a SKILL pattern into a `Matcher`, panicking on any error (test helper).
    fn mk(pattern: &str) -> Matcher {
        match_pattern::compile(pattern, MatchKind::Exact).expect("test pattern must compile")
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

        let matcher = mk(filter);
        let found = discover(tree.path(), Some(&matcher)).expect("discover");
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

        // With a pattern matching only "foo", the invalid file is never opened.
        let matcher = mk("foo");
        let found = discover(tree.path(), Some(&matcher)).expect("discover must not error");
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

        let matcher = mk("foo");
        let found = discover(tree.path(), Some(&matcher)).expect("discover");
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

    // ── parse_scenarios_str ───────────────────────────────────────────────────

    #[test]
    fn parse_scenarios_str_loads_zero_scenarios_from_empty_string() {
        let result = parse_scenarios_str("", Path::new("<inline>")).expect("parse");
        assert!(result.is_empty());
    }

    #[test]
    fn parse_scenarios_str_loads_one_scenario_from_single_entry_list() {
        let yaml = "- name: hello\n  when:\n    argv: [creft, foo]\n";
        let scenarios =
            parse_scenarios_str(yaml, Path::new("<inline>")).expect("parse single entry");
        assert_eq!(scenarios.len(), 1);
        assert_eq!(scenarios[0].name, "hello");
        assert_eq!(scenarios[0].when.argv, vec!["creft", "foo"]);
    }

    #[test]
    fn parse_scenarios_str_rejects_non_list_top_level() {
        let yaml = "name: hello\nwhen:\n  argv: [creft, foo]\n";
        let err = parse_scenarios_str(yaml, Path::new("<inline>")).unwrap_err();
        assert!(
            matches!(err, FixtureError::Parse { .. }),
            "expected Parse error for non-list top level, got: {err}"
        );
    }

    #[test]
    fn parse_scenarios_str_rejects_missing_when_argv() {
        let yaml = "- name: no-argv\n  when:\n    stdin: hello\n";
        let err = parse_scenarios_str(yaml, Path::new("<inline>")).unwrap_err();
        assert!(
            matches!(err, FixtureError::MissingField { field, .. } if field == "when.argv"),
            "expected MissingField(when.argv), got: {err}"
        );
    }

    // ── parse_scenarios_yaml ──────────────────────────────────────────────────

    #[test]
    fn parse_scenarios_yaml_returns_raw_mapping_nodes() {
        let yaml = "- name: hello\n  when:\n    argv: [creft, foo]\n";
        let nodes = parse_scenarios_yaml(yaml, Path::new("<inline>")).expect("parse yaml");
        assert_eq!(nodes.len(), 1);
        assert!(nodes[0].as_hash().is_some(), "expected a Yaml::Hash node");
    }

    #[test]
    fn parse_scenarios_yaml_skips_schema_validation() {
        // Missing when.argv would fail parse_scenarios_str but parse_scenarios_yaml
        // returns the raw nodes without schema validation.
        let yaml = "- name: no-argv\n  when:\n    stdin: hello\n";
        let nodes = parse_scenarios_yaml(yaml, Path::new("<inline>"))
            .expect("raw parse must succeed even without when.argv");
        assert_eq!(nodes.len(), 1);
    }

    #[test]
    fn parse_scenarios_yaml_rejects_non_list_top_level() {
        let yaml = "name: hello\nwhen:\n  argv: [creft, foo]\n";
        let err = parse_scenarios_yaml(yaml, Path::new("<inline>")).unwrap_err();
        assert!(
            matches!(err, FixtureError::Parse { .. }),
            "expected Parse error for non-list top level, got: {err}"
        );
    }

    // ── find_scenario_by_name ─────────────────────────────────────────────────

    fn make_scenario(name: &str) -> Scenario {
        Scenario {
            name: name.to_owned(),
            source_file: PathBuf::from("<test>"),
            source_index: 0,
            notes: None,
            given: Given::default(),
            before: None,
            when: When {
                argv: vec!["creft".to_owned(), "foo".to_owned()],
                stdin: None,
                env: Vec::new(),
                timeout_seconds: None,
            },
            then: Then::default(),
            after: None,
        }
    }

    #[test]
    fn find_scenario_by_name_returns_some_for_match() {
        let scenarios = vec![
            make_scenario("alpha"),
            make_scenario("beta"),
            make_scenario("gamma"),
        ];
        assert_eq!(find_scenario_by_name(&scenarios, "beta"), Some(1));
    }

    #[test]
    fn find_scenario_by_name_returns_none_for_no_match() {
        let scenarios = vec![make_scenario("alpha"), make_scenario("gamma")];
        assert_eq!(find_scenario_by_name(&scenarios, "beta"), None);
    }

    // ── append_scenario ───────────────────────────────────────────────────────

    #[test]
    fn append_scenario_to_empty_string_creates_single_entry_list() {
        let new_yaml = "name: hello\nwhen:\n  argv:\n  - creft\n  - foo\n";
        let result = append_scenario("", new_yaml);
        assert!(
            result.starts_with("- name: hello"),
            "result should start with list entry: {result:?}"
        );
        // Must parse as a valid single-scenario fixture.
        let scenarios =
            parse_scenarios_str(&result, Path::new("<inline>")).expect("round-trip parse");
        assert_eq!(scenarios.len(), 1);
        assert_eq!(scenarios[0].name, "hello");
    }

    #[test]
    fn append_scenario_to_existing_file_preserves_existing_bytes() {
        let existing = "- name: first\n  when:\n    argv: [creft, foo]\n";
        let new_yaml = "name: second\nwhen:\n  argv:\n  - creft\n  - bar\n";
        let result = append_scenario(existing, new_yaml);
        // Original bytes are unchanged at the start.
        assert!(
            result.starts_with(existing),
            "existing content must be preserved verbatim"
        );
        // Both scenarios parse cleanly.
        let scenarios =
            parse_scenarios_str(&result, Path::new("<inline>")).expect("round-trip parse");
        assert_eq!(scenarios.len(), 2);
        assert_eq!(scenarios[0].name, "first");
        assert_eq!(scenarios[1].name, "second");
    }

    #[test]
    fn append_scenario_handles_missing_trailing_newline() {
        // Existing content with no trailing newline.
        let existing = "- name: first\n  when:\n    argv: [creft, foo]";
        let new_yaml = "name: second\nwhen:\n  argv:\n  - creft\n  - bar\n";
        let result = append_scenario(existing, new_yaml);
        let scenarios =
            parse_scenarios_str(&result, Path::new("<inline>")).expect("round-trip parse");
        assert_eq!(scenarios.len(), 2);
    }

    #[test]
    fn append_scenario_round_trips_through_parse_scenarios_str() {
        let existing =
            "- name: original\n  when:\n    argv: [creft, setup]\n  then:\n    exit_code: 0\n";
        let rendered = {
            let nodes = parse_scenarios_yaml(
                "- name: appended\n  when:\n    argv: [creft, list]\n",
                Path::new("<inline>"),
            )
            .expect("parse new");
            render_scenario_yaml(&nodes[0])
        };
        let result = append_scenario(existing, &rendered);
        let scenarios =
            parse_scenarios_str(&result, Path::new("<inline>")).expect("round-trip parse");
        assert_eq!(scenarios.len(), 2);
        assert_eq!(scenarios[0].name, "original");
        assert_eq!(scenarios[1].name, "appended");
    }

    #[test]
    fn append_scenario_preserves_trailing_comments() {
        let existing = "- name: first\n  when:\n    argv: [creft, foo]\n# trailing comment\n";
        let new_yaml = "name: second\nwhen:\n  argv:\n  - creft\n  - bar\n";
        let result = append_scenario(existing, new_yaml);
        assert!(
            result.contains("# trailing comment"),
            "trailing comment must be preserved verbatim"
        );
        // The comment is before the new entry but that's fine — we preserve existing bytes.
        let scenarios =
            parse_scenarios_str(&result, Path::new("<inline>")).expect("round-trip parse");
        assert_eq!(scenarios.len(), 2);
    }

    #[test]
    fn append_scenario_preserves_leading_comments() {
        let existing = "# leading comment\n- name: first\n  when:\n    argv: [creft, foo]\n";
        let new_yaml = "name: second\nwhen:\n  argv:\n  - creft\n  - bar\n";
        let result = append_scenario(existing, new_yaml);
        assert!(
            result.starts_with("# leading comment"),
            "leading comment must be preserved verbatim"
        );
        let scenarios =
            parse_scenarios_str(&result, Path::new("<inline>")).expect("round-trip parse");
        assert_eq!(scenarios.len(), 2);
    }

    // ── replace_scenario ──────────────────────────────────────────────────────

    #[test]
    fn replace_scenario_swaps_only_the_named_entry() {
        let existing = concat!(
            "- name: alpha\n  when:\n    argv: [creft, a]\n",
            "- name: beta\n  when:\n    argv: [creft, b]\n",
            "- name: gamma\n  when:\n    argv: [creft, c]\n",
        );
        let replacement_yaml = "- name: beta-replaced\n  when:\n    argv: [creft, b2]\n";
        let new_node = &parse_scenarios_yaml(replacement_yaml, Path::new("<inline>"))
            .expect("parse replacement")[0];
        let result =
            replace_scenario(existing, 1, new_node, Path::new("<inline>")).expect("replace");
        let scenarios =
            parse_scenarios_str(&result, Path::new("<inline>")).expect("round-trip parse");
        assert_eq!(scenarios.len(), 3);
        assert_eq!(scenarios[0].name, "alpha");
        assert_eq!(scenarios[1].name, "beta-replaced");
        assert_eq!(scenarios[2].name, "gamma");
    }

    #[test]
    fn replace_scenario_out_of_bounds_returns_parse_error() {
        let existing = "- name: only\n  when:\n    argv: [creft, foo]\n";
        let node = Yaml::Hash(Default::default());
        let err = replace_scenario(existing, 5, &node, Path::new("<inline>")).unwrap_err();
        assert!(
            matches!(err, FixtureError::Parse { .. }),
            "expected Parse error for out-of-bounds index, got: {err}"
        );
    }

    // ── is_fixture_match with Matcher ─────────────────────────────────────────

    #[test]
    fn is_fixture_match_with_no_filter_accepts_any_test_yaml() {
        assert!(
            is_fixture_match(Path::new("setup.test.yaml"), None),
            "no filter: .test.yaml accepted"
        );
        assert!(
            !is_fixture_match(Path::new("setup.md"), None),
            "no filter: .md rejected"
        );
        assert!(
            !is_fixture_match(Path::new("setup.yaml"), None),
            "no filter: plain .yaml rejected (must end in .test.yaml)"
        );
    }

    #[test]
    fn is_fixture_match_with_exact_plain_text_matches_basename() {
        // Exact plain-text "setup" matches the stem "setup" exactly.
        // Negative cases ensure non-matching basenames and non-test-yaml extensions are rejected.
        let matcher = mk("setup");
        assert!(
            is_fixture_match(Path::new("setup.test.yaml"), Some(&matcher)),
            "exact basename matches"
        );
        assert!(
            !is_fixture_match(Path::new("other.test.yaml"), Some(&matcher)),
            "non-matching basename rejected"
        );
        assert!(
            !is_fixture_match(Path::new("setup.md"), Some(&matcher)),
            ".md extension rejected even with matching stem"
        );
    }

    #[test]
    fn is_fixture_match_with_glob_matches_prefix() {
        let matcher = mk("merge*");
        assert!(
            is_fixture_match(Path::new("merge-clean.test.yaml"), Some(&matcher)),
            "merge-clean matches merge*"
        );
        assert!(
            is_fixture_match(Path::new("merge-conflict.test.yaml"), Some(&matcher)),
            "merge-conflict matches merge*"
        );
        assert!(
            !is_fixture_match(Path::new("setup.test.yaml"), Some(&matcher)),
            "setup does not match merge*"
        );
        assert!(
            !is_fixture_match(Path::new("pre-merge.test.yaml"), Some(&matcher)),
            "pre-merge does not match anchored merge*"
        );
    }

    #[test]
    fn is_fixture_match_with_exact_plain_text_matches_whole_basename() {
        let matcher = mk("clean");
        assert!(
            is_fixture_match(Path::new("clean.test.yaml"), Some(&matcher)),
            "clean matches exact basename 'clean'"
        );
        assert!(
            !is_fixture_match(Path::new("merge-clean.test.yaml"), Some(&matcher)),
            "merge-clean must not match exact pattern 'clean'"
        );
        assert!(
            !is_fixture_match(Path::new("setup.test.yaml"), Some(&matcher)),
            "setup does not match 'clean'"
        );
    }

    #[test]
    fn is_fixture_match_rejects_non_test_yaml_extension() {
        let matcher = mk("setup");
        assert!(
            !is_fixture_match(Path::new("setup.md"), Some(&matcher)),
            ".md rejected"
        );
        assert!(
            !is_fixture_match(Path::new("setup.txt"), Some(&matcher)),
            ".txt rejected"
        );
        assert!(
            !is_fixture_match(Path::new("setup.yaml"), Some(&matcher)),
            "plain .yaml rejected (must end in .test.yaml)"
        );
    }

    // ── remove_scenario_at ────────────────────────────────────────────────────

    fn label() -> &'static Path {
        Path::new("<inline>")
    }

    /// Two-entry fixture used across multiple removal tests.
    const TWO_ENTRIES: &str = "\
- name: alpha
  when:
    argv: [creft, a]
  then:
    exit_code: 0
- name: beta
  when:
    argv: [creft, b]
  then:
    exit_code: 0
";

    #[test]
    fn remove_scenario_at_removes_first_of_two_entries() {
        use pretty_assertions::assert_str_eq;

        let result =
            remove_scenario_at(TWO_ENTRIES, 0, label()).expect("remove_scenario_at must succeed");

        let expected = "\
- name: beta
  when:
    argv: [creft, b]
  then:
    exit_code: 0
";
        assert_str_eq!(expected, result);
    }

    #[test]
    fn remove_scenario_at_removes_last_of_two_entries() {
        use pretty_assertions::assert_str_eq;

        let result =
            remove_scenario_at(TWO_ENTRIES, 1, label()).expect("remove_scenario_at must succeed");

        let expected = "\
- name: alpha
  when:
    argv: [creft, a]
  then:
    exit_code: 0
";
        assert_str_eq!(expected, result);
    }

    #[test]
    fn remove_scenario_at_removes_middle_of_three() {
        use pretty_assertions::assert_str_eq;

        let three = "\
- name: alpha
  when:
    argv: [creft, a]
  then:
    exit_code: 0
- name: beta
  when:
    argv: [creft, b]
  then:
    exit_code: 0
- name: gamma
  when:
    argv: [creft, c]
  then:
    exit_code: 0
";
        let result =
            remove_scenario_at(three, 1, label()).expect("remove_scenario_at must succeed");

        let expected = "\
- name: alpha
  when:
    argv: [creft, a]
  then:
    exit_code: 0
- name: gamma
  when:
    argv: [creft, c]
  then:
    exit_code: 0
";
        assert_str_eq!(expected, result);
    }

    #[test]
    fn remove_scenario_at_preserves_file_level_comment() {
        use pretty_assertions::assert_str_eq;

        let fixture = "\
# header comment
# second header line
- name: alpha
  when:
    argv: [creft, a]
  then:
    exit_code: 0
";
        let result =
            remove_scenario_at(fixture, 0, label()).expect("remove_scenario_at must succeed");

        let expected = "\
# header comment
# second header line
";
        assert_str_eq!(expected, result);
    }

    #[test]
    fn remove_scenario_at_removes_only_entry_to_empty_or_header() {
        use pretty_assertions::assert_str_eq;

        // No header: result is empty.
        let single = "\
- name: only
  when:
    argv: [creft, a]
  then:
    exit_code: 0
";
        let result =
            remove_scenario_at(single, 0, label()).expect("remove_scenario_at must succeed");
        assert_str_eq!("", result);

        // With header: result is just the header.
        let with_header = "\
# this is the only scenario
- name: only
  when:
    argv: [creft, a]
  then:
    exit_code: 0
";
        let result = remove_scenario_at(with_header, 0, label())
            .expect("remove_scenario_at must succeed for fixture with header");
        assert_str_eq!("# this is the only scenario\n", result);
    }

    #[test]
    fn inter_entry_comment_attaches_to_previous_entry() {
        use pretty_assertions::assert_str_eq;

        let fixture = "\
- name: alpha
  when:
    argv: [creft, a]
  then:
    exit_code: 0
# this comment lives just above beta
- name: beta
  when:
    argv: [creft, b]
  then:
    exit_code: 0
";

        // Remove index 0: alpha and the comment between it and beta are gone.
        let remove_alpha =
            remove_scenario_at(fixture, 0, label()).expect("remove index 0 must succeed");
        let expected_alpha_removed = "\
- name: beta
  when:
    argv: [creft, b]
  then:
    exit_code: 0
";
        assert_str_eq!(expected_alpha_removed, remove_alpha);

        // Remove index 1: alpha and the comment both survive because the comment
        // is within alpha's byte range (between alpha's `-` line and beta's `-` line).
        let remove_beta =
            remove_scenario_at(fixture, 1, label()).expect("remove index 1 must succeed");
        let expected_beta_removed = "\
- name: alpha
  when:
    argv: [creft, a]
  then:
    exit_code: 0
# this comment lives just above beta
";
        assert_str_eq!(expected_beta_removed, remove_beta);
    }

    #[test]
    fn inter_entry_blank_line_attaches_to_previous_entry() {
        use pretty_assertions::assert_str_eq;

        let fixture = "\
- name: alpha
  when:
    argv: [creft, a]
  then:
    exit_code: 0

- name: beta
  when:
    argv: [creft, b]
  then:
    exit_code: 0
";

        // Remove index 0: blank line is within alpha's range and is removed.
        let remove_alpha =
            remove_scenario_at(fixture, 0, label()).expect("remove index 0 must succeed");
        let expected_alpha_removed = "\
- name: beta
  when:
    argv: [creft, b]
  then:
    exit_code: 0
";
        assert_str_eq!(expected_alpha_removed, remove_alpha);

        // Remove index 1: blank line is within alpha's range and survives.
        let remove_beta =
            remove_scenario_at(fixture, 1, label()).expect("remove index 1 must succeed");
        let expected_beta_removed = "\
- name: alpha
  when:
    argv: [creft, a]
  then:
    exit_code: 0

";
        assert_str_eq!(expected_beta_removed, remove_beta);
    }

    #[test]
    fn remove_scenario_at_out_of_bounds_returns_parse_error() {
        let err = remove_scenario_at(TWO_ENTRIES, 99, label()).unwrap_err();
        assert!(
            matches!(err, FixtureError::Parse { .. }),
            "expected Parse error for out-of-bounds index, got: {err}"
        );
    }

    #[test]
    fn remove_scenario_at_flow_style_returns_parse_error() {
        let flow = "[{name: alpha}, {name: beta}]";
        let err = remove_scenario_at(flow, 0, label()).unwrap_err();
        assert!(
            matches!(err, FixtureError::Parse { .. }),
            "expected Parse error for flow-style, got: {err}"
        );
        let msg = err.to_string();
        assert!(
            msg.contains("flow-style"),
            "error message must mention flow-style; got: {msg}"
        );
    }

    #[rstest]
    #[case::marker_only(
        "---\n- name: alpha\n  when:\n    argv: [creft, a]\n  then:\n    exit_code: 0\n- name: beta\n  when:\n    argv: [creft, b]\n  then:\n    exit_code: 0\n"
    )]
    #[case::marker_after_comment(
        "# header\n---\n- name: alpha\n  when:\n    argv: [creft, a]\n  then:\n    exit_code: 0\n"
    )]
    #[case::marker_crlf_line_ending(
        "---\r\n- name: alpha\n  when:\n    argv: [creft, a]\n  then:\n    exit_code: 0\n"
    )]
    #[case::marker_trailing_space(
        "--- \n- name: alpha\n  when:\n    argv: [creft, a]\n  then:\n    exit_code: 0\n"
    )]
    fn remove_scenario_at_rejects_document_start_marker(#[case] input: &str) {
        let err = remove_scenario_at(input, 0, label()).unwrap_err();
        assert!(
            matches!(err, FixtureError::Parse { .. }),
            "expected Parse error for document-start marker, got: {err}"
        );
        let msg = err.to_string();
        assert!(
            msg.contains("---") || msg.contains("document-start"),
            "error message must reference '---' or 'document-start'; got: {msg}"
        );
    }

    #[test]
    fn remove_scenario_at_handles_dash_dash_dash_in_block_scalar() {
        use pretty_assertions::assert_str_eq;

        // A fixture with `---` appearing inside a `|` block scalar (markdown
        // frontmatter is the canonical real-world case). Step 0 must not reject
        // this — `---` at indented positions is plain text, not a document-start
        // marker. The document-start marker check is column-zero only.
        let fixture = "\
- name: with frontmatter
  given:
    files:
      \"doc.md\": |
        ---
        title: my doc
        ---
        Body
  when:
    argv: [creft, a]
  then:
    exit_code: 0
- name: other scenario
  when:
    argv: [creft, b]
  then:
    exit_code: 0
";

        // Removing entry 1 must succeed and leave entry 0 byte-for-byte intact,
        // including the `---` lines inside the block scalar.
        let result =
            remove_scenario_at(fixture, 1, label()).expect("block-scalar --- must not be rejected");
        let expected = "\
- name: with frontmatter
  given:
    files:
      \"doc.md\": |
        ---
        title: my doc
        ---
        Body
  when:
    argv: [creft, a]
  then:
    exit_code: 0
";
        assert_str_eq!(expected, result);
    }

    #[test]
    fn step1_entry_count_matches_schema_validator_on_real_fixtures() {
        // Corpus drift protection: the byte-scan matcher and the typed schema
        // validator must agree on the entry count for every real fixture in the
        // repo's commands/ tree. The inline corpus is always checked as a
        // baseline; real fixtures are added when the commands/ tree is non-empty.

        // Walk the repo's commands/ tree for *.test.yaml files. CARGO_MANIFEST_DIR
        // points to the crate root (the repo root for this single-crate project).
        let commands_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("commands");

        /// Recursively collect all *.test.yaml paths under `dir`. Returns an
        /// empty Vec when the directory does not exist.
        fn collect_test_yaml_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return Vec::new();
            };
            let mut found = Vec::new();
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    found.extend(collect_test_yaml_files(&path));
                } else if path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.ends_with(".test.yaml"))
                    .unwrap_or(false)
                {
                    found.push(path);
                }
            }
            found
        }

        let real_fixtures = collect_test_yaml_files(&commands_dir);

        // Inline baseline — always asserted regardless of whether real fixtures exist.
        let inline_corpus: &[(&str, &str)] = &[
            (
                "<two-entries>",
                "\
- name: alpha
  when:
    argv: [creft, a]
  then:
    exit_code: 0
- name: beta
  when:
    argv: [creft, b]
  then:
    exit_code: 0
",
            ),
            (
                "<three-entries>",
                "\
- name: first
  when:
    argv: [creft, x]
  then:
    exit_code: 0
- name: second
  when:
    argv: [creft, y]
  then:
    exit_code: 0
- name: third
  when:
    argv: [creft, z]
  then:
    exit_code: 0
",
            ),
        ];

        for (label, content) in inline_corpus {
            let byte_scan_count = collect_top_level_entry_offsets(content).len();
            let schema_count = parse_scenarios_str(content, Path::new(label))
                .expect("inline fixture must parse")
                .len();
            assert_eq!(
                byte_scan_count, schema_count,
                "byte-scan entry count ({byte_scan_count}) must match schema validator count \
                 ({schema_count}) for fixture '{label}'"
            );
        }

        // Assert parity for every real fixture discovered in commands/.
        for fixture_path in &real_fixtures {
            let content = std::fs::read_to_string(fixture_path)
                .unwrap_or_else(|e| panic!("failed to read {}: {e}", fixture_path.display()));
            let label = fixture_path.display().to_string();
            let byte_scan_count = collect_top_level_entry_offsets(&content).len();
            let schema_count = parse_scenarios_str(&content, fixture_path)
                .unwrap_or_else(|e| {
                    panic!(
                        "real fixture {} must parse cleanly: {e}",
                        fixture_path.display()
                    )
                })
                .len();
            assert_eq!(
                byte_scan_count, schema_count,
                "byte-scan entry count ({byte_scan_count}) must match schema validator count \
                 ({schema_count}) for real fixture '{label}'"
            );
        }
    }

    #[test]
    fn remove_scenario_at_excludes_hyphen_prefixed_scalar_at_base_indent_if_present() {
        // Guard for Step 1 clause (2). A `-foo: bar` line at base-indent
        // column must NOT be counted as an entry indicator by the byte scan.
        //
        // First: confirm parse_scenarios_str rejects this input (it is not a
        // valid top-level sequence of scenario mappings), establishing that the
        // layered-guard chain catches this upstream.
        let constructed = "\
- name: alpha
  when:
    argv: [creft, a]
  then:
    exit_code: 0
-foo: this-is-a-plain-scalar-not-an-entry-indicator
- name: beta
  when:
    argv: [creft, b]
  then:
    exit_code: 0
";
        let parse_result = parse_scenarios_str(constructed, label());
        assert!(
            parse_result.is_err(),
            "parse_scenarios_str must reject a fixture with a hyphen-prefixed plain scalar at base indent; got Ok"
        );

        // Second: the byte scan itself must return exactly two offsets — the
        // `- name: alpha` line and the `- name: beta` line. The `-foo: ...`
        // line must not be counted. This is the future-widening guard: even if
        // parse_scenarios_str ever loosens, the byte scan stays correct.
        let offsets = collect_top_level_entry_offsets(constructed);
        assert_eq!(
            offsets.len(),
            2,
            "byte scan must find exactly 2 entry indicators (alpha and beta), not the -foo line; got {offsets:?}"
        );
        // Verify the two offsets land on the `- name:` lines.
        assert!(
            &constructed[offsets[0]..].starts_with("- name: alpha"),
            "first offset must point to alpha's line"
        );
        assert!(
            &constructed[offsets[1]..].starts_with("- name: beta"),
            "second offset must point to beta's line"
        );
    }

    #[test]
    fn remove_scenario_at_does_not_round_trip_through_emitter() {
        use pretty_assertions::assert_str_eq;

        // A fixture with hand-formatted multi-line scalars. The YAML emitter
        // would reflow these; the byte-scan removal must not.
        let fixture = "\
- name: alpha
  given:
    files:
      \"notes.txt\": |
        First line.
        Second line.
  when:
    argv: [creft, a]
  then:
    exit_code: 0
- name: beta
  when:
    argv: [creft, b]
  then:
    exit_code: 0
";
        let result =
            remove_scenario_at(fixture, 1, label()).expect("remove_scenario_at must succeed");

        let expected = "\
- name: alpha
  given:
    files:
      \"notes.txt\": |
        First line.
        Second line.
  when:
    argv: [creft, a]
  then:
    exit_code: 0
";
        assert_str_eq!(expected, result);
    }

    #[test]
    fn remove_scenario_at_handles_non_ascii_in_plain_scalar() {
        use pretty_assertions::assert_str_eq;

        let fixture = "\
- name: \"✓ basic\"
  when:
    argv: [creft, a]
  then:
    exit_code: 0
- name: ascii-entry
  when:
    argv: [creft, b]
  then:
    exit_code: 0
";
        // Remove index 1: the non-ASCII entry 0 must survive byte-for-byte.
        let remove_1 =
            remove_scenario_at(fixture, 1, label()).expect("remove index 1 must succeed");
        let expected_after_1 = "\
- name: \"✓ basic\"
  when:
    argv: [creft, a]
  then:
    exit_code: 0
";
        assert_str_eq!(expected_after_1, remove_1);

        // Remove index 0: entry 1 must survive byte-for-byte.
        let remove_0 =
            remove_scenario_at(fixture, 0, label()).expect("remove index 0 must succeed");
        let expected_after_0 = "\
- name: ascii-entry
  when:
    argv: [creft, b]
  then:
    exit_code: 0
";
        assert_str_eq!(expected_after_0, remove_0);
    }

    #[test]
    fn remove_scenario_at_handles_non_ascii_in_block_scalar() {
        use pretty_assertions::assert_str_eq;

        // Regression guard for the char-vs-byte index inconsistency in
        // yaml-rust2's scan_block_scalar_content_line. An implementation
        // deriving byte offsets from Marker.index() would fail here.
        let fixture = "\
- name: alpha
  given:
    files:
      \"src/note.md\": |
        Status: \u{2713} done
        Author: J\u{00FC}rgen
  when:
    argv: [creft, a]
  then:
    exit_code: 0
- name: beta
  given:
    files: {}
  when:
    argv: [creft, b]
  then:
    exit_code: 0
";
        // Remove index 1: alpha's bytes — including the multibyte block scalar
        // content — must survive verbatim.
        let remove_beta =
            remove_scenario_at(fixture, 1, label()).expect("remove beta must succeed");
        let expected_alpha_only = "\
- name: alpha
  given:
    files:
      \"src/note.md\": |
        Status: \u{2713} done
        Author: J\u{00FC}rgen
  when:
    argv: [creft, a]
  then:
    exit_code: 0
";
        assert_str_eq!(expected_alpha_only, remove_beta);

        // Remove index 0: the surviving bytes must start exactly at beta's `-` line.
        let remove_alpha =
            remove_scenario_at(fixture, 0, label()).expect("remove alpha must succeed");
        let expected_beta_only = "\
- name: beta
  given:
    files: {}
  when:
    argv: [creft, b]
  then:
    exit_code: 0
";
        assert_str_eq!(expected_beta_only, remove_alpha);
    }

    #[test]
    fn remove_scenario_at_handles_non_ascii_in_folded_scalar() {
        use pretty_assertions::assert_str_eq;

        // Same shape as the block-scalar test but using `>` (folded) style.
        // Both scalar styles route through scan_block_scalar_content_line, so
        // coverage of one implies the other — this is the explicit second case.
        let fixture = "\
- name: alpha
  given:
    files:
      \"readme.md\": >
        Status: \u{2713} done.
        Author: J\u{00FC}rgen.
  when:
    argv: [creft, a]
  then:
    exit_code: 0
- name: beta
  given:
    files: {}
  when:
    argv: [creft, b]
  then:
    exit_code: 0
";
        let remove_beta =
            remove_scenario_at(fixture, 1, label()).expect("remove beta must succeed");
        let expected_alpha_only = "\
- name: alpha
  given:
    files:
      \"readme.md\": >
        Status: \u{2713} done.
        Author: J\u{00FC}rgen.
  when:
    argv: [creft, a]
  then:
    exit_code: 0
";
        assert_str_eq!(expected_alpha_only, remove_beta);

        let remove_alpha =
            remove_scenario_at(fixture, 0, label()).expect("remove alpha must succeed");
        let expected_beta_only = "\
- name: beta
  given:
    files: {}
  when:
    argv: [creft, b]
  then:
    exit_code: 0
";
        assert_str_eq!(expected_beta_only, remove_alpha);
    }
}
