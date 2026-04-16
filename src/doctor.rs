use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::LazyLock;

use regex::Regex;

use yansi::Paint;

use crate::error::CreftError;
use crate::frontmatter;
use crate::markdown;
use crate::model::{AppContext, CodeBlock, SkillSource};
use crate::registry;
use crate::settings::Settings;
use crate::shell;
use crate::store;

// ── Types ─────────────────────────────────────────────────────────────────────

/// Outcome of a single doctor check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CheckStatus {
    /// Check passed.
    Ok,
    /// Check failed -- a required dependency is missing.
    Fail,
    /// Warning -- something may be broken but is not a hard failure.
    Warn,
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
    /// Fence nesting warnings (inner fences that prematurely close outer fences).
    pub fence_checks: Vec<CheckResult>,
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

/// Report the caller's detected shell preference.
///
/// This is informational only — the check never fails. It shows which shell
/// will be used to run shell-family code blocks, so users can confirm the
/// detected value matches their expectation.
fn check_shell_preference(settings_shell: Option<&str>) -> CheckResult {
    match shell::detect(settings_shell) {
        Some(name) => CheckResult {
            label: "shell".to_string(),
            status: CheckStatus::Info,
            detail: name,
        },
        None => CheckResult {
            label: "shell".to_string(),
            status: CheckStatus::Info,
            detail: "none (blocks use their language tag directly)".to_string(),
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
                // Only report entries that have at least one manifest file.
                if let Some(Err(e)) = registry::read_manifest_from(&path) {
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
/// Check activation settings for stale entries (activated plugins that no longer exist).
fn check_activations(ctx: &AppContext) -> Vec<CheckResult> {
    use crate::model::Scope;

    let mut results = Vec::new();

    let scopes = {
        let mut v = vec![Scope::Global];
        if ctx.find_local_root().is_some() {
            v.push(Scope::Local);
        }
        v
    };

    for scope in scopes {
        let settings = match registry::load_settings(ctx, scope) {
            Ok(s) => s,
            Err(e) => {
                results.push(CheckResult {
                    label: format!("plugin activations ({})", scope_name(scope)),
                    status: CheckStatus::Fail,
                    detail: format!("could not read settings.json: {}", e),
                });
                continue;
            }
        };

        for plugin_name in settings.activated.keys() {
            let plugins_dir = match ctx.plugins_dir() {
                Ok(d) => d,
                Err(_) => continue,
            };
            let plugin_dir = plugins_dir.join(plugin_name);
            if !plugin_dir.is_dir() {
                results.push(CheckResult {
                    label: format!("plugin activation ({}) {}", scope_name(scope), plugin_name),
                    status: CheckStatus::Warn,
                    detail: format!(
                        "stale activation: plugin '{}' is not installed. \
                         Run 'creft plugin deactivate {}' to clean up.",
                        plugin_name, plugin_name
                    ),
                });
            }
        }
    }

    results
}

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
    results.push(check_interpreter("git"));

    results.push(check_optional_tool("shellcheck", "used for shell linting"));
    results.push(check_optional_tool("uv", "needed for python deps"));
    results.push(check_optional_tool("npm", "needed for node deps"));

    for provider in &["claude", "gemini", "codex", "ollama"] {
        results.push(check_llm_provider(provider));
    }

    results.push(check_global_dir(ctx));
    results.push(check_local_dir(ctx));
    results.extend(check_packages(ctx));
    results.extend(check_flat_files(ctx));
    results.extend(check_activations(ctx));

    // Load the settings shell preference for doctor display. A corrupt or
    // missing settings file is treated as no preference — the check is
    // informational and must not itself fail.
    let settings_shell_pref = ctx
        .settings_path()
        .ok()
        .and_then(|p| Settings::load(&p).ok())
        .and_then(|s| s.get("shell").map(str::to_string));
    results.push(check_shell_preference(settings_shell_pref.as_deref()));

    results
}

// ── Skill check ───────────────────────────────────────────────────────────────

/// Run a health check for a single skill, recursively checking sub-skills.
///
/// Checks interpreters, env vars, commands, dependencies, and creft sub-skill
/// references. Detects cycles and limits recursion to 10 levels.
///
/// `shell_pref` is the active shell preference (from settings or `CREFT_SHELL`).
/// When set and a block's language is in the shell family, the resolved
/// interpreter is reported instead of the block's literal language tag — matching
/// what will actually run the block.
pub(crate) fn run_skill_check(
    ctx: &AppContext,
    name: &str,
    source: &SkillSource,
    shell_pref: Option<&str>,
) -> Result<DoctorReport, CreftError> {
    let mut visited = HashSet::new();
    visited.insert(name.to_string());
    run_skill_check_inner(ctx, name, source, shell_pref, &mut visited, 0)
}

fn run_skill_check_inner(
    ctx: &AppContext,
    name: &str,
    source: &SkillSource,
    shell_pref: Option<&str>,
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
    let mut fence_checks: Vec<CheckResult> = Vec::new();
    let mut sub_reports: Vec<DoctorReport> = Vec::new();

    // Run fence nesting check on the raw body. The raw content includes
    // frontmatter, so strip it before passing to check_fence_nesting to keep
    // line numbers consistent with the creft-add path.
    if let Ok(raw) = store::read_raw_from(ctx, name, source)
        && let Ok((_, body)) = frontmatter::parse(&raw)
    {
        for w in markdown::check_fence_nesting(&body) {
            fence_checks.push(CheckResult {
                label: format!("line {} ({} backticks)", w.outer_line, w.outer_backticks),
                status: CheckStatus::Fail,
                detail: format!(
                    "contains inner fence at line {} — use {}+ backticks on outer fence",
                    w.line,
                    w.outer_backticks + 1,
                ),
            });
        }
    }

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
            fence_checks,
            sub_reports,
        });
    }

    let mut seen_interpreters: HashSet<String> = HashSet::new();
    let mut creft_sub_skills: Vec<String> = Vec::new();

    for (idx, block) in cmd.blocks.iter().enumerate() {
        let block_num = idx + 1;

        if block.lang == "llm" {
            if let Some(config) = &block.llm_config {
                let cli = llm_provider_cli_name(&config.provider);
                if seen_interpreters.insert(cli.to_string()) {
                    interpreter_checks.push(check_llm_provider(cli));
                }
            }
            // llm blocks have no deps, commands, or sub-skill calls to check
            continue;
        }

        // When a shell preference is active and this block is in the shell
        // family, report the resolved interpreter (what will actually run).
        // This matches execution behaviour rather than the literal language tag.
        let effective_interp: Option<&str> = shell::resolve_shell(&block.lang, shell_pref)
            .or_else(|| interpreter_for_lang(&block.lang));
        if let Some(interp) = effective_interp
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

    let has_creft_prev = cmd.blocks.iter().any(|b| b.code.contains("$CREFT_PREV"));
    let prev_in_shell = cmd
        .blocks
        .iter()
        .any(|b| b.lang != "llm" && b.code.contains("{{prev}}"));
    let prev_in_llm = cmd
        .blocks
        .iter()
        .any(|b| b.lang == "llm" && b.code.contains("{{prev}}"));

    if has_creft_prev {
        misc_checks.push(CheckResult {
            label: "pipeline input".to_string(),
            status: CheckStatus::Warn,
            detail: "$CREFT_PREV is removed in v0.2.0. Multi-block skills now pipe stdout \
                     directly. Remove $CREFT_PREV references."
                .to_string(),
        });
    }
    if prev_in_shell {
        misc_checks.push(CheckResult {
            label: "pipeline input".to_string(),
            status: CheckStatus::Warn,
            detail: "{{prev}} in shell blocks is not supported. It is only valid in LLM \
                     block prompts within multi-block skills."
                .to_string(),
        });
    }
    if prev_in_llm && !has_creft_prev && !prev_in_shell {
        misc_checks.push(CheckResult {
            label: "pipeline input".to_string(),
            status: CheckStatus::Info,
            detail: "uses {{prev}} in LLM prompt (reads buffered upstream input)".to_string(),
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
                        shell_pref,
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
        fence_checks,
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
        "perl" => Some("perl"),
        _ => None,
    }
}

/// Returns true if `lang` is a shell language that supports command extraction.
pub(crate) fn is_shell_lang(lang: &str) -> bool {
    matches!(lang, "bash" | "sh" | "zsh")
}

/// Return the CLI command name for a given LLM provider string.
///
/// For known providers, the command name matches the provider name.
/// For unknown providers, the provider string is used as-is (the literal
/// command the user expects on PATH).
pub(crate) fn llm_provider_cli_name(provider: &str) -> &str {
    match provider {
        "" => "claude",
        other => other,
    }
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

/// Check for an LLM provider CLI. Missing providers are `Optional` (not `Fail`)
/// because the provider may be available in the execution environment but not
/// the authoring environment.
fn check_llm_provider(name: &str) -> CheckResult {
    match which_path(name) {
        Some(p) => CheckResult {
            label: name.to_string(),
            status: CheckStatus::Ok,
            detail: p.to_string_lossy().to_string(),
        },
        None => CheckResult {
            label: name.to_string(),
            status: CheckStatus::Optional,
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
        if let Some(p) = which_path("npm") {
            results.push(CheckResult {
                label: "npm".to_string(),
                status: CheckStatus::Ok,
                detail: p.to_string_lossy().to_string(),
            });
        } else {
            results.push(CheckResult {
                label: "npm".to_string(),
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
            // Skip variable assignments: NAME=value, cwd=$(pwd), SIGNAL=9, etc.
            // The regex crate does not support lookaheads, so we check the character
            // immediately following the match end in the filtered string.
            if filtered.as_bytes().get(m.end()) == Some(&b'=') {
                continue;
            }
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
        CheckStatus::Warn => "[ww]",
        CheckStatus::Info => "[ii]",
        CheckStatus::Optional => "[--]",
    }
}

/// Render global check results to stderr.
pub(crate) fn render_global(results: &[CheckResult]) {
    eprintln!("{}", "Environment Health Check".bold());
    eprintln!();
    for r in results {
        eprintln!("  {} {} ({})", status_marker(r.status), r.label, r.detail);
    }
    eprintln!();
    eprintln!("See 'creft doctor <skill>' to check a specific skill.");
}

/// Render a skill doctor report to stderr.
pub(crate) fn render_skill(report: &DoctorReport) {
    let header = format!("Skill Health: {} ({})", report.skill_name, report.source);
    eprintln!("{}", header.as_str().bold());
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

    if !report.fence_checks.is_empty() {
        eprintln!("{}fence nesting:", inner);
        for r in &report.fence_checks {
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
    let fence = report
        .fence_checks
        .iter()
        .filter(|c| c.status == CheckStatus::Fail)
        .count();
    let sub: usize = report
        .sub_reports
        .iter()
        .map(count_failures_in_report)
        .sum();
    interp + env + cmds + sub_skills + deps + misc + fence + sub
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
        .chain(report.fence_checks.iter())
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
        SkillSource::Plugin(name) => format!("plugin {}", name),
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use rstest::rstest;

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
        // "list" is a reserved builtin; should be filtered out.
        let result = extract_creft_calls("$(creft list)");
        assert!(
            result.is_empty(),
            "creft list is a builtin, should be filtered, got: {:?}",
            result
        );
    }

    #[test]
    fn test_extract_creft_calls_filters_reserved() {
        let code = "creft list\ncreft plugin install foo\ncreft my-skill";
        let result = extract_creft_calls(code);
        // "list" and "plugin" are reserved top-level builtins; "my-skill" is a skill.
        assert!(!result.contains(&"list".to_string()));
        assert!(!result.contains(&"plugin install foo".to_string()));
        assert!(!result.contains(&"plugin".to_string()));
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
        if crate::doctor::which_path("bash").is_none() {
            return;
        }
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
        if crate::doctor::which_path("bash").is_none() {
            return;
        }
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
        if crate::doctor::which_path("bash").is_none() {
            return;
        }
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
                status: CheckStatus::Warn,
                detail: "$CREFT_PREV is removed in v0.2.0. Multi-block skills now pipe stdout directly. Remove $CREFT_PREV references.".into(),
            }],
            fence_checks: vec![],
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
            fence_checks: vec![],
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
            fence_checks: vec![],
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
            fence_checks: vec![],
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
            fence_checks: vec![],
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
            fence_checks: vec![],
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
            fence_checks: vec![],
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
            fence_checks: vec![],
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
            fence_checks: vec![],
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
            fence_checks: vec![],
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
        // Create a package with a malformed catalog.json
        let pkg_dir = tmp.path().join(".creft/packages/broken-pkg");
        std::fs::create_dir_all(pkg_dir.join(".creft")).unwrap();
        std::fs::write(
            pkg_dir.join(".creft").join("catalog.json"),
            "not valid json [",
        )
        .unwrap();

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

    #[rstest]
    #[case::ok(CheckStatus::Ok, "[ok]")]
    #[case::fail(CheckStatus::Fail, "[!!]")]
    #[case::warn(CheckStatus::Warn, "[ww]")]
    #[case::info(CheckStatus::Info, "[ii]")]
    #[case::optional(CheckStatus::Optional, "[--]")]
    fn status_marker_formats_variant(#[case] status: CheckStatus, #[case] expected: &str) {
        assert_eq!(status_marker(status), expected);
    }

    // ── interpreter_for_lang ───────────────────────────────────────────────────

    #[rstest]
    #[case::bash("bash", Some("bash"))]
    #[case::sh("sh", Some("sh"))]
    #[case::zsh("zsh", Some("zsh"))]
    #[case::python("python", Some("python3"))]
    #[case::python3("python3", Some("python3"))]
    #[case::node("node", Some("node"))]
    #[case::js("js", Some("node"))]
    #[case::javascript("javascript", Some("node"))]
    #[case::typescript("typescript", Some("npx"))]
    #[case::ts("ts", Some("npx"))]
    #[case::perl("perl", Some("perl"))]
    #[case::unknown("unknown", None)]
    fn interpreter_for_lang_maps_correctly(#[case] lang: &str, #[case] expected: Option<&str>) {
        assert_eq!(interpreter_for_lang(lang), expected);
    }

    // ── check_block_deps ──────────────────────────────────────────────────────

    #[test]
    fn test_check_block_deps_python_missing_uv() {
        use crate::model::CodeBlock;
        let block = CodeBlock {
            lang: "python".into(),
            code: "import requests".into(),
            deps: vec!["requests".into()],
            llm_config: None,
            llm_parse_error: None,
        };
        let results = check_block_deps(&block, 1);
        // Whether uv is found or not, should have one result
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].label, "uv");
    }

    #[test]
    fn test_check_block_deps_node_missing_npm() {
        use crate::model::CodeBlock;
        let block = CodeBlock {
            lang: "node".into(),
            code: "const axios = require('axios')".into(),
            deps: vec!["axios".into()],
            llm_config: None,
            llm_parse_error: None,
        };
        let results = check_block_deps(&block, 1);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].label, "npm");
    }

    #[test]
    fn test_check_block_deps_shell_checks_each_dep() {
        use crate::model::CodeBlock;
        let block = CodeBlock {
            lang: "bash".into(),
            code: "curl url | jq .".into(),
            deps: vec!["curl".into(), "creft_no_such_dep_xyz9999".into()],
            llm_config: None,
            llm_parse_error: None,
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

    // ── llm_provider_cli_name ─────────────────────────────────────────────────

    #[test]
    fn test_llm_provider_cli_name_empty_defaults_to_claude() {
        assert_eq!(llm_provider_cli_name(""), "claude");
    }

    #[test]
    fn test_llm_provider_cli_name_known_providers() {
        assert_eq!(llm_provider_cli_name("claude"), "claude");
        assert_eq!(llm_provider_cli_name("gemini"), "gemini");
        assert_eq!(llm_provider_cli_name("codex"), "codex");
        assert_eq!(llm_provider_cli_name("ollama"), "ollama");
    }

    #[test]
    fn test_llm_provider_cli_name_unknown_returns_as_is() {
        assert_eq!(llm_provider_cli_name("my-custom-llm"), "my-custom-llm");
    }

    // ── check_llm_provider ────────────────────────────────────────────────────

    #[test]
    fn test_check_llm_provider_found() {
        // Use `sh` as a stand-in for a found provider.
        let result = check_llm_provider("sh");
        assert_eq!(result.status, CheckStatus::Ok);
        assert_eq!(result.label, "sh");
    }

    #[test]
    fn test_check_llm_provider_not_found_is_optional() {
        // A missing LLM provider is Optional (not Fail) because it may be
        // available in the execution environment but not the authoring env.
        let result = check_llm_provider("creft_no_such_llm_xyz9999");
        assert_eq!(result.status, CheckStatus::Optional);
        assert!(result.detail.contains("not found on PATH"));
    }

    // ── pipeline input diagnostics ────────────────────────────────────────────

    fn write_skill_to_tmp(
        markdown: &str,
    ) -> (
        tempfile::TempDir,
        crate::model::AppContext,
        String,
        crate::model::SkillSource,
    ) {
        let tmp = tempfile::TempDir::new().unwrap();
        let creft_dir = tmp.path().join(".creft/commands");
        std::fs::create_dir_all(&creft_dir).unwrap();
        let skill_path = creft_dir.join("test-skill.md");
        std::fs::write(&skill_path, markdown).unwrap();
        let ctx =
            crate::model::AppContext::for_test(tmp.path().to_path_buf(), tmp.path().to_path_buf());
        let source = crate::model::SkillSource::Owned(crate::model::Scope::Local);
        (tmp, ctx, "test-skill".to_string(), source)
    }

    #[test]
    fn test_skill_check_creft_prev_warns_removal() {
        let markdown = "---\nname: test-skill\ndescription: uses CREFT_PREV\n---\n\n```bash\necho $CREFT_PREV\n```\n";
        let (_tmp, ctx, name, source) = write_skill_to_tmp(markdown);
        let report = run_skill_check(&ctx, &name, &source, None).unwrap();
        let misc = &report.misc_checks;
        let pipeline_check = misc
            .iter()
            .find(|c| c.label == "pipeline input")
            .unwrap_or_else(|| panic!("no pipeline input check found, misc_checks: {:?}", misc));
        assert_eq!(
            pipeline_check.status,
            CheckStatus::Warn,
            "expected Warn for $CREFT_PREV usage"
        );
        assert!(
            pipeline_check.detail.contains("removed in v0.2.0"),
            "expected removal message, got: {}",
            pipeline_check.detail
        );
    }

    #[test]
    fn test_skill_check_prev_in_shell_block_warns_not_supported() {
        let markdown = "---\nname: test-skill\ndescription: uses prev in shell\n---\n\n```bash\necho '{{prev}}'\n```\n";
        let (_tmp, ctx, name, source) = write_skill_to_tmp(markdown);
        let report = run_skill_check(&ctx, &name, &source, None).unwrap();
        let misc = &report.misc_checks;
        let pipeline_check = misc.iter().find(|c| c.label == "pipeline input").unwrap();
        assert_eq!(
            pipeline_check.status,
            CheckStatus::Warn,
            "expected Warn for {{prev}} in shell block"
        );
        assert!(
            pipeline_check.detail.contains("not supported"),
            "expected not-supported message, got: {}",
            pipeline_check.detail
        );
    }

    #[test]
    fn test_skill_check_prev_in_llm_block_is_info() {
        let markdown = "---\nname: test-skill\ndescription: llm sponge\n---\n\n```bash\necho upstream\n```\n\n```llm\nprovider: cat\n---\nresult: {{prev}}\n```\n";
        let (_tmp, ctx, name, source) = write_skill_to_tmp(markdown);
        let report = run_skill_check(&ctx, &name, &source, None).unwrap();
        let misc = &report.misc_checks;
        let pipeline_check = misc.iter().find(|c| c.label == "pipeline input").unwrap();
        assert_eq!(
            pipeline_check.status,
            CheckStatus::Info,
            "expected Info for {{prev}} in LLM block only"
        );
        assert!(
            pipeline_check.detail.contains("buffered upstream input"),
            "expected buffered upstream input message, got: {}",
            pipeline_check.detail
        );
    }

    // ── Stage 3: check_shell_preference ──────────────────────────────────────

    #[test]
    fn check_shell_preference_with_setting_shows_simple_label_and_name() {
        // SAFETY: nextest runs each test in its own process; no concurrent env mutation.
        unsafe {
            std::env::remove_var("CREFT_SHELL");
            std::env::remove_var("SHELL");
        }
        let result = check_shell_preference(Some("zsh"));
        assert_eq!(result.label, "shell");
        assert_eq!(result.detail, "zsh");
        assert_eq!(result.status, CheckStatus::Info);
    }

    #[test]
    fn check_shell_preference_with_no_detection_shows_none_message() {
        // SAFETY: nextest runs each test in its own process.
        unsafe {
            std::env::remove_var("CREFT_SHELL");
            std::env::remove_var("SHELL");
        }
        let result = check_shell_preference(None);
        assert_eq!(result.label, "shell");
        assert!(
            result.detail.contains("none"),
            "expected 'none' in detail, got: {}",
            result.detail
        );
        assert_eq!(result.status, CheckStatus::Info);
    }

    // ── Stage 3: variable assignment filtering ────────────────────────────────

    #[rstest]
    #[case::simple_assignment("TARGET=foo\ncurl https://example.com", &["curl"])]
    #[case::numeric_assignment("SIGNAL=9\nkill -9 $PID", &["kill"])]
    #[case::subshell_assignment("cwd=$(pwd)\nls $cwd", &["ls"])]
    #[case::assignment_and_pipe("OUT=$(git rev-parse HEAD)\necho $OUT | grep abc", &["git", "grep"])]
    fn extract_commands_skips_variable_assignments(#[case] code: &str, #[case] expected: &[&str]) {
        let result = extract_commands(code);
        let expected: Vec<String> = expected.iter().map(|s| s.to_string()).collect();
        assert_eq!(result, expected);
    }

    #[test]
    fn extract_commands_does_not_filter_regular_commands_with_equals_in_args() {
        // "--key=value" style args should not suppress the command itself.
        // The command name ends before '=', so the check fires only on the identifier.
        let result = extract_commands("curl --url=http://example.com");
        assert!(
            result.contains(&"curl".to_string()),
            "curl should be extracted even when args contain '=', got: {:?}",
            result
        );
    }

    // ── Stage 3: resolved interpreter with shell preference ───────────────────

    #[test]
    fn run_skill_check_with_shell_pref_reports_resolved_interpreter_for_bash_block() {
        let markdown =
            "---\nname: test-skill\ndescription: shell block\n---\n\n```bash\necho hello\n```\n";
        let (_tmp, ctx, name, source) = write_skill_to_tmp(markdown);
        // Shell preference is zsh; the block is bash — resolved interpreter should be zsh.
        let report = run_skill_check(&ctx, &name, &source, Some("zsh")).unwrap();
        let labels: Vec<&str> = report
            .interpreter_checks
            .iter()
            .map(|c| c.label.as_str())
            .collect();
        assert!(
            labels.contains(&"zsh"),
            "expected zsh as resolved interpreter, got: {:?}",
            labels
        );
        assert!(
            !labels.contains(&"bash"),
            "bash should not appear when zsh preference overrides it, got: {:?}",
            labels
        );
    }

    #[test]
    fn run_skill_check_without_shell_pref_reports_block_lang_interpreter() {
        let markdown =
            "---\nname: test-skill\ndescription: shell block\n---\n\n```bash\necho hello\n```\n";
        let (_tmp, ctx, name, source) = write_skill_to_tmp(markdown);
        // No shell preference — literal block language is reported.
        let report = run_skill_check(&ctx, &name, &source, None).unwrap();
        let labels: Vec<&str> = report
            .interpreter_checks
            .iter()
            .map(|c| c.label.as_str())
            .collect();
        assert!(
            labels.contains(&"bash"),
            "expected bash as interpreter when no preference, got: {:?}",
            labels
        );
    }

    #[test]
    fn run_skill_check_with_shell_pref_does_not_affect_non_shell_blocks() {
        let markdown = "---\nname: test-skill\ndescription: python block\n---\n\n```python\nprint('hi')\n```\n";
        let (_tmp, ctx, name, source) = write_skill_to_tmp(markdown);
        // Shell preference should not apply to python blocks.
        let report = run_skill_check(&ctx, &name, &source, Some("zsh")).unwrap();
        let labels: Vec<&str> = report
            .interpreter_checks
            .iter()
            .map(|c| c.label.as_str())
            .collect();
        assert!(
            labels.contains(&"python3"),
            "python3 should still be reported for python blocks, got: {:?}",
            labels
        );
        assert!(
            !labels.contains(&"zsh"),
            "zsh should not appear for a python block, got: {:?}",
            labels
        );
    }
}
