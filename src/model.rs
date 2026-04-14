use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use yansi::Paint;

use crate::error::CreftError;

/// Matches `{{name}}` and `{{name|default}}` placeholders in skill templates.
pub(crate) static PLACEHOLDER_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"\{\{([a-zA-Z_][a-zA-Z0-9_-]*)(?:\|([^}]*))?\}\}").unwrap()
});

/// Resolved environment context for the creft CLI.
///
/// Holds all paths that would otherwise require reading process-global state
/// (env vars, CWD). Constructed once at program startup; passed as `&AppContext`
/// to all functions that need path resolution.
///
/// Test code creates `AppContext` directly with temp directory paths,
/// eliminating the need for `#[serial]` and env var mutation.
#[derive(Debug, Clone)]
pub struct AppContext {
    /// User's home directory. Resolved from `$HOME` (Unix) or `$USERPROFILE` (Windows).
    /// `None` if the variable is not set or empty.
    pub home_dir: Option<PathBuf>,

    /// Override root directory. When set, both local and global scopes resolve to this path.
    /// Resolved from `$CREFT_HOME`. `None` if not set or empty.
    pub creft_home: Option<PathBuf>,

    /// Process current working directory at startup.
    pub cwd: PathBuf,
}

impl AppContext {
    /// Construct from the real process environment.
    ///
    /// Reads `$HOME`/`$USERPROFILE`, `$CREFT_HOME`, and `current_dir()`.
    /// Returns `Err` if `current_dir()` fails (deleted CWD).
    pub fn from_env() -> Result<Self, CreftError> {
        let home_dir = Self::read_home_dir();
        let creft_home = std::env::var("CREFT_HOME")
            .ok()
            .filter(|v| !v.is_empty())
            .map(PathBuf::from);
        let cwd = std::env::current_dir().map_err(CreftError::Io)?;

        Ok(Self {
            home_dir,
            creft_home,
            cwd,
        })
    }

    fn read_home_dir() -> Option<PathBuf> {
        #[cfg(windows)]
        let var = "USERPROFILE";
        #[cfg(not(windows))]
        let var = "HOME";

        std::env::var(var)
            .ok()
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    }

    /// Construct for testing with explicit paths.
    ///
    /// All three fields are set directly. No env vars or CWD are read.
    #[cfg(test)]
    pub fn for_test(home_dir: PathBuf, cwd: PathBuf) -> Self {
        Self {
            home_dir: Some(home_dir),
            creft_home: None,
            cwd,
        }
    }

    /// Construct for testing with CREFT_HOME override.
    #[cfg(test)]
    pub fn for_test_with_creft_home(creft_home: PathBuf, cwd: PathBuf) -> Self {
        Self {
            home_dir: None,
            creft_home: Some(creft_home),
            cwd,
        }
    }

    /// Global creft root directory (`~/.creft/`).
    ///
    /// Returns `Err` if `home_dir` is `None` (no HOME set).
    pub fn global_root(&self) -> Result<PathBuf, CreftError> {
        match &self.home_dir {
            Some(home) => Ok(home.join(".creft")),
            None => Err(CreftError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "could not determine home directory. Set HOME (or USERPROFILE on Windows).",
            ))),
        }
    }

    /// Walk up from CWD looking for `.creft/`.
    ///
    /// Returns `None` immediately when `creft_home` is set -- in CREFT_HOME mode
    /// the concept of a local root is meaningless; all scopes resolve to the
    /// CREFT_HOME directory. This guard is structural so callers cannot accidentally
    /// bypass it.
    pub fn find_local_root(&self) -> Option<PathBuf> {
        if self.creft_home.is_some() {
            return None;
        }
        let found = find_local_root_from(&self.cwd)?;
        // ~/.creft/ is the global store, not a project-local root.
        // If the walk-up lands on it, treat that as "no local root".
        // Canonicalize both sides to handle symlinks (e.g. macOS /var -> /private/var).
        if let Ok(global) = self.global_root() {
            let found_canon = std::fs::canonicalize(&found).unwrap_or_else(|_| found.clone());
            let global_canon = std::fs::canonicalize(&global).unwrap_or(global);
            if found_canon == global_canon {
                return None;
            }
        }
        Some(found)
    }

    /// Root directory for a given scope.
    ///
    /// When `creft_home` is set, both scopes resolve to it.
    /// `Local` falls back to global when no local root exists.
    pub fn resolve_root(&self, scope: Scope) -> Result<PathBuf, CreftError> {
        if let Some(home) = &self.creft_home {
            return Ok(home.clone());
        }
        match scope {
            Scope::Local => Ok(self
                .find_local_root()
                .map_or_else(|| self.global_root(), Ok)?),
            Scope::Global => self.global_root(),
        }
    }

    /// Commands directory for the given scope.
    pub fn commands_dir_for(&self, scope: Scope) -> Result<PathBuf, CreftError> {
        Ok(self.resolve_root(scope)?.join("commands"))
    }

    /// Default write scope when no `--global` flag is given.
    pub fn default_write_scope(&self) -> Scope {
        if self.creft_home.is_some() {
            return Scope::Global;
        }
        if self.find_local_root().is_some() {
            Scope::Local
        } else {
            Scope::Global
        }
    }

    /// Packages directory for the given scope.
    pub fn packages_dir_for(&self, scope: Scope) -> Result<PathBuf, CreftError> {
        Ok(self.resolve_root(scope)?.join("packages"))
    }

    /// Global plugin cache directory (`~/.creft/plugins/`).
    ///
    /// Uses `resolve_root(Scope::Global)` so `CREFT_HOME` redirects plugin
    /// storage for test isolation. Install is always global — there is no
    /// per-scope plugin directory.
    pub fn plugins_dir(&self) -> Result<PathBuf, CreftError> {
        Ok(self.resolve_root(Scope::Global)?.join("plugins"))
    }

    /// Path to the plugin activation settings file for a scope.
    ///
    /// Local scope: `.creft/plugins/settings.json` (nearest project root).
    /// Global scope: `~/.creft/plugins/settings.json`.
    pub fn plugin_settings_path(&self, scope: Scope) -> Result<PathBuf, CreftError> {
        Ok(self
            .resolve_root(scope)?
            .join("plugins")
            .join("settings.json"))
    }

    /// Path to the global settings file (`~/.creft/settings.json`).
    pub fn settings_path(&self) -> Result<std::path::PathBuf, CreftError> {
        Ok(self.resolve_root(Scope::Global)?.join("settings.json"))
    }

    /// Derive CWD for subprocess execution based on skill source.
    ///
    /// - Local skills: project root (parent of `.creft/`)
    /// - Global skills and plugin skills: captured CWD
    /// - `CREFT_HOME` mode: captured CWD (no project root concept)
    pub fn derive_cwd(&self, source: &SkillSource) -> PathBuf {
        if self.creft_home.is_some() {
            return self.cwd.clone();
        }
        match source {
            SkillSource::Owned(Scope::Local) | SkillSource::Package(_, Scope::Local) => self
                .find_local_root()
                .and_then(|creft_dir| creft_dir.parent().map(|p| p.to_path_buf()))
                .unwrap_or_else(|| self.cwd.clone()),
            SkillSource::Owned(Scope::Global)
            | SkillSource::Package(_, Scope::Global)
            | SkillSource::Plugin(_) => self.cwd.clone(),
        }
    }
}

/// Walk up from `start` looking for a `.creft/` directory.
///
/// Returns `None` if no `.creft/` directory is found before reaching the filesystem root.
pub fn find_local_root_from(start: &Path) -> Option<PathBuf> {
    let mut dir = start.to_path_buf();
    loop {
        let candidate = dir.join(".creft");
        if candidate.is_dir() {
            return Some(candidate);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// A single entry in a grouped skill listing.
///
/// Represents either a leaf skill (no further nesting) or a collapsed
/// namespace containing multiple skills.
#[derive(Debug, Clone)]
pub enum NamespaceEntry {
    /// A single skill with no namespace prefix at this level.
    Skill(CommandDef, SkillSource),
    /// A collapsed namespace showing only the count and source info.
    Namespace {
        /// The namespace prefix at this level (e.g., "tavily", "aws").
        name: String,
        /// Number of skills (recursively) under this namespace.
        skill_count: usize,
        /// Whether this namespace contains any package skills, and if so, which package.
        /// `None` means all skills are owned. `Some(pkg_name)` means the namespace
        /// maps to an installed package.
        package: Option<String>,
    },
}

/// Where a skill or package is stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Local `.creft/` directory (project-level, discovered by walking up from CWD).
    Local,
    /// Global `~/.creft/` directory.
    Global,
}

/// Where a resolved skill came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillSource {
    /// A user-created skill, with its storage scope.
    Owned(Scope),
    /// An installed package skill, with its storage scope.
    Package(String, Scope),
    /// A skill from an activated plugin in the global plugin cache.
    Plugin(String),
}

/// Skill definition parsed from YAML frontmatter.
#[derive(Debug, Clone)]
pub struct CommandDef {
    pub name: String,
    pub description: String,
    pub args: Vec<Arg>,
    pub flags: Vec<Flag>,
    pub env: Vec<EnvVar>,
    pub tags: Vec<String>,
    /// Runtime features this command supports (e.g., "dry-run").
    /// When a feature is declared here and the corresponding runtime flag
    /// is passed, creft delegates handling to the command instead of
    /// implementing it generically.
    pub supports: Vec<String>,
}

/// A positional argument declared in skill frontmatter.
#[derive(Debug, Clone)]
pub struct Arg {
    pub name: String,
    pub description: String,
    pub default: Option<String>,
    /// Whether this arg must be provided by the caller. Default: false.
    /// When false and no value is provided, the arg is not bound.
    /// Template substitution uses `{{name|default}}` if present,
    /// or errors on `{{name}}` with no default.
    pub required: bool,
    /// Regex pattern for validation. Applied to the final value.
    pub validation: Option<String>,
}

/// A named option declared in skill frontmatter.
#[derive(Debug, Clone)]
pub struct Flag {
    pub name: String,
    /// Single-char short form (e.g., "v" for -v)
    pub short: Option<String>,
    pub description: String,
    /// "bool" (presence flag) or "string" (takes a value). Default: "string".
    pub r#type: String,
    pub default: Option<String>,
    /// Regex pattern for validation (only for string flags).
    pub validation: Option<String>,
}

/// An environment variable dependency declared in skill frontmatter.
#[derive(Debug, Clone)]
pub struct EnvVar {
    pub name: String,
    pub required: bool,
}

fn default_provider() -> String {
    "claude".to_string()
}

/// Configuration for an `llm` code block, parsed from the YAML header.
///
/// All fields are optional strings for forward-compatibility with unknown
/// providers and future provider features.
#[derive(Debug, Clone)]
pub struct LlmConfig {
    /// CLI tool to invoke. Defaults to `"claude"` when absent.
    /// Known providers have specific command patterns; unknown providers
    /// are invoked as literal command names.
    pub provider: String,

    /// Model name passed to the provider CLI. Omitted from the command
    /// when empty (provider uses its own default).
    pub model: String,

    /// Raw parameter string appended to the command. Split on whitespace
    /// before appending as individual arguments. This is the escape hatch
    /// for any provider flag creft doesn't model explicitly.
    pub params: String,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            provider: default_provider(),
            model: String::new(),
            params: String::new(),
        }
    }
}

/// A fenced code block extracted from a skill's markdown body.
#[derive(Debug, Clone)]
pub struct CodeBlock {
    pub lang: String,
    pub code: String,
    pub deps: Vec<String>,
    /// LLM configuration, present only when `lang == "llm"`.
    /// Parsed from the YAML header before `---` in the block content.
    pub llm_config: Option<LlmConfig>,
    /// When `lang == "llm"` and the YAML header failed to parse,
    /// this holds the parse error message. Used by validation to
    /// emit a diagnostic. `None` for all non-llm blocks and for
    /// llm blocks that parsed successfully.
    pub llm_parse_error: Option<String>,
}

impl CodeBlock {
    /// Whether this block requires buffered (sponge) execution in a pipe chain.
    ///
    /// Sponge stages buffer all upstream input before spawning the block's
    /// process. This is needed when the block's input model requires the
    /// complete input before it can begin (e.g., LLM providers that read
    /// the full prompt from stdin before producing output).
    pub fn needs_sponge(&self) -> bool {
        self.lang == "llm"
    }
}

/// A fully parsed skill ready for execution.
#[derive(Debug, Clone)]
pub struct ParsedCommand {
    pub def: CommandDef,
    pub docs: Option<String>,
    pub blocks: Vec<CodeBlock>,
}

impl CommandDef {
    /// Check if this command declares support for a given runtime feature.
    pub fn supports_feature(&self, feature: &str) -> bool {
        self.supports.iter().any(|s| s == feature)
    }

    /// Split the command name into its whitespace-delimited namespace tokens.
    pub fn name_parts(&self) -> Vec<&str> {
        self.name.split_whitespace().collect()
    }

    /// A command is hidden if any token in its name starts with `_`.
    ///
    /// Hidden commands are excluded from `creft list` output but remain
    /// fully functional for execution, show, cat, edit, and rm.
    pub fn is_hidden(&self) -> bool {
        self.name_parts().iter().any(|part| part.starts_with('_'))
    }
}

impl ParsedCommand {
    /// Build the `Usage:` line for this skill.
    ///
    /// Required args render as `<ARG>`, optional/defaulted args as `[ARG]`.
    /// Arg names are uppercased to match clap convention.
    fn usage_line(&self) -> String {
        let mut usage = format!("Usage: creft {}", self.def.name);
        if !self.def.flags.is_empty() {
            usage.push_str(" [OPTIONS]");
        }
        for arg in &self.def.args {
            let name_upper = arg.name.to_uppercase();
            if arg.default.is_some() || !arg.required {
                usage.push_str(&format!(" [{}]", name_upper));
            } else {
                usage.push_str(&format!(" <{}>", name_upper));
            }
        }
        usage
    }

    /// Render the full help text for this skill with ANSI bold formatting.
    ///
    /// Whether ANSI escapes are emitted is controlled by yansi's global condition,
    /// set at startup via `style::init_color()`. No `ansi: bool` parameter is
    /// needed — the global condition handles enable/disable transparently.
    pub fn help_text(&self) -> String {
        // First line is the description only — user already typed the skill name.
        let mut out = format!("{}\n", self.def.description);

        out.push('\n');
        let usage = self.usage_line();
        // Always starts with "Usage:" — bold the label, keep the rest plain.
        if let Some(rest) = usage.strip_prefix("Usage:") {
            out.push_str(&format!("{}{}\n", "Usage:".bold(), rest));
        } else {
            out.push_str(&format!("{}\n", usage));
        }

        if let Some(docs) = &self.docs {
            out.push('\n');
            out.push_str(docs);
            out.push('\n');
        }

        if !self.def.args.is_empty() {
            out.push_str(&format!("\n{}\n", "Arguments:".bold()));
            let max_name = self
                .def
                .args
                .iter()
                .map(|a| a.name.len())
                .max()
                .unwrap_or(0);
            for arg in &self.def.args {
                let default_hint = arg
                    .default
                    .as_ref()
                    .map(|d| format!(" [default: {}]", d))
                    .unwrap_or_default();
                // Column width is computed from plain name length to preserve alignment
                // when ANSI escapes are present (they inflate byte count but not display width).
                let pad = " ".repeat(max_name - arg.name.len());
                out.push_str(&format!(
                    "  {}{pad}  {}{}\n",
                    arg.name.as_str().bold(),
                    arg.description,
                    default_hint,
                ));
            }
        }

        if !self.def.flags.is_empty() {
            out.push_str(&format!("\n{}\n", "Options:".bold()));
            let max_flag = self
                .def
                .flags
                .iter()
                .map(|f| {
                    let short = f
                        .short
                        .as_ref()
                        .map(|s| format!("-{}, ", s))
                        .unwrap_or_default();
                    let type_hint = if f.r#type == "bool" { "" } else { " <value>" };
                    format!("{}--{}{}", short, f.name, type_hint).len()
                })
                .max()
                .unwrap_or(0);
            for flag in &self.def.flags {
                let short = flag
                    .short
                    .as_ref()
                    .map(|s| format!("-{}, ", s))
                    .unwrap_or_default();
                let type_hint = if flag.r#type == "bool" {
                    ""
                } else {
                    " <value>"
                };
                let label = format!("{}--{}{}", short, flag.name, type_hint);
                let default_hint = flag
                    .default
                    .as_ref()
                    .map(|d| format!(" [default: {}]", d))
                    .unwrap_or_default();
                // Column width computed from plain label length (not bold-wrapped).
                let pad = " ".repeat(max_flag - label.len());
                out.push_str(&format!(
                    "  {}{pad}  {}{}\n",
                    label.as_str().bold(),
                    flag.description,
                    default_hint,
                ));
            }
        }

        if !self.def.env.is_empty() {
            out.push_str(&format!("\n{}\n", "Environment:".bold()));
            for var in &self.def.env {
                let req = if var.required {
                    "(required)"
                } else {
                    "(optional)"
                };
                out.push_str(&format!("  {}  {}\n", var.name, req));
            }
        }

        if !self.def.tags.is_empty() {
            out.push_str(&format!(
                "\n{} {}\n",
                "Tags:".bold(),
                self.def.tags.join(", ")
            ));
        }

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[allow(unused_imports)]
    use pretty_assertions::{assert_eq, assert_ne};
    use rstest::rstest;

    #[test]
    fn test_name_parts_simple() {
        let def = CommandDef {
            name: "hello".into(),
            description: "test".into(),
            args: vec![],
            flags: vec![],
            env: vec![],
            tags: vec![],
            supports: vec![],
        };
        assert_eq!(def.name_parts(), vec!["hello"]);
    }

    #[test]
    fn test_name_parts_namespaced() {
        let def = CommandDef {
            name: "gh issue-body".into(),
            description: "test".into(),
            args: vec![],
            flags: vec![],
            env: vec![],
            tags: vec![],
            supports: vec![],
        };
        assert_eq!(def.name_parts(), vec!["gh", "issue-body"]);
    }

    #[rstest]
    #[case::hidden_top_level("_internal", true)]
    #[case::hidden_subcommand("hooks _guard", true)]
    #[case::hidden_namespace("_private mycommand", true)]
    #[case::underscore_mid_word("my_command", false)]
    #[case::visible("visible", false)]
    fn is_hidden_matches_underscore_prefix_tokens(#[case] name: &str, #[case] expected: bool) {
        let def = CommandDef {
            name: name.into(),
            description: "test".into(),
            args: vec![],
            flags: vec![],
            env: vec![],
            tags: vec![],
            supports: vec![],
        };
        assert_eq!(def.is_hidden(), expected);
    }

    #[test]
    fn test_help_text() {
        let cmd = ParsedCommand {
            def: CommandDef {
                name: "gh issue-body".into(),
                description: "Fetch issue body".into(),
                args: vec![
                    Arg {
                        name: "repo".into(),
                        description: "owner/repo".into(),
                        default: None,
                        required: true,
                        validation: None,
                    },
                    Arg {
                        name: "number".into(),
                        description: "issue number".into(),
                        default: None,
                        required: true,
                        validation: None,
                    },
                ],
                flags: vec![],
                env: vec![EnvVar {
                    name: "GITHUB_TOKEN".into(),
                    required: true,
                }],
                tags: vec!["github".into(), "api".into()],
                supports: vec![],
            },
            docs: Some("Fetches the body as raw markdown.".into()),
            blocks: vec![],
        };
        yansi::disable();
        let help = cmd.help_text();
        yansi::enable();
        // First line is description only — no "name — " prefix
        assert!(help.starts_with("Fetch issue body\n"));
        assert!(!help.contains("gh issue-body —"));
        assert!(help.contains("Usage: creft gh issue-body"));
        assert!(help.contains("Fetches the body as raw markdown."));
        // Section headers use title case, not ALL-CAPS.
        assert!(help.contains("Arguments:"));
        assert!(!help.contains("ARGS:"));
        assert!(help.contains("repo"));
        assert!(help.contains("Environment:"));
        assert!(help.contains("GITHUB_TOKEN"));
        assert!(help.contains("Tags:"));
        assert!(!help.contains("TAGS:"));
        assert!(help.contains("github, api"));
    }

    #[test]
    fn test_supports_feature_match() {
        let def = CommandDef {
            name: "deploy".into(),
            description: "deploy something".into(),
            args: vec![],
            flags: vec![],
            env: vec![],
            tags: vec![],
            supports: vec!["dry-run".into()],
        };
        assert!(def.supports_feature("dry-run"));
    }

    #[test]
    fn test_supports_feature_no_match() {
        let def = CommandDef {
            name: "deploy".into(),
            description: "deploy something".into(),
            args: vec![],
            flags: vec![],
            env: vec![],
            tags: vec![],
            supports: vec!["dry-run".into()],
        };
        assert!(!def.supports_feature("verbose"));
    }

    #[test]
    fn test_supports_feature_empty() {
        let def = CommandDef {
            name: "deploy".into(),
            description: "deploy something".into(),
            args: vec![],
            flags: vec![],
            env: vec![],
            tags: vec![],
            supports: vec![],
        };
        assert!(!def.supports_feature("dry-run"));
    }

    // ── global_root when home_dir is None ─────────────────────────────────────

    #[test]
    fn test_global_root_no_home_returns_err() {
        let ctx = AppContext {
            home_dir: None,
            creft_home: None,
            cwd: std::path::PathBuf::from("/tmp"),
        };
        let result = ctx.global_root();
        assert!(
            result.is_err(),
            "global_root() should error when home_dir is None"
        );
    }

    // ── default_flag_type / default_true ─────────────────────────────────────

    #[test]
    fn test_default_flag_type_is_string() {
        // Flag deserialized without a type field should default to "string"
        let yaml = r#"
name: verbose
description: verbose mode
"#;
        let flag: Flag = crate::yaml::from_str(yaml).unwrap();
        assert_eq!(flag.r#type, "string");
    }

    #[test]
    fn test_default_env_required_is_true() {
        // EnvVar deserialized without a required field should default to true
        let yaml = r#"
name: MY_TOKEN
"#;
        let env_var: EnvVar = crate::yaml::from_str(yaml).unwrap();
        assert!(env_var.required, "default required should be true");
    }

    // ── help_text: flags section ──────────────────────────────────────────────

    #[test]
    fn test_help_text_bool_flag_no_value_hint() {
        let cmd = ParsedCommand {
            def: CommandDef {
                name: "test".into(),
                description: "test cmd".into(),
                args: vec![],
                flags: vec![Flag {
                    name: "verbose".into(),
                    short: Some("v".into()),
                    description: "verbose mode".into(),
                    r#type: "bool".into(),
                    default: None,
                    validation: None,
                }],
                env: vec![],
                tags: vec![],
                supports: vec![],
            },
            docs: None,
            blocks: vec![],
        };
        yansi::disable();
        let help = cmd.help_text();
        assert!(
            !help.contains("<value>"),
            "bool flag should not have <value> hint"
        );
        assert!(help.contains("--verbose"));
        assert!(help.contains("-v,"));
        // Section header uses title case, not "FLAGS:".
        assert!(help.contains("Options:"));
        assert!(!help.contains("FLAGS:"));
    }

    #[test]
    fn test_help_text_string_flag_with_default() {
        let cmd = ParsedCommand {
            def: CommandDef {
                name: "test".into(),
                description: "test cmd".into(),
                args: vec![],
                flags: vec![Flag {
                    name: "format".into(),
                    short: None,
                    description: "output format".into(),
                    r#type: "string".into(),
                    default: Some("json".into()),
                    validation: None,
                }],
                env: vec![],
                tags: vec![],
                supports: vec![],
            },
            docs: None,
            blocks: vec![],
        };
        yansi::disable();
        let help = cmd.help_text();
        assert!(
            help.contains("<value>"),
            "string flag should have <value> hint"
        );
        // Default format uses square brackets to match clap convention
        assert!(help.contains("[default: json]"));
        assert!(!help.contains("(default: json)"));
        assert!(help.contains("--format"));
    }

    #[test]
    fn test_help_text_env_optional() {
        let cmd = ParsedCommand {
            def: CommandDef {
                name: "test".into(),
                description: "test cmd".into(),
                args: vec![],
                flags: vec![],
                env: vec![EnvVar {
                    name: "OPTIONAL_TOKEN".into(),
                    required: false,
                }],
                tags: vec![],
                supports: vec![],
            },
            docs: None,
            blocks: vec![],
        };
        yansi::disable();
        let help = cmd.help_text();
        assert!(help.contains("OPTIONAL_TOKEN"));
        assert!(help.contains("(optional)"));
        assert!(!help.contains("(required)"));
        // Section header uses title case
        assert!(help.contains("Environment:"));
        assert!(!help.contains("ENV:"));
    }

    #[test]
    fn test_help_text_arg_with_default() {
        let cmd = ParsedCommand {
            def: CommandDef {
                name: "test".into(),
                description: "test cmd".into(),
                args: vec![Arg {
                    name: "count".into(),
                    description: "number of items".into(),
                    default: Some("10".into()),
                    required: false,
                    validation: None,
                }],
                flags: vec![],
                env: vec![],
                tags: vec![],
                supports: vec![],
            },
            docs: None,
            blocks: vec![],
        };
        yansi::disable();
        let help = cmd.help_text();
        assert!(help.contains("count"));
        // Default format uses square brackets to match clap convention
        assert!(help.contains("[default: 10]"));
        assert!(!help.contains("(default: 10)"));
    }

    // ── help_text: usage line construction ───────────────────────────────────

    #[test]
    fn test_help_text_usage_line_with_required_arg() {
        let cmd = ParsedCommand {
            def: CommandDef {
                name: "fetch".into(),
                description: "Fetch something".into(),
                args: vec![Arg {
                    name: "repo".into(),
                    description: "owner/repo".into(),
                    default: None,
                    required: true,
                    validation: None,
                }],
                flags: vec![],
                env: vec![],
                tags: vec![],
                supports: vec![],
            },
            docs: None,
            blocks: vec![],
        };
        yansi::disable();
        let help = cmd.help_text();
        assert!(
            help.contains("<REPO>"),
            "required arg should appear as <REPO> in usage line; got:\n{help}"
        );
    }

    #[test]
    fn test_help_text_usage_line_with_optional_arg() {
        let cmd = ParsedCommand {
            def: CommandDef {
                name: "count-items".into(),
                description: "Count items".into(),
                args: vec![Arg {
                    name: "count".into(),
                    description: "number of items".into(),
                    default: Some("10".into()),
                    required: false,
                    validation: None,
                }],
                flags: vec![],
                env: vec![],
                tags: vec![],
                supports: vec![],
            },
            docs: None,
            blocks: vec![],
        };
        yansi::disable();
        let help = cmd.help_text();
        assert!(
            help.contains("[COUNT]"),
            "optional arg should appear as [COUNT] in usage line; got:\n{help}"
        );
    }

    #[test]
    fn test_help_text_usage_line_with_options() {
        let cmd = ParsedCommand {
            def: CommandDef {
                name: "lint".into(),
                description: "Run linter".into(),
                args: vec![],
                flags: vec![Flag {
                    name: "fix".into(),
                    short: Some("f".into()),
                    description: "Auto-fix".into(),
                    r#type: "bool".into(),
                    default: None,
                    validation: None,
                }],
                env: vec![],
                tags: vec![],
                supports: vec![],
            },
            docs: None,
            blocks: vec![],
        };
        yansi::disable();
        let help = cmd.help_text();
        // Flags produce [OPTIONS] in the usage line
        assert!(
            help.contains("[OPTIONS]"),
            "skill with flags should show [OPTIONS] in usage line; got:\n{help}"
        );
    }

    #[test]
    fn test_help_text_usage_line_no_flags_no_args() {
        let cmd = ParsedCommand {
            def: CommandDef {
                name: "ping".into(),
                description: "Ping something".into(),
                args: vec![],
                flags: vec![],
                env: vec![],
                tags: vec![],
                supports: vec![],
            },
            docs: None,
            blocks: vec![],
        };
        yansi::disable();
        let help = cmd.help_text();
        assert!(
            help.contains("Usage: creft ping\n"),
            "minimal skill usage line should have no [OPTIONS] or arg placeholders; got:\n{help}"
        );
        assert!(
            !help.contains("[OPTIONS]"),
            "no flags means no [OPTIONS] in usage line"
        );
    }

    #[test]
    fn test_help_text_description_only_first_line() {
        let cmd = ParsedCommand {
            def: CommandDef {
                name: "my-skill".into(),
                description: "Does something useful".into(),
                args: vec![],
                flags: vec![],
                env: vec![],
                tags: vec![],
                supports: vec![],
            },
            docs: None,
            blocks: vec![],
        };
        yansi::disable();
        let help = cmd.help_text();
        let first_line = help.lines().next().unwrap_or("");
        assert_eq!(
            first_line, "Does something useful",
            "first line should be description only, no name prefix"
        );
        assert!(
            !first_line.contains("my-skill"),
            "skill name must not appear in the first line"
        );
    }

    // ── find_local_root CREFT_HOME guard ─────────────────────────────────────

    #[test]
    fn test_find_local_root_returns_none_when_creft_home_set() {
        let dir = tempfile::tempdir().unwrap();
        // Create a .creft/ directory that would normally be found by walk-up.
        std::fs::create_dir_all(dir.path().join(".creft")).unwrap();

        let ctx = AppContext {
            home_dir: None,
            creft_home: Some(dir.path().to_path_buf()),
            cwd: dir.path().to_path_buf(),
        };

        assert!(
            ctx.find_local_root().is_none(),
            "find_local_root must return None when creft_home is set"
        );
    }

    #[test]
    fn test_find_local_root_excludes_global_root() {
        // HOME is a temp dir containing ~/.creft/ (the global store).
        // CWD is a subdirectory of HOME with no .creft/ of its own.
        // find_local_root() must return None — the global store is not a project root.
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join(".creft")).unwrap();
        let subdir = home.path().join("myproject");
        std::fs::create_dir_all(&subdir).unwrap();

        let ctx = AppContext::for_test(home.path().to_path_buf(), subdir);

        assert!(
            ctx.find_local_root().is_none(),
            "find_local_root must return None when walk-up reaches the global ~/.creft/"
        );
    }

    #[test]
    fn test_find_local_root_finds_real_project_root() {
        // HOME is a temp dir containing ~/.creft/ (the global store).
        // CWD is a subdirectory that has its own .creft/ — a real project root.
        // find_local_root() must return the project-local root, not None.
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join(".creft")).unwrap();
        let project = home.path().join("myproject");
        std::fs::create_dir_all(project.join(".creft")).unwrap();
        let subdir = project.join("src");
        std::fs::create_dir_all(&subdir).unwrap();

        let ctx = AppContext::for_test(home.path().to_path_buf(), subdir);

        let found = ctx
            .find_local_root()
            .expect("find_local_root must find the project-local .creft/");
        assert_eq!(
            found,
            project.join(".creft"),
            "find_local_root must return the project-local root"
        );
    }

    // ── help_text: ANSI bold formatting ──────────────────────────────────────

    fn make_full_cmd() -> ParsedCommand {
        ParsedCommand {
            def: CommandDef {
                name: "gh issue-body".into(),
                description: "Fetch issue body".into(),
                args: vec![
                    Arg {
                        name: "repo".into(),
                        description: "owner/repo".into(),
                        default: None,
                        required: true,
                        validation: None,
                    },
                    Arg {
                        name: "number".into(),
                        description: "issue number".into(),
                        default: Some("42".into()),
                        required: false,
                        validation: None,
                    },
                ],
                flags: vec![Flag {
                    name: "verbose".into(),
                    short: Some("v".into()),
                    description: "verbose output".into(),
                    r#type: "bool".into(),
                    default: None,
                    validation: None,
                }],
                env: vec![EnvVar {
                    name: "GITHUB_TOKEN".into(),
                    required: true,
                }],
                tags: vec!["github".into()],
                supports: vec![],
            },
            docs: None,
            blocks: vec![],
        }
    }

    #[test]
    fn test_help_text_ansi_section_headers_bold() {
        let cmd = make_full_cmd();
        yansi::enable();
        let help = cmd.help_text();
        assert!(
            help.contains("\x1b[1mUsage:\x1b[0m"),
            "Usage: header should be bold; got:\n{help}"
        );
        assert!(
            help.contains("\x1b[1mArguments:\x1b[0m"),
            "Arguments: header should be bold; got:\n{help}"
        );
        assert!(
            help.contains("\x1b[1mOptions:\x1b[0m"),
            "Options: header should be bold; got:\n{help}"
        );
        assert!(
            help.contains("\x1b[1mEnvironment:\x1b[0m"),
            "Environment: header should be bold; got:\n{help}"
        );
        assert!(
            help.contains("\x1b[1mTags:\x1b[0m"),
            "Tags: header should be bold; got:\n{help}"
        );
    }

    #[test]
    fn test_help_text_ansi_arg_names_bold() {
        let cmd = make_full_cmd();
        yansi::enable();
        let help = cmd.help_text();
        assert!(
            help.contains("\x1b[1mrepo\x1b[0m"),
            "arg name 'repo' should be bold; got:\n{help}"
        );
        assert!(
            help.contains("\x1b[1mnumber\x1b[0m"),
            "arg name 'number' should be bold; got:\n{help}"
        );
    }

    #[test]
    fn test_help_text_ansi_flag_labels_bold() {
        let cmd = make_full_cmd();
        yansi::enable();
        let help = cmd.help_text();
        // Flag label "-v, --verbose" should be bold.
        assert!(
            help.contains("\x1b[1m-v, --verbose\x1b[0m"),
            "flag label '-v, --verbose' should be bold; got:\n{help}"
        );
    }

    #[test]
    fn test_help_text_ansi_description_not_bold() {
        let cmd = make_full_cmd();
        yansi::enable();
        let help = cmd.help_text();
        assert!(
            !help.contains("\x1b[1mFetch issue body"),
            "description text must not be bold; got:\n{help}"
        );
        assert!(
            !help.contains("\x1b[1mowner/repo"),
            "arg description text must not be bold; got:\n{help}"
        );
    }

    #[test]
    fn test_help_text_ansi_default_hints_not_bold() {
        let cmd = make_full_cmd();
        yansi::enable();
        let help = cmd.help_text();
        assert!(
            help.contains("[default: 42]"),
            "default hint should appear in output; got:\n{help}"
        );
        assert!(
            !help.contains("\x1b[1m[default:"),
            "default hints must not be bold; got:\n{help}"
        );
    }

    #[test]
    fn test_help_text_plain_and_ansi_same_structure() {
        // Both plain and ANSI outputs should contain the same sections and content.
        let cmd = make_full_cmd();
        yansi::disable();
        let plain = cmd.help_text();
        yansi::enable();
        let ansi_out = cmd.help_text();

        // Key identifiers present in both.
        for needle in &[
            "Arguments:",
            "Options:",
            "Environment:",
            "Tags:",
            "repo",
            "GITHUB_TOKEN",
        ] {
            assert!(plain.contains(needle), "plain output missing {needle}");
            assert!(ansi_out.contains(needle), "ansi output missing {needle}");
        }
    }

    // ── LlmConfig deserialization ─────────────────────────────────────────────

    #[test]
    fn test_llm_config_deserialize_full() {
        let yaml = r#"
provider: openai
model: gpt-4o
params: "--max-tokens 1000"
"#;
        let config: LlmConfig = crate::yaml::from_str(yaml).unwrap();
        assert_eq!(config.provider, "openai");
        assert_eq!(config.model, "gpt-4o");
        assert_eq!(config.params, "--max-tokens 1000");
    }

    #[test]
    fn test_llm_config_deserialize_defaults() {
        let yaml = "{}";
        let config: LlmConfig = crate::yaml::from_str(yaml).unwrap();
        assert_eq!(config.provider, "claude");
        assert!(config.model.is_empty());
        assert!(config.params.is_empty());
    }

    #[test]
    fn test_llm_config_deserialize_provider_only() {
        let yaml = "provider: gemini";
        let config: LlmConfig = crate::yaml::from_str(yaml).unwrap();
        assert_eq!(config.provider, "gemini");
        assert!(config.model.is_empty());
        assert!(config.params.is_empty());
    }

    #[test]
    fn test_deserialize_ignores_pipe_field() {
        // YAML with pipe: true must deserialize without error. Field is silently ignored.
        let yaml = "name: hello\ndescription: test\npipe: true\n";
        let def: CommandDef = crate::yaml::from_str(yaml).unwrap();
        assert_eq!(def.name, "hello");
    }

    #[test]
    fn test_deserialize_ignores_sequential_field() {
        // YAML with sequential: true must deserialize without error. Field is silently ignored.
        let yaml = "name: hello\ndescription: test\nsequential: true\n";
        let def: CommandDef = crate::yaml::from_str(yaml).unwrap();
        assert_eq!(def.name, "hello");
    }

    fn make_block(lang: &str) -> CodeBlock {
        CodeBlock {
            lang: lang.to_string(),
            code: String::new(),
            deps: vec![],
            llm_config: None,
            llm_parse_error: None,
        }
    }

    #[rstest]
    #[case::llm("llm", true)]
    #[case::bash("bash", false)]
    #[case::python("python", false)]
    #[case::node("node", false)]
    #[case::unknown_language("typescript", false)]
    fn needs_sponge(#[case] lang: &str, #[case] expected: bool) {
        assert_eq!(make_block(lang).needs_sponge(), expected);
    }
}
