use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use yansi::Paint;

use crate::error::CreftError;
use crate::wrap::{MAX_WIDTH, wrap_description, wrap_text};

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

    /// Project-local `.creft/` directories between `cwd` and the filesystem root,
    /// nearest first. Empty when no local root exists, when `creft_home` is set,
    /// or when the only `.creft/` on the walk is the global root (`~/.creft/`).
    pub local_roots: Vec<PathBuf>,
}

/// A single entry in the local-root chain.
///
/// `depth` is 0 for the nearest root and increases toward the filesystem root.
/// Useful for diagnostics and for callers that want to label roots in output
/// (e.g., `creft alias list` and `creft doctor`).
// Stage 2 callers (`list_all_with_source`, `resolve_command`) and Stage 3
// callers (`alias list`, `doctor`) use this type. Established here so the
// public API is stable across the implementation stages.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy)]
pub struct LocalRootRef<'a> {
    pub path: &'a Path,
    pub depth: usize,
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

        let local_roots = if creft_home.is_some() {
            Vec::new()
        } else {
            Self::build_local_roots(&cwd, home_dir.as_deref())
        };

        Ok(Self {
            home_dir,
            creft_home,
            cwd,
            local_roots,
        })
    }

    /// Walk up from `start` collecting `.creft/` directories, filtering out
    /// the global root (`home_dir/.creft/`).
    fn build_local_roots(start: &Path, home_dir: Option<&Path>) -> Vec<PathBuf> {
        let global_root_canon = home_dir.map(|h| {
            let global = h.join(".creft");
            std::fs::canonicalize(&global).unwrap_or(global)
        });

        walk_local_roots_from(start)
            .into_iter()
            .filter(|entry| {
                if let Some(ref global_canon) = global_root_canon {
                    let entry_canon =
                        std::fs::canonicalize(entry).unwrap_or_else(|_| entry.clone());
                    entry_canon != *global_canon
                } else {
                    true
                }
            })
            .collect()
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
    /// Walks the filesystem from `cwd` to populate `local_roots`, mirroring
    /// `from_env`'s behavior. The `~/.creft/` exclusion uses the supplied
    /// `home_dir` (canonicalize-and-skip applied per chain entry). Tests that
    /// construct a real `.creft/` tree under a tempdir with `cwd` inside it
    /// receive a context whose `local_roots()` reflects that tree, with no
    /// further setup.
    #[cfg(test)]
    pub fn for_test(home_dir: PathBuf, cwd: PathBuf) -> Self {
        let local_roots = Self::build_local_roots(&cwd, Some(home_dir.as_path()));
        Self {
            home_dir: Some(home_dir),
            creft_home: None,
            cwd,
            local_roots,
        }
    }

    /// Construct for testing with CREFT_HOME override.
    ///
    /// `local_roots` is empty: the CREFT_HOME mode short-circuits the chain
    /// walk, matching `from_env`'s structural guard.
    #[cfg(test)]
    pub fn for_test_with_creft_home(creft_home: PathBuf, cwd: PathBuf) -> Self {
        Self {
            home_dir: None,
            creft_home: Some(creft_home),
            cwd,
            local_roots: Vec::new(),
        }
    }

    /// Construct for testing with an explicit chain of local roots, nearest first.
    ///
    /// Bypasses the filesystem walk. Use when the test wants to assert the
    /// chain shape without staging real `.creft/` directories on disk, or to
    /// inject a chain that does not match the actual filesystem layout.
    #[cfg(test)]
    pub fn for_test_with_local_roots(
        home_dir: PathBuf,
        cwd: PathBuf,
        local_roots: Vec<PathBuf>,
    ) -> Self {
        Self {
            home_dir: Some(home_dir),
            creft_home: None,
            cwd,
            local_roots,
        }
    }

    /// All local roots in nearest-first order. Empty when no local root exists.
    pub fn local_roots(&self) -> &[PathBuf] {
        &self.local_roots
    }

    /// The nearest local root, or `None` when the chain is empty.
    pub fn nearest_local_root(&self) -> Option<&Path> {
        self.local_roots().first().map(PathBuf::as_path)
    }

    /// Iterate the chain with depth labels.
    ///
    /// Depth 0 is the nearest root; depth increases toward the filesystem root.
    // Stage 2 and Stage 3 callers use this iterator. Established here so the
    // chain abstraction is complete at the model layer.
    #[allow(dead_code)]
    pub fn iter_local_roots(&self) -> impl Iterator<Item = LocalRootRef<'_>> {
        self.local_roots()
            .iter()
            .enumerate()
            .map(|(depth, path)| LocalRootRef {
                path: path.as_path(),
                depth,
            })
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
                .nearest_local_root()
                .map(|p| p.to_path_buf())
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
        if self.nearest_local_root().is_some() {
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

    /// Index directory for the given scope.
    ///
    /// Returns `<scope_root>/indexes/`. The directory is not created here;
    /// callers create it lazily before writing.
    pub fn index_dir_for(&self, scope: Scope) -> Result<PathBuf, CreftError> {
        Ok(self.resolve_root(scope)?.join("indexes"))
    }

    /// Store directory for the given scope.
    ///
    /// Returns `<scope_root>/stores/`. The directory is not created here;
    /// callers create it lazily before writing.
    // Called by the channel handler added in Stage 2.
    #[allow(dead_code)]
    pub fn store_dir_for(&self, scope: Scope) -> Result<PathBuf, CreftError> {
        Ok(self.resolve_root(scope)?.join("stores"))
    }

    /// Path to the alias file for the given scope (`<scope_root>/aliases.yaml`).
    ///
    /// The file may not exist; callers treat a missing file as an empty alias
    /// map rather than an error.
    // Called by aliases::load_for_scope and aliases::save_for_scope.
    #[allow(dead_code)]
    pub fn aliases_path_for(&self, scope: Scope) -> Result<PathBuf, CreftError> {
        Ok(self.resolve_root(scope)?.join("aliases.yaml"))
    }

    /// Derive CWD for subprocess execution based on skill source.
    ///
    /// - Local skills: project root (parent of the owning `.creft/`)
    /// - Global skills and plugin skills: captured CWD
    /// - `CREFT_HOME` mode: captured CWD (no project root concept)
    pub fn derive_cwd(&self, source: &SkillSource) -> PathBuf {
        if self.creft_home.is_some() {
            return self.cwd.clone();
        }
        // The owning root is carried on the source by constructor invariant.
        // No chain walk here: source.local_root() is the authoritative answer.
        if let Some(creft_dir) = source.local_root() {
            creft_dir
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| self.cwd.clone())
        } else {
            self.cwd.clone()
        }
    }
}

/// Walk up from `start` collecting every `.creft/` directory, nearest first.
///
/// Returns an empty `Vec` when no `.creft/` exists between `start` and the
/// filesystem root. Entries are the `.creft/` directory paths themselves, not
/// their parents.
pub fn walk_local_roots_from(start: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let mut dir = start.to_path_buf();
    loop {
        let candidate = dir.join(".creft");
        if candidate.is_dir() {
            roots.push(candidate);
        }
        if !dir.pop() {
            return roots;
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Scope {
    /// Local `.creft/` directory (project-level, discovered by walking up from CWD).
    Local,
    /// Global `~/.creft/` directory.
    Global,
}

/// Where a resolved skill came from.
///
/// Construction is via the four constructors below — direct literal
/// construction would let callers bypass the `root.is_some() ⇔ scope == Local`
/// invariant. The fields are visible for read-side destructuring (`match`
/// arms remain idiomatic) but the variants must be built through the
/// constructors so the invariant is enforced at every construction site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillSource {
    /// A user-created skill.
    ///
    /// **Invariant:** `root.is_some()` if and only if `scope == Scope::Local`.
    /// Construct via [`SkillSource::owned_local`] or [`SkillSource::owned_global`].
    Owned { scope: Scope, root: Option<PathBuf> },
    /// An installed package skill.
    ///
    /// **Invariant:** `root.is_some()` if and only if `scope == Scope::Local`.
    /// Construct via [`SkillSource::package_local`] or [`SkillSource::package_global`].
    Package {
        name: String,
        scope: Scope,
        root: Option<PathBuf>,
    },
    /// A skill from an activated plugin in the global plugin cache.
    ///
    /// Plugins live in `~/.creft/plugins/`; no local root applies.
    Plugin(String),
}

impl SkillSource {
    /// Construct a local-scope owned source. The owning local root is required.
    pub fn owned_local(root: PathBuf) -> Self {
        SkillSource::Owned {
            scope: Scope::Local,
            root: Some(root),
        }
    }

    /// Construct a global-scope owned source.
    pub fn owned_global() -> Self {
        SkillSource::Owned {
            scope: Scope::Global,
            root: None,
        }
    }

    /// Construct a local-scope package source. The owning local root is required.
    pub fn package_local(name: String, root: PathBuf) -> Self {
        SkillSource::Package {
            name,
            scope: Scope::Local,
            root: Some(root),
        }
    }

    /// Construct a global-scope package source.
    pub fn package_global(name: String) -> Self {
        SkillSource::Package {
            name,
            scope: Scope::Global,
            root: None,
        }
    }

    /// The local root that owns this skill, when the skill is local-scoped.
    ///
    /// Returns `Some(root)` for `Owned { scope: Scope::Local, .. }` and
    /// `Package { scope: Scope::Local, .. }`. Returns `None` for global and
    /// plugin sources.
    ///
    /// # Panics
    ///
    /// Panics if a local-tagged variant has `root == None`. Such a value can
    /// only exist if a caller bypassed the constructors and built the variant
    /// by literal expression. The panic surfaces a real bug rather than
    /// producing the silent wrong behavior that motivated this fix.
    pub fn local_root(&self) -> Option<&Path> {
        match self {
            SkillSource::Owned {
                scope: Scope::Local,
                root,
            }
            | SkillSource::Package {
                scope: Scope::Local,
                root,
                ..
            } => Some(root.as_deref().expect(
                "local-tagged SkillSource missing root: invariant violation; \
                     use SkillSource::owned_local / package_local",
            )),
            SkillSource::Owned { .. } | SkillSource::Package { .. } | SkillSource::Plugin(_) => {
                None
            }
        }
    }

    /// The storage scope. `Plugin(_)` is always treated as `Scope::Global`.
    // Used by Stage 2 tests and Stage 3 callers (cmd::alias, doctor). Not yet
    // referenced in production binary code at this stage.
    #[allow(dead_code)]
    pub fn scope(&self) -> Scope {
        match self {
            SkillSource::Owned { scope, .. } | SkillSource::Package { scope, .. } => *scope,
            SkillSource::Plugin(_) => Scope::Global,
        }
    }
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

/// Returns true when `lang` is a known language family.
///
/// Known families have language-specific runners with preambles, file
/// extensions, and side-channel wiring. Tags outside this set are treated as
/// opaque binaries by the runner: invoked verbatim, default to stdin mode,
/// optionally forced into file mode via `# extension:`.
pub(crate) fn is_known_family(lang: &str) -> bool {
    matches!(
        lang,
        "bash"
            | "sh"
            | "zsh"
            | "python"
            | "python3"
            | "node"
            | "javascript"
            | "js"
            | "typescript"
            | "ts"
            | "llm"
    )
}

/// A fenced code block extracted from a skill's markdown body.
#[derive(Debug, Clone)]
pub struct CodeBlock {
    pub lang: String,
    pub code: String,
    pub deps: Vec<String>,
    /// File extension override for the temp script written in file mode.
    ///
    /// When `Some(ext)`, the runner writes the block to a temp file ending
    /// in `.<ext>` and invokes the binary with that path as the final
    /// argument, regardless of whether the language tag is a known family.
    /// When `None`, known families use their conventional extension;
    /// unknown tags use stdin mode and the temp file is unused.
    pub extension: Option<String>,
    /// Runner argument string, expanded with the same `{{var}}` machinery
    /// as the block body and split on whitespace at execution time.
    ///
    /// Placement: between the interpreter and the script path in file mode;
    /// immediately after the interpreter in stdin mode.
    // Read by the runner dispatch layer introduced in the next stage.
    #[allow(dead_code)]
    pub flags: Option<String>,
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
        self.lang == "llm" || self.runs_via_stdin()
    }

    /// Whether this block delivers its source via stdin rather than a temp file.
    ///
    /// True for non-family language tags without an `# extension:` directive.
    /// LLM blocks are sponged but are not stdin-mode in this sense: their
    /// command shape is provider-specific, not generic-interpreter.
    pub fn runs_via_stdin(&self) -> bool {
        !is_known_family(&self.lang) && self.extension.is_none()
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
        let mut out = format!("{}\n", wrap_text(&self.def.description, MAX_WIDTH, 0));

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
            out.push_str(&wrap_text(docs, MAX_WIDTH, 0));
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
            let desc_col = 2 + max_name + 2;
            let desc_budget = MAX_WIDTH.saturating_sub(desc_col);
            for arg in &self.def.args {
                let default_hint = arg
                    .default
                    .as_ref()
                    .filter(|d| !d.is_empty())
                    .map(|d| format!(" [default: {}]", d))
                    .unwrap_or_default();
                // Column width is computed from plain name length to preserve alignment
                // when ANSI escapes are present (they inflate byte count but not display width).
                let pad = " ".repeat(max_name - arg.name.len());
                if arg.description.is_empty() && default_hint.is_empty() {
                    // No description or default: omit the description column entirely
                    // so the line ends cleanly at the name rather than with trailing spaces.
                    out.push_str(&format!("  {}{pad}\n", arg.name.as_str().bold()));
                } else {
                    let full_desc = format!("{}{}", arg.description, default_hint);
                    let wrapped = wrap_description(&full_desc, desc_budget, desc_col);
                    out.push_str(&format!("  {}{pad}  {wrapped}\n", arg.name.as_str().bold(),));
                }
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
            let flag_desc_col = 2 + max_flag + 2;
            let flag_desc_budget = MAX_WIDTH.saturating_sub(flag_desc_col);
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
                    .filter(|d| !d.is_empty())
                    .map(|d| format!(" [default: {}]", d))
                    .unwrap_or_default();
                // Column width computed from plain label length (not bold-wrapped).
                let pad = " ".repeat(max_flag - label.len());
                let full_desc = format!("{}{}", flag.description, default_hint);
                let wrapped = wrap_description(&full_desc, flag_desc_budget, flag_desc_col);
                out.push_str(&format!("  {}{pad}  {wrapped}\n", label.as_str().bold(),));
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

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[allow(unused_imports)]
    use pretty_assertions::{assert_eq, assert_ne};
    use rstest::rstest;
    use serial_test::serial;

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
    #[serial]
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
        // Tags are search metadata for `creft list --tag` — not rendered in --help.
        assert!(!help.contains("Tags:"));
        assert!(!help.contains("github, api"));
    }

    #[test]
    #[serial]
    fn help_text_hides_empty_default() {
        let cmd = ParsedCommand {
            def: CommandDef {
                name: "deploy".into(),
                description: "deploy a service".into(),
                args: vec![Arg {
                    name: "env".into(),
                    description: "target environment".into(),
                    default: Some(String::new()),
                    required: false,
                    validation: None,
                }],
                flags: vec![Flag {
                    name: "region".into(),
                    description: "cloud region".into(),
                    short: None,
                    default: Some(String::new()),
                    r#type: "string".into(),
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
        yansi::enable();
        assert!(
            !help.contains("[default: ]"),
            "empty default must not appear"
        );
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
            local_roots: Vec::new(),
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
    #[serial]
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
    #[serial]
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
    #[serial]
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
    #[serial]
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

    /// Arg with no description and no default renders as `  name` with no trailing spaces.
    ///
    /// When a skill author omits the description field, the arg line must end cleanly
    /// at the name rather than leaving a dangling two-space separator column that
    /// makes the output look ragged compared to built-in help.
    #[test]
    #[serial]
    fn test_help_text_arg_without_description_has_no_trailing_spaces() {
        let cmd = ParsedCommand {
            def: CommandDef {
                name: "greet".into(),
                description: "Greet someone".into(),
                args: vec![Arg {
                    name: "who".into(),
                    description: String::new(),
                    default: None,
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
        yansi::enable();
        assert!(help.contains("Arguments:"));
        assert!(
            help.contains("  who\n"),
            "arg line must end at the name with no trailing spaces; got:\n{help}"
        );
        // No description separator should appear when there is nothing to describe.
        let arg_line = help
            .lines()
            .find(|l| l.trim_start().starts_with("who"))
            .unwrap_or("");
        assert!(
            !arg_line.ends_with("  "),
            "arg line must not have trailing spaces; got: {arg_line:?}",
        );
    }

    // ── help_text: usage line construction ───────────────────────────────────

    #[test]
    #[serial]
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
    #[serial]
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
    #[serial]
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
    #[serial]
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
    #[serial]
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

    // ── nearest_local_root / local_roots chain ────────────────────────────────

    #[test]
    fn test_nearest_local_root_returns_none_when_creft_home_set() {
        let dir = tempfile::tempdir().unwrap();
        // Create a .creft/ directory that would normally be found by walk-up.
        std::fs::create_dir_all(dir.path().join(".creft")).unwrap();

        let ctx = AppContext::for_test_with_creft_home(
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
        );

        assert!(
            ctx.nearest_local_root().is_none(),
            "nearest_local_root must return None when creft_home is set"
        );
        assert!(
            ctx.local_roots().is_empty(),
            "local_roots must be empty when creft_home is set"
        );
    }

    #[test]
    fn test_nearest_local_root_excludes_global_root() {
        // HOME is a temp dir containing ~/.creft/ (the global store).
        // CWD is a subdirectory of HOME with no .creft/ of its own.
        // nearest_local_root() must return None — the global store is not a project root.
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join(".creft")).unwrap();
        let subdir = home.path().join("myproject");
        std::fs::create_dir_all(&subdir).unwrap();

        let ctx = AppContext::for_test(home.path().to_path_buf(), subdir);

        assert!(
            ctx.nearest_local_root().is_none(),
            "nearest_local_root must return None when walk-up reaches the global ~/.creft/"
        );
    }

    #[test]
    fn test_nearest_local_root_finds_real_project_root() {
        // HOME is a temp dir containing ~/.creft/ (the global store).
        // CWD is a subdirectory that has its own .creft/ — a real project root.
        // nearest_local_root() must return the project-local root, not None.
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join(".creft")).unwrap();
        let project = home.path().join("myproject");
        std::fs::create_dir_all(project.join(".creft")).unwrap();
        let subdir = project.join("src");
        std::fs::create_dir_all(&subdir).unwrap();

        let ctx = AppContext::for_test(home.path().to_path_buf(), subdir);

        let found = ctx
            .nearest_local_root()
            .expect("nearest_local_root must find the project-local .creft/");
        assert_eq!(
            found,
            project.join(".creft"),
            "nearest_local_root must return the project-local root"
        );
    }

    #[test]
    fn test_local_roots_empty_when_no_creft_in_ancestry() {
        // CWD with no .creft/ ancestors (other than possibly global).
        // for_test walk finds nothing and local_roots is empty.
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        // No .creft/ in either; home has no .creft/ either, so exclusion is moot.
        let ctx = AppContext::for_test(home.path().to_path_buf(), cwd.path().to_path_buf());
        assert!(
            ctx.local_roots().is_empty(),
            "local_roots must be empty when no .creft/ exists in ancestry"
        );
        assert!(ctx.nearest_local_root().is_none());
    }

    #[test]
    fn test_local_roots_single_entry_for_single_root_project() {
        // Project has one .creft/ at its root; CWD is a subdirectory.
        // local_roots must have exactly one entry pointing at that root.
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(project.path().join(".creft")).unwrap();
        let subdir = project.path().join("src");
        std::fs::create_dir_all(&subdir).unwrap();

        let ctx = AppContext::for_test(home.path().to_path_buf(), subdir);

        assert_eq!(
            ctx.local_roots().len(),
            1,
            "single-root project must produce exactly one entry in local_roots"
        );
        assert_eq!(
            ctx.nearest_local_root().unwrap(),
            project.path().join(".creft")
        );
    }

    #[test]
    fn test_local_roots_two_element_chain_nearest_first() {
        // BL-4 repro: intermediate directory has an empty .creft/, ancestor has a
        // populated one. Both must appear in the chain, nearest first.
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        // Ancestor .creft/
        std::fs::create_dir_all(project.path().join(".creft").join("commands")).unwrap();
        // Intermediate empty .creft/
        let infra = project.path().join("infra").join("rackroom");
        std::fs::create_dir_all(infra.join(".creft")).unwrap();
        // CWD is inside the intermediate
        let cwd = infra.join("deploy");
        std::fs::create_dir_all(&cwd).unwrap();

        let ctx = AppContext::for_test(home.path().to_path_buf(), cwd);

        assert_eq!(
            ctx.local_roots().len(),
            2,
            "two-element chain must have exactly two entries"
        );
        assert_eq!(
            ctx.local_roots()[0],
            infra.join(".creft"),
            "nearest root (intermediate) must be first"
        );
        assert_eq!(
            ctx.local_roots()[1],
            project.path().join(".creft"),
            "farthest root (project) must be second"
        );
        assert_eq!(ctx.nearest_local_root().unwrap(), infra.join(".creft"));
    }

    #[test]
    fn test_local_roots_excludes_global_root_per_entry() {
        // HOME has .creft/ (global root). A subdirectory of HOME also has a project
        // .creft/. Only the project root must appear in local_roots; the global
        // root must be filtered out even though the walk visits it.
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join(".creft")).unwrap();
        let project = home.path().join("myproject");
        std::fs::create_dir_all(project.join(".creft")).unwrap();
        let subdir = project.join("src");
        std::fs::create_dir_all(&subdir).unwrap();

        let ctx = AppContext::for_test(home.path().to_path_buf(), subdir);

        // Must have exactly the project root, not the global root.
        assert_eq!(
            ctx.local_roots().len(),
            1,
            "global root must be excluded from local_roots"
        );
        assert_eq!(ctx.local_roots()[0], project.join(".creft"));
    }

    #[test]
    fn test_for_test_with_creft_home_has_empty_local_roots() {
        let creft_home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        // Even with a .creft/ in the cwd tree, CREFT_HOME mode yields empty chain.
        std::fs::create_dir_all(cwd.path().join(".creft")).unwrap();

        let ctx = AppContext::for_test_with_creft_home(
            creft_home.path().to_path_buf(),
            cwd.path().to_path_buf(),
        );
        assert!(
            ctx.local_roots().is_empty(),
            "CREFT_HOME mode must produce an empty local_roots"
        );
    }

    #[test]
    fn test_for_test_with_local_roots_exposes_supplied_chain() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let root_a = PathBuf::from("/fake/a/.creft");
        let root_b = PathBuf::from("/fake/b/.creft");
        let chain = vec![root_a.clone(), root_b.clone()];

        let ctx = AppContext::for_test_with_local_roots(
            home.path().to_path_buf(),
            cwd.path().to_path_buf(),
            chain,
        );
        assert_eq!(ctx.local_roots(), &[root_a.clone(), root_b.clone()]);
        assert_eq!(ctx.nearest_local_root().unwrap(), root_a.as_path());
    }

    #[test]
    fn test_iter_local_roots_depth_labels() {
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        // Build a 2-element chain via for_test_with_local_roots (synthetic, no FS).
        let root_near = project.path().join("infra").join(".creft");
        let root_far = project.path().join(".creft");
        let ctx = AppContext::for_test_with_local_roots(
            home.path().to_path_buf(),
            project.path().to_path_buf(),
            vec![root_near.clone(), root_far.clone()],
        );

        let entries: Vec<LocalRootRef<'_>> = ctx.iter_local_roots().collect();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].depth, 0);
        assert_eq!(entries[0].path, root_near.as_path());
        assert_eq!(entries[1].depth, 1);
        assert_eq!(entries[1].path, root_far.as_path());
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
    #[serial]
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
        // Tags are not rendered in --help output.
        assert!(
            !help.contains("\x1b[1mTags:\x1b[0m"),
            "Tags: must not appear in --help; got:\n{help}"
        );
    }

    #[test]
    #[serial]
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
    #[serial]
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
    #[serial]
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
    #[serial]
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
    #[serial]
    fn test_help_text_plain_and_ansi_same_structure() {
        // Both plain and ANSI outputs should contain the same sections and content.
        let cmd = make_full_cmd();
        yansi::disable();
        let plain = cmd.help_text();
        yansi::enable();
        let ansi_out = cmd.help_text();

        // Key identifiers present in both. Tags are excluded — they are search
        // metadata and are not rendered in --help output.
        for needle in &[
            "Arguments:",
            "Options:",
            "Environment:",
            "repo",
            "GITHUB_TOKEN",
        ] {
            assert!(plain.contains(needle), "plain output missing {needle}");
            assert!(ansi_out.contains(needle), "ansi output missing {needle}");
        }
        // Tags must not appear in either rendering.
        assert!(
            !plain.contains("Tags:"),
            "plain output must not contain Tags:"
        );
        assert!(
            !ansi_out.contains("Tags:"),
            "ansi output must not contain Tags:"
        );
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
            extension: None,
            flags: None,
            llm_config: None,
            llm_parse_error: None,
        }
    }

    #[rstest]
    #[case::llm("llm", true)]
    #[case::bash("bash", false)]
    #[case::python("python", false)]
    #[case::node("node", false)]
    #[case::typescript("typescript", false)]
    // Unknown tags without an extension directive are stdin-mode → sponged.
    #[case::unknown_tag("ruby", true)]
    fn needs_sponge(#[case] lang: &str, #[case] expected: bool) {
        assert_eq!(make_block(lang).needs_sponge(), expected);
    }

    #[rstest]
    #[case::bash("bash", false)]
    #[case::sh("sh", false)]
    #[case::zsh("zsh", false)]
    #[case::python("python", false)]
    #[case::python3("python3", false)]
    #[case::node("node", false)]
    #[case::javascript("javascript", false)]
    #[case::js("js", false)]
    #[case::typescript("typescript", false)]
    #[case::ts("ts", false)]
    #[case::llm("llm", false)]
    #[case::ruby("ruby", true)]
    #[case::zx("zx", true)]
    #[case::deno("deno", true)]
    fn runs_via_stdin_no_extension(#[case] lang: &str, #[case] expected: bool) {
        assert_eq!(make_block(lang).runs_via_stdin(), expected);
    }

    #[test]
    fn runs_via_stdin_false_when_extension_set() {
        let block = CodeBlock {
            lang: "ruby".to_string(),
            code: String::new(),
            deps: vec![],
            extension: Some("rb".to_string()),
            flags: None,
            llm_config: None,
            llm_parse_error: None,
        };
        assert!(!block.runs_via_stdin());
    }

    #[test]
    fn is_known_family_covers_expected_tags() {
        let known = [
            "bash",
            "sh",
            "zsh",
            "python",
            "python3",
            "node",
            "javascript",
            "js",
            "typescript",
            "ts",
            "llm",
        ];
        for tag in known {
            assert!(
                super::is_known_family(tag),
                "expected '{tag}' to be a known family"
            );
        }
        let unknown = ["ruby", "zx", "deno", "bun", "perl", "go", "rust", ""];
        for tag in unknown {
            assert!(
                !super::is_known_family(tag),
                "expected '{tag}' to NOT be a known family"
            );
        }
    }
}
