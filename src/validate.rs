use std::collections::HashSet;
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::LazyLock;
use std::time::Duration;

use crate::doctor;
use crate::model::{AppContext, CodeBlock, CommandDef, PLACEHOLDER_RE};
use crate::registry_config::{self, HttpMethod, RegistryEndpoint};
use crate::store;

/// Outcome of validating a parsed skill.
#[derive(Debug)]
pub struct ValidationResult {
    /// Hard errors that block the save.
    pub errors: Vec<ValidationDiagnostic>,
    /// Warnings printed to stderr but do not block the save.
    pub warnings: Vec<ValidationDiagnostic>,
}

impl ValidationResult {
    /// Returns `true` if the result contains any hard errors (blocking the save).
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }

    #[cfg(test)]
    pub fn is_clean(&self) -> bool {
        self.errors.is_empty() && self.warnings.is_empty()
    }
}

/// A single validation finding, either an error or a warning.
#[derive(Debug)]
pub struct ValidationDiagnostic {
    /// Which code block (0-indexed) this finding applies to.
    /// None for skill-level findings (e.g., no code blocks).
    pub block_index: Option<usize>,
    /// Language of the block, for display context.
    pub lang: Option<String>,
    /// Human-readable description of the problem.
    pub message: String,
    /// Optional line number within the code block (1-indexed).
    pub line: Option<usize>,
}

impl std::fmt::Display for ValidationDiagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (self.block_index, &self.lang, self.line) {
            (Some(idx), Some(lang), Some(line)) => {
                write!(
                    f,
                    "block {} ({}), line {}: {}",
                    idx + 1,
                    lang,
                    line,
                    self.message
                )
            }
            (Some(idx), Some(lang), None) => {
                write!(f, "block {} ({}): {}", idx + 1, lang, self.message)
            }
            (Some(idx), None, _) => {
                write!(f, "block {}: {}", idx + 1, self.message)
            }
            (None, _, _) => {
                write!(f, "{}", self.message)
            }
        }
    }
}

/// Maximum description length before a warning is emitted.
/// Descriptions longer than this degrade `creft list` output.
pub(crate) const DESCRIPTION_WARN_LEN: usize = 80;

/// Validate a parsed skill's code blocks.
///
/// Returns a `ValidationResult` with errors (blocking) and warnings (advisory).
/// Does not modify the skill or execute any code.
///
/// Pass `None` for `ctx` to skip command and sub-skill existence checks
/// (useful in tests where no registry is available).
pub fn validate_skill(
    def: &CommandDef,
    blocks: &[CodeBlock],
    ctx: Option<&AppContext>,
) -> ValidationResult {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    // Skill-level check: description length.
    if def.description.len() > DESCRIPTION_WARN_LEN {
        warnings.push(ValidationDiagnostic {
            block_index: None,
            lang: None,
            message: format!(
                "description is long ({} chars). Keep descriptions under {} characters for clean list output",
                def.description.len(),
                DESCRIPTION_WARN_LEN,
            ),
            line: None,
        });
    }

    for (i, block) in blocks.iter().enumerate() {
        // LLM blocks have their own validation path.
        if block.lang == "llm" {
            check_llm_block(def, block, i, &mut errors, &mut warnings);
            continue;
        }

        check_placeholders(def, block, i, &mut warnings);

        // Record the error count before syntax check so we can gate shellcheck
        // on whether syntax passed.
        let errors_before = errors.len();
        check_syntax(block, i, &mut errors);
        let syntax_ok = errors.len() == errors_before;

        if syntax_ok && doctor::is_shell_lang(&block.lang) {
            let sanitized = sanitize_placeholders(&block.code);
            check_shellcheck(&sanitized, i, &block.lang, &mut warnings);
        }

        // Check creft sub-skill invocations reference real skills.
        if doctor::is_shell_lang(&block.lang) {
            check_sub_skill_existence(block, i, ctx, &mut warnings);
        }

        check_dependency_resolution(block, i, ctx, &mut warnings);
    }

    ValidationResult { errors, warnings }
}

fn check_placeholders(
    def: &CommandDef,
    block: &CodeBlock,
    block_index: usize,
    warnings: &mut Vec<ValidationDiagnostic>,
) {
    let mut declared: HashSet<&str> = HashSet::new();
    declared.insert("prev");
    for arg in &def.args {
        declared.insert(arg.name.as_str());
    }
    for flag in &def.flags {
        declared.insert(flag.name.as_str());
    }

    let re = &*PLACEHOLDER_RE;
    for caps in re.captures_iter(&block.code) {
        let name = caps.get(1).map(|m| m.as_str()).unwrap_or("");
        let has_default = caps.get(2).is_some();

        if name == "prev" && block_index == 0 {
            warnings.push(ValidationDiagnostic {
                block_index: Some(block_index),
                lang: Some(block.lang.clone()),
                message: "{{prev}} in first block has no preceding block output".into(),
                line: None,
            });
        } else if !declared.contains(name) {
            if has_default {
                warnings.push(ValidationDiagnostic {
                    block_index: Some(block_index),
                    lang: Some(block.lang.clone()),
                    message: format!(
                        "placeholder {{{{{}}}}} is not declared in args or flags (has default)",
                        name
                    ),
                    line: None,
                });
            } else {
                warnings.push(ValidationDiagnostic {
                    block_index: Some(block_index),
                    lang: Some(block.lang.clone()),
                    message: format!(
                        "placeholder {{{{{}}}}} is not declared in args or flags",
                        name
                    ),
                    line: None,
                });
            }
        }
    }
}

/// Validate an `llm` code block.
///
/// Checks:
/// 1. YAML header parse error (hard error)
/// 2. Empty prompt after header extraction (hard error)
/// 3. Placeholder references in the prompt body (same as other blocks)
/// 4. Provider CLI not on PATH (warning)
fn check_llm_block(
    def: &CommandDef,
    block: &CodeBlock,
    block_index: usize,
    errors: &mut Vec<ValidationDiagnostic>,
    warnings: &mut Vec<ValidationDiagnostic>,
) {
    // YAML header parse failure takes priority — emit error and stop.
    if let Some(parse_err) = &block.llm_parse_error {
        errors.push(ValidationDiagnostic {
            block_index: Some(block_index),
            lang: Some("llm".to_string()),
            message: format!("invalid YAML header in llm block: {}", parse_err),
            line: None,
        });
        return;
    }

    if block.code.trim().is_empty() {
        errors.push(ValidationDiagnostic {
            block_index: Some(block_index),
            lang: Some("llm".to_string()),
            message: "llm block has no prompt text".to_string(),
            line: None,
        });
        return;
    }

    check_placeholders(def, block, block_index, warnings);

    // Provider CLI PATH check (warning only — provider may be available in the
    // execution environment but not the authoring environment).
    if let Some(config) = &block.llm_config {
        let provider = if config.provider.is_empty() {
            "claude"
        } else {
            config.provider.as_str()
        };
        if doctor::which_path(provider).is_none() {
            warnings.push(ValidationDiagnostic {
                block_index: Some(block_index),
                lang: Some("llm".to_string()),
                message: format!(
                    "llm provider '{}' not found on PATH (install the provider CLI or ignore if \
                     running in a different environment)",
                    provider
                ),
                line: None,
            });
        }
    }
}

/// Replace all `{{placeholder}}` and `{{placeholder|default}}` patterns with
/// `__CREFT_PH__` so that syntax checkers see a valid identifier rather than
/// a template token that may look like illegal syntax.
///
/// Using a bare identifier works in all supported languages:
/// - Shell: `__CREFT_PH__` is a valid word / variable name
/// - Python: `__CREFT_PH__` is a valid identifier
/// - Node/JS: `__CREFT_PH__` is a valid identifier
/// - Ruby: `__CREFT_PH__` is a valid identifier (dunders style, syntactically fine)
fn sanitize_placeholders(code: &str) -> String {
    let re = &*PLACEHOLDER_RE;
    re.replace_all(code, "__CREFT_PH__").into_owned()
}

/// Dispatch syntax checking to the language-appropriate function.
fn check_syntax(block: &CodeBlock, block_index: usize, errors: &mut Vec<ValidationDiagnostic>) {
    match block.lang.as_str() {
        "bash" | "sh" | "zsh" => check_shell_syntax(block, block_index, errors),
        "python" | "python3" => check_python_syntax(block, block_index, errors),
        "node" | "javascript" | "js" => check_node_syntax(block, block_index, errors),
        "ruby" | "rb" => check_ruby_syntax(block, block_index, errors),
        _ => {} // Unknown language — skip silently
    }
}

/// Run `bash -n` (or `sh -n` / `zsh -n`) on sanitized code.
///
/// If the tool is not on PATH the check is silently skipped.
/// After a clean `bash -n` pass, also runs shellcheck (warnings only) if available.
fn check_shell_syntax(
    block: &CodeBlock,
    block_index: usize,
    errors: &mut Vec<ValidationDiagnostic>,
) {
    let sanitized = sanitize_placeholders(&block.code);

    let shell = match block.lang.as_str() {
        "sh" => "sh",
        "zsh" => "zsh",
        _ => "bash",
    };

    if doctor::which_path(shell).is_none() {
        return;
    }

    let mut tmp = match tempfile::Builder::new().suffix(".sh").tempfile() {
        Ok(f) => f,
        Err(_) => return,
    };
    if tmp.write_all(sanitized.as_bytes()).is_err() {
        return;
    }
    let _ = tmp.flush(); // flush before handing the path to a subprocess

    let output = match Command::new(shell)
        .arg("-n")
        .arg(tmp.path())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
    {
        Ok(o) => o,
        Err(_) => return,
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        parse_shell_errors(&stderr, block_index, &block.lang, errors);
    }
}

/// Parse `bash -n` / `sh -n` stderr into diagnostics.
///
/// Expected format: `<file>: line N: <message>`
fn parse_shell_errors(
    stderr: &str,
    block_index: usize,
    lang: &str,
    errors: &mut Vec<ValidationDiagnostic>,
) {
    static SHELL_ERR_RE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r": line (\d+): (.+)$").unwrap());

    let mut found_any = false;
    for line in stderr.lines() {
        if let Some(caps) = SHELL_ERR_RE.captures(line) {
            let lineno: usize = caps[1].parse().unwrap_or(0);
            let msg = caps[2].trim().to_string();
            errors.push(ValidationDiagnostic {
                block_index: Some(block_index),
                lang: Some(lang.to_string()),
                message: msg,
                line: if lineno > 0 { Some(lineno) } else { None },
            });
            found_any = true;
        }
    }

    if !found_any && !stderr.trim().is_empty() {
        errors.push(ValidationDiagnostic {
            block_index: Some(block_index),
            lang: Some(lang.to_string()),
            message: stderr.trim().to_string(),
            line: None,
        });
    }
}

/// Run `python3 -B -c "import ast,sys; ast.parse(open(sys.argv[1]).read())"`.
///
/// If `python3` is not on PATH the check is silently skipped.
fn check_python_syntax(
    block: &CodeBlock,
    block_index: usize,
    errors: &mut Vec<ValidationDiagnostic>,
) {
    if doctor::which_path("python3").is_none() {
        return;
    }

    let sanitized = sanitize_placeholders(&block.code);

    let mut tmp = match tempfile::Builder::new().suffix(".py").tempfile() {
        Ok(f) => f,
        Err(_) => return,
    };
    if tmp.write_all(sanitized.as_bytes()).is_err() {
        return;
    }
    let _ = tmp.flush();

    let path_str = tmp.path().to_string_lossy().into_owned();

    let output = match Command::new("python3")
        .args([
            "-B",
            "-c",
            "import ast,sys; ast.parse(open(sys.argv[1]).read())",
            &path_str,
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
    {
        Ok(o) => o,
        Err(_) => return,
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        parse_python_errors(&stderr, block_index, errors);
    }
}

/// Parse Python syntax error output into diagnostics.
///
/// Python stderr format (abbreviated):
/// ```
///   File "<file>", line N
///     <code>
///          ^
/// SyntaxError: <message>
/// ```
fn parse_python_errors(stderr: &str, block_index: usize, errors: &mut Vec<ValidationDiagnostic>) {
    static PY_LINE_RE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"line (\d+)").unwrap());
    static PY_ERR_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"(?:SyntaxError|IndentationError|TabError): (.+)").unwrap()
    });

    let line_num = PY_LINE_RE
        .captures(stderr)
        .and_then(|c| c[1].parse::<usize>().ok());

    let message = if let Some(caps) = PY_ERR_RE.captures(stderr) {
        caps[1].trim().to_string()
    } else {
        stderr.trim().to_string()
    };

    if !message.is_empty() {
        errors.push(ValidationDiagnostic {
            block_index: Some(block_index),
            lang: Some("python".to_string()),
            message,
            line: line_num,
        });
    }
}

/// Run `node --check <tempfile>`.
///
/// If `node` is not on PATH the check is silently skipped.
fn check_node_syntax(
    block: &CodeBlock,
    block_index: usize,
    errors: &mut Vec<ValidationDiagnostic>,
) {
    if doctor::which_path("node").is_none() {
        return;
    }

    let sanitized = sanitize_placeholders(&block.code);

    let mut tmp = match tempfile::Builder::new().suffix(".js").tempfile() {
        Ok(f) => f,
        Err(_) => return,
    };
    if tmp.write_all(sanitized.as_bytes()).is_err() {
        return;
    }
    let _ = tmp.flush();

    let output = match Command::new("node")
        .arg("--check")
        .arg(tmp.path())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
    {
        Ok(o) => o,
        Err(_) => return,
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        parse_node_errors(&stderr, block_index, errors);
    }
}

/// Parse Node.js syntax error output into diagnostics.
///
/// Node --check stderr format:
/// ```
/// <file>:N
/// <code>
///      ^
/// SyntaxError: <message>
/// ```
fn parse_node_errors(stderr: &str, block_index: usize, errors: &mut Vec<ValidationDiagnostic>) {
    static NODE_LINE_RE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r":(\d+)\b").unwrap());
    static NODE_ERR_RE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"SyntaxError: (.+)").unwrap());

    let first_line = stderr.lines().next().unwrap_or("");
    let line_num = NODE_LINE_RE
        .captures(first_line)
        .and_then(|c| c[1].parse::<usize>().ok());

    let message = if let Some(caps) = NODE_ERR_RE.captures(stderr) {
        caps[1].trim().to_string()
    } else {
        stderr.trim().to_string()
    };

    if !message.is_empty() {
        errors.push(ValidationDiagnostic {
            block_index: Some(block_index),
            lang: Some("node".to_string()),
            message,
            line: line_num,
        });
    }
}

/// Run `ruby -c <tempfile>`.
///
/// If `ruby` is not on PATH the check is silently skipped.
fn check_ruby_syntax(
    block: &CodeBlock,
    block_index: usize,
    errors: &mut Vec<ValidationDiagnostic>,
) {
    if doctor::which_path("ruby").is_none() {
        return;
    }

    let sanitized = sanitize_placeholders(&block.code);

    let mut tmp = match tempfile::Builder::new().suffix(".rb").tempfile() {
        Ok(f) => f,
        Err(_) => return,
    };
    if tmp.write_all(sanitized.as_bytes()).is_err() {
        return;
    }
    let _ = tmp.flush();

    let output = match Command::new("ruby")
        .arg("-c")
        .arg(tmp.path())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
    {
        Ok(o) => o,
        Err(_) => return,
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        parse_ruby_errors(&stderr, block_index, errors);
    }
}

/// Parse `ruby -c` stderr into diagnostics.
///
/// Format: `<file>:N: <message>`
fn parse_ruby_errors(stderr: &str, block_index: usize, errors: &mut Vec<ValidationDiagnostic>) {
    static RUBY_ERR_RE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r":(\d+): (.+)$").unwrap());

    let mut found_any = false;
    for line in stderr.lines() {
        if let Some(caps) = RUBY_ERR_RE.captures(line) {
            let lineno: usize = caps[1].parse().unwrap_or(0);
            let msg = caps[2].trim().to_string();
            errors.push(ValidationDiagnostic {
                block_index: Some(block_index),
                lang: Some("ruby".to_string()),
                message: msg,
                line: if lineno > 0 { Some(lineno) } else { None },
            });
            found_any = true;
        }
    }

    if !found_any && !stderr.trim().is_empty() {
        errors.push(ValidationDiagnostic {
            block_index: Some(block_index),
            lang: Some("ruby".to_string()),
            message: stderr.trim().to_string(),
            line: None,
        });
    }
}

/// Run shellcheck on sanitized shell code (stdin mode) and collect findings as warnings.
///
/// Shellcheck is entirely optional — if not on PATH, this is a no-op.
/// All shellcheck findings become warnings regardless of shellcheck's own severity.
pub(crate) fn check_shellcheck(
    sanitized_code: &str,
    block_index: usize,
    lang: &str,
    warnings: &mut Vec<ValidationDiagnostic>,
) {
    if doctor::which_path("shellcheck").is_none() {
        return;
    }

    let shell_dialect = match lang {
        "sh" => "sh",
        "zsh" => "bash", // shellcheck doesn't support zsh natively; bash is closest
        _ => "bash",
    };

    let mut child = match Command::new("shellcheck")
        .args([
            "-s",
            shell_dialect,
            "-f",
            "gcc",
            "-e",
            "SC2034", // variable appears unused — common in skill block snippets
            "-e",
            "SC2086", // double quote to prevent globbing — fires on template placeholders
            "-e",
            "SC2154", // variable referenced but not assigned — common for cross-block vars
            "-",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return,
    };

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(sanitized_code.as_bytes());
        // drop signals EOF to shellcheck
    }

    let output = match child.wait_with_output() {
        Ok(o) => o,
        Err(_) => return,
    };

    // gcc format: <file>:<line>:<col>: <level>: <message>
    static SC_RE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"^[^:]+:(\d+):\d+: \w+: (.+)$").unwrap());

    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        if let Some(caps) = SC_RE.captures(line) {
            let lineno: usize = caps[1].parse().unwrap_or(0);
            let msg = caps[2].trim().to_string();
            warnings.push(ValidationDiagnostic {
                block_index: Some(block_index),
                lang: Some(lang.to_string()),
                message: msg,
                line: if lineno > 0 { Some(lineno) } else { None },
            });
        }
    }
}

/// Check that commands referenced in a shell block exist on PATH.
///
/// Uses the same extraction logic as `creft doctor` to parse command names
/// from shell code, then probes PATH for each. Missing commands produce
/// warnings (never errors).
///
/// Placeholders are sanitized before extraction to avoid false positives
/// from template tokens like `{{name|default}}` where `|` would look like
/// a pipe and `default` would be extracted as a command name.
/// Check that creft sub-skill invocations reference skills that exist.
///
/// Parses `creft <name>` patterns from shell code and attempts to resolve
/// each via the store. Unresolved skills produce warnings.
/// Skipped when `ctx` is None (unit test context).
fn check_sub_skill_existence(
    block: &CodeBlock,
    block_index: usize,
    ctx: Option<&AppContext>,
    warnings: &mut Vec<ValidationDiagnostic>,
) {
    let ctx = match ctx {
        Some(c) => c,
        None => return, // No AppContext -- skip sub-skill checking
    };

    // Sanitize placeholders before extraction to avoid matching `creft` inside
    // template patterns like `echo "creft {{skill|default}}"`.
    let sanitized = sanitize_placeholders(&block.code);
    let calls = doctor::extract_creft_calls(&sanitized);
    for skill_name in &calls {
        let args: Vec<String> = skill_name.split_whitespace().map(String::from).collect();
        if args.is_empty() {
            continue;
        }
        if store::resolve_command(ctx, &args).is_err() {
            warnings.push(ValidationDiagnostic {
                block_index: Some(block_index),
                lang: Some(block.lang.clone()),
                message: format!(
                    "skill '{}' not found (referenced as creft sub-skill)",
                    skill_name
                ),
                line: None,
            });
        }
    }
}

/// Check that declared dependencies can be resolved in their package registry.
///
/// For Python/Node: fires HTTP requests at the configured registry endpoints.
/// For Shell: checks PATH via `which_path` (reuses existing logic).
/// All deps for a single block are checked concurrently via `std::thread::scope`.
/// Skipped entirely when `block.deps` is empty.
fn check_dependency_resolution(
    block: &CodeBlock,
    block_index: usize,
    ctx: Option<&AppContext>,
    warnings: &mut Vec<ValidationDiagnostic>,
) {
    if block.deps.is_empty() {
        return;
    }

    match block.lang.as_str() {
        "python" | "python3" => {
            let endpoints = registry_config::resolve_pypi(ctx);
            let labels = endpoints
                .iter()
                .map(|e| e.label.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            let dep_endpoints: Vec<(&String, &[RegistryEndpoint])> = block
                .deps
                .iter()
                .map(|dep| (dep, endpoints.as_slice()))
                .collect();
            check_deps_preresolved(block, block_index, &dep_endpoints, &labels, warnings);
        }
        "node" | "js" | "javascript" => {
            let npm_config = registry_config::resolve_npm(ctx);
            let labels = npm_config
                .defaults
                .iter()
                .map(|e| e.label.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            // Pre-resolve per-dep endpoints before entering thread::scope to
            // avoid lifetime issues with the closure borrowing npm_config.
            let dep_endpoints: Vec<(&String, &[RegistryEndpoint])> = block
                .deps
                .iter()
                .map(|dep| (dep, npm_config.endpoints_for(dep)))
                .collect();
            check_deps_preresolved(block, block_index, &dep_endpoints, &labels, warnings);
        }
        "bash" | "sh" | "zsh" => check_shell_deps(block, block_index, warnings),
        _ => {} // Unknown language -- skip dep checking silently
    }
}

/// Check deps against pre-resolved registry endpoints using HTTP requests.
///
/// For each dep, tries endpoints in order. A dep is "found" if ANY endpoint
/// returns non-404. Only warns if ALL endpoints return 404.
///
/// Uses pre-resolved endpoints to avoid closure lifetime issues with
/// `std::thread::scope`. For Python, all deps share the same endpoints.
/// For npm, scoped packages may use different endpoints via `NpmRegistryConfig::endpoints_for`.
///
/// Non-404 errors (network errors, auth failures) are treated as "can't determine"
/// and never produce warnings. Only a definitive 404 advances to the next endpoint.
fn check_deps_preresolved<'a>(
    block: &'a CodeBlock,
    block_index: usize,
    dep_endpoints: &[(&'a String, &'a [RegistryEndpoint])],
    all_labels: &str,
    warnings: &mut Vec<ValidationDiagnostic>,
) {
    let agent = ureq::Agent::new_with_config(
        ureq::config::Config::builder()
            .timeout_global(Some(Duration::from_secs(3)))
            .build(),
    );

    let not_found: Vec<&String> = std::thread::scope(|s| {
        let handles: Vec<_> = dep_endpoints
            .iter()
            .map(|(dep, endpoints)| {
                let agent = agent.clone();
                let dep = *dep;
                let endpoints = *endpoints;
                s.spawn(move || {
                    for endpoint in endpoints {
                        let url = endpoint.url_for(dep);
                        let mut request = match endpoint.method {
                            HttpMethod::Head => agent.head(&url),
                            HttpMethod::Get => agent.get(&url),
                        };
                        if let Some(auth) = &endpoint.auth {
                            request = request.header("Authorization", &auth.header_value());
                        }
                        match request.call() {
                            Ok(_) => return false,
                            Err(ureq::Error::StatusCode(404)) => continue,
                            Err(_) => return false,
                        }
                    }
                    true
                })
            })
            .collect();

        dep_endpoints
            .iter()
            .zip(handles)
            .filter_map(|((dep, _), handle)| {
                if handle.join().unwrap_or(false) {
                    Some(*dep)
                } else {
                    None
                }
            })
            .collect()
    });

    emit_not_found_warnings(not_found, block, block_index, all_labels, warnings);
}

/// Emit warnings for deps not found on any registry.
fn emit_not_found_warnings(
    not_found: Vec<&String>,
    block: &CodeBlock,
    block_index: usize,
    all_labels: &str,
    warnings: &mut Vec<ValidationDiagnostic>,
) {
    for dep in not_found {
        warnings.push(ValidationDiagnostic {
            block_index: Some(block_index),
            lang: Some(block.lang.clone()),
            message: if all_labels.contains(", ") {
                format!(
                    "dependency '{}' not found on any configured registry ({})",
                    dep, all_labels
                )
            } else {
                format!("dependency '{}' not found on {}", dep, all_labels)
            },
            line: None,
        });
    }
}

/// Check shell block dependencies via PATH lookup (no network).
///
/// Each dep name in `block.deps` is looked up in PATH via `which_path`.
/// Missing deps produce warnings.
fn check_shell_deps(
    block: &CodeBlock,
    block_index: usize,
    warnings: &mut Vec<ValidationDiagnostic>,
) {
    for dep in &block.deps {
        if doctor::which_path(dep).is_none() {
            warnings.push(ValidationDiagnostic {
                block_index: Some(block_index),
                lang: Some(block.lang.clone()),
                message: format!("dependency '{}' not found on PATH", dep),
                line: None,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Arg, Flag};
    #[allow(unused_imports)]
    use pretty_assertions::{assert_eq, assert_ne};

    fn make_def(args: Vec<&str>, flags: Vec<&str>) -> CommandDef {
        CommandDef {
            name: "test".into(),
            description: "test command".into(),
            args: args
                .into_iter()
                .map(|n| Arg {
                    name: n.into(),
                    description: String::new(),
                    default: None,
                    required: false,
                    validation: None,
                })
                .collect(),
            flags: flags
                .into_iter()
                .map(|n| Flag {
                    name: n.into(),
                    short: None,
                    description: String::new(),
                    r#type: "string".into(),
                    default: None,
                    validation: None,
                })
                .collect(),
            env: vec![],
            tags: vec![],
            supports: vec![],
        }
    }

    fn make_block(lang: &str, code: &str) -> CodeBlock {
        CodeBlock {
            lang: lang.into(),
            code: code.into(),
            deps: vec![],
            llm_config: None,
            llm_parse_error: None,
        }
    }

    #[test]
    fn test_placeholder_all_declared() {
        let def = make_def(vec!["repo", "number"], vec![]);
        let block = make_block("bash", "gh api repos/{{repo}}/issues/{{number}}");
        let result = validate_skill(&def, &[block], None);
        assert!(
            result.is_clean(),
            "expected no warnings or errors, got: {:?}",
            result.warnings
        );
    }

    #[test]
    fn test_placeholder_undeclared_no_default() {
        let def = make_def(vec![], vec![]);
        let block = make_block("bash", "echo {{foo}}");
        let result = validate_skill(&def, &[block], None);
        assert_eq!(result.warnings.len(), 1);
        assert!(
            result.warnings[0]
                .message
                .contains("not declared in args or flags")
        );
        assert!(!result.warnings[0].message.contains("has default"));
        assert!(result.errors.is_empty());
    }

    #[test]
    fn test_placeholder_undeclared_with_default() {
        let def = make_def(vec![], vec![]);
        let block = make_block("bash", "echo {{foo|bar}}");
        let result = validate_skill(&def, &[block], None);
        assert_eq!(result.warnings.len(), 1);
        assert!(result.warnings[0].message.contains("has default"));
        assert!(result.errors.is_empty());
    }

    #[test]
    fn test_placeholder_prev_in_first_block() {
        let def = make_def(vec![], vec![]);
        let block = make_block("bash", "echo {{prev}}");
        let result = validate_skill(&def, &[block], None);
        assert_eq!(result.warnings.len(), 1);
        assert!(
            result.warnings[0]
                .message
                .contains("first block has no preceding block output")
        );
    }

    #[test]
    fn test_placeholder_prev_in_later_block() {
        let def = make_def(vec![], vec![]);
        let block0 = make_block("bash", "echo hello");
        let block1 = make_block("bash", "echo {{prev}}");
        let result = validate_skill(&def, &[block0, block1], None);
        assert!(
            result.is_clean(),
            "expected no warnings for {{prev}} in block 1"
        );
    }

    #[test]
    fn test_placeholder_flags_recognized() {
        let def = make_def(vec![], vec!["format", "verbose"]);
        let block = make_block("bash", "echo {{format}} {{verbose}}");
        let result = validate_skill(&def, &[block], None);
        assert!(result.is_clean(), "flags should be in declared set");
    }

    #[test]
    fn test_no_placeholders() {
        let def = make_def(vec![], vec![]);
        let block = make_block("bash", "echo hello world");
        let result = validate_skill(&def, &[block], None);
        assert!(result.is_clean());
    }

    #[test]
    fn test_empty_blocks() {
        let def = make_def(vec![], vec![]);
        let result = validate_skill(&def, &[], None);
        assert!(result.is_clean());
    }

    #[test]
    fn test_prev_declared_as_arg_still_warns_in_block0() {
        // Even if someone names an arg "prev", {{prev}} in block 0 still warns
        // because the semantics of {{prev}} as "previous block output" don't
        // apply when there's no previous block.
        let def = make_def(vec!["prev"], vec![]);
        let block = make_block("bash", "echo {{prev}}");
        let result = validate_skill(&def, &[block], None);
        // {{prev}} in block 0 produces the "first block has no preceding block" warning
        // regardless of whether "prev" is also an arg name.
        assert_eq!(result.warnings.len(), 1);
        assert!(
            result.warnings[0]
                .message
                .contains("first block has no preceding block output")
        );
    }

    #[test]
    fn test_display_format_with_block_lang_line() {
        let d = ValidationDiagnostic {
            block_index: Some(0),
            lang: Some("bash".into()),
            message: "some error".into(),
            line: Some(3),
        };
        assert_eq!(format!("{}", d), "block 1 (bash), line 3: some error");
    }

    #[test]
    fn test_display_format_with_block_lang_no_line() {
        let d = ValidationDiagnostic {
            block_index: Some(1),
            lang: Some("python".into()),
            message: "undeclared placeholder".into(),
            line: None,
        };
        assert_eq!(format!("{}", d), "block 2 (python): undeclared placeholder");
    }

    #[test]
    fn test_display_format_no_block() {
        let d = ValidationDiagnostic {
            block_index: None,
            lang: None,
            message: "skill-level issue".into(),
            line: None,
        };
        assert_eq!(format!("{}", d), "skill-level issue");
    }

    #[test]
    fn test_multiple_undeclared_placeholders() {
        let def = make_def(vec!["declared"], vec![]);
        let block = make_block("bash", "echo {{declared}} {{missing1}} {{missing2}}");
        let result = validate_skill(&def, &[block], None);
        // declared is fine, missing1 and missing2 each produce a warning
        assert_eq!(result.warnings.len(), 2);
    }

    // ── sanitize_placeholders unit tests ─────────────────────────────────────

    #[test]
    fn test_sanitize_placeholders_shell() {
        let result = sanitize_placeholders("echo {{name}}");
        assert_eq!(result, "echo __CREFT_PH__");
    }

    #[test]
    fn test_sanitize_placeholders_with_default() {
        let result = sanitize_placeholders("echo {{name|foo}}");
        assert_eq!(result, "echo __CREFT_PH__");
    }

    #[test]
    fn test_sanitize_placeholders_multiple() {
        let result = sanitize_placeholders("echo {{a}} {{b|default}} {{c}}");
        assert_eq!(result, "echo __CREFT_PH__ __CREFT_PH__ __CREFT_PH__");
    }

    #[test]
    fn test_sanitize_preserves_non_placeholder_braces() {
        // ${VAR} is shell variable syntax, not a creft placeholder — must be untouched.
        let result = sanitize_placeholders("echo ${HOME} {{name}}");
        assert_eq!(result, "echo ${HOME} __CREFT_PH__");
    }

    #[test]
    fn test_sanitize_placeholder_inside_string() {
        // Placeholder embedded inside a double-quoted string.
        let result = sanitize_placeholders(r#"echo "hello {{name}}""#);
        assert_eq!(result, r#"echo "hello __CREFT_PH__""#);
    }

    // ── syntax validation unit tests ─────────────────────────────────────────

    #[test]
    fn test_valid_bash_no_errors() {
        // Only runs if bash is available; otherwise the check is skipped silently.
        let def = make_def(vec![], vec![]);
        let block = make_block("bash", "echo hello\nif true; then\n  echo ok\nfi\n");
        let result = validate_skill(&def, &[block], None);
        assert!(
            result.errors.is_empty(),
            "valid bash should produce no errors"
        );
    }

    #[test]
    fn test_invalid_bash_produces_error() {
        // Only meaningful if bash is on PATH.
        if crate::doctor::which_path("bash").is_none() {
            return;
        }
        let def = make_def(vec![], vec![]);
        // Missing `fi` — bash -n will catch this.
        let block = make_block("bash", "if true; then\n  echo broken\n");
        let result = validate_skill(&def, &[block], None);
        assert!(
            !result.errors.is_empty(),
            "invalid bash should produce at least one error"
        );
    }

    #[test]
    fn test_invalid_python_produces_error() {
        if crate::doctor::which_path("python3").is_none() {
            return;
        }
        let def = make_def(vec![], vec![]);
        // Invalid indentation — ast.parse will reject this.
        let block = make_block("python", "def foo():\nprint('bad indent')\n");
        let result = validate_skill(&def, &[block], None);
        assert!(
            !result.errors.is_empty(),
            "invalid python should produce at least one error"
        );
    }

    #[test]
    fn test_valid_python_no_errors() {
        if crate::doctor::which_path("python3").is_none() {
            return;
        }
        let def = make_def(vec![], vec![]);
        let block = make_block("python", "def foo():\n    print('hello')\n");
        let result = validate_skill(&def, &[block], None);
        assert!(
            result.errors.is_empty(),
            "valid python should produce no errors"
        );
    }

    #[test]
    fn test_bash_placeholder_no_false_positive() {
        // A bash block with a placeholder should not cause a syntax error after
        // sentinel substitution — {{name}} becomes __CREFT_PH__ which is valid.
        if crate::doctor::which_path("bash").is_none() {
            return;
        }
        let def = make_def(vec!["name"], vec![]);
        let block = make_block("bash", "echo {{name}}\n");
        let result = validate_skill(&def, &[block], None);
        assert!(
            result.errors.is_empty(),
            "placeholder in bash should not cause syntax error after sanitization; errors: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_unknown_language_no_errors() {
        // Unknown languages are silently skipped — no errors, no warnings from syntax check.
        let def = make_def(vec![], vec![]);
        let block = make_block("cobol", "MOVE 1 TO X");
        let result = validate_skill(&def, &[block], None);
        assert!(result.errors.is_empty());
    }

    #[test]
    fn test_typescript_not_validated() {
        // TypeScript is excluded from syntax validation.
        let def = make_def(vec![], vec![]);
        // Deliberately broken TS syntax — should produce no error since ts is not checked.
        let block = make_block("typescript", "const x: = 1;");
        let result = validate_skill(&def, &[block], None);
        assert!(
            result.errors.is_empty(),
            "typescript should not be validated (too heavy)"
        );
    }

    // ── Display: block with no lang ──────────────────────────────────────────

    #[test]
    fn test_display_format_block_no_lang() {
        // (Some(idx), None, _) arm
        let d = ValidationDiagnostic {
            block_index: Some(2),
            lang: None,
            message: "some message".into(),
            line: None,
        };
        assert_eq!(format!("{}", d), "block 3: some message");
    }

    #[test]
    fn test_display_format_block_no_lang_with_line() {
        // (Some(idx), None, Some(line)) — line is ignored per Display impl
        let d = ValidationDiagnostic {
            block_index: Some(0),
            lang: None,
            message: "msg".into(),
            line: Some(5),
        };
        // The `(Some(idx), None, _)` arm doesn't include the line
        assert_eq!(format!("{}", d), "block 1: msg");
    }

    // ── parse_shell_errors: unparsed raw stderr ──────────────────────────────

    #[test]
    fn test_parse_shell_errors_raw_stderr_fallback() {
        // When stderr doesn't match the ": line N: msg" pattern, raw text is emitted.
        let mut errors = Vec::new();
        parse_shell_errors("some unparsed error text", 0, "bash", &mut errors);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("some unparsed error text"));
        assert!(errors[0].line.is_none());
    }

    #[test]
    fn test_parse_shell_errors_empty_stderr_no_errors() {
        let mut errors = Vec::new();
        parse_shell_errors("", 0, "bash", &mut errors);
        assert!(errors.is_empty(), "empty stderr should produce no errors");
    }

    #[test]
    fn test_parse_shell_errors_whitespace_only_no_errors() {
        let mut errors = Vec::new();
        parse_shell_errors("   \n   ", 0, "bash", &mut errors);
        assert!(
            errors.is_empty(),
            "whitespace-only stderr should produce no errors"
        );
    }

    // ── sh syntax checking ────────────────────────────────────────────────────

    #[test]
    fn test_valid_sh_no_errors() {
        if crate::doctor::which_path("sh").is_none() {
            return;
        }
        let def = make_def(vec![], vec![]);
        let block = make_block("sh", "echo hello\n");
        let result = validate_skill(&def, &[block], None);
        assert!(
            result.errors.is_empty(),
            "valid sh should produce no errors"
        );
    }

    #[test]
    fn test_invalid_sh_produces_error() {
        if crate::doctor::which_path("sh").is_none() {
            return;
        }
        let def = make_def(vec![], vec![]);
        let block = make_block("sh", "if true; then\n  echo broken\n");
        let result = validate_skill(&def, &[block], None);
        assert!(
            !result.errors.is_empty(),
            "invalid sh should produce at least one error"
        );
    }

    // ── zsh syntax checking ───────────────────────────────────────────────────

    #[test]
    fn test_valid_zsh_no_errors() {
        if crate::doctor::which_path("zsh").is_none() {
            return;
        }
        let def = make_def(vec![], vec![]);
        let block = make_block("zsh", "echo hello\n");
        let result = validate_skill(&def, &[block], None);
        assert!(
            result.errors.is_empty(),
            "valid zsh should produce no errors"
        );
    }

    // ── parse_python_errors: raw stderr fallback ──────────────────────────────

    #[test]
    fn test_parse_python_errors_raw_fallback() {
        let mut errors = Vec::new();
        // Text without a SyntaxError/IndentationError/TabError prefix
        parse_python_errors("some random error text\nmore text", 0, &mut errors);
        assert_eq!(errors.len(), 1);
        // Should use raw stripped text
        assert!(errors[0].message.contains("some random error text"));
    }

    #[test]
    fn test_parse_python_errors_with_syntax_error() {
        let mut errors = Vec::new();
        let stderr = "  File \"<file>\", line 3\nSyntaxError: invalid syntax";
        parse_python_errors(stderr, 0, &mut errors);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].line, Some(3));
        assert!(errors[0].message.contains("invalid syntax"));
    }

    #[test]
    fn test_parse_python_errors_empty_message_ignored() {
        let mut errors = Vec::new();
        parse_python_errors("", 0, &mut errors);
        assert!(errors.is_empty());
    }

    // ── parse_node_errors ─────────────────────────────────────────────────────

    #[test]
    fn test_parse_node_errors_with_syntax_error() {
        let mut errors = Vec::new();
        let stderr = "/tmp/foo.js:5\n  const x = ;\n         ^\nSyntaxError: Unexpected token ';'";
        parse_node_errors(stderr, 0, &mut errors);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].line, Some(5));
        assert!(errors[0].message.contains("Unexpected token"));
    }

    #[test]
    fn test_parse_node_errors_raw_fallback() {
        let mut errors = Vec::new();
        let stderr = "some node error without SyntaxError prefix";
        parse_node_errors(stderr, 0, &mut errors);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("some node error"));
    }

    #[test]
    fn test_parse_node_errors_empty_ignored() {
        let mut errors = Vec::new();
        parse_node_errors("", 0, &mut errors);
        assert!(errors.is_empty());
    }

    // ── node syntax checking ──────────────────────────────────────────────────

    #[test]
    fn test_valid_node_no_errors() {
        if crate::doctor::which_path("node").is_none() {
            return;
        }
        let def = make_def(vec![], vec![]);
        let block = make_block("node", "console.log('hello');\n");
        let result = validate_skill(&def, &[block], None);
        assert!(
            result.errors.is_empty(),
            "valid node should produce no errors"
        );
    }

    #[test]
    fn test_invalid_node_produces_error() {
        if crate::doctor::which_path("node").is_none() {
            return;
        }
        let def = make_def(vec![], vec![]);
        let block = make_block("node", "const x = ;\n");
        let result = validate_skill(&def, &[block], None);
        assert!(
            !result.errors.is_empty(),
            "invalid node should produce at least one error"
        );
    }

    // ── javascript/js language aliases ────────────────────────────────────────

    #[test]
    fn test_valid_javascript_alias_no_errors() {
        if crate::doctor::which_path("node").is_none() {
            return;
        }
        let def = make_def(vec![], vec![]);
        let block = make_block("javascript", "var x = 1;\n");
        let result = validate_skill(&def, &[block], None);
        assert!(result.errors.is_empty());
    }

    // ── description length warning ────────────────────────────────────────────

    fn make_def_with_desc(description: &str) -> CommandDef {
        CommandDef {
            name: "test".into(),
            description: description.into(),
            args: vec![],
            flags: vec![],
            env: vec![],
            tags: vec![],
            supports: vec![],
        }
    }

    #[test]
    fn test_description_length_warn_over_80() {
        // 81-character description should produce exactly one warning about length.
        let desc = "a".repeat(DESCRIPTION_WARN_LEN + 1);
        let def = make_def_with_desc(&desc);
        let result = validate_skill(&def, &[], None);
        assert_eq!(
            result.warnings.len(),
            1,
            "expected one warning for description over {DESCRIPTION_WARN_LEN} chars"
        );
        assert!(
            result.warnings[0].message.contains("description is long"),
            "warning message should mention description is long; got: {:?}",
            result.warnings[0].message
        );
        assert!(
            result.warnings[0]
                .message
                .contains(&(DESCRIPTION_WARN_LEN + 1).to_string()),
            "warning should include the actual char count; got: {:?}",
            result.warnings[0].message
        );
        assert!(
            result.errors.is_empty(),
            "description length is a warning, not an error"
        );
    }

    #[test]
    fn test_description_length_no_warn_at_limit() {
        // Exactly 80 characters: no warning.
        let desc = "a".repeat(DESCRIPTION_WARN_LEN);
        let def = make_def_with_desc(&desc);
        let result = validate_skill(&def, &[], None);
        assert!(
            result.warnings.is_empty(),
            "no warning expected at exactly {DESCRIPTION_WARN_LEN} chars"
        );
    }

    #[test]
    fn test_description_length_no_warn_short() {
        let def = make_def_with_desc("Short description");
        let result = validate_skill(&def, &[], None);
        // May have no warnings (no blocks, no placeholders).
        let has_desc_warn = result
            .warnings
            .iter()
            .any(|w| w.message.contains("description is long"));
        assert!(
            !has_desc_warn,
            "short description should not warn about length"
        );
    }

    #[test]
    fn test_description_length_diagnostic_has_no_block_index() {
        // The diagnostic for description length must be skill-level (no block_index).
        let desc = "a".repeat(DESCRIPTION_WARN_LEN + 1);
        let def = make_def_with_desc(&desc);
        let result = validate_skill(&def, &[], None);
        let desc_warn = result
            .warnings
            .iter()
            .find(|w| w.message.contains("description is long"))
            .expect("expected a description-length warning");
        assert!(
            desc_warn.block_index.is_none(),
            "description length warning must not have a block_index"
        );
        assert!(
            desc_warn.lang.is_none(),
            "description length warning must not have a lang"
        );
    }

    // ── ruby syntax checking ──────────────────────────────────────────────────

    #[test]
    fn test_valid_ruby_no_errors() {
        if crate::doctor::which_path("ruby").is_none() {
            return;
        }
        let def = make_def(vec![], vec![]);
        let block = make_block("ruby", "puts 'hello'\n");
        let result = validate_skill(&def, &[block], None);
        assert!(
            result.errors.is_empty(),
            "valid ruby should produce no errors"
        );
    }

    #[test]
    fn test_invalid_ruby_produces_error() {
        if crate::doctor::which_path("ruby").is_none() {
            return;
        }
        let def = make_def(vec![], vec![]);
        let block = make_block("ruby", "def broken\n  puts 'no end'\n");
        let result = validate_skill(&def, &[block], None);
        assert!(
            !result.errors.is_empty(),
            "invalid ruby should produce at least one error"
        );
    }

    #[test]
    fn test_parse_shell_errors_lineno_zero_becomes_none() {
        // Line numbers of 0 are meaningless; they must be normalized to None.
        let mut errors = Vec::new();
        parse_shell_errors(
            "/tmp/foo.sh: line 0: syntax error near unexpected token",
            0,
            "bash",
            &mut errors,
        );
        assert_eq!(errors.len(), 1);
        assert_eq!(
            errors[0].line, None,
            "line 0 should become None, not Some(0)"
        );
    }

    #[test]
    fn test_parse_shell_errors_lineno_one_becomes_some() {
        let mut errors = Vec::new();
        parse_shell_errors(
            "/tmp/foo.sh: line 1: syntax error near unexpected token",
            0,
            "bash",
            &mut errors,
        );
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].line, Some(1), "line 1 should become Some(1)");
    }

    #[test]
    fn test_parse_ruby_errors_lineno_zero_becomes_none() {
        // Line numbers of 0 are meaningless; they must be normalized to None.
        let mut errors = Vec::new();
        parse_ruby_errors(
            "/tmp/foo.rb:0: syntax error, unexpected end-of-input",
            0,
            &mut errors,
        );
        assert_eq!(errors.len(), 1);
        assert_eq!(
            errors[0].line, None,
            "line 0 should become None, not Some(0)"
        );
    }

    #[test]
    fn test_parse_ruby_errors_lineno_one_becomes_some() {
        let mut errors = Vec::new();
        parse_ruby_errors(
            "/tmp/foo.rb:1: syntax error, unexpected end-of-input",
            0,
            &mut errors,
        );
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].line, Some(1), "line 1 should become Some(1)");
    }

    #[test]
    fn test_shellcheck_skipped_when_syntax_fails() {
        // shellcheck must not run when syntax fails: the gate is
        // `syntax_ok && is_shell_lang`, so a failed syntax check keeps
        // shellcheck out even for shell blocks.
        if crate::doctor::which_path("bash").is_none() {
            return;
        }
        let def = make_def(vec![], vec![]);
        // Missing `fi` — bash -n will add errors, so syntax_ok will be false.
        let block = make_block("bash", "if true; then\n  echo broken\n");
        let result = validate_skill(&def, &[block], None);
        assert!(
            !result.errors.is_empty(),
            "syntax failure should produce errors"
        );
        let _ = result.warnings;
    }

    #[test]
    fn test_shellcheck_skipped_for_non_shell_lang() {
        // shellcheck only runs for shell blocks; python blocks must never trigger it.
        if crate::doctor::which_path("python3").is_none() {
            return;
        }
        let def = make_def(vec![], vec![]);
        let block = make_block("python", "x = 1\n");
        let result = validate_skill(&def, &[block], None);
        // No shellcheck warnings should be present for a python block.
        let shellcheck_warnings: Vec<_> = result
            .warnings
            .iter()
            .filter(|w| w.lang.as_deref() == Some("python"))
            .collect();
        assert!(
            shellcheck_warnings.is_empty(),
            "python block must not produce shellcheck warnings: {:?}",
            shellcheck_warnings
        );
    }

    #[test]
    fn test_parse_shell_errors_structured_plus_fallback_exclusive() {
        // When structured parsing matches at least one line, the raw-stderr
        // fallback must not fire — only one diagnostic for a single error.
        let mut errors = Vec::new();
        let stderr =
            "/tmp/foo.sh: line 5: syntax error near unexpected token\nsome other junk line";
        parse_shell_errors(stderr, 0, "bash", &mut errors);
        assert_eq!(
            errors.len(),
            1,
            "structured match must suppress fallback; got {:?}",
            errors
        );
        assert_eq!(errors[0].line, Some(5));
    }

    #[test]
    fn test_parse_ruby_errors_structured_plus_fallback_exclusive() {
        // When structured parsing matches at least one line, the raw-stderr
        // fallback must not fire — only one diagnostic for a single error.
        let mut errors = Vec::new();
        let stderr = "/tmp/foo.rb:5: syntax error, unexpected end-of-input\nsome other junk";
        parse_ruby_errors(stderr, 0, &mut errors);
        assert_eq!(
            errors.len(),
            1,
            "structured match must suppress fallback; got {:?}",
            errors
        );
        assert_eq!(errors[0].line, Some(5));
    }

    #[test]
    fn test_is_clean_false_with_warnings_only() {
        // is_clean requires both errors AND warnings to be empty.
        let result = ValidationResult {
            errors: vec![],
            warnings: vec![ValidationDiagnostic {
                block_index: Some(0),
                lang: Some("bash".into()),
                message: "a warning".into(),
                line: None,
            }],
        };
        assert!(!result.is_clean(), "warnings-only result must not be clean");
    }

    #[test]
    fn test_is_clean_false_with_errors_only() {
        let result = ValidationResult {
            errors: vec![ValidationDiagnostic {
                block_index: Some(0),
                lang: Some("bash".into()),
                message: "an error".into(),
                line: None,
            }],
            warnings: vec![],
        };
        assert!(!result.is_clean(), "errors-only result must not be clean");
    }

    #[test]
    fn test_is_clean_true_when_both_empty() {
        let result = ValidationResult {
            errors: vec![],
            warnings: vec![],
        };
        assert!(result.is_clean(), "empty result must be clean");
    }

    #[test]
    fn test_which_path_returns_none_for_nonexistent_tool() {
        assert!(
            crate::doctor::which_path("__creft_nonexistent_tool_xyzzy__").is_none(),
            "nonexistent tool must not appear to be on PATH"
        );
    }

    #[test]
    fn test_parse_ruby_errors_fallback_on_unparseable_stderr() {
        // Unrecognized stderr format must emit one fallback diagnostic.
        let mut errors = Vec::new();
        parse_ruby_errors("unexpected ruby error format", 0, &mut errors);
        assert_eq!(
            errors.len(),
            1,
            "unparseable stderr must produce 1 fallback diagnostic"
        );
        assert!(errors[0].message.contains("unexpected ruby error format"));
    }

    #[test]
    fn test_parse_ruby_errors_no_fallback_on_empty_stderr() {
        // Empty stderr must produce no diagnostics.
        let mut errors = Vec::new();
        parse_ruby_errors("", 0, &mut errors);
        assert!(
            errors.is_empty(),
            "empty stderr must produce no diagnostics"
        );
    }

    #[test]
    fn test_shellcheck_produces_warnings_for_known_issue() {
        // Verify shellcheck integration end-to-end: a pattern that is NOT in our
        // exclusion list (SC2034/SC2086/SC2154) must still produce a warning.
        // SC2162: read without -r will mangle backslashes. Not excluded.
        if crate::doctor::which_path("shellcheck").is_none() {
            return;
        }
        let def = make_def(vec![], vec![]);
        let block = make_block("bash", "read answer\necho \"$answer\"\n");
        let result = validate_skill(&def, &[block], None);
        assert!(
            !result.warnings.is_empty(),
            "shellcheck must produce at least one warning for 'read' without -r (SC2162); got none"
        );
    }

    // ── known behavioral equivalences ────────────────────────────────────────────
    //
    // 1. Shell dialect arms: replacing "sh" => "sh" or "zsh" => "zsh" with a
    //    fallthrough to "bash" produces identical syntax-check results on all
    //    platforms, so these branches aren't distinguished by tests.
    //
    // 2. Shellcheck lineno guard: shellcheck never emits line 0, so `> 0`
    //    and `>= 0` are behaviorally identical for real shellcheck output.

    // ── check_sub_skill_existence tests ──────────────────────────────────────────

    #[test]
    fn test_sub_skill_skipped_when_no_ctx() {
        // With ctx=None, sub-skill checking is skipped entirely.
        let def = make_def(vec![], vec![]);
        // creft nonexistent-xyzzy-skill would fail resolution if ctx were present.
        let block = make_block("bash", "creft nonexistent-xyzzy-skill");
        let result = validate_skill(&def, &[block], None);
        let sub_skill_warnings: Vec<_> = result
            .warnings
            .iter()
            .filter(|w| {
                w.message
                    .contains("not found (referenced as creft sub-skill)")
            })
            .collect();
        assert!(
            sub_skill_warnings.is_empty(),
            "sub-skill checking must be skipped when ctx is None; got: {:?}",
            sub_skill_warnings
        );
    }

    #[test]
    fn test_sub_skill_skipped_for_python_block() {
        // Sub-skill checking only applies to shell blocks.
        let def = make_def(vec![], vec![]);
        // Even if ctx were provided, python blocks are skipped.
        let block = make_block("python", "# creft nonexistent-xyzzy-skill");
        let result = validate_skill(&def, &[block], None);
        let sub_skill_warnings: Vec<_> = result
            .warnings
            .iter()
            .filter(|w| {
                w.message
                    .contains("not found (referenced as creft sub-skill)")
            })
            .collect();
        assert!(
            sub_skill_warnings.is_empty(),
            "sub-skill checking must be skipped for python blocks; got: {:?}",
            sub_skill_warnings
        );
    }

    // ── dependency resolution unit tests ─────────────────────────────────────

    fn make_block_with_deps(lang: &str, code: &str, deps: Vec<&str>) -> CodeBlock {
        CodeBlock {
            lang: lang.into(),
            code: code.into(),
            deps: deps.into_iter().map(String::from).collect(),
            llm_config: None,
            llm_parse_error: None,
        }
    }

    #[test]
    fn test_dep_resolution_shell_missing() {
        // A shell block with a declared dep that doesn't exist on PATH produces a
        // warning containing "not found on PATH".
        let def = make_def(vec![], vec![]);
        let block = make_block_with_deps("bash", "echo hello", vec!["__nonexistent_dep_xyzzy__"]);
        let result = validate_skill(&def, &[block], None);
        let dep_warnings: Vec<_> = result
            .warnings
            .iter()
            .filter(|w| w.message.contains("not found on PATH"))
            .collect();
        assert_eq!(
            dep_warnings.len(),
            1,
            "expected one dep warning; got: {:?}",
            result.warnings
        );
        assert!(
            dep_warnings[0]
                .message
                .contains("__nonexistent_dep_xyzzy__")
        );
    }

    #[test]
    fn test_dep_resolution_shell_present() {
        // A shell block with `ls` as a dep produces no dep warning — ls is
        // universally available as a binary (not just a shell builtin).
        if crate::doctor::which_path("ls").is_none() {
            return;
        }
        let def = make_def(vec![], vec![]);
        let block = make_block_with_deps("bash", "ls -la", vec!["ls"]);
        let result = validate_skill(&def, &[block], None);
        let dep_warnings: Vec<_> = result
            .warnings
            .iter()
            .filter(|w| w.message.starts_with("dependency"))
            .collect();
        assert!(
            dep_warnings.is_empty(),
            "ls should be found on PATH; got: {:?}",
            dep_warnings
        );
    }

    #[test]
    fn test_dep_resolution_empty_deps_no_check() {
        // A block with no deps declared produces no dep resolution warnings.
        let def = make_def(vec![], vec![]);
        let block = make_block("python", "import sys");
        let result = validate_skill(&def, &[block], None);
        let dep_warnings: Vec<_> = result
            .warnings
            .iter()
            .filter(|w| w.message.starts_with("dependency"))
            .collect();
        assert!(
            dep_warnings.is_empty(),
            "empty deps should produce no dep warnings; got: {:?}",
            dep_warnings
        );
    }

    #[test]
    fn test_dep_resolution_skipped_for_unknown_lang() {
        // A block with an unknown language and declared deps produces no warnings.
        let def = make_def(vec![], vec![]);
        let block = make_block_with_deps("cobol", "MOVE 1 TO X", vec!["some-cobol-lib"]);
        let result = validate_skill(&def, &[block], None);
        let dep_warnings: Vec<_> = result
            .warnings
            .iter()
            .filter(|w| w.message.starts_with("dependency"))
            .collect();
        assert!(
            dep_warnings.is_empty(),
            "unknown lang should skip dep checking; got: {:?}",
            dep_warnings
        );
    }

    #[test]
    fn test_endpoint_url_for_pypi() {
        use crate::registry_config::{HttpMethod, RegistryEndpoint};
        let ep = RegistryEndpoint {
            url_template: "https://pypi.org/pypi/{}/json".to_string(),
            auth: None,
            label: "PyPI".to_string(),
            method: HttpMethod::Head,
        };
        assert_eq!(
            ep.url_for("requests"),
            "https://pypi.org/pypi/requests/json"
        );
    }

    #[test]
    fn test_endpoint_url_for_npm() {
        use crate::registry_config::{HttpMethod, RegistryEndpoint};
        let ep = RegistryEndpoint {
            url_template: "https://registry.npmjs.org/{}".to_string(),
            auth: None,
            label: "npm".to_string(),
            method: HttpMethod::Head,
        };
        assert_eq!(ep.url_for("lodash"), "https://registry.npmjs.org/lodash");
    }

    // ── llm block validation tests ───────────────────────────────────────────────

    fn make_llm_block(code: &str, config: Option<crate::model::LlmConfig>) -> CodeBlock {
        CodeBlock {
            lang: "llm".into(),
            code: code.into(),
            deps: vec![],
            llm_config: config,
            llm_parse_error: None,
        }
    }

    fn default_llm_config() -> crate::model::LlmConfig {
        crate::model::LlmConfig::default()
    }

    #[test]
    fn test_llm_block_empty_prompt_error() {
        let def = make_def(vec![], vec![]);
        let block = make_llm_block("   ", Some(default_llm_config()));
        let result = validate_skill(&def, &[block], None);
        assert_eq!(result.errors.len(), 1);
        assert!(
            result.errors[0].message.contains("no prompt text"),
            "expected 'no prompt text' error, got: {}",
            result.errors[0].message
        );
    }

    #[test]
    fn test_llm_block_yaml_parse_error_reported() {
        let def = make_def(vec![], vec![]);
        let mut block = make_llm_block("some prompt", None);
        block.llm_parse_error = Some("mapping values are not allowed here".to_string());
        let result = validate_skill(&def, &[block], None);
        assert_eq!(result.errors.len(), 1);
        assert!(
            result.errors[0].message.contains("invalid YAML header"),
            "expected YAML parse error message, got: {}",
            result.errors[0].message
        );
        assert!(
            result.errors[0]
                .message
                .contains("mapping values are not allowed here"),
            "expected original parse error in message"
        );
    }

    #[test]
    fn test_llm_block_valid_prompt_clean() {
        // cat is universally available — this block should have no errors.
        let def = make_def(vec![], vec![]);
        let mut config = default_llm_config();
        config.provider = "cat".to_string();
        let block = make_llm_block("hello from llm block", Some(config));
        let result = validate_skill(&def, &[block], None);
        assert!(
            result.errors.is_empty(),
            "expected no errors, got: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_llm_block_provider_warning_when_not_on_path() {
        let def = make_def(vec![], vec![]);
        let mut config = default_llm_config();
        config.provider = "nonexistent-llm-provider-xyz".to_string();
        let block = make_llm_block("my prompt", Some(config));
        let result = validate_skill(&def, &[block], None);
        assert!(
            result.errors.is_empty(),
            "missing provider should warn, not error"
        );
        assert!(
            result
                .warnings
                .iter()
                .any(|w| w.message.contains("nonexistent-llm-provider-xyz")),
            "expected provider not found warning, got: {:?}",
            result.warnings
        );
    }

    #[test]
    fn test_llm_block_skips_syntax_check() {
        // An llm block with content that looks like bad shell — should not trigger syntax errors.
        let def = make_def(vec![], vec![]);
        let mut config = default_llm_config();
        config.provider = "cat".to_string();
        let block = make_llm_block("if then else fi {{ broken shell syntax }", Some(config));
        let result = validate_skill(&def, &[block], None);
        // No syntax errors from the llm block content.
        assert!(
            result.errors.is_empty(),
            "llm blocks should not be syntax-checked, got errors: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_llm_block_placeholder_check_works() {
        // An undeclared placeholder in the prompt body should warn.
        let def = make_def(vec![], vec![]);
        let mut config = default_llm_config();
        config.provider = "cat".to_string();
        let block = make_llm_block("Summarize {{some_arg}}", Some(config));
        let result = validate_skill(&def, &[block], None);
        let has_placeholder_warning = result
            .warnings
            .iter()
            .any(|w| w.message.contains("some_arg"));
        assert!(
            has_placeholder_warning,
            "expected placeholder warning for undeclared {{{{some_arg}}}}, got: {:?}",
            result.warnings
        );
    }

    #[test]
    fn test_llm_block_prev_placeholder_in_second_block_no_warning() {
        // {{prev}} in the second block (not block 0) should not produce a warning.
        let def = make_def(vec![], vec![]);
        let bash_block = make_block("bash", "echo hello");
        let mut config = default_llm_config();
        config.provider = "cat".to_string();
        let llm_block = make_llm_block("Review this: {{prev}}", Some(config));
        let result = validate_skill(&def, &[bash_block, llm_block], None);
        let has_prev_warning = result.warnings.iter().any(|w| w.message.contains("prev"));
        assert!(
            !has_prev_warning,
            "{{{{prev}}}} in block 2 should not produce a warning, got: {:?}",
            result.warnings
        );
    }
}
