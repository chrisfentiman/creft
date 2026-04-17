use std::path::{Path, PathBuf};

use crate::error::CreftError;

/// How a harness receives creft context at session start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InstallStrategy {
    /// Install a session-start hook that runs `creft _creft session start`.
    /// The harness injects the hook's stdout into agent context automatically.
    Hook,
    /// Write a static instruction file. Used for harnesses without session-start
    /// hooks or where hook output is not injected into context.
    StaticFile,
}

/// The `_creft session start` skill content, maintained as a standalone markdown file.
/// Written to `.creft/commands/_creft/session/start.md` during `creft up`.
const SESSION_SKILL_CONTENT: &str = include_str!("../docs/skills/session-start.md");

/// Supported coding AI systems.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum System {
    ClaudeCode,
    Cursor,
    Windsurf,
    Aider,
    Copilot,
    Codex,
    Gemini,
}

impl System {
    /// Returns the canonical kebab-case identifier used in CLI arguments.
    pub fn name(&self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude-code",
            Self::Cursor => "cursor",
            Self::Windsurf => "windsurf",
            Self::Aider => "aider",
            Self::Copilot => "copilot",
            Self::Codex => "codex",
            Self::Gemini => "gemini",
        }
    }

    /// Returns the human-readable product name shown in output.
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::ClaudeCode => "Claude Code",
            Self::Cursor => "Cursor",
            Self::Windsurf => "Windsurf",
            Self::Aider => "Aider",
            Self::Copilot => "GitHub Copilot",
            Self::Codex => "Codex",
            Self::Gemini => "Gemini CLI",
        }
    }

    /// Returns all supported systems in a stable order.
    pub fn all() -> &'static [System] {
        &[
            Self::ClaudeCode,
            Self::Cursor,
            Self::Windsurf,
            Self::Aider,
            Self::Copilot,
            Self::Codex,
            Self::Gemini,
        ]
    }

    /// Returns the install strategy for this harness.
    fn strategy(&self) -> InstallStrategy {
        match self {
            Self::ClaudeCode | Self::Gemini => InstallStrategy::Hook,
            Self::Cursor | Self::Windsurf | Self::Aider | Self::Copilot | Self::Codex => {
                InstallStrategy::StaticFile
            }
        }
    }

    /// Parses a system from a CLI name or alias, returning `None` if unrecognized.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "claude-code" | "claude" => Some(Self::ClaudeCode),
            "cursor" => Some(Self::Cursor),
            "windsurf" => Some(Self::Windsurf),
            "aider" => Some(Self::Aider),
            "copilot" | "github-copilot" => Some(Self::Copilot),
            "codex" | "openai-codex" => Some(Self::Codex),
            "gemini" | "gemini-cli" => Some(Self::Gemini),
            _ => None,
        }
    }
}

/// Detect which coding systems are present in a directory.
pub fn detect_systems(dir: &Path) -> Vec<System> {
    let mut found = Vec::new();

    if dir.join(".claude").exists() {
        found.push(System::ClaudeCode);
    }
    if dir.join(".cursor").exists() || dir.join(".cursorrules").exists() {
        found.push(System::Cursor);
    }
    if dir.join(".windsurf").exists() || dir.join(".windsurfrules").exists() {
        found.push(System::Windsurf);
    }
    if dir.join(".aider.conf.yml").exists() || dir.join("CONVENTIONS.md").exists() {
        found.push(System::Aider);
    }
    if dir.join(".github/copilot-instructions.md").exists() {
        found.push(System::Copilot);
    }
    if dir.join("AGENTS.md").exists() || dir.join(".codex").exists() {
        found.push(System::Codex);
    }
    if dir.join("GEMINI.md").exists() || dir.join(".gemini").exists() {
        found.push(System::Gemini);
    }

    found
}

/// Version marker embedded in installed instruction files. Used to detect whether
/// the installed instructions match the running binary, so stale copies can be
/// refreshed automatically.
const VERSION_MARKER: &str = concat!("<!-- creft:", env!("CARGO_PKG_VERSION"), " -->");

/// The creft instruction content — teaches the LLM about creft.
const CREFT_INSTRUCTIONS: &str = concat!(
    "\
# creft

CLI that runs markdown-defined commands as subcommands. Commands persist
between sessions -- create once, use everywhere.

## When to use creft

  Reusable workflow       creft <name> [args] [--flags]
  Check what exists       creft list
  Drill into namespace    creft list <namespace>
  Understand a command    creft <name> --help
  See full definition     creft show <name>
  Save a new command      creft add --help (for format reference)

## Decision triggers

  Want to run a project task?     Check `creft list` first -- it may exist
  Repeating a shell recipe?       Save it: `creft add <<'EOF' ... EOF`
  Need a command's syntax?        Run `creft <name> --help`, not memory
",
    concat!("<!-- creft:", env!("CARGO_PKG_VERSION"), " -->"),
    "\n"
);

/// Install creft for `system` into `project_dir` (or the user's home when
/// `global` is true).
///
/// For hook-native harnesses (Claude Code, Gemini), merges a `SessionStart`
/// hook entry into the harness's JSON settings file. For all other harnesses,
/// writes a static instruction file.
///
/// # Errors
///
/// Returns an error if the target file cannot be created or written, or if
/// `global` requires a home directory that is not set.
pub fn install(
    ctx: &crate::model::AppContext,
    system: System,
    project_dir: &Path,
    global: bool,
) -> Result<PathBuf, CreftError> {
    let home_dir = ctx.home_dir.as_deref();
    match system.strategy() {
        InstallStrategy::Hook => install_hook(ctx, system, project_dir, global, home_dir),
        InstallStrategy::StaticFile => install_static(system, project_dir, global, home_dir),
    }
}

/// Install a session-start hook for a Tier 1 harness (Claude Code or Gemini).
fn install_hook(
    _ctx: &crate::model::AppContext,
    system: System,
    project_dir: &Path,
    global: bool,
    home_dir: Option<&Path>,
) -> Result<PathBuf, CreftError> {
    match system {
        System::ClaudeCode => install_hook_claude_code(project_dir, global, home_dir),
        System::Gemini => install_hook_gemini(project_dir, global, home_dir),
        _ => unreachable!("install_hook called for non-hook system {system:?}"),
    }
}

/// Write a static instruction file for a Tier 2/3 harness.
///
/// This is the original `install()` body, unchanged in behavior.
fn install_static(
    system: System,
    project_dir: &Path,
    global: bool,
    home_dir: Option<&Path>,
) -> Result<PathBuf, CreftError> {
    let (path, content) = match system {
        System::Cursor => install_cursor(project_dir, global)?,
        System::Windsurf => install_windsurf(project_dir, global)?,
        System::Aider => install_aider(project_dir, global, home_dir)?,
        System::Copilot => install_copilot(project_dir, global)?,
        System::Codex => install_codex(project_dir, global, home_dir)?,
        System::ClaudeCode | System::Gemini => {
            unreachable!("install_static called for hook system {system:?}")
        }
    };

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    if path.exists() {
        let existing = std::fs::read_to_string(&path)?;

        if existing.contains(VERSION_MARKER) {
            eprintln!(
                "  skipped: {} (creft instructions are current)",
                path.display()
            );
            return Ok(path);
        }

        if existing.contains("# creft") {
            // Creft-owned files are replaced wholesale; shared files (e.g. CONVENTIONS.md)
            // get only the creft section replaced so other content is preserved.
            if is_creft_owned_file(system) {
                std::fs::write(&path, &content)?;
                eprintln!(
                    "  updated: {} (refreshed creft instructions)",
                    path.display()
                );
            } else {
                let updated = replace_creft_section(&existing, &content);
                std::fs::write(&path, updated)?;
                eprintln!("  updated: {} (refreshed creft section)", path.display());
            }
            return Ok(path);
        }

        let merged = format!("{}\n\n{}", existing.trim_end(), content);
        std::fs::write(&path, merged)?;
        eprintln!(
            "  updated: {} (appended creft instructions)",
            path.display()
        );
    } else {
        std::fs::write(&path, &content)?;
        eprintln!("  created: {}", path.display());
    }

    Ok(path)
}

/// Returns true if the target file is exclusively owned by creft
/// (i.e., creft created it and no other content is expected).
///
/// Claude Code and Gemini are not listed here because they use hook-based
/// installation — creft owns an entry within a shared JSON config, not the
/// whole file.
fn is_creft_owned_file(system: System) -> bool {
    match system {
        System::Cursor | System::Windsurf => true,
        System::Aider | System::Copilot | System::Codex => false,
        System::ClaudeCode | System::Gemini => false,
    }
}

/// Replace the creft section in a shared file.
///
/// Looks for a line starting with `# creft` and ending at the next top-level
/// heading or end of file. Replaces that range with `new_content`. Falls back
/// to appending if no creft heading is found.
fn replace_creft_section(existing: &str, new_content: &str) -> String {
    let lines: Vec<&str> = existing.lines().collect();

    let start = lines.iter().position(|line| {
        let trimmed = line.trim();
        trimmed.eq_ignore_ascii_case("# creft")
            || trimmed.to_ascii_lowercase().starts_with("# creft ")
            || trimmed.to_ascii_lowercase().starts_with("# creft\u{2014}")
            || trimmed.to_ascii_lowercase().starts_with("# creft --")
    });

    let Some(start_idx) = start else {
        return format!("{}\n\n{}", existing.trim_end(), new_content);
    };

    // Stop at the next top-level H1 (not `##` or deeper); everything under the
    // creft heading is considered part of the creft section.
    let end_idx = lines[start_idx + 1..]
        .iter()
        .position(|line| {
            let trimmed = line.trim();
            trimmed.starts_with("# ") && !trimmed.starts_with("## ")
        })
        .map(|pos| start_idx + 1 + pos)
        .unwrap_or(lines.len());

    let mut result = String::new();

    if start_idx > 0 {
        for line in &lines[..start_idx] {
            result.push_str(line);
            result.push('\n');
        }
    }

    result.push_str(new_content.trim_end());
    result.push('\n');

    if end_idx < lines.len() {
        result.push('\n');
        for line in &lines[end_idx..] {
            result.push_str(line);
            result.push('\n');
        }
    }

    result
}

/// Read a JSON config file, apply a mutation, and write it back.
///
/// Creates the file with an empty JSON object `{}` if it does not exist.
/// Creates parent directories as needed.
///
/// # Errors
///
/// Returns an error if the existing file contains invalid JSON, if the
/// mutator returns an error, or if the file cannot be read or written.
/// The file is never truncated before a successful parse.
fn read_modify_write_json(
    path: &Path,
    mutate: impl FnOnce(&mut serde_json::Value) -> Result<(), CreftError>,
) -> Result<(), CreftError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut value: serde_json::Value = if path.exists() {
        let raw = std::fs::read_to_string(path)?;
        serde_json::from_str(&raw)
            .map_err(|e| CreftError::Setup(format!("failed to parse {}: {}", path.display(), e)))?
    } else {
        serde_json::Value::Object(serde_json::Map::new())
    };

    mutate(&mut value)?;

    let serialized = serde_json::to_string_pretty(&value)
        .map_err(|e| CreftError::Serialization(e.to_string()))?;
    std::fs::write(path, serialized + "\n")?;
    Ok(())
}

/// Merge the creft `SessionStart` hook entry into a Claude Code or Gemini
/// settings file.
///
/// Navigates to `value["hooks"]["SessionStart"]`, finds or creates the creft
/// entry (identified by `"creft_managed": true` on the inner hook object), and
/// updates `command` and `creft_version` in place.
///
/// # Errors
///
/// Returns an error with the file path for context if the root value is not a
/// JSON object, if the `hooks` key maps to a non-object value, or if the
/// `SessionStart` key maps to a non-array value. These indicate a corrupt or
/// hand-edited settings file.
fn merge_session_start_hook(
    value: &mut serde_json::Value,
    command: &str,
    timeout: u64,
    path: &Path,
) -> Result<(), CreftError> {
    use serde_json::{Value, json};

    if !value.is_object() {
        return Err(CreftError::Setup(format!(
            "expected JSON object at root of {}, got {}",
            path.display(),
            value_type_name(value),
        )));
    }
    let hooks = value
        .as_object_mut()
        .expect("checked above")
        .entry("hooks")
        .or_insert_with(|| json!({}));

    if !hooks.is_object() {
        return Err(CreftError::Setup(format!(
            "expected \"hooks\" to be a JSON object in {}, got {}",
            path.display(),
            value_type_name(hooks),
        )));
    }
    let session_start = hooks
        .as_object_mut()
        .expect("checked above")
        .entry("SessionStart")
        .or_insert_with(|| Value::Array(vec![]));

    if !session_start.is_array() {
        return Err(CreftError::Setup(format!(
            "expected \"hooks.SessionStart\" to be a JSON array in {}, got {}",
            path.display(),
            value_type_name(session_start),
        )));
    }
    let entries = session_start.as_array_mut().expect("checked above");

    // Search for an existing creft-managed entry (identified by creft_managed: true
    // on the inner hook object within a matcher group).
    let creft_entry_pos = entries.iter().position(|group| {
        group
            .get("hooks")
            .and_then(|h| h.as_array())
            .map(|hooks| {
                hooks
                    .iter()
                    .any(|h| h.get("creft_managed") == Some(&json!(true)))
            })
            .unwrap_or(false)
    });

    let version = env!("CARGO_PKG_VERSION");

    if let Some(pos) = creft_entry_pos {
        // Update the existing entry in place.
        if let Some(inner_hooks) = entries[pos].get_mut("hooks").and_then(|h| h.as_array_mut()) {
            for hook in inner_hooks.iter_mut() {
                if hook.get("creft_managed") == Some(&json!(true)) {
                    hook["command"] = json!(command);
                    hook["creft_version"] = json!(version);
                    hook["timeout"] = json!(timeout);
                }
            }
        }
    } else {
        // Append a new matcher group with the creft hook.
        entries.push(json!({
            "matcher": "",
            "hooks": [
                {
                    "type": "command",
                    "command": command,
                    "timeout": timeout,
                    "creft_managed": true,
                    "creft_version": version
                }
            ]
        }));
    }

    Ok(())
}

/// Returns a human-readable type name for a JSON value, used in error messages.
fn value_type_name(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

/// Returns `true` if the creft-managed hook entry in `settings_path` already
/// carries `creft_version` equal to the running binary's version.
///
/// Returns `false` if the file does not exist, cannot be parsed, or has no
/// creft-managed entry.
fn hook_version_is_current(settings_path: &Path) -> bool {
    let Ok(raw) = std::fs::read_to_string(settings_path) else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return false;
    };
    let version = env!("CARGO_PKG_VERSION");
    let Some(entries) = value["hooks"]["SessionStart"].as_array() else {
        return false;
    };
    entries.iter().any(|group| {
        group
            .get("hooks")
            .and_then(|h| h.as_array())
            .map(|hooks| {
                hooks.iter().any(|h| {
                    h.get("creft_managed") == Some(&serde_json::json!(true))
                        && h.get("creft_version").and_then(|v| v.as_str()) == Some(version)
                })
            })
            .unwrap_or(false)
    })
}

/// Install a `SessionStart` hook into `.claude/settings.json` and write the
/// skill fallback at `.claude/skills/creft/SKILL.md`.
///
/// The hook entry is merged into the existing settings file; other hooks and
/// user configuration are preserved. The skill file is written as a resilience
/// measure against the known Claude Code SessionStart stdout bug (GitHub #13650).
///
/// Skips the write if the hook entry already carries the current `creft_version`.
fn install_hook_claude_code(
    project_dir: &Path,
    global: bool,
    home_dir: Option<&Path>,
) -> Result<PathBuf, CreftError> {
    let base = if global {
        home_dir
            .ok_or_else(|| {
                CreftError::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "could not determine home directory. Set HOME (or USERPROFILE on Windows).",
                ))
            })?
            .to_path_buf()
    } else {
        project_dir.to_path_buf()
    };

    let settings_path = base.join(".claude/settings.json");

    if hook_version_is_current(&settings_path) {
        eprintln!(
            "  skipped: {} (creft instructions are current)",
            settings_path.display()
        );
        return Ok(settings_path);
    }

    read_modify_write_json(&settings_path, |value| {
        merge_session_start_hook(value, "creft _creft session start", 10, &settings_path)
    })?;

    eprintln!(
        "  created: {} (session start hook)",
        settings_path.display()
    );

    // Write the skill fallback — resilience against the known SessionStart stdout bug.
    let (skill_path, skill_content) = install_claude_code(project_dir, global, home_dir)?;
    if let Some(parent) = skill_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&skill_path, &skill_content)?;

    Ok(settings_path)
}

/// Install a `SessionStart` hook into `.gemini/settings.json`.
///
/// The hook command pipes through `jq` to produce the JSON-wrapped output
/// Gemini CLI requires. The timeout is in milliseconds (Gemini CLI convention).
///
/// Skips the write if the hook entry already carries the current `creft_version`.
fn install_hook_gemini(
    project_dir: &Path,
    global: bool,
    home_dir: Option<&Path>,
) -> Result<PathBuf, CreftError> {
    let base = if global {
        home_dir
            .ok_or_else(|| {
                CreftError::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "could not determine home directory. Set HOME (or USERPROFILE on Windows).",
                ))
            })?
            .to_path_buf()
    } else {
        project_dir.to_path_buf()
    };

    let settings_path = base.join(".gemini/settings.json");

    if hook_version_is_current(&settings_path) {
        eprintln!(
            "  skipped: {} (creft instructions are current)",
            settings_path.display()
        );
        return Ok(settings_path);
    }

    // Gemini hooks must output JSON to stdout; plain text causes a parse error.
    // The jq pipeline wraps the skill's plain text output in the required structure.
    let command =
        r#"creft _creft session start | jq -Rs '{"hookSpecificOutput": {"additionalContext": .}}'"#;

    read_modify_write_json(&settings_path, |value| {
        // Gemini CLI uses milliseconds for timeouts (10000 = 10 seconds),
        // unlike Claude Code which uses seconds.
        merge_session_start_hook(value, command, 10000, &settings_path)
    })?;

    eprintln!(
        "  created: {} (session start hook)",
        settings_path.display()
    );

    Ok(settings_path)
}

fn install_claude_code(
    project_dir: &Path,
    global: bool,
    home_dir: Option<&std::path::Path>,
) -> Result<(PathBuf, String), CreftError> {
    let base = if global {
        home_dir
            .ok_or_else(|| {
                CreftError::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "could not determine home directory. Set HOME (or USERPROFILE on Windows).",
                ))
            })?
            .to_path_buf()
    } else {
        project_dir.to_path_buf()
    };

    let path = base.join(".claude/skills/creft/SKILL.md");

    let content = format!(
        "\
---
name: creft
description: Saves reusable CLI commands as markdown. Use when you discover a useful shell recipe, API call, or workflow you want to save for later reuse. Trigger on repeated command patterns or when the user asks to save a command.
---

{}",
        CREFT_INSTRUCTIONS
    );

    Ok((path, content))
}

fn install_cursor(project_dir: &Path, global: bool) -> Result<(PathBuf, String), CreftError> {
    if global {
        return Err(CreftError::Setup(
            "Cursor global rules must be set in Cursor Settings > Rules. \
             Use project-level instead, or manually add to Cursor settings."
                .into(),
        ));
    }

    // Must be .mdc — the .mdc extension is required for Cursor's rules engine.
    let path = project_dir.join(".cursor/rules/creft.mdc");

    let content = format!(
        "\
---
description: creft is installed in this project. Use it to discover and run reusable skills (creft list, creft <skill>), create new skills (creft add), and install plugins (creft plugin install <git-url>).
globs:
alwaysApply: true
---

{}",
        CREFT_INSTRUCTIONS
    );

    Ok((path, content))
}

fn install_windsurf(project_dir: &Path, global: bool) -> Result<(PathBuf, String), CreftError> {
    if global {
        return Err(CreftError::Setup(
            "Windsurf global rules must be set in Windsurf settings. \
             Use project-level instead."
                .into(),
        ));
    }

    let path = project_dir.join(".windsurf/rules/creft.md");

    let content = format!(
        "\
---
description: creft is installed in this project. Use it to discover and run reusable skills.
trigger: always
---

{}",
        CREFT_INSTRUCTIONS
    );

    Ok((path, content))
}

fn install_aider(
    project_dir: &Path,
    global: bool,
    home_dir: Option<&std::path::Path>,
) -> Result<(PathBuf, String), CreftError> {
    if global {
        // Modifying ~/.aider.conf.yml would be risky; write a standalone
        // CONVENTIONS file the user can reference from their config instead.
        let home = home_dir
            .ok_or_else(|| {
                CreftError::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "could not determine home directory. Set HOME (or USERPROFILE on Windows).",
                ))
            })?
            .to_path_buf();
        let path = home.join(".creft/CONVENTIONS-creft.md");
        eprintln!(
            "  note: add 'read: {}' to ~/.aider.conf.yml to activate globally",
            path.display()
        );
        return Ok((path, CREFT_INSTRUCTIONS.to_string()));
    }

    let path = project_dir.join("CONVENTIONS.md");
    Ok((path, CREFT_INSTRUCTIONS.to_string()))
}

fn install_copilot(project_dir: &Path, global: bool) -> Result<(PathBuf, String), CreftError> {
    if global {
        return Err(CreftError::Setup(
            "GitHub Copilot global instructions require an organization .github-private repo. \
             Use project-level instead."
                .into(),
        ));
    }

    let path = project_dir.join(".github/copilot-instructions.md");
    Ok((path, CREFT_INSTRUCTIONS.to_string()))
}

fn install_codex(
    project_dir: &Path,
    global: bool,
    home_dir: Option<&std::path::Path>,
) -> Result<(PathBuf, String), CreftError> {
    if global {
        let home = home_dir
            .ok_or_else(|| {
                CreftError::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "could not determine home directory. Set HOME (or USERPROFILE on Windows).",
                ))
            })?
            .to_path_buf();
        let path = home.join(".codex").join("instructions.md");
        return Ok((path, CREFT_INSTRUCTIONS.to_string()));
    }

    let path = project_dir.join("AGENTS.md");
    Ok((path, CREFT_INSTRUCTIONS.to_string()))
}

/// Ensure the `_creft session start` skill exists on disk at the expected location.
///
/// Creates or updates the file if the content has changed. The skill is written
/// to the `.creft/commands/_creft/session/start.md` path within the target scope:
/// - Project-level: inside `project_dir`
/// - Global: inside `~/.creft/`
///
/// Returns the path to the written skill file.
///
/// # Errors
///
/// Returns an error if `global` is `true` and no home directory is available,
/// or if the file cannot be created or written.
pub fn ensure_session_skill(
    ctx: &crate::model::AppContext,
    project_dir: &Path,
    global: bool,
) -> Result<PathBuf, CreftError> {
    let root = if global {
        ctx.home_dir
            .as_deref()
            .ok_or_else(|| {
                CreftError::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "could not determine home directory. Set HOME (or USERPROFILE on Windows).",
                ))
            })?
            .join(".creft")
    } else {
        project_dir.join(".creft")
    };

    let skill_path = root.join("commands/_creft/session/start.md");

    if let Some(parent) = skill_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Skip the write if the content is already current — avoids unnecessary git noise.
    if skill_path.exists() {
        let existing = std::fs::read_to_string(&skill_path)?;
        if existing == SESSION_SKILL_CONTENT {
            return Ok(skill_path);
        }
    }

    std::fs::write(&skill_path, SESSION_SKILL_CONTENT)?;
    Ok(skill_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use rstest::rstest;
    use tempfile::TempDir;

    /// Construct an AppContext for tests that don't need a real home directory.
    /// Uses a temp dir as home so global paths resolve without env var mutation.
    fn ctx_with_home(home_dir: &std::path::Path) -> crate::model::AppContext {
        crate::model::AppContext::for_test(home_dir.to_path_buf(), home_dir.to_path_buf())
    }

    /// Construct an AppContext for project-level tests that don't need home.
    fn ctx_no_home(cwd: &std::path::Path) -> crate::model::AppContext {
        crate::model::AppContext::for_test(cwd.to_path_buf(), cwd.to_path_buf())
    }

    // ── System enum ───────────────────────────────────────────────────────────

    #[test]
    fn test_system_names_round_trip() {
        // Every variant must round-trip through name() -> from_name().
        for sys in System::all() {
            let name = sys.name();
            let recovered = System::from_name(name);
            assert_eq!(
                recovered,
                Some(*sys),
                "from_name({name:?}) should return {sys:?}"
            );
        }
    }

    #[test]
    fn test_system_aliases() {
        // Verify aliases map to the expected variants.
        assert_eq!(System::from_name("gemini-cli"), Some(System::Gemini));
        assert_eq!(System::from_name("claude"), Some(System::ClaudeCode));
        assert_eq!(System::from_name("github-copilot"), Some(System::Copilot));
        assert_eq!(System::from_name("openai-codex"), Some(System::Codex));
        assert_eq!(System::from_name("unknown"), None);
    }

    #[test]
    fn test_system_all_includes_gemini() {
        let all = System::all();
        assert_eq!(all.len(), 7);
        assert!(
            all.contains(&System::Gemini),
            "System::all() must include Gemini"
        );
    }

    // ── Detection ─────────────────────────────────────────────────────────────

    #[test]
    fn test_detect_no_gemini_when_absent() {
        let dir = TempDir::new().unwrap();
        let detected = detect_systems(dir.path());
        assert!(!detected.contains(&System::Gemini));
    }

    // ── Gemini hook install ───────────────────────────────────────────────────

    #[test]
    fn test_install_gemini_project_hook() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx_no_home(dir.path());
        let path = install(&ctx, System::Gemini, dir.path(), false).unwrap();
        assert_eq!(path, dir.path().join(".gemini/settings.json"));
        let content = std::fs::read_to_string(&path).unwrap();
        let value: serde_json::Value = serde_json::from_str(&content).unwrap();
        let hooks = value["hooks"]["SessionStart"].as_array().unwrap();
        assert!(
            !hooks.is_empty(),
            "SessionStart must have at least one entry"
        );
        let inner = &hooks[0]["hooks"].as_array().unwrap()[0];
        assert_eq!(inner["creft_managed"], serde_json::json!(true));
        assert!(
            inner["command"].as_str().unwrap().contains("jq"),
            "Gemini command must include jq pipeline"
        );
        assert_eq!(
            inner["timeout"],
            serde_json::json!(10000),
            "Gemini timeout must be in milliseconds"
        );
    }

    #[test]
    fn test_install_gemini_global_hook() {
        let home_dir = TempDir::new().unwrap();
        let project_dir = TempDir::new().unwrap();
        let ctx = ctx_with_home(home_dir.path());
        let path = install(&ctx, System::Gemini, project_dir.path(), true).unwrap();
        assert_eq!(path, home_dir.path().join(".gemini/settings.json"));
        assert!(path.exists(), "global Gemini settings file must be created");
    }

    #[test]
    fn test_install_gemini_does_not_write_static_file() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx_no_home(dir.path());
        install(&ctx, System::Gemini, dir.path(), false).unwrap();
        assert!(
            !dir.path().join("GEMINI.md").exists(),
            "Gemini hook install must not write GEMINI.md"
        );
    }

    #[test]
    fn test_instructions_contain_discovery_commands() {
        // The instructions must teach agents how to discover what exists before
        // reaching for memory or running a command blindly.
        assert!(
            CREFT_INSTRUCTIONS.contains("creft list"),
            "CREFT_INSTRUCTIONS must include the creft list discovery command"
        );
        assert!(
            CREFT_INSTRUCTIONS.contains("creft add"),
            "CREFT_INSTRUCTIONS must include the creft add creation command"
        );
        assert!(
            CREFT_INSTRUCTIONS.contains("creft show"),
            "CREFT_INSTRUCTIONS must include the creft show command"
        );
    }

    // ── Cursor extension fix ──────────────────────────────────────────────────

    #[test]
    fn test_install_cursor_uses_mdc_extension() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx_no_home(dir.path());
        let path = install(&ctx, System::Cursor, dir.path(), false).unwrap();
        assert!(
            path.to_str().unwrap().ends_with(".mdc"),
            "Cursor rule file must use .mdc extension, got: {}",
            path.display()
        );
        assert_eq!(path, dir.path().join(".cursor/rules/creft.mdc"));
    }

    // ── Codex global path fix ─────────────────────────────────────────────────

    #[test]
    fn test_install_codex_global_path() {
        let home_dir = TempDir::new().unwrap();
        let project_dir = TempDir::new().unwrap();
        let ctx = ctx_with_home(home_dir.path());
        let path = install(&ctx, System::Codex, project_dir.path(), true).unwrap();
        assert_eq!(path, home_dir.path().join(".codex/instructions.md"));
        // Must not use AGENTS.md for global
        assert!(
            !path.to_str().unwrap().ends_with("AGENTS.md"),
            "global codex path must not be AGENTS.md"
        );
    }

    #[test]
    fn test_install_codex_project_path() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx_no_home(dir.path());
        let path = install(&ctx, System::Codex, dir.path(), false).unwrap();
        assert_eq!(path, dir.path().join("AGENTS.md"));
    }

    // ── Version marker / skip / update ───────────────────────────────────────

    #[test]
    fn test_install_skip_current_version() {
        let dir = TempDir::new().unwrap();
        // Use Aider (static file) to test version-skip logic.
        let path = dir.path().join("CONVENTIONS.md");
        let current_marker = VERSION_MARKER;
        std::fs::write(&path, format!("# creft\nsome content\n{current_marker}\n")).unwrap();
        let original_content = std::fs::read_to_string(&path).unwrap();
        let ctx = ctx_no_home(dir.path());

        install(&ctx, System::Aider, dir.path(), false).unwrap();

        let after_content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            original_content, after_content,
            "file should be unchanged when current version is already installed"
        );
    }

    #[test]
    fn test_install_updates_stale_version() {
        let dir = TempDir::new().unwrap();
        // Use Aider (static file) to test stale-version update logic.
        let path = dir.path().join("CONVENTIONS.md");
        std::fs::write(&path, "# creft\nold content without version marker\n").unwrap();
        let ctx = ctx_no_home(dir.path());

        install(&ctx, System::Aider, dir.path(), false).unwrap();

        let after_content = std::fs::read_to_string(&path).unwrap();
        assert!(
            after_content.contains(VERSION_MARKER),
            "stale instructions should be updated to include current version marker"
        );
        assert!(
            !after_content.contains("old content without version marker"),
            "old content should be replaced"
        );
    }

    #[test]
    fn test_install_upgrades_old_marker_to_current() {
        let dir = TempDir::new().unwrap();
        // Use Aider (static file) to test old-marker upgrade.
        let path = dir.path().join("CONVENTIONS.md");
        std::fs::write(
            &path,
            "# creft\ncreft list\ncreft add\n<!-- creft:0.1.0 -->\n",
        )
        .unwrap();
        let ctx = ctx_no_home(dir.path());

        install(&ctx, System::Aider, dir.path(), false).unwrap();

        let after_content = std::fs::read_to_string(&path).unwrap();
        assert!(
            after_content.contains(VERSION_MARKER),
            "old marker should be upgraded to current version marker"
        );
        assert!(
            !after_content.contains("<!-- creft:0.1.0 -->"),
            "old marker should not remain after upgrade"
        );
        assert!(
            after_content.contains("creft list"),
            "updated content should use current command paths"
        );
    }

    // ── replace_creft_section ─────────────────────────────────────────────────

    #[test]
    fn test_replace_creft_section_replaces_content() {
        let existing = "# Other Section\nother content\n\n# creft\nold creft stuff\n";
        let new_content = &format!("# creft\nnew creft content\n{VERSION_MARKER}\n");
        let result = replace_creft_section(existing, new_content);
        assert!(result.contains("# Other Section"));
        assert!(result.contains("other content"));
        assert!(result.contains("new creft content"));
        assert!(!result.contains("old creft stuff"));
    }

    #[test]
    fn test_replace_creft_section_no_heading_appends() {
        let existing = "# Other Section\nno creft here\n";
        let new_content = "# creft\nnew content\n";
        let result = replace_creft_section(existing, new_content);
        assert!(result.contains("# Other Section"));
        assert!(result.contains("no creft here"));
        assert!(result.contains("# creft\nnew content"));
    }

    #[test]
    fn test_replace_creft_section_preserves_content_after() {
        let existing = "# creft\nold stuff\n\n# After Section\nafter content\n";
        let new_content = &format!("# creft\nnew creft\n{VERSION_MARKER}\n");
        let result = replace_creft_section(existing, new_content);
        assert!(result.contains("new creft"));
        assert!(!result.contains("old stuff"));
        assert!(result.contains("# After Section"));
        assert!(result.contains("after content"));
    }

    // ── Windsurf frontmatter ──────────────────────────────────────────────────

    #[test]
    fn test_install_windsurf_has_frontmatter() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx_no_home(dir.path());
        let path = install(&ctx, System::Windsurf, dir.path(), false).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("trigger: always"));
        assert!(content.contains("description:"));
    }

    // ── is_creft_owned_file ───────────────────────────────────────────────────

    #[test]
    fn test_creft_owned_file_classification() {
        // Hook-based systems (ClaudeCode, Gemini) write into shared JSON — not owned.
        assert!(!is_creft_owned_file(System::ClaudeCode));
        assert!(!is_creft_owned_file(System::Gemini));
        // Static-file systems that own their whole file.
        assert!(is_creft_owned_file(System::Cursor));
        assert!(is_creft_owned_file(System::Windsurf));
        // Static-file systems that share their file (append/section-replace).
        assert!(!is_creft_owned_file(System::Aider));
        assert!(!is_creft_owned_file(System::Copilot));
        assert!(!is_creft_owned_file(System::Codex));
    }

    // ── detect_systems: detection by directory/file markers ──────────────────

    /// `is_dir` controls whether to create a directory or file for the marker.
    #[rstest]
    #[case::gemini_file("GEMINI.md", false, System::Gemini, "# test")]
    #[case::gemini_directory(".gemini", true, System::Gemini, "")]
    #[case::cursor_directory(".cursor", true, System::Cursor, "")]
    #[case::cursor_rules_file(".cursorrules", false, System::Cursor, "rules")]
    #[case::windsurf_directory(".windsurf", true, System::Windsurf, "")]
    #[case::windsurf_rules_file(".windsurfrules", false, System::Windsurf, "rules")]
    #[case::aider_conf_file(".aider.conf.yml", false, System::Aider, "model: gpt-4")]
    #[case::aider_conventions_file("CONVENTIONS.md", false, System::Aider, "# conventions")]
    #[case::copilot_instructions_file(
        ".github/copilot-instructions.md",
        false,
        System::Copilot,
        "# copilot"
    )]
    #[case::codex_agents_file("AGENTS.md", false, System::Codex, "# agents")]
    #[case::codex_directory(".codex", true, System::Codex, "")]
    #[case::claude_code_directory(".claude", true, System::ClaudeCode, "")]
    fn detect_systems_recognizes_marker(
        #[case] marker: &str,
        #[case] is_dir: bool,
        #[case] expected: System,
        #[case] content: &str,
    ) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(marker);
        if is_dir {
            std::fs::create_dir(&path).unwrap();
        } else {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(&path, content).unwrap();
        }
        let detected = detect_systems(dir.path());
        assert!(
            detected.contains(&expected),
            "expected {expected:?} to be detected via {marker}"
        );
    }

    #[test]
    fn detect_systems_github_directory_alone_does_not_trigger_copilot() {
        // Every GitHub-hosted repo has .github/; creft must not auto-install
        // Copilot instructions just because .github/ exists. Only the presence
        // of .github/copilot-instructions.md signals explicit Copilot use.
        let dir = TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join(".github")).unwrap();
        // Also create a common subdirectory that is present in virtually every repo.
        std::fs::create_dir_all(dir.path().join(".github/workflows")).unwrap();
        std::fs::write(dir.path().join(".github/workflows/ci.yml"), "name: CI\n").unwrap();
        let detected = detect_systems(dir.path());
        assert!(
            !detected.contains(&System::Copilot),
            ".github directory alone must not trigger Copilot detection"
        );
    }

    // ── install() paths: append to existing non-creft file ───────────────────

    #[test]
    fn test_install_appends_to_existing_non_creft_file() {
        // For a non-owned system (Aider), install to an existing file that has no creft content.
        let dir = TempDir::new().unwrap();
        let ctx = ctx_no_home(dir.path());
        let conventions_path = dir.path().join("CONVENTIONS.md");
        std::fs::write(&conventions_path, "# Existing conventions\nsome content").unwrap();

        install(&ctx, System::Aider, dir.path(), false).unwrap();

        let content = std::fs::read_to_string(&conventions_path).unwrap();
        assert!(content.contains("# Existing conventions"));
        assert!(content.contains("some content"));
        assert!(
            content.contains("# creft"),
            "creft content should be appended"
        );
    }

    #[test]
    fn test_install_replaces_stale_creft_section_in_shared_file() {
        // Non-owned file (Aider CONVENTIONS.md) with stale creft — should replace creft section
        let dir = TempDir::new().unwrap();
        let ctx = ctx_no_home(dir.path());
        let conventions_path = dir.path().join("CONVENTIONS.md");
        std::fs::write(
            &conventions_path,
            "# creft\nold instructions\n\n# Other section\nother content",
        )
        .unwrap();

        install(&ctx, System::Aider, dir.path(), false).unwrap();

        let content = std::fs::read_to_string(&conventions_path).unwrap();
        assert!(content.contains(VERSION_MARKER));
        assert!(!content.contains("old instructions"));
        // Other section should be preserved
        assert!(content.contains("# Other section"));
        assert!(content.contains("other content"));
    }

    // ── install() global paths that error ────────────────────────────────────

    #[test]
    fn test_install_cursor_global_errors() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx_no_home(dir.path());
        let result = install(&ctx, System::Cursor, dir.path(), true);
        assert!(
            result.is_err(),
            "Cursor global install should return an error"
        );
        let err_str = result.unwrap_err().to_string();
        assert!(
            err_str.contains("Cursor") || err_str.contains("global"),
            "error should mention Cursor or global, got: {err_str}"
        );
    }

    #[test]
    fn test_install_windsurf_global_errors() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx_no_home(dir.path());
        let result = install(&ctx, System::Windsurf, dir.path(), true);
        assert!(
            result.is_err(),
            "Windsurf global install should return an error"
        );
    }

    #[test]
    fn test_install_copilot_global_errors() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx_no_home(dir.path());
        let result = install(&ctx, System::Copilot, dir.path(), true);
        assert!(
            result.is_err(),
            "Copilot global install should return an error"
        );
    }

    // ── install() global paths that require home_dir ─────────────────────────

    #[test]
    fn test_install_claude_code_global_path() {
        let home_dir = TempDir::new().unwrap();
        let project_dir = TempDir::new().unwrap();
        let ctx = ctx_with_home(home_dir.path());
        let path = install(&ctx, System::ClaudeCode, project_dir.path(), true).unwrap();
        // Hook installers return the JSON settings file path, not the skill file.
        assert_eq!(path, home_dir.path().join(".claude/settings.json"));
    }

    #[test]
    fn test_install_aider_global_creates_conventions_file() {
        let home_dir = TempDir::new().unwrap();
        let project_dir = TempDir::new().unwrap();
        let ctx = ctx_with_home(home_dir.path());
        let path = install(&ctx, System::Aider, project_dir.path(), true).unwrap();
        assert_eq!(path, home_dir.path().join(".creft/CONVENTIONS-creft.md"));
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("# creft"));
    }

    #[rstest]
    #[case::aider(System::Aider)]
    #[case::codex(System::Codex)]
    #[case::gemini(System::Gemini)]
    #[case::claude_code(System::ClaudeCode)]
    fn install_global_no_home_errors(#[case] system: System) {
        let project_dir = TempDir::new().unwrap();
        let ctx = crate::model::AppContext {
            home_dir: None,
            creft_home: None,
            cwd: project_dir.path().to_path_buf(),
        };
        let result = install(&ctx, system, project_dir.path(), true);
        assert!(
            result.is_err(),
            "{system:?} global install with no home dir should error"
        );
    }

    // ── Claude Code hook install ──────────────────────────────────────────────

    #[test]
    fn test_install_claude_code_hook_creates_settings_json() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx_no_home(dir.path());
        let path = install(&ctx, System::ClaudeCode, dir.path(), false).unwrap();
        assert_eq!(path, dir.path().join(".claude/settings.json"));
        let content = std::fs::read_to_string(&path).unwrap();
        let value: serde_json::Value = serde_json::from_str(&content).unwrap();
        let hooks = value["hooks"]["SessionStart"].as_array().unwrap();
        assert!(!hooks.is_empty());
        let inner = &hooks[0]["hooks"].as_array().unwrap()[0];
        assert_eq!(inner["creft_managed"], serde_json::json!(true));
        assert_eq!(
            inner["command"],
            serde_json::json!("creft _creft session start")
        );
        assert_eq!(
            inner["timeout"],
            serde_json::json!(10),
            "Claude Code timeout must be in seconds"
        );
    }

    #[test]
    fn test_install_claude_code_hook_also_writes_skill_fallback() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx_no_home(dir.path());
        install(&ctx, System::ClaudeCode, dir.path(), false).unwrap();
        let skill_path = dir.path().join(".claude/skills/creft/SKILL.md");
        assert!(
            skill_path.exists(),
            "skill fallback must be written alongside hook config"
        );
        let content = std::fs::read_to_string(&skill_path).unwrap();
        assert!(
            content.contains("# creft"),
            "skill fallback must contain creft instructions"
        );
    }

    #[test]
    fn test_install_claude_code_hook_merges_preserving_user_hooks() {
        use serde_json::json;
        let dir = TempDir::new().unwrap();
        let ctx = ctx_no_home(dir.path());

        // Pre-write settings.json with an existing user hook.
        let settings_path = dir.path().join(".claude/settings.json");
        std::fs::create_dir_all(settings_path.parent().unwrap()).unwrap();
        let existing = json!({
            "hooks": {
                "SessionStart": [
                    {
                        "matcher": "my-project",
                        "hooks": [
                            { "type": "command", "command": "echo hello" }
                        ]
                    }
                ]
            }
        });
        std::fs::write(
            &settings_path,
            serde_json::to_string_pretty(&existing).unwrap(),
        )
        .unwrap();

        install(&ctx, System::ClaudeCode, dir.path(), false).unwrap();

        let content = std::fs::read_to_string(&settings_path).unwrap();
        let value: serde_json::Value = serde_json::from_str(&content).unwrap();
        let hooks = value["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(
            hooks.len(),
            2,
            "user hook must be preserved alongside creft hook"
        );

        // Verify the user hook is still intact.
        let user_hook = hooks.iter().find(|g| g["matcher"] == "my-project");
        assert!(user_hook.is_some(), "original user hook must survive merge");
    }

    #[test]
    fn test_install_claude_code_hook_idempotent_no_duplicate() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx_no_home(dir.path());

        install(&ctx, System::ClaudeCode, dir.path(), false).unwrap();
        install(&ctx, System::ClaudeCode, dir.path(), false).unwrap();

        let settings_path = dir.path().join(".claude/settings.json");
        let content = std::fs::read_to_string(&settings_path).unwrap();
        let value: serde_json::Value = serde_json::from_str(&content).unwrap();
        let hooks = value["hooks"]["SessionStart"].as_array().unwrap();

        let creft_entries: Vec<_> = hooks
            .iter()
            .filter(|g| {
                g.get("hooks")
                    .and_then(|h| h.as_array())
                    .map(|inner| {
                        inner
                            .iter()
                            .any(|h| h.get("creft_managed") == Some(&serde_json::json!(true)))
                    })
                    .unwrap_or(false)
            })
            .collect();
        assert_eq!(
            creft_entries.len(),
            1,
            "re-running install must update, not duplicate, the creft hook entry"
        );
    }

    // ── Gemini hook: JSON merge ───────────────────────────────────────────────

    #[test]
    fn test_install_gemini_hook_merges_preserving_user_hooks() {
        use serde_json::json;
        let dir = TempDir::new().unwrap();
        let ctx = ctx_no_home(dir.path());

        let settings_path = dir.path().join(".gemini/settings.json");
        std::fs::create_dir_all(settings_path.parent().unwrap()).unwrap();
        let existing = json!({
            "hooks": {
                "SessionStart": [
                    {
                        "matcher": "",
                        "hooks": [
                            { "type": "command", "command": "echo gemini-custom" }
                        ]
                    }
                ]
            }
        });
        std::fs::write(
            &settings_path,
            serde_json::to_string_pretty(&existing).unwrap(),
        )
        .unwrap();

        install(&ctx, System::Gemini, dir.path(), false).unwrap();

        let content = std::fs::read_to_string(&settings_path).unwrap();
        let value: serde_json::Value = serde_json::from_str(&content).unwrap();
        let hooks = value["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(
            hooks.len(),
            2,
            "user hook must be preserved alongside creft hook"
        );
    }

    #[test]
    fn test_install_gemini_hook_idempotent_no_duplicate() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx_no_home(dir.path());

        install(&ctx, System::Gemini, dir.path(), false).unwrap();
        install(&ctx, System::Gemini, dir.path(), false).unwrap();

        let settings_path = dir.path().join(".gemini/settings.json");
        let content = std::fs::read_to_string(&settings_path).unwrap();
        let value: serde_json::Value = serde_json::from_str(&content).unwrap();
        let hooks = value["hooks"]["SessionStart"].as_array().unwrap();

        let creft_entries: Vec<_> = hooks
            .iter()
            .filter(|g| {
                g.get("hooks")
                    .and_then(|h| h.as_array())
                    .map(|inner| {
                        inner
                            .iter()
                            .any(|h| h.get("creft_managed") == Some(&serde_json::json!(true)))
                    })
                    .unwrap_or(false)
            })
            .collect();
        assert_eq!(
            creft_entries.len(),
            1,
            "re-running install must update, not duplicate, the creft hook entry"
        );
    }

    // ── read_modify_write_json ────────────────────────────────────────────────

    #[test]
    fn read_modify_write_json_creates_file_when_missing() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("subdir/settings.json");
        read_modify_write_json(&path, |v| {
            v["key"] = serde_json::json!("value");
            Ok(())
        })
        .unwrap();
        assert!(path.exists());
        let content = std::fs::read_to_string(&path).unwrap();
        let val: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(val["key"], serde_json::json!("value"));
    }

    #[test]
    fn read_modify_write_json_preserves_existing_keys() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, r#"{"existing": "data"}"#).unwrap();
        read_modify_write_json(&path, |v| {
            v["new_key"] = serde_json::json!(42);
            Ok(())
        })
        .unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        let val: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(val["existing"], serde_json::json!("data"));
        assert_eq!(val["new_key"], serde_json::json!(42));
    }

    #[test]
    fn read_modify_write_json_errors_on_invalid_json() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, "not valid json {{").unwrap();
        let result = read_modify_write_json(&path, |_| Ok(()));
        assert!(
            result.is_err(),
            "invalid JSON must produce an error, not silently overwrite"
        );
        let err_str = result.unwrap_err().to_string();
        assert!(
            err_str.contains("settings.json"),
            "error must include the file path: {err_str}"
        );
    }

    #[test]
    fn merge_session_start_hook_errors_on_root_json_array() {
        // A settings file whose root is an array (not an object) must produce a
        // descriptive error naming the file — not a panic.
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, "[]").unwrap();
        let result = read_modify_write_json(&path, |value| {
            merge_session_start_hook(value, "creft _creft session start", 10, &path)
        });
        assert!(result.is_err(), "root JSON array must produce an error");
        let err_str = result.unwrap_err().to_string();
        assert!(
            err_str.contains("settings.json"),
            "error must include the file path: {err_str}"
        );
        assert!(
            err_str.contains("object"),
            "error must mention expected type: {err_str}"
        );
    }

    #[test]
    fn merge_session_start_hook_errors_on_hooks_non_object() {
        // A settings file where "hooks" maps to a string (not an object) must
        // produce a descriptive error — not a panic.
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, r#"{"hooks": "not-an-object"}"#).unwrap();
        let result = read_modify_write_json(&path, |value| {
            merge_session_start_hook(value, "creft _creft session start", 10, &path)
        });
        assert!(
            result.is_err(),
            "non-object hooks value must produce an error"
        );
        let err_str = result.unwrap_err().to_string();
        assert!(
            err_str.contains("settings.json"),
            "error must include the file path: {err_str}"
        );
        assert!(
            err_str.contains("hooks"),
            "error must mention the hooks key: {err_str}"
        );
    }

    // ── InstallStrategy dispatch ──────────────────────────────────────────────

    #[test]
    fn test_install_strategy_hook_systems() {
        assert_eq!(System::ClaudeCode.strategy(), InstallStrategy::Hook);
        assert_eq!(System::Gemini.strategy(), InstallStrategy::Hook);
    }

    #[test]
    fn test_install_strategy_static_file_systems() {
        assert_eq!(System::Cursor.strategy(), InstallStrategy::StaticFile);
        assert_eq!(System::Windsurf.strategy(), InstallStrategy::StaticFile);
        assert_eq!(System::Aider.strategy(), InstallStrategy::StaticFile);
        assert_eq!(System::Copilot.strategy(), InstallStrategy::StaticFile);
        assert_eq!(System::Codex.strategy(), InstallStrategy::StaticFile);
    }

    // ── install() creates new file path with parent dirs ─────────────────────

    #[test]
    fn test_install_creates_parent_directories() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx_no_home(dir.path());
        // Cursor project-level writes into .cursor/rules/creft.mdc —
        // parent dirs don't exist yet.
        let path = install(&ctx, System::Cursor, dir.path(), false).unwrap();
        assert!(
            path.exists(),
            "file should be created including parent dirs"
        );
    }

    // ── replace_creft_section: heading case-insensitive match ────────────────

    #[test]
    fn test_replace_creft_section_case_insensitive() {
        // "# CREFT" should match as a creft heading
        let existing = "# CREFT\nold stuff\n";
        let new_content = &format!("# creft\nnew stuff\n{VERSION_MARKER}\n");
        let result = replace_creft_section(existing, new_content);
        assert!(result.contains("new stuff"));
        assert!(!result.contains("old stuff"));
    }

    // ── ensure_session_skill ──────────────────────────────────────────────────

    #[test]
    fn ensure_session_skill_creates_file_at_project_path() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx_no_home(dir.path());
        let path = ensure_session_skill(&ctx, dir.path(), false).unwrap();

        assert_eq!(
            path,
            dir.path().join(".creft/commands/_creft/session/start.md")
        );
        assert!(path.exists(), "skill file must be created");
    }

    #[test]
    fn ensure_session_skill_writes_correct_content() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx_no_home(dir.path());
        let path = ensure_session_skill(&ctx, dir.path(), false).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, SESSION_SKILL_CONTENT);
    }

    #[test]
    fn ensure_session_skill_content_has_valid_frontmatter() {
        // The skill must have name and description fields so creft can parse it.
        assert!(
            SESSION_SKILL_CONTENT.contains("name: _creft session start"),
            "skill must declare name: _creft session start"
        );
        assert!(
            SESSION_SKILL_CONTENT.contains("description:"),
            "skill must have a description field"
        );
    }

    #[test]
    fn ensure_session_skill_content_includes_creft_reference() {
        // Agents must be able to discover and run skills from the session context.
        assert!(
            SESSION_SKILL_CONTENT.contains("creft list"),
            "skill output must include discovery command"
        );
        assert!(
            SESSION_SKILL_CONTENT.contains("creft add"),
            "skill output must include creation command"
        );
        assert!(
            SESSION_SKILL_CONTENT.contains("creft show"),
            "skill output must include show command"
        );
    }

    #[test]
    fn ensure_session_skill_content_has_dynamic_availability_check() {
        // The skill must check for creft on PATH before running creft list,
        // so that harnesses without creft in their environment fail silently.
        assert!(
            SESSION_SKILL_CONTENT.contains("command -v creft"),
            "skill must guard creft list with a command -v check"
        );
        assert!(
            SESSION_SKILL_CONTENT.contains("creft list"),
            "skill must include dynamic creft list invocation"
        );
    }

    #[test]
    fn ensure_session_skill_is_idempotent() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx_no_home(dir.path());

        let path1 = ensure_session_skill(&ctx, dir.path(), false).unwrap();
        let mtime1 = std::fs::metadata(&path1).unwrap().modified().unwrap();

        // A brief pause is needed on filesystems with 1-second mtime resolution
        // to detect whether the second call modified the file.
        std::thread::sleep(std::time::Duration::from_millis(10));

        let path2 = ensure_session_skill(&ctx, dir.path(), false).unwrap();
        let mtime2 = std::fs::metadata(&path2).unwrap().modified().unwrap();

        assert_eq!(path1, path2, "path must be the same on both calls");
        assert_eq!(
            mtime1, mtime2,
            "file must not be rewritten when content is already current"
        );
    }

    #[test]
    fn ensure_session_skill_updates_stale_content() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx_no_home(dir.path());
        let skill_path = dir.path().join(".creft/commands/_creft/session/start.md");

        // Pre-write outdated content.
        std::fs::create_dir_all(skill_path.parent().unwrap()).unwrap();
        std::fs::write(
            &skill_path,
            "---\nname: _creft session start\n---\nold content\n",
        )
        .unwrap();

        ensure_session_skill(&ctx, dir.path(), false).unwrap();

        let content = std::fs::read_to_string(&skill_path).unwrap();
        assert_eq!(
            content, SESSION_SKILL_CONTENT,
            "stale skill content must be replaced with current content"
        );
    }

    #[test]
    fn ensure_session_skill_global_writes_to_home_creft() {
        let home_dir = TempDir::new().unwrap();
        let project_dir = TempDir::new().unwrap();
        let ctx = ctx_with_home(home_dir.path());

        let path = ensure_session_skill(&ctx, project_dir.path(), true).unwrap();

        assert_eq!(
            path,
            home_dir
                .path()
                .join(".creft/commands/_creft/session/start.md")
        );
        assert!(path.exists(), "global skill file must be created");
    }

    #[test]
    fn ensure_session_skill_global_no_home_errors() {
        let project_dir = TempDir::new().unwrap();
        let ctx = crate::model::AppContext {
            home_dir: None,
            creft_home: None,
            cwd: project_dir.path().to_path_buf(),
        };

        let result = ensure_session_skill(&ctx, project_dir.path(), true);
        assert!(
            result.is_err(),
            "global ensure_session_skill with no home dir must error"
        );
    }
}
