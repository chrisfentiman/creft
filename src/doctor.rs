use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::LazyLock;

use regex::Regex;

use crate::error::CreftError;
use crate::model::{AppContext, CodeBlock, SkillSource};
use crate::registry;
use crate::store;
use crate::style;

// ── Types ─────────────────────────────────────────────────────────────────────

/// Outcome of a single doctor check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CheckStatus {
    /// Check passed.
    Ok,
    /// Check failed -- a required dependency is missing.
    Fail,
    /// Informational -- not an error, just reporting state.
    Info,
    /// Optional tool missing -- not a failure.
    Optional,
}

/// A single check outcome with a human-readable label.
#[derive(Debug, Clone)]
pub(crate) struct CheckResult {
    /// What was checked (e.g., "bash", "python3", "~/.creft/ writable").
    pub label: String,
    /// Outcome of the check.
    pub status: CheckStatus,
    /// Human-readable detail (e.g., "/usr/bin/bash", "not found", "3 packages installed").
    pub detail: String,
}

/// Complete doctor report for a skill, including recursive sub-skill checks.
///
/// Checks are stored by category to support grouped output rendering.
#[derive(Debug)]
pub(crate) struct DoctorReport {
    /// The skill name being checked.
    pub skill_name: String,
    /// Where the skill was resolved from.
    pub source: String,
    /// Interpreter availability checks (one per unique interpreter, deduplicated).
    pub interpreter_checks: Vec<CheckResult>,
    /// Environment variable checks (required = Fail if missing, optional = Info).
    pub env_checks: Vec<CheckResult>,
    /// Command availability checks per bash block: (block_number, checks).
    pub command_checks: Vec<(usize, Vec<CheckResult>)>,
    /// Sub-skill resolution checks (one per discovered creft invocation).
    pub sub_skill_checks: Vec<CheckResult>,
    /// Dependency tool checks per block: (block_number, checks).
    pub dep_checks: Vec<(usize, Vec<CheckResult>)>,
    /// Miscellaneous informational checks (cycle detection, depth limit, prev usage).
    pub misc_checks: Vec<CheckResult>,
    /// Reports for sub-skills (recursive).
    pub sub_reports: Vec<DoctorReport>,
}

// ── Shell builtins and keyword filter ────────────────────────────────────────

/// Shell builtins and keywords filtered out during command extraction.
///
/// These are never external programs on PATH, so checking for them would
/// produce false positives.
pub(crate) const SHELL_BUILTINS: &[&str] = &[
    "echo", "printf", "read", "export", "set", "unset", "local", "return", "exit", "shift",
    "source", "eval", "exec", "test", "true", "false", "cd", "pwd", "pushd", "popd", "dirs",
    "alias", "unalias", "type", "hash", "wait", "trap", "if", "then", "else", "elif", "fi", "for",
    "while", "until", "do", "done", "case", "esac", "in", "function", "break", "continue",
    "declare", "typeset", "readonly", "let", "getopts", "umask", "ulimit", "bg", "fg", "jobs",
];

// ── Regexes ───────────────────────────────────────────────────────────────────

/// Extracts command names from shell code.
///
/// Catches common invocation patterns: start of line, after pipe/`&&`/`||`,
/// and inside `$()` subshells. Does not catch dynamic invocations
/// (`$CMD arg`, `eval "..."`) or commands inside heredocs.
pub(crate) static COMMAND_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)(?:^|[|&;]\s*|\$\(\s*)([a-zA-Z_][a-zA-Z0-9_.-]*)").unwrap());

/// Matches `creft <words>` invocations in shell code.
///
/// Captures everything after `creft ` up to a pipe, semicolon, newline,
/// or redirect operator.
pub(crate) static CREFT_CALL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)\bcreft\s+([a-zA-Z_][a-zA-Z0-9_ -]*)").unwrap());

// ── PATH lookup ───────────────────────────────────────────────────────────────

/// Look up a command on PATH, returning the resolved path if found.
///
/// On Windows also checks `.exe`, `.cmd`, and `.bat` extensions.
/// On Unix checks the executable bit.
pub(crate) fn which_path(name: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&paths) {
        let candidate = dir.join(name);
        if candidate.exists() && is_executable(&candidate) {
            return Some(candidate);
        }
        // Windows: check with common extensions
        #[cfg(windows)]
        for ext in &[".exe", ".cmd", ".bat"] {
            let with_ext = dir.join(format!("{}{}", name, ext));
            if with_ext.exists() && is_executable(&with_ext) {
                return Some(with_ext);
            }
        }
    }
    None
}

#[cfg(unix)]
fn is_executable(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .map(|m| m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(_path: &std::path::Path) -> bool {
    true // Windows uses file extensions, not permission bits
}

// ── Individual check helpers ──────────────────────────────────────────────────

/// Check for a required interpreter; `bash` and `sh` are `Fail` if missing,
/// everything else is `Optional`.
fn check_interpreter(name: &str) -> CheckResult {
    match which_path(name) {
        Some(p) => CheckResult {
            label: name.to_string(),
            status: CheckStatus::Ok,
            detail: p.to_string_lossy().to_string(),
        },
        None => {
            let (status, detail) = if name == "bash" || name == "sh" {
                (CheckStatus::Fail, "not found on PATH".to_string())
            } else if name == "zsh" {
                (
                    CheckStatus::Optional,
                    "not found on PATH -- not required on non-macOS".to_string(),
                )
            } else if name == "git" {
                (
                    CheckStatus::Optional,
                    "not found on PATH (needed for creft install)".to_string(),
                )
            } else {
                (CheckStatus::Optional, "not found on PATH".to_string())
            };
            CheckResult {
                label: name.to_string(),
                status,
                detail,
            }
        }
    }
}

/// Check for an optional tool with a description of its purpose.
fn check_optional_tool(name: &str, purpose: &str) -> CheckResult {
    match which_path(name) {
        Some(p) => CheckResult {
            label: name.to_string(),
            status: CheckStatus::Ok,
            detail: p.to_string_lossy().to_string(),
        },
        None => CheckResult {
            label: name.to_string(),
            status: CheckStatus::Optional,
            detail: format!("not found ({})", purpose),
        },
    }
}

/// Check if `~/.creft/` exists and is writable.
fn check_global_dir(ctx: &AppContext) -> CheckResult {
    let root = match ctx.global_root() {
        Ok(r) => r,
        Err(_) => {
            return CheckResult {
                label: "~/.creft/".to_string(),
                status: CheckStatus::Fail,
                detail: "home directory not set".to_string(),
            };
        }
    };
    if !root.exists() {
        return CheckResult {
            label: "~/.creft/".to_string(),
            status: CheckStatus::Info,
            detail: "does not exist yet (will be created on first use)".to_string(),
        };
    }

    // Try creating a temp file to confirm writability.
    // Using tempfile::Builder ensures the file is cleaned up even if the
    // process is killed, and avoids race conditions from fixed-name test files.
    match tempfile::Builder::new().tempfile_in(&root) {
        Ok(_tmp) => CheckResult {
            label: "~/.creft/".to_string(),
            status: CheckStatus::Ok,
            detail: "writable".to_string(),
        },
        Err(_) => CheckResult {
            label: "~/.creft/".to_string(),
            status: CheckStatus::Fail,
            detail: "not writable".to_string(),
        },
    }
}

/// Report whether a local `.creft/` directory was found.
fn check_local_dir(ctx: &AppContext) -> CheckResult {
    match ctx.find_local_root() {
        Some(p) => CheckResult {
            label: ".creft/".to_string(),
            status: CheckStatus::Info,
            detail: p.to_string_lossy().to_string(),
        },
        None => CheckResult {
            label: ".creft/".to_string(),
            status: CheckStatus::Info,
            detail: "no local .creft/ found".to_string(),
        },
    }
}

/// Check installed packages from both scopes. Returns one result per broken
/// manifest plus a summary result.
fn check_packages(ctx: &AppContext) -> Vec<CheckResult> {
    use crate::model::Scope;

    let mut results: Vec<CheckResult> = Vec::new();

    let mut total = 0usize;
    let mut failures = 0usize;

    for scope in &[Scope::Global, Scope::Local] {
        match registry::list_packages_in(ctx, *scope) {
            Ok(pkgs) => {
                total += pkgs.len();
            }
            Err(e) => {
                failures += 1;
                results.push(CheckResult {
                    label: format!("packages ({})", scope_name(*scope)),
                    status: CheckStatus::Fail,
                    detail: format!("could not list packages: {}", e),
                });
            }
        }
    }

    // list_packages_in silently skips unreadable manifests; scan dirs directly
    // to surface broken ones as explicit Fail results.
    for scope in &[Scope::Global, Scope::Local] {
        let base = match ctx.packages_dir_for(*scope) {
            Ok(p) => p,
            Err(_) => continue,
        };
        if !base.exists() {
            continue;
        }
        if let Ok(entries) = std::fs::read_dir(&base) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                let manifest_path = path.join("creft.yaml");
                if !manifest_path.exists() {
                    continue;
                }
                match std::fs::read_to_string(&manifest_path) {
                    Ok(content) => {
                        if let Err(e) =
                            serde_yaml_ng::from_str::<registry::PackageManifest>(&content)
                        {
                            failures += 1;
                            results.push(CheckResult {
                                label: format!(
                                    "package {}",
                                    path.file_name()
                                        .map(|n| n.to_string_lossy().to_string())
                                        .unwrap_or_else(|| "unknown".to_string())
                                ),
                                status: CheckStatus::Fail,
                                detail: format!("invalid manifest: {}", e),
                            });
                        }
                    }
                    Err(e) => {
                        failures += 1;
                        results.push(CheckResult {
                            label: format!(
                                "package {}",
                                path.file_name()
                                    .map(|n| n.to_string_lossy().to_string())
                                    .unwrap_or_else(|| "unknown".to_string())
                            ),
                            status: CheckStatus::Fail,
                            detail: format!("could not read manifest: {}", e),
                        });
                    }
                }
            }
        }
    }

    let summary_status = if failures > 0 {
        CheckStatus::Fail
    } else {
        CheckStatus::Info
    };
    let summary_detail = if total == 0 {
        "no packages installed".to_string()
    } else if failures == 0 {
        format!("{} installed, all manifests valid", total)
    } else {
        format!("{} installed, {} with broken manifests", total, failures)
    };
    results.push(CheckResult {
        label: "packages".to_string(),
        status: summary_status,
        detail: summary_detail,
    });

    results
}

fn scope_name(scope: crate::model::Scope) -> &'static str {
    match scope {
        crate::model::Scope::Local => "local",
        crate::model::Scope::Global => "global",
    }
}

/// Check for flat files with spaces in filename that should be directories.
///
/// Scans the commands directory (both local and global) for `.md` files
/// whose names contain spaces. These files are unreachable by the resolver
/// and should be migrated to directory structure.
pub(crate) fn check_flat_files(ctx: &AppContext) -> Vec<CheckResult> {
    use crate::model::Scope;

    let mut results: Vec<CheckResult> = Vec::new();

    let scopes: Vec<Scope> = if ctx.find_local_root().is_some() {
        vec![Scope::Local, Scope::Global]
    } else {
        vec![Scope::Global]
    };

    for scope in scopes {
        let commands_dir = match ctx.commands_dir_for(scope) {
            Ok(p) => p,
            Err(_) => continue,
        };
        if !commands_dir.exists() {
            continue;
        }
        let entries = match std::fs::read_dir(&commands_dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let stem = match path.file_stem().and_then(|s| s.to_str()) {
                Some(s) => s.to_string(),
                None => continue,
            };
            if stem.contains(' ') {
                // Convert "a b c" to the expected directory path "a/b/c.md".
                let dir_path: String = {
                    let parts: Vec<&str> = stem.split(' ').collect();
                    format!(
                        "{}/{}.md",
                        parts[..parts.len() - 1].join("/"),
                        parts.last().unwrap_or(&stem.as_str())
                    )
                };
                let flat_name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| format!("{stem}.md"));
                results.push(CheckResult {
                    label: "flat file".to_string(),
                    status: CheckStatus::Fail,
                    detail: format!(
                        "\"{flat_name}\" should be {dir_path} -- run the command once to auto-migrate"
                    ),
                });
            }
        }
    }

    results
}

// ── Global check ──────────────────────────────────────────────────────────────

/// Run all global health checks and return the results.
pub(crate) fn run_global_check(ctx: &AppContext) -> Vec<CheckResult> {
    let mut results = Vec::new();

    results.push(check_interpreter("bash"));
    results.push(check_interpreter("sh"));
    results.push(check_interpreter("zsh"));

    // Falls back to `python` alias when `python3` is not on PATH.
    let python_result = match which_path("python3") {
        Some(p) => CheckResult {
            label: "python3".to_string(),
            status: CheckStatus::Ok,
            detail: p.to_string_lossy().to_string(),
        },
        None => match which_path("python") {
            Some(p) => CheckResult {
                label: "python3".to_string(),
                status: CheckStatus::Ok,
                detail: format!("{} (via python alias)", p.to_string_lossy()),
            },
            None => CheckResult {
                label: "python3".to_string(),
                status: CheckStatus::Optional,
                detail: "not found on PATH".to_string(),
            },
        },
    };
    results.push(python_result);

    results.push(check_interpreter("node"));
    results.push(check_interpreter("ruby"));
    results.push(check_interpreter("git"));

    results.push(check_optional_tool("shellcheck", "used for shell linting"));
    results.push(check_optional_tool("uv", "needed for python deps"));
    results.push(check_optional_tool("npx", "needed for node deps"));

    results.push(check_global_dir(ctx));
    results.push(check_local_dir(ctx));
    results.extend(check_packages(ctx));
    results.extend(check_flat_files(ctx));

    results
}

// ── Skill check ───────────────────────────────────────────────────────────────

/// Run a health check for a single skill, recursively checking sub-skills.
///
/// Checks interpreters, env vars, commands, dependencies, and creft sub-skill
/// references. Detects cycles and limits recursion to 10 levels.
pub(crate) fn run_skill_check(
    ctx: &AppContext,
    name: &str,
    source: &SkillSource,
) -> Result<DoctorReport, CreftError> {
    let mut visited = HashSet::new();
    visited.insert(name.to_string());
    run_skill_check_inner(ctx, name, source, &mut visited, 0)
}

fn run_skill_check_inner(
    ctx: &AppContext,
    name: &str,
    source: &SkillSource,
    visited: &mut HashSet<String>,
    depth: usize,
) -> Result<DoctorReport, CreftError> {
    const MAX_DEPTH: usize = 10;

    let cmd = store::load_from(ctx, name, source)?;
    let source_label = describe_source(source);

    let mut interpreter_checks: Vec<CheckResult> = Vec::new();
    let mut env_checks: Vec<CheckResult> = Vec::new();
    let mut command_checks: Vec<(usize, Vec<CheckResult>)> = Vec::new();
    let mut sub_skill_checks: Vec<CheckResult> = Vec::new();
    let mut dep_checks: Vec<(usize, Vec<CheckResult>)> = Vec::new();
    let mut misc_checks: Vec<CheckResult> = Vec::new();
    let mut sub_reports: Vec<DoctorReport> = Vec::new();

    if cmd.blocks.is_empty() {
        misc_checks.push(CheckResult {
            label: "code blocks".to_string(),
            status: CheckStatus::Info,
            detail: "no code blocks".to_string(),
        });
        return Ok(DoctorReport {
            skill_name: name.to_string(),
            source: source_label,
            interpreter_checks,
            env_checks,
            command_checks,
            sub_skill_checks,
            dep_checks,
            misc_checks,
            sub_reports,
        });
    }

    let mut seen_interpreters: HashSet<String> = HashSet::new();
    let mut creft_sub_skills: Vec<String> = Vec::new();

    for (idx, block) in cmd.blocks.iter().enumerate() {
        let block_num = idx + 1;

        if let Some(interp) = interpreter_for_lang(&block.lang)
            && seen_interpreters.insert(interp.to_string())
        {
            interpreter_checks.push(check_block_interpreter(interp));
        }

        if !block.deps.is_empty() {
            let results = check_block_deps(block, block_num);
            if !results.is_empty() {
                dep_checks.push((block_num, results));
            }
        }

        if is_shell_lang(&block.lang) {
            let cmds = extract_commands(&block.code);
            if !cmds.is_empty() {
                let block_cmd_checks: Vec<CheckResult> = cmds
                    .into_iter()
                    .map(|cmd_name| match which_path(&cmd_name) {
                        Some(p) => CheckResult {
                            label: cmd_name,
                            status: CheckStatus::Ok,
                            detail: p.to_string_lossy().to_string(),
                        },
                        None => CheckResult {
                            label: cmd_name,
                            status: CheckStatus::Fail,
                            detail: "not found".to_string(),
                        },
                    })
                    .collect();
                command_checks.push((block_num, block_cmd_checks));
            }

            let calls = extract_creft_calls(&block.code);
            creft_sub_skills.extend(calls);
        }
    }

    for env_var in &cmd.def.env {
        let result = if std::env::var(&env_var.name).is_ok() {
            CheckResult {
                label: env_var.name.clone(),
                status: CheckStatus::Ok,
                detail: "set".to_string(),
            }
        } else if env_var.required {
            CheckResult {
                label: env_var.name.clone(),
                status: CheckStatus::Fail,
                detail: "not set".to_string(),
            }
        } else {
            CheckResult {
                label: env_var.name.clone(),
                status: CheckStatus::Info,
                detail: "optional, not set".to_string(),
            }
        };
        env_checks.push(result);
    }

    let uses_prev = cmd
        .blocks
        .iter()
        .any(|b| b.code.contains("$CREFT_PREV") || b.code.contains("{{prev}}"));
    if uses_prev {
        misc_checks.push(CheckResult {
            label: "pipeline input".to_string(),
            status: CheckStatus::Info,
            detail: "expects pipeline input (uses $CREFT_PREV or {{prev}})".to_string(),
        });
    }

    if depth >= MAX_DEPTH {
        misc_checks.push(CheckResult {
            label: "recursion".to_string(),
            status: CheckStatus::Info,
            detail: "dependency tree exceeds maximum depth (10), stopping".to_string(),
        });
    } else {
        for sub_name in &creft_sub_skills {
            let sub_name = sub_name.trim().to_string();
            if sub_name.is_empty() {
                continue;
            }

            if visited.contains(&sub_name) {
                misc_checks.push(CheckResult {
                    label: format!("sub-skill: {}", sub_name),
                    status: CheckStatus::Info,
                    detail: format!("circular reference detected: {} -> {}", name, sub_name),
                });
                continue;
            }

            let args: Vec<String> = sub_name.split_whitespace().map(String::from).collect();
            match store::resolve_command(ctx, &args) {
                Ok((resolved_name, _, sub_source)) => {
                    sub_skill_checks.push(CheckResult {
                        label: sub_name.clone(),
                        status: CheckStatus::Ok,
                        detail: "resolved".to_string(),
                    });
                    visited.insert(sub_name.clone());
                    match run_skill_check_inner(
                        ctx,
                        &resolved_name,
                        &sub_source,
                        visited,
                        depth + 1,
                    ) {
                        Ok(report) => sub_reports.push(report),
                        Err(e) => {
                            sub_skill_checks.push(CheckResult {
                                label: format!("{} (load)", sub_name),
                                status: CheckStatus::Fail,
                                detail: format!("could not load: {}", e),
                            });
                        }
                    }
                    visited.remove(&sub_name);
                }
                Err(_) => {
                    sub_skill_checks.push(CheckResult {
                        label: sub_name.clone(),
                        status: CheckStatus::Fail,
                        detail: "not found".to_string(),
                    });
                }
            }
        }
    }

    Ok(DoctorReport {
        skill_name: name.to_string(),
        source: source_label,
        interpreter_checks,
        env_checks,
        command_checks,
        sub_skill_checks,
        dep_checks,
        misc_checks,
        sub_reports,
    })
}

// ── Interpreter mapping ───────────────────────────────────────────────────────

/// Map a code block language tag to the interpreter command.
fn interpreter_for_lang(lang: &str) -> Option<&'static str> {
    match lang {
        "bash" => Some("bash"),
        "sh" => Some("sh"),
        "zsh" => Some("zsh"),
        "python" | "python3" => Some("python3"),
        "node" | "js" | "javascript" => Some("node"),
        "typescript" | "ts" => Some("npx"),
        "ruby" | "rb" => Some("ruby"),
        "perl" => Some("perl"),
        _ => None,
    }
}

/// Returns true if `lang` is a shell language that supports command extraction.
pub(crate) fn is_shell_lang(lang: &str) -> bool {
    matches!(lang, "bash" | "sh" | "zsh")
}

fn check_block_interpreter(interp: &str) -> CheckResult {
    match which_path(interp) {
        Some(p) => CheckResult {
            label: interp.to_string(),
            status: CheckStatus::Ok,
            detail: p.to_string_lossy().to_string(),
        },
        None => CheckResult {
            label: interp.to_string(),
            status: CheckStatus::Fail,
            detail: "not found on PATH".to_string(),
        },
    }
}

fn check_block_deps(block: &CodeBlock, _block_idx: usize) -> Vec<CheckResult> {
    let mut results = Vec::new();
    let lang = block.lang.as_str();

    if matches!(lang, "python" | "python3") {
        if let Some(p) = which_path("uv") {
            results.push(CheckResult {
                label: "uv".to_string(),
                status: CheckStatus::Ok,
                detail: p.to_string_lossy().to_string(),
            });
        } else {
            results.push(CheckResult {
                label: "uv".to_string(),
                status: CheckStatus::Fail,
                detail: "not found (needed for python deps)".to_string(),
            });
        }
    } else if matches!(lang, "node" | "js" | "javascript") {
        if let Some(p) = which_path("npx") {
            results.push(CheckResult {
                label: "npx".to_string(),
                status: CheckStatus::Ok,
                detail: p.to_string_lossy().to_string(),
            });
        } else {
            results.push(CheckResult {
                label: "npx".to_string(),
                status: CheckStatus::Fail,
                detail: "not found (needed for node deps)".to_string(),
            });
        }
    } else if is_shell_lang(lang) {
        for dep in &block.deps {
            let result = match which_path(dep) {
                Some(p) => CheckResult {
                    label: dep.clone(),
                    status: CheckStatus::Ok,
                    detail: p.to_string_lossy().to_string(),
                },
                None => CheckResult {
                    label: dep.clone(),
                    status: CheckStatus::Fail,
                    detail: "not found".to_string(),
                },
            };
            results.push(result);
        }
    }

    results
}

// ── Command extraction ────────────────────────────────────────────────────────

/// Extract external command names from shell code.
///
/// Pre-filters comment lines, then applies regex to extract command names,
/// filtering out shell builtins and keywords.
pub(crate) fn extract_commands(code: &str) -> Vec<String> {
    // Strip comment lines before applying the regex so `# curl` is not extracted.
    let filtered: String = code
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n");

    let mut commands = Vec::new();
    for cap in COMMAND_RE.captures_iter(&filtered) {
        if let Some(m) = cap.get(1) {
            let cmd_name = m.as_str();
            if !SHELL_BUILTINS.contains(&cmd_name) && !cmd_name.is_empty() {
                commands.push(cmd_name.to_string());
            }
        }
    }

    let mut seen = HashSet::new();
    commands.retain(|c| seen.insert(c.clone()));
    commands
}

/// Extract creft sub-skill invocations from shell code.
///
/// Returns skill names with flags stripped. Filters out creft builtins.
pub(crate) fn extract_creft_calls(code: &str) -> Vec<String> {
    let mut calls = Vec::new();

    for cap in CREFT_CALL_RE.captures_iter(code) {
        if let Some(m) = cap.get(1) {
            let raw = m.as_str().trim();
            // Flags start with '-'; stop at the first one to isolate the skill name.
            let parts: Vec<&str> = raw
                .split_whitespace()
                .take_while(|w| !w.starts_with('-'))
                .collect();
            if parts.is_empty() {
                continue;
            }
            let skill_name = parts.join(" ");
            if store::is_reserved(parts[0]) {
                continue;
            }
            calls.push(skill_name);
        }
    }

    let mut seen = HashSet::new();
    calls.retain(|c| seen.insert(c.clone()));
    calls
}

// ── Rendering ─────────────────────────────────────────────────────────────────

fn status_marker(status: CheckStatus) -> &'static str {
    match status {
        CheckStatus::Ok => "[ok]",
        CheckStatus::Fail => "[!!]",
        CheckStatus::Info => "[ii]",
        CheckStatus::Optional => "[--]",
    }
}

/// Render global check results to stderr.
pub(crate) fn render_global(results: &[CheckResult]) {
    let ansi = style::use_ansi();
    eprintln!("{}", style::bold("Environment Health Check", ansi));
    eprintln!();
    for r in results {
        eprintln!("  {} {} ({})", status_marker(r.status), r.label, r.detail);
    }
    eprintln!();
    eprintln!("See 'creft doctor <skill>' to check a specific skill.");
}

/// Render a skill doctor report to stderr.
pub(crate) fn render_skill(report: &DoctorReport) {
    let ansi = style::use_ansi();
    let header = format!("Skill Health: {} ({})", report.skill_name, report.source);
    eprintln!("{}", style::bold(&header, ansi));
    eprintln!();
    render_skill_indented(report, 0);

    let total_fails = count_failures_in_report(report);
    if total_fails > 0 {
        eprintln!();
        eprintln!(
            "  {} issue{} found",
            total_fails,
            if total_fails == 1 { "" } else { "s" }
        );
    }
}

fn render_skill_indented(report: &DoctorReport, indent: usize) {
    let pad = " ".repeat(indent);
    let inner = format!("{}  ", pad);
    let item = format!("{}    ", pad);

    if !report.interpreter_checks.is_empty() {
        eprintln!("{}interpreters:", inner);
        for r in &report.interpreter_checks {
            eprintln!(
                "{}{} {} ({})",
                item,
                status_marker(r.status),
                r.label,
                r.detail
            );
        }
    }

    if !report.env_checks.is_empty() {
        eprintln!("{}env vars:", inner);
        for r in &report.env_checks {
            eprintln!(
                "{}{} {} ({})",
                item,
                status_marker(r.status),
                r.label,
                r.detail
            );
        }
    }

    for (block_num, cmds) in &report.command_checks {
        eprintln!("{}commands (bash block {}):", inner, block_num);
        for r in cmds {
            eprintln!(
                "{}{} {} ({})",
                item,
                status_marker(r.status),
                r.label,
                r.detail
            );
        }
    }

    if !report.sub_skill_checks.is_empty() {
        eprintln!("{}sub-skills:", inner);
        for r in &report.sub_skill_checks {
            eprintln!(
                "{}{} {} ({})",
                item,
                status_marker(r.status),
                r.label,
                r.detail
            );
        }
    }

    let multi_dep_blocks = report.dep_checks.len() > 1;
    for (block_num, deps) in &report.dep_checks {
        if multi_dep_blocks {
            eprintln!("{}deps (block {}):", inner, block_num);
        } else {
            eprintln!("{}deps:", inner);
        }
        for r in deps {
            eprintln!(
                "{}{} {} ({})",
                item,
                status_marker(r.status),
                r.label,
                r.detail
            );
        }
    }

    for r in &report.misc_checks {
        eprintln!(
            "{}{} {} ({})",
            inner,
            status_marker(r.status),
            r.label,
            r.detail
        );
    }

    for sub in &report.sub_reports {
        eprintln!();
        eprintln!("{}  sub-skill: {}", pad, sub.skill_name);
        render_skill_indented(sub, indent + 2);
    }
}

fn count_failures_in_report(report: &DoctorReport) -> usize {
    let interp = report
        .interpreter_checks
        .iter()
        .filter(|c| c.status == CheckStatus::Fail)
        .count();
    let env = report
        .env_checks
        .iter()
        .filter(|c| c.status == CheckStatus::Fail)
        .count();
    let cmds: usize = report
        .command_checks
        .iter()
        .flat_map(|(_, v)| v.iter())
        .filter(|c| c.status == CheckStatus::Fail)
        .count();
    let sub_skills = report
        .sub_skill_checks
        .iter()
        .filter(|c| c.status == CheckStatus::Fail)
        .count();
    let deps: usize = report
        .dep_checks
        .iter()
        .flat_map(|(_, v)| v.iter())
        .filter(|c| c.status == CheckStatus::Fail)
        .count();
    let misc = report
        .misc_checks
        .iter()
        .filter(|c| c.status == CheckStatus::Fail)
        .count();
    let sub: usize = report
        .sub_reports
        .iter()
        .map(count_failures_in_report)
        .sum();
    interp + env + cmds + sub_skills + deps + misc + sub
}

/// Returns true if any check result in the slice has `Fail` status.
pub(crate) fn has_failures(results: &[CheckResult]) -> bool {
    results.iter().any(|r| r.status == CheckStatus::Fail)
}

/// Returns true if any check result in the report tree has `Fail` status.
pub(crate) fn report_has_failures(report: &DoctorReport) -> bool {
    let own = report
        .interpreter_checks
        .iter()
        .chain(report.env_checks.iter())
        .chain(report.sub_skill_checks.iter())
        .chain(report.misc_checks.iter())
        .any(|r| r.status == CheckStatus::Fail)
        || report
            .command_checks
            .iter()
            .flat_map(|(_, v)| v.iter())
            .any(|r| r.status == CheckStatus::Fail)
        || report
            .dep_checks
            .iter()
            .flat_map(|(_, v)| v.iter())
            .any(|r| r.status == CheckStatus::Fail);
    own || report.sub_reports.iter().any(report_has_failures)
}

// ── Source label helper ───────────────────────────────────────────────────────

fn describe_source(source: &SkillSource) -> String {
    match source {
        SkillSource::Owned(scope) => format!("{} owned", scope_name(*scope)),
        SkillSource::Package(pkg, scope) => format!("package {} ({})", pkg, scope_name(*scope)),
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    // ── which_path tests ──────────────────────────────────────────────────────

    #[test]
    fn test_which_path_finds_existing() {
        // "sh" should exist on any Unix system where tests run.
        #[cfg(unix)]
        {
            let result = which_path("sh");
            assert!(result.is_some(), "expected sh to be found on PATH");
        }
    }

    #[test]
    fn test_which_path_returns_none_for_nonexistent() {
        let result = which_path("creft_doctor_nonexistent_xyz");
        assert!(result.is_none());
    }

    #[test]
    fn test_which_path_returns_absolute_path() {
        #[cfg(unix)]
        {
            if let Some(p) = which_path("sh") {
                assert!(
                    p.is_absolute(),
                    "path should be absolute, got: {}",
                    p.display()
                );
            }
        }
    }

    // ── extract_commands tests ────────────────────────────────────────────────

    #[test]
    fn test_extract_commands_simple() {
        let result = extract_commands("curl -X POST url");
        assert!(
            result.contains(&"curl".to_string()),
            "expected curl, got: {:?}",
            result
        );
    }

    #[test]
    fn test_extract_commands_pipe() {
        let result = extract_commands("cat file | jq .");
        assert!(
            result.contains(&"cat".to_string()),
            "expected cat, got: {:?}",
            result
        );
        assert!(
            result.contains(&"jq".to_string()),
            "expected jq, got: {:?}",
            result
        );
    }

    #[test]
    fn test_extract_commands_and_chain() {
        let result = extract_commands("make && deploy");
        assert!(
            result.contains(&"make".to_string()),
            "expected make, got: {:?}",
            result
        );
        assert!(
            result.contains(&"deploy".to_string()),
            "expected deploy, got: {:?}",
            result
        );
    }

    #[test]
    fn test_extract_commands_subshell() {
        // echo is a builtin and should be filtered; git should be extracted.
        let result = extract_commands("echo $(git rev-parse HEAD)");
        assert!(
            !result.contains(&"echo".to_string()),
            "echo should be filtered as builtin"
        );
        assert!(
            result.contains(&"git".to_string()),
            "expected git, got: {:?}",
            result
        );
    }

    #[test]
    fn test_extract_commands_filters_builtins() {
        let result = extract_commands("echo hello && export FOO=bar");
        assert!(
            result.is_empty(),
            "all builtins should be filtered, got: {:?}",
            result
        );
    }

    #[test]
    fn test_extract_commands_multiline() {
        let code = "curl -s https://example.com\ngh api repos/owner/repo\njq '.title'";
        let result = extract_commands(code);
        assert!(result.contains(&"curl".to_string()));
        assert!(result.contains(&"gh".to_string()));
        assert!(result.contains(&"jq".to_string()));
    }

    #[test]
    fn test_extract_commands_comments_skipped() {
        // Lines starting with # are comment lines and should not produce commands.
        let code = "# curl http://example.com\necho hello";
        let result = extract_commands(code);
        assert!(
            !result.contains(&"curl".to_string()),
            "curl in comment should be skipped"
        );
        // echo is a builtin so also filtered.
        assert!(result.is_empty(), "expected empty, got: {:?}", result);
    }

    // ── extract_creft_calls tests ─────────────────────────────────────────────

    #[test]
    fn test_extract_creft_calls_simple() {
        let result = extract_creft_calls("creft deploy prod");
        assert_eq!(result, vec!["deploy prod"]);
    }

    #[test]
    fn test_extract_creft_calls_namespace() {
        let result = extract_creft_calls("creft readme gather-shipped");
        assert_eq!(result, vec!["readme gather-shipped"]);
    }

    #[test]
    fn test_extract_creft_calls_with_flags() {
        // Flags (words starting with '-') are stripped.
        let result = extract_creft_calls("creft deploy --env prod");
        assert_eq!(result, vec!["deploy"]);
    }

    #[test]
    fn test_extract_creft_calls_in_subshell() {
        // "cat" is a creft reserved builtin; should be filtered out.
        let result = extract_creft_calls("$(creft cat some-skill)");
        assert!(
            result.is_empty(),
            "creft cat is a builtin, should be filtered, got: {:?}",
            result
        );
    }

    #[test]
    fn test_extract_creft_calls_filters_reserved() {
        let code = "creft list\ncreft add --name foo\ncreft my-skill";
        let result = extract_creft_calls(code);
        // "list" and "add" are reserved; "my-skill" is a skill.
        assert!(!result.contains(&"list".to_string()));
        assert!(!result.contains(&"add".to_string()));
        assert!(
            result.contains(&"my-skill".to_string()),
            "got: {:?}",
            result
        );
    }

    // ── has_failures tests ────────────────────────────────────────────────────

    #[test]
    fn test_has_failures_with_all_ok() {
        let results = vec![
            CheckResult {
                label: "bash".into(),
                status: CheckStatus::Ok,
                detail: "/usr/bin/bash".into(),
            },
            CheckResult {
                label: "sh".into(),
                status: CheckStatus::Ok,
                detail: "/bin/sh".into(),
            },
        ];
        assert!(!has_failures(&results));
    }

    #[test]
    fn test_has_failures_with_fail() {
        let results = vec![
            CheckResult {
                label: "bash".into(),
                status: CheckStatus::Ok,
                detail: "/usr/bin/bash".into(),
            },
            CheckResult {
                label: "something".into(),
                status: CheckStatus::Fail,
                detail: "not found".into(),
            },
        ];
        assert!(has_failures(&results));
    }

    #[test]
    fn test_has_failures_ignores_optional() {
        let results = vec![
            CheckResult {
                label: "shellcheck".into(),
                status: CheckStatus::Optional,
                detail: "not found".into(),
            },
            CheckResult {
                label: "uv".into(),
                status: CheckStatus::Info,
                detail: "info".into(),
            },
        ];
        assert!(!has_failures(&results));
    }

    // ── Global check tests ────────────────────────────────────────────────────

    #[test]
    fn test_global_check_returns_results() {
        let ctx =
            crate::model::AppContext::from_env().expect("AppContext::from_env() failed in test");
        let results = run_global_check(&ctx);
        assert!(
            !results.is_empty(),
            "global check should return at least one result"
        );
    }

    #[test]
    fn test_global_check_includes_bash() {
        let ctx =
            crate::model::AppContext::from_env().expect("AppContext::from_env() failed in test");
        let results = run_global_check(&ctx);
        assert!(
            results.iter().any(|r| r.label == "bash"),
            "global check should include a bash check"
        );
    }

    #[test]
    fn test_global_check_includes_packages() {
        let ctx =
            crate::model::AppContext::from_env().expect("AppContext::from_env() failed in test");
        let results = run_global_check(&ctx);
        assert!(
            results.iter().any(|r| r.label == "packages"),
            "global check should include a packages check"
        );
    }

    // ── check_interpreter: missing tool paths ─────────────────────────────────

    #[test]
    fn test_check_interpreter_missing_zsh_is_optional() {
        // We cannot control whether zsh is installed, but we can test the
        // logic branch by checking the detail string when it IS missing.
        // If zsh is found the result is Ok; if missing it should be Optional.
        let result = check_interpreter("zsh");
        if result.status == CheckStatus::Ok {
            // zsh found — Ok is valid
        } else {
            assert_eq!(
                result.status,
                CheckStatus::Optional,
                "missing zsh should be Optional"
            );
            assert!(
                result.detail.contains("not required on non-macOS"),
                "zsh detail: {}",
                result.detail
            );
        }
    }

    #[test]
    fn test_check_interpreter_missing_git_is_optional() {
        let result = check_interpreter("git");
        if result.status == CheckStatus::Ok {
            // git found — Ok is valid
        } else {
            assert_eq!(
                result.status,
                CheckStatus::Optional,
                "missing git should be Optional"
            );
            assert!(
                result.detail.contains("creft install"),
                "git detail should mention install, got: {}",
                result.detail
            );
        }
    }

    #[test]
    fn test_check_interpreter_unknown_missing_is_optional() {
        // Use a name guaranteed not to exist
        let result = check_interpreter("creft_no_such_interp_xyz9999");
        assert_eq!(result.status, CheckStatus::Optional);
        assert!(result.detail.contains("not found on PATH"));
    }

    #[test]
    fn test_check_interpreter_bash_missing_is_fail() {
        // We can't remove bash from PATH, so we test the function directly
        // with the known-missing tool that tests the "bash"/"sh" branch.
        // Instead we validate the check_interpreter logic by checking
        // that a found interpreter returns Ok.
        let result = check_interpreter("bash");
        // On any sane CI machine bash exists, but if not it should be Fail.
        assert!(
            result.status == CheckStatus::Ok || result.status == CheckStatus::Fail,
            "bash should be Ok or Fail, never Optional"
        );
    }

    // ── check_optional_tool: missing ──────────────────────────────────────────

    #[test]
    fn test_check_optional_tool_missing() {
        let result = check_optional_tool("creft_no_such_tool_xyz9999", "used for testing");
        assert_eq!(result.status, CheckStatus::Optional);
        assert!(
            result.detail.contains("used for testing"),
            "detail should include purpose, got: {}",
            result.detail
        );
    }

    #[test]
    fn test_check_optional_tool_found() {
        // "sh" should be on PATH everywhere
        let result = check_optional_tool("sh", "shell");
        assert_eq!(result.status, CheckStatus::Ok);
    }

    // ── check_global_dir ──────────────────────────────────────────────────────

    #[test]
    fn test_check_global_dir_no_home() {
        let ctx = crate::model::AppContext {
            home_dir: None,
            creft_home: None,
            cwd: std::path::PathBuf::from("/tmp"),
        };
        let result = check_global_dir(&ctx);
        assert_eq!(result.status, CheckStatus::Fail);
        assert!(result.detail.contains("home directory not set"));
    }

    #[test]
    fn test_check_global_dir_not_exists() {
        let tmp = tempfile::TempDir::new().unwrap();
        // Use a subdirectory that doesn't contain .creft/
        let ctx =
            crate::model::AppContext::for_test(tmp.path().to_path_buf(), tmp.path().to_path_buf());
        // .creft/ doesn't exist under tmp
        let result = check_global_dir(&ctx);
        // Should be Info (will be created on first use)
        assert_eq!(result.status, CheckStatus::Info);
        assert!(result.detail.contains("does not exist yet"));
    }

    #[test]
    fn test_check_global_dir_writable() {
        let tmp = tempfile::TempDir::new().unwrap();
        let creft_dir = tmp.path().join(".creft");
        std::fs::create_dir(&creft_dir).unwrap();
        let ctx =
            crate::model::AppContext::for_test(tmp.path().to_path_buf(), tmp.path().to_path_buf());
        let result = check_global_dir(&ctx);
        assert_eq!(result.status, CheckStatus::Ok);
        assert_eq!(result.detail, "writable");
    }

    // ── check_local_dir ────────────────────────────────────────────────────────

    #[test]
    fn test_check_local_dir_not_found() {
        let tmp = tempfile::TempDir::new().unwrap();
        let ctx =
            crate::model::AppContext::for_test(tmp.path().to_path_buf(), tmp.path().to_path_buf());
        let result = check_local_dir(&ctx);
        assert_eq!(result.status, CheckStatus::Info);
        assert!(result.detail.contains("no local .creft/ found"));
    }

    #[test]
    fn test_check_local_dir_found() {
        let tmp = tempfile::TempDir::new().unwrap();
        let creft_dir = tmp.path().join(".creft");
        std::fs::create_dir(&creft_dir).unwrap();
        let ctx =
            crate::model::AppContext::for_test(tmp.path().to_path_buf(), tmp.path().to_path_buf());
        let result = check_local_dir(&ctx);
        assert_eq!(result.status, CheckStatus::Info);
        // detail should be the path to .creft/
        assert!(result.detail.contains(".creft"));
    }

    // ── render_global ─────────────────────────────────────────────────────────

    #[test]
    fn test_render_global_does_not_panic() {
        // render_global writes to stderr — just ensure it doesn't panic
        let results = vec![
            CheckResult {
                label: "bash".into(),
                status: CheckStatus::Ok,
                detail: "/bin/bash".into(),
            },
            CheckResult {
                label: "missing-tool".into(),
                status: CheckStatus::Fail,
                detail: "not found".into(),
            },
            CheckResult {
                label: "optional-tool".into(),
                status: CheckStatus::Optional,
                detail: "not found".into(),
            },
            CheckResult {
                label: "info-check".into(),
                status: CheckStatus::Info,
                detail: "some info".into(),
            },
        ];
        render_global(&results); // should not panic
    }

    // ── render_skill ──────────────────────────────────────────────────────────

    #[test]
    fn test_render_skill_does_not_panic() {
        // Build a DoctorReport with all sections populated to exercise render_skill
        let report = DoctorReport {
            skill_name: "test-skill".into(),
            source: "global owned".into(),
            interpreter_checks: vec![CheckResult {
                label: "bash".into(),
                status: CheckStatus::Ok,
                detail: "/bin/bash".into(),
            }],
            env_checks: vec![CheckResult {
                label: "SOME_VAR".into(),
                status: CheckStatus::Fail,
                detail: "not set".into(),
            }],
            command_checks: vec![(
                1,
                vec![CheckResult {
                    label: "curl".into(),
                    status: CheckStatus::Ok,
                    detail: "/usr/bin/curl".into(),
                }],
            )],
            sub_skill_checks: vec![CheckResult {
                label: "sub-skill".into(),
                status: CheckStatus::Fail,
                detail: "not found".into(),
            }],
            dep_checks: vec![(
                1,
                vec![CheckResult {
                    label: "jq".into(),
                    status: CheckStatus::Ok,
                    detail: "/usr/bin/jq".into(),
                }],
            )],
            misc_checks: vec![CheckResult {
                label: "pipeline input".into(),
                status: CheckStatus::Info,
                detail: "uses $CREFT_PREV".into(),
            }],
            sub_reports: vec![],
        };
        render_skill(&report); // should not panic
    }

    #[test]
    fn test_render_skill_with_sub_reports_does_not_panic() {
        let sub_report = DoctorReport {
            skill_name: "sub-skill".into(),
            source: "global owned".into(),
            interpreter_checks: vec![],
            env_checks: vec![],
            command_checks: vec![],
            sub_skill_checks: vec![],
            dep_checks: vec![],
            misc_checks: vec![],
            sub_reports: vec![],
        };
        let report = DoctorReport {
            skill_name: "main-skill".into(),
            source: "global owned".into(),
            interpreter_checks: vec![],
            env_checks: vec![],
            command_checks: vec![],
            sub_skill_checks: vec![],
            dep_checks: vec![],
            misc_checks: vec![],
            sub_reports: vec![sub_report],
        };
        render_skill(&report); // should not panic
    }

    #[test]
    fn test_render_skill_multi_dep_blocks_does_not_panic() {
        // Two dep blocks → triggers "deps (block N):" label path
        let report = DoctorReport {
            skill_name: "test".into(),
            source: "global owned".into(),
            interpreter_checks: vec![],
            env_checks: vec![],
            command_checks: vec![],
            sub_skill_checks: vec![],
            dep_checks: vec![
                (
                    1,
                    vec![CheckResult {
                        label: "uv".into(),
                        status: CheckStatus::Ok,
                        detail: "/usr/bin/uv".into(),
                    }],
                ),
                (
                    2,
                    vec![CheckResult {
                        label: "npx".into(),
                        status: CheckStatus::Ok,
                        detail: "/usr/bin/npx".into(),
                    }],
                ),
            ],
            misc_checks: vec![],
            sub_reports: vec![],
        };
        render_skill(&report); // should not panic
    }

    // ── report_has_failures ───────────────────────────────────────────────────

    #[test]
    fn test_report_has_failures_no_fails() {
        let report = DoctorReport {
            skill_name: "test".into(),
            source: "global owned".into(),
            interpreter_checks: vec![CheckResult {
                label: "bash".into(),
                status: CheckStatus::Ok,
                detail: "/bin/bash".into(),
            }],
            env_checks: vec![],
            command_checks: vec![],
            sub_skill_checks: vec![],
            dep_checks: vec![],
            misc_checks: vec![],
            sub_reports: vec![],
        };
        assert!(!report_has_failures(&report));
    }

    #[test]
    fn test_report_has_failures_with_fail_in_interpreter() {
        let report = DoctorReport {
            skill_name: "test".into(),
            source: "global owned".into(),
            interpreter_checks: vec![CheckResult {
                label: "ruby".into(),
                status: CheckStatus::Fail,
                detail: "not found".into(),
            }],
            env_checks: vec![],
            command_checks: vec![],
            sub_skill_checks: vec![],
            dep_checks: vec![],
            misc_checks: vec![],
            sub_reports: vec![],
        };
        assert!(report_has_failures(&report));
    }

    #[test]
    fn test_report_has_failures_with_fail_in_commands() {
        let report = DoctorReport {
            skill_name: "test".into(),
            source: "global owned".into(),
            interpreter_checks: vec![],
            env_checks: vec![],
            command_checks: vec![(
                1,
                vec![CheckResult {
                    label: "missing-cmd".into(),
                    status: CheckStatus::Fail,
                    detail: "not found".into(),
                }],
            )],
            sub_skill_checks: vec![],
            dep_checks: vec![],
            misc_checks: vec![],
            sub_reports: vec![],
        };
        assert!(report_has_failures(&report));
    }

    #[test]
    fn test_report_has_failures_with_fail_in_deps() {
        let report = DoctorReport {
            skill_name: "test".into(),
            source: "global owned".into(),
            interpreter_checks: vec![],
            env_checks: vec![],
            command_checks: vec![],
            sub_skill_checks: vec![],
            dep_checks: vec![(
                1,
                vec![CheckResult {
                    label: "uv".into(),
                    status: CheckStatus::Fail,
                    detail: "not found".into(),
                }],
            )],
            misc_checks: vec![],
            sub_reports: vec![],
        };
        assert!(report_has_failures(&report));
    }

    #[test]
    fn test_report_has_failures_propagates_from_sub_reports() {
        let sub_report = DoctorReport {
            skill_name: "sub".into(),
            source: "global owned".into(),
            interpreter_checks: vec![CheckResult {
                label: "python3".into(),
                status: CheckStatus::Fail,
                detail: "not found".into(),
            }],
            env_checks: vec![],
            command_checks: vec![],
            sub_skill_checks: vec![],
            dep_checks: vec![],
            misc_checks: vec![],
            sub_reports: vec![],
        };
        let report = DoctorReport {
            skill_name: "main".into(),
            source: "global owned".into(),
            interpreter_checks: vec![],
            env_checks: vec![],
            command_checks: vec![],
            sub_skill_checks: vec![],
            dep_checks: vec![],
            misc_checks: vec![],
            sub_reports: vec![sub_report],
        };
        assert!(report_has_failures(&report));
    }

    // ── describe_source ────────────────────────────────────────────────────────

    #[test]
    fn test_describe_source_owned_global() {
        use crate::model::{Scope, SkillSource};
        let s = describe_source(&SkillSource::Owned(Scope::Global));
        assert_eq!(s, "global owned");
    }

    #[test]
    fn test_describe_source_owned_local() {
        use crate::model::{Scope, SkillSource};
        let s = describe_source(&SkillSource::Owned(Scope::Local));
        assert_eq!(s, "local owned");
    }

    #[test]
    fn test_describe_source_package() {
        use crate::model::{Scope, SkillSource};
        let s = describe_source(&SkillSource::Package("mypkg".into(), Scope::Global));
        assert_eq!(s, "package mypkg (global)");
    }

    // ── check_packages: broken manifest ───────────────────────────────────────

    #[test]
    fn test_check_packages_broken_manifest_reported() {
        let tmp = tempfile::TempDir::new().unwrap();
        let ctx =
            crate::model::AppContext::for_test(tmp.path().to_path_buf(), tmp.path().to_path_buf());
        // Create a broken manifest
        let pkg_dir = tmp.path().join(".creft/packages/broken-pkg");
        std::fs::create_dir_all(&pkg_dir).unwrap();
        std::fs::write(pkg_dir.join("creft.yaml"), "not: valid: yaml: [").unwrap();

        let results = check_packages(&ctx);
        // Should include at least one Fail result for the broken manifest
        assert!(
            results.iter().any(|r| r.status == CheckStatus::Fail),
            "broken manifest should produce a Fail result, got: {:?}",
            results
        );
    }

    // ── check_block_interpreter ────────────────────────────────────────────────

    #[test]
    fn test_check_block_interpreter_found() {
        let result = check_block_interpreter("sh");
        assert_eq!(result.status, CheckStatus::Ok);
    }

    #[test]
    fn test_check_block_interpreter_not_found() {
        let result = check_block_interpreter("creft_no_such_interp_xyz9999");
        assert_eq!(result.status, CheckStatus::Fail);
        assert!(result.detail.contains("not found on PATH"));
    }

    // ── status_marker ──────────────────────────────────────────────────────────

    #[test]
    fn test_status_marker_all_variants() {
        assert_eq!(status_marker(CheckStatus::Ok), "[ok]");
        assert_eq!(status_marker(CheckStatus::Fail), "[!!]");
        assert_eq!(status_marker(CheckStatus::Info), "[ii]");
        assert_eq!(status_marker(CheckStatus::Optional), "[--]");
    }

    // ── interpreter_for_lang ───────────────────────────────────────────────────

    #[test]
    fn test_interpreter_for_lang_all() {
        assert_eq!(interpreter_for_lang("bash"), Some("bash"));
        assert_eq!(interpreter_for_lang("sh"), Some("sh"));
        assert_eq!(interpreter_for_lang("zsh"), Some("zsh"));
        assert_eq!(interpreter_for_lang("python"), Some("python3"));
        assert_eq!(interpreter_for_lang("python3"), Some("python3"));
        assert_eq!(interpreter_for_lang("node"), Some("node"));
        assert_eq!(interpreter_for_lang("js"), Some("node"));
        assert_eq!(interpreter_for_lang("javascript"), Some("node"));
        assert_eq!(interpreter_for_lang("typescript"), Some("npx"));
        assert_eq!(interpreter_for_lang("ts"), Some("npx"));
        assert_eq!(interpreter_for_lang("ruby"), Some("ruby"));
        assert_eq!(interpreter_for_lang("rb"), Some("ruby"));
        assert_eq!(interpreter_for_lang("perl"), Some("perl"));
        assert_eq!(interpreter_for_lang("unknown"), None);
    }

    // ── check_block_deps ──────────────────────────────────────────────────────

    #[test]
    fn test_check_block_deps_python_missing_uv() {
        use crate::model::CodeBlock;
        let block = CodeBlock {
            lang: "python".into(),
            code: "import requests".into(),
            deps: vec!["requests".into()],
        };
        let results = check_block_deps(&block, 1);
        // Whether uv is found or not, should have one result
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].label, "uv");
    }

    #[test]
    fn test_check_block_deps_node_missing_npx() {
        use crate::model::CodeBlock;
        let block = CodeBlock {
            lang: "node".into(),
            code: "const axios = require('axios')".into(),
            deps: vec!["axios".into()],
        };
        let results = check_block_deps(&block, 1);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].label, "npx");
    }

    #[test]
    fn test_check_block_deps_shell_checks_each_dep() {
        use crate::model::CodeBlock;
        let block = CodeBlock {
            lang: "bash".into(),
            code: "curl url | jq .".into(),
            deps: vec!["curl".into(), "creft_no_such_dep_xyz9999".into()],
        };
        let results = check_block_deps(&block, 1);
        assert_eq!(results.len(), 2);
        // curl should be found; the fake dep should fail
        let curl_result = results.iter().find(|r| r.label == "curl").unwrap();
        let fake_result = results
            .iter()
            .find(|r| r.label == "creft_no_such_dep_xyz9999")
            .unwrap();
        assert_eq!(curl_result.status, CheckStatus::Ok);
        assert_eq!(fake_result.status, CheckStatus::Fail);
    }
}
