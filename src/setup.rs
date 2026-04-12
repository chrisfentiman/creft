use std::path::{Path, PathBuf};

use crate::error::CreftError;

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
    if dir.join(".github").exists() {
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
# creft -- Executable Skills for AI Agents

creft is a skill system that saves reusable commands as markdown files and
runs them as CLI subcommands. Skills can contain bash, python, node, or
any interpreter -- with arguments, flags, dependency management, and
multi-step pipelines.

## Discovering skills

  creft cmd list                  Show all skills, grouped by namespace
  creft cmd list <namespace>      Drill into a namespace
  creft <skill> --help            See what a skill does and what it accepts
  creft cmd show <skill>          Read the full skill definition

## Running skills

Skills are invoked directly as creft subcommands:

  creft <name> [args...] [--flags...]

## Creating skills

  creft cmd add <<'EOF'
  ---
  name: deploy
  description: Deploys the app to staging or production.
  args:
    - name: env
      description: target environment
  ---

  ```bash
  echo \"Deploying to {{env}}...\"
  ```
  EOF

Run `creft cmd add --help` for the complete format reference.

## Managing skills

  creft cmd list                  List skills
  creft cmd show <name>           View full definition
  creft cmd cat <name>            View code blocks only
  creft cmd rm <name>             Remove a skill
  creft cmd add --force <<'EOF'   Update an existing skill

## Plugins

  creft plugins install <git-url>           Install a plugin
  creft plugins activate <plugin>/<cmd>     Activate a command
  creft plugins list                        List installed plugins

## Skill storage

  Local:   .creft/ in the project directory (travels with the repo)
  Global:  ~/.creft/ (available everywhere)

Local skills shadow global ones with the same name.
",
    concat!("<!-- creft:", env!("CARGO_PKG_VERSION"), " -->"),
    "\n"
);

/// Write creft instructions for `system` into `project_dir` (or the user's home
/// when `global` is true). Skips if the current version marker is already present;
/// updates in-place if an older version is found.
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
    let (path, content) = match system {
        System::ClaudeCode => install_claude_code(project_dir, global, home_dir)?,
        System::Cursor => install_cursor(project_dir, global)?,
        System::Windsurf => install_windsurf(project_dir, global)?,
        System::Aider => install_aider(project_dir, global, home_dir)?,
        System::Copilot => install_copilot(project_dir, global)?,
        System::Codex => install_codex(project_dir, global, home_dir)?,
        System::Gemini => install_gemini(project_dir, global, home_dir)?,
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
fn is_creft_owned_file(system: System) -> bool {
    match system {
        System::ClaudeCode | System::Cursor | System::Windsurf => true,
        System::Aider | System::Copilot | System::Codex | System::Gemini => false,
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
description: creft is installed in this project. Use it to discover and run reusable skills (creft cmd list, creft <skill>), create new skills (creft cmd add), and install plugins (creft plugins install <git-url>).
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

fn install_gemini(
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
        let path = home.join(".gemini").join("instructions.md");
        return Ok((path, CREFT_INSTRUCTIONS.to_string()));
    }

    let path = project_dir.join("GEMINI.md");
    Ok((path, CREFT_INSTRUCTIONS.to_string()))
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

    // ── Gemini install ────────────────────────────────────────────────────────

    #[test]
    fn test_install_gemini_project() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx_no_home(dir.path());
        let path = install(&ctx, System::Gemini, dir.path(), false).unwrap();
        assert_eq!(path, dir.path().join("GEMINI.md"));
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("# creft"));
        assert!(content.contains(VERSION_MARKER));
        assert!(content.contains("creft cmd list"));
    }

    #[test]
    fn test_install_gemini_global() {
        let home_dir = TempDir::new().unwrap();
        let project_dir = TempDir::new().unwrap();
        let ctx = ctx_with_home(home_dir.path());
        let path = install(&ctx, System::Gemini, project_dir.path(), true).unwrap();
        assert_eq!(path, home_dir.path().join(".gemini/instructions.md"));
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("# creft"));
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
        // Pre-write a file that already contains the current version marker.
        let path = dir.path().join("GEMINI.md");
        let current_marker = VERSION_MARKER;
        std::fs::write(
            &path,
            format!("# creft\nsome content\n{current_marker}\n"),
        )
        .unwrap();
        let original_content = std::fs::read_to_string(&path).unwrap();
        let ctx = ctx_no_home(dir.path());

        install(&ctx, System::Gemini, dir.path(), false).unwrap();

        let after_content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            original_content, after_content,
            "file should be unchanged when current version is already installed"
        );
    }

    #[test]
    fn test_install_updates_stale_version() {
        let dir = TempDir::new().unwrap();
        // Pre-write a file with old creft instructions (has # creft heading but no version marker).
        let path = dir.path().join("GEMINI.md");
        std::fs::write(&path, "# creft\nold content without version marker\n").unwrap();
        let ctx = ctx_no_home(dir.path());

        install(&ctx, System::Gemini, dir.path(), false).unwrap();

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
        // Pre-write a file with an older version marker — these are stale and must be replaced.
        let path = dir.path().join("GEMINI.md");
        std::fs::write(
            &path,
            "# creft\ncreft list\ncreft add\n<!-- creft:0.1.0 -->\n",
        )
        .unwrap();
        let ctx = ctx_no_home(dir.path());

        install(&ctx, System::Gemini, dir.path(), false).unwrap();

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
            after_content.contains("creft cmd list"),
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
        assert!(is_creft_owned_file(System::ClaudeCode));
        assert!(is_creft_owned_file(System::Cursor));
        assert!(is_creft_owned_file(System::Windsurf));
        assert!(!is_creft_owned_file(System::Aider));
        assert!(!is_creft_owned_file(System::Copilot));
        assert!(!is_creft_owned_file(System::Codex));
        assert!(!is_creft_owned_file(System::Gemini));
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
    #[case::copilot_github_directory(".github", true, System::Copilot, "")]
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
            std::fs::write(&path, content).unwrap();
        }
        let detected = detect_systems(dir.path());
        assert!(
            detected.contains(&expected),
            "expected {expected:?} to be detected via {marker}"
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
        assert_eq!(path, home_dir.path().join(".claude/skills/creft/SKILL.md"));
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

    // ── install() creates new file path with parent dirs ─────────────────────

    #[test]
    fn test_install_creates_parent_directories() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx_no_home(dir.path());
        // ClaudeCode project-level writes into .claude/skills/creft/SKILL.md
        // The parent dirs don't exist yet.
        let path = install(&ctx, System::ClaudeCode, dir.path(), false).unwrap();
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
}
