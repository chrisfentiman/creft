/// One-line tagline shown in the root help header.
pub const ROOT_ABOUT: &str = "Executable skills for AI agents";

/// Extended description shown by `creft add --help`, covering skill format, frontmatter fields, and validation.
pub const ADD_LONG_ABOUT: &str = "\
Saves a new skill to the registry

Use this when you have a shell recipe, API call, or multi-step workflow
you want to reuse. Pipe the skill definition as markdown to stdin.

Examples:
  creft add <<'EOF'                          Save from stdin (recommended)
  creft add --force <<'EOF'                  Overwrite existing skill
  creft add --no-validate <<'EOF'            Skip validation only

Skill Definition Format:
  Start with YAML frontmatter between --- delimiters, then add one or more
  fenced code blocks. The language tag on each code block determines which
  interpreter runs it (bash, python, node, etc.).

  Minimal example:
    ---
    name: hello
    description: Greets someone by name.
    args:
      - name: who
    ---

    ```bash
    echo \"Hello, {{who}}!\"
    ```

  Full example (all frontmatter fields):
    ---
    name: gh issue-summary
    description: Summarizes open issues for a GitHub repo.
    args:
      - name: repo
        description: GitHub repository (owner/name)
        required: true
      - name: branch
        description: Target branch
        default: main
    flags:
      - name: format
        short: f
        type: string
        default: json
        description: Output format
        validation: \"^(json|yaml|text)$\"
      - name: verbose
        short: v
        type: bool
        description: Show detailed output
    env:
      - name: GITHUB_TOKEN
        required: true
      - name: GH_API_URL
        required: false
    tags: [git, api]
    ---

Frontmatter Fields:
  name          Required. Spaces create namespaces: 'gh issue-body' -> 'creft gh issue-body'
  description   Required. One line: what it does and when to use it
  args          Positional arguments. Each has: name, description, default, required, validation
  flags         Named --flags. Each has: name, short, type (bool/string), default, description, validation
  env           Environment variables. Each has: name, required (default true)
  tags          List of tags for filtering with 'creft list --tag'

  Arg/flag validation values are regex strings matched against the input.

Code Blocks:
  Each fenced block is an executable step. Language tag sets the interpreter.
  Blocks connect as a pipeline: each block's stdout feeds the next block's
  stdin. All blocks in the pipeline run concurrently (like Unix pipes).
  If any block fails, the pipeline stops and its exit code propagates.
  Single-block skills run standalone (no pipeline).

  Exit codes:
    0     Success, continue to the next block
    1-98  Error, stop the pipeline and propagate the exit code
    99    Early successful return -- stop the pipeline, creft exits 0
    100+  Error, stop the pipeline and propagate the exit code

  Interpreters: bash, python, node, zsh, ruby, docs (not executed -- shown in --help)

  LLM Blocks:
    Use ```llm to send prompts to AI providers as pipeline steps. Add a YAML
    header before --- to configure the provider:

      ```llm
      provider: claude
      model: haiku
      params: \"--max-tokens 500\"
      ---
      Summarize this: {{prev}}
      ```

    Providers: claude (default), gemini, codex, ollama, or any CLI tool name.
    The provider handles authentication (API keys, config files).
    Template placeholders ({{name}}) work in the prompt body.
    LLM blocks buffer all upstream input before sending to the provider
    (sponge pattern -- all stdin is collected before the prompt is sent).
    Use {{prev}} in the prompt to reference the buffered input.
    On non-Unix systems, multi-block skills with LLM blocks are not supported.

  Dependencies (first line comment):
    # deps: requests, pandas          Python (uses uv run --with)
    // deps: lodash, chalk            Node (uses npm install + NODE_PATH)
    # deps: jq, yq                    Shell (warns if not on PATH)

Template Placeholders:
  {{name}}            Positional arg or flag value
  {{name|default}}    Value with fallback
  {{prev}}            Buffered output from previous block (LLM blocks only --
                      other blocks receive previous output via stdin)

Storage:
  Skills save to nearest .creft/ directory, or ~/.creft/ if none exists.
  Use --global to always save to ~/.creft/.

Validation:
  Checks syntax (bash -n, python ast, node --check, ruby -c),
  shellcheck warnings, command availability, dependency resolution,
  and sub-skill references. Use --force to skip all checks,
  or --no-validate to skip validation only (keeps overwrite check).";

/// Extended description shown by `creft list --help`, covering namespace grouping and filtering options.
pub const LIST_LONG_ABOUT: &str = "\
Shows available skills, grouped by namespace

Namespaces are collapsed by default -- each shows the number of skills
inside. Drill into a namespace to see its skills.

Examples:
  creft list              All skills, grouped by namespace
  creft list tavily       Skills in the 'tavily' namespace
  creft list aws s3       Skills in the 'aws s3' sub-namespace
  creft list --tag api    Only skills tagged 'api' (grouped)
  creft list --all        Flat list without grouping

Use 'creft <skill> --help' as a shortcut for 'creft list <skill>'.";

/// Extended description shown by `creft show --help`, explaining full markdown output mode.
pub const SHOW_LONG_ABOUT: &str = "\
Prints a skill's full markdown definition

Shows frontmatter, docs, and code blocks. Use this to understand what a
skill does before running it, or to review an existing skill's implementation.

Options:
  --blocks    Print only the executable code blocks (strips frontmatter and docs)

Examples:
  creft show hello
  creft show gh issue-body
  creft show hello --blocks";

/// Extended description shown by `creft remove --help`, including namespace cleanup behavior.
pub const REMOVE_LONG_ABOUT: &str = "\
Deletes a skill from the registry

Removes the skill file. Empty namespace directories are cleaned up
automatically.

Examples:
  creft remove hello
  creft remove gh issue-body";

/// Extended description shown by `creft up --help`, listing supported AI coding systems and install locations.
pub const UP_LONG_ABOUT: &str = "\
Installs creft instructions for your coding AI

Detects which AI coding systems are present and installs the appropriate
instruction file so the LLM knows how to discover and use creft skills.

Examples:
  creft up                  Auto-detect and install for all found systems
  creft up claude-code      Install for Claude Code only
  creft up cursor           Install for Cursor only
  creft up -g claude-code   Install globally (~/.claude/skills/creft/)

Supported Systems:
  claude-code    .claude/skills/creft/SKILL.md
  cursor         .cursor/rules/creft.mdc
  windsurf       .windsurf/rules/creft.md
  aider          CONVENTIONS.md (appends)
  copilot        .github/copilot-instructions.md
  codex          AGENTS.md (appends)
  gemini         GEMINI.md (appends)

The installer never overwrites existing content. Existing creft sections
are refreshed; non-creft content is preserved. Files already containing
current instructions are skipped. Some systems (Cursor, Windsurf) don't
support global rules via files.";

/// Extended description shown by `creft doctor --help`, covering global and skill-specific check modes.
pub const DOCTOR_LONG_ABOUT: &str = "\
Checks whether your environment can run creft skills

Two modes: global check (no arguments) or skill-specific check.

Examples:
  creft doctor              Check your environment
  creft doctor hello        Check the 'hello' skill
  creft doctor deploy       Check a specific skill's requirements

Exit Codes:
  0    All required checks passed
  1    One or more checks failed
  2    Skill not found (skill check mode)

Global Check:
  Checks interpreters (bash, python3, node, ruby), tools (git, shellcheck,
  uv, npm), AI providers (claude, gemini, codex, ollama -- all optional),
  storage directories, and installed package manifests.

Skill Check:
  Checks interpreters needed by each code block, commands used in bash blocks,
  required environment variables, dependency tools, sub-skills called via
  'creft <name>' in bash blocks, and LLM provider CLIs needed by llm blocks.
  Recursively checks sub-skill dependencies.";

/// Extended description shown by `creft init --help`, explaining local `.creft/` directory creation.
pub const INIT_LONG_ABOUT: &str = "\
Creates local skill storage for this project

Creates a .creft/commands/ directory in the current directory. Local skills
are project-specific: they travel with your repo and shadow global skills
with the same name.

Examples:
  cd my-project
  creft init              Create .creft/commands/
  creft add --name hello  New skills now save locally

Safe to run multiple times -- if .creft/ already exists, prints a message
and exits successfully.";

/// Extended description shown by `creft plugin --help`, listing plugin management subcommands.
pub const PLUGIN_LONG_ABOUT: &str = "\
Manages creft plugins

Plugins extend creft with new commands from a git repository.
Install a plugin globally, then activate specific commands in a project.

Subcommands:
  install     Install a plugin from a git repo
  update      Update installed plugins
  uninstall   Remove an installed plugin
  activate    Make plugin commands available in a scope
  deactivate  Remove plugin commands from a scope
  list        List installed plugins or commands in a plugin
  search      Search for commands across installed plugins

Examples:
  creft plugin install https://github.com/user/my-plugin
  creft plugin update
  creft plugin uninstall my-plugin

Bare 'creft plugin' with no subcommand runs 'creft plugin list'.";

/// Extended description shown by `creft plugin install --help`.
pub const PLUGIN_INSTALL_LONG_ABOUT: &str = "\
Installs a plugin from a git repository into the global plugin cache

Plugin installs are always global (~/.creft/plugins/). Activate commands
in a project scope with 'creft plugin activate'.

A plugin is a git repo with a .creft/catalog.json manifest.
Any .md file with valid creft frontmatter
becomes an available command, namespaced under the plugin name.

Examples:
  creft plugin install https://github.com/user/my-plugin
  creft plugin install git@github.com:user/my-plugin.git
  creft plugin install /path/to/local/plugin-repo
  creft plugin install https://github.com/org/multi-plugin --plugin fetch";

/// Extended description shown by `creft plugin update --help`.
pub const PLUGIN_UPDATE_LONG_ABOUT: &str = "\
Updates installed plugins

Runs git pull on the plugin's cloned repository.

Examples:
  creft plugin update my-plugin    Update a specific plugin
  creft plugin update              Update all installed plugins";

/// Extended description shown by `creft plugin uninstall --help`.
pub const PLUGIN_UNINSTALL_LONG_ABOUT: &str = "\
Removes an installed plugin from the global cache

Deletes the plugin directory and all its commands.

Examples:
  creft plugin uninstall my-plugin";

/// Extended description shown by `creft plugin activate --help`.
pub const PLUGIN_ACTIVATE_LONG_ABOUT: &str = "\
Makes commands from an installed plugin available in a scope

Writes activation state to .creft/plugins/settings.json (local scope,
default) or ~/.creft/plugins/settings.json (global scope, --global).

Examples:
  creft plugin activate my-plugin            Activate all commands
  creft plugin activate my-plugin/fetch      Activate a single command
  creft plugin activate my-plugin --global   Activate globally";

/// Extended description shown by `creft plugin deactivate --help`.
pub const PLUGIN_DEACTIVATE_LONG_ABOUT: &str = "\
Removes plugin commands from a scope

Examples:
  creft plugin deactivate my-plugin           Deactivate all commands
  creft plugin deactivate my-plugin/fetch     Deactivate a single command
  creft plugin deactivate my-plugin --global  Deactivate from global scope";

/// Extended description shown by `creft plugin list --help`.
pub const PLUGIN_LIST_LONG_ABOUT: &str = "\
Lists installed plugins, or commands in a specific plugin

Examples:
  creft plugin list               Show all installed plugins
  creft plugin list my-plugin     Show commands in my-plugin";

/// Extended description shown by `creft plugin search --help`.
pub const PLUGIN_SEARCH_LONG_ABOUT: &str = "\
Searches for commands across installed plugins

Matches against command name, description, and tags.

Examples:
  creft plugin search deploy
  creft plugin search kubernetes deploy";

/// Extended description shown by `creft settings --help`, listing configuration management subcommands.
pub const SETTINGS_LONG_ABOUT: &str = "\
Manages creft configuration settings

Subcommands:
  show    Show current settings
  set     Set a configuration value

Examples:
  creft settings show
  creft settings set shell zsh
  creft settings set shell none    Disable shell preference

Known settings:
  shell   Preferred shell for block execution (bash, zsh, sh, or 'none').
          Applies to shell-family blocks only (bash, sh, zsh are interchangeable).

Shell preference resolution order:
  1. CREFT_SHELL env var (highest priority)
  2. creft settings set shell <name>
  3. $SHELL env var
  4. block language tag (no substitution)

Bare 'creft settings' with no subcommand runs 'creft settings show'.";

/// Extended description shown by `creft completions --help`.
pub const COMPLETIONS_LONG_ABOUT: &str = "\
Generates shell completion scripts

Prints a completion script for the requested shell to stdout. Source the
output in your shell startup file to enable tab completion for creft commands.

Supported shells: bash, zsh, fish

Examples:
  creft completions bash >> ~/.bashrc
  creft completions zsh > ~/.zfunc/_creft
  creft completions fish > ~/.config/fish/completions/creft.fish";

// ── Short help constants ─────────────────────────────────────────────────────
//
// Each constant is the `long_about` section for the short (--help) page.
// These are intentionally concise — one description, one example, one footer.
// The full reference text is in the ADD_LONG_ABOUT etc. constants above.

const ADD_SHORT_ABOUT: &str = "\
Pipe the skill definition as markdown to stdin.

Examples:
  creft add <<'EOF'                          Save from stdin
  creft add --force <<'EOF'                  Overwrite existing

Run 'creft add --docs' for the full reference.";

const LIST_SHORT_ABOUT: &str = "\
Namespaces are collapsed by default. Drill in to see skills.

Examples:
  creft list                List all skills
  creft list tavily         Skills in the 'tavily' namespace

Run 'creft list --docs' for the full reference.";

const SHOW_SHORT_ABOUT: &str = "\
Shows frontmatter, docs, and code blocks.

Example:
  creft show gh issue-body

Run 'creft show --docs' for the full reference.";

const REMOVE_SHORT_ABOUT: &str = "\
Removes the skill file. Empty namespace directories are cleaned up.

Example:
  creft remove gh issue-body

Run 'creft remove --docs' for the full reference.";

const UP_SHORT_ABOUT: &str = "\
Detects AI coding systems and installs the appropriate instruction file.

Examples:
  creft up                  Auto-detect and install for all found systems
  creft up claude-code      Install for Claude Code only

Run 'creft up --docs' for the full reference.";

const DOCTOR_SHORT_ABOUT: &str = "\
Two modes: global check (no arguments) or skill-specific check.

Examples:
  creft doctor              Check your environment
  creft doctor hello        Check the 'hello' skill

Run 'creft doctor --docs' for the full reference.";

const INIT_SHORT_ABOUT: &str = "\
Creates a .creft/commands/ directory in the current directory.

Example:
  creft init

Run 'creft init --docs' for the full reference.";

const PLUGIN_SHORT_ABOUT: &str = "\
Install a plugin globally, then activate specific commands in a project.

Examples:
  creft plugin install https://github.com/user/my-plugin
  creft plugin list

Run 'creft plugin --docs' for the full reference.";

const PLUGIN_INSTALL_SHORT_ABOUT: &str = "\
Plugin installs are always global. Activate with 'creft plugin activate'.

Example:
  creft plugin install https://github.com/user/my-plugin

Run 'creft plugin install --docs' for the full reference.";

const PLUGIN_UPDATE_SHORT_ABOUT: &str = "\
Runs git pull on the plugin's cloned repository.

Examples:
  creft plugin update my-plugin    Update a specific plugin
  creft plugin update              Update all installed plugins

Run 'creft plugin update --docs' for the full reference.";

const PLUGIN_UNINSTALL_SHORT_ABOUT: &str = "\
Deletes the plugin directory and all its commands.

Example:
  creft plugin uninstall my-plugin

Run 'creft plugin uninstall --docs' for the full reference.";

const PLUGIN_ACTIVATE_SHORT_ABOUT: &str = "\
Writes activation state to .creft/plugins/settings.json (local, default).

Examples:
  creft plugin activate my-plugin            Activate all commands
  creft plugin activate my-plugin/fetch      Activate a single command

Run 'creft plugin activate --docs' for the full reference.";

const PLUGIN_DEACTIVATE_SHORT_ABOUT: &str = "\
Removes activation state from the scope's settings.json.

Examples:
  creft plugin deactivate my-plugin           Deactivate all commands
  creft plugin deactivate my-plugin/fetch     Deactivate a single command

Run 'creft plugin deactivate --docs' for the full reference.";

const PLUGIN_LIST_SHORT_ABOUT: &str = "\
Shows all installed plugins, or commands in a specific plugin.

Examples:
  creft plugin list               Show all installed plugins
  creft plugin list my-plugin     Show commands in my-plugin

Run 'creft plugin list --docs' for the full reference.";

const PLUGIN_SEARCH_SHORT_ABOUT: &str = "\
Matches against command name, description, and tags.

Example:
  creft plugin search deploy

Run 'creft plugin search --docs' for the full reference.";

const SETTINGS_SHORT_ABOUT: &str = "\
Subcommands: show, set

Examples:
  creft settings show
  creft settings set shell zsh

Run 'creft settings --docs' for the full reference.";

const SETTINGS_SHOW_SHORT_ABOUT: &str = "\
Prints all current configuration values.

Example:
  creft settings show

Run 'creft settings show --docs' for the full reference.";

const SETTINGS_SET_SHORT_ABOUT: &str = "\
Sets a configuration key to the specified value.

Examples:
  creft settings set shell zsh
  creft settings set shell none    Disable shell preference

Run 'creft settings set --docs' for the full reference.";

const COMPLETIONS_SHORT_ABOUT: &str = "\
Supported shells: bash, zsh, fish

Examples:
  creft completions bash >> ~/.bashrc
  creft completions zsh > ~/.zfunc/_creft

Run 'creft completions --docs' for the full reference.";

// ── Builtin entry list ──────────────────────────────────────────────────────

/// A built-in command's listing metadata, used by the root listing.
pub(crate) struct BuiltinEntry {
    /// Command name as it appears in the listing (e.g., `"add"`, `"plugin"`).
    pub name: &'static str,
    /// Short description for the column-aligned listing.
    pub description: &'static str,
}

const BUILTIN_ENTRIES: &[BuiltinEntry] = &[
    BuiltinEntry {
        name: "add",
        description: "Save a skill from stdin",
    },
    BuiltinEntry {
        name: "completions",
        description: "Generate shell completions",
    },
    BuiltinEntry {
        name: "doctor",
        description: "Check environment and skill health",
    },
    BuiltinEntry {
        name: "init",
        description: "Initialize local skill storage",
    },
    BuiltinEntry {
        name: "list",
        description: "List available skills",
    },
    BuiltinEntry {
        name: "plugin",
        description: "Manage skill collections",
    },
    BuiltinEntry {
        name: "remove",
        description: "Delete a skill",
    },
    BuiltinEntry {
        name: "settings",
        description: "Manage settings",
    },
    BuiltinEntry {
        name: "show",
        description: "Show a skill's full definition",
    },
    BuiltinEntry {
        name: "up",
        description: "Install creft for your coding AI",
    },
];

/// Returns the list of top-level built-in commands for the root listing.
///
/// Entries are alphabetically sorted by name. Only top-level commands appear —
/// subcommands (`plugin install`, `settings set`) are shown in their parent's
/// `--help` page, not here.
pub(crate) fn builtins() -> &'static [BuiltinEntry] {
    BUILTIN_ENTRIES
}

// ── Help renderer ────────────────────────────────────────────────────────────

/// Identifies a built-in command for help rendering.
///
/// Each variant maps to a detailed help page shown by `<command> --help`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BuiltinHelp {
    Add,
    List,
    Show,
    Remove,
    Plugin,
    PluginInstall,
    PluginUpdate,
    PluginUninstall,
    PluginActivate,
    PluginDeactivate,
    PluginList,
    PluginSearch,
    Settings,
    SettingsShow,
    SettingsSet,
    Up,
    Init,
    Doctor,
    Completions,
}

/// Render the short help page for a built-in command (`--help`).
///
/// Returns 10-15 lines: one-line description, usage, a single example, and a
/// footer directing the user to `--docs` for the full reference. ANSI styling
/// is controlled by yansi's global condition (set at startup via
/// `style::init_color()`).
pub(crate) fn render_short(which: BuiltinHelp) -> String {
    match which {
        BuiltinHelp::Add => renderer::render_add_short(),
        BuiltinHelp::List => renderer::render_list_short(),
        BuiltinHelp::Show => renderer::render_show_short(),
        BuiltinHelp::Remove => renderer::render_remove_short(),
        BuiltinHelp::Plugin => renderer::render_plugin_short(),
        BuiltinHelp::PluginInstall => renderer::render_plugin_install_short(),
        BuiltinHelp::PluginUpdate => renderer::render_plugin_update_short(),
        BuiltinHelp::PluginUninstall => renderer::render_plugin_uninstall_short(),
        BuiltinHelp::PluginActivate => renderer::render_plugin_activate_short(),
        BuiltinHelp::PluginDeactivate => renderer::render_plugin_deactivate_short(),
        BuiltinHelp::PluginList => renderer::render_plugin_list_short(),
        BuiltinHelp::PluginSearch => renderer::render_plugin_search_short(),
        BuiltinHelp::Settings => renderer::render_settings_short(),
        BuiltinHelp::SettingsShow => renderer::render_settings_show_short(),
        BuiltinHelp::SettingsSet => renderer::render_settings_set_short(),
        BuiltinHelp::Up => renderer::render_up_short(),
        BuiltinHelp::Init => renderer::render_init_short(),
        BuiltinHelp::Doctor => renderer::render_doctor_short(),
        BuiltinHelp::Completions => renderer::render_completions_short(),
    }
}

/// Render the full reference page for a built-in command (`--docs`).
///
/// Returns the complete help text that was previously shown by `--help`.
/// ANSI styling is controlled by yansi's global condition.
pub(crate) fn render_docs(which: BuiltinHelp) -> String {
    match which {
        BuiltinHelp::Add => renderer::render_add(),
        BuiltinHelp::List => renderer::render_list(),
        BuiltinHelp::Show => renderer::render_show(),
        BuiltinHelp::Remove => renderer::render_remove(),
        BuiltinHelp::Plugin => renderer::render_plugin(),
        BuiltinHelp::PluginInstall => renderer::render_plugin_install(),
        BuiltinHelp::PluginUpdate => renderer::render_plugin_update(),
        BuiltinHelp::PluginUninstall => renderer::render_plugin_uninstall(),
        BuiltinHelp::PluginActivate => renderer::render_plugin_activate(),
        BuiltinHelp::PluginDeactivate => renderer::render_plugin_deactivate(),
        BuiltinHelp::PluginList => renderer::render_plugin_list(),
        BuiltinHelp::PluginSearch => renderer::render_plugin_search(),
        BuiltinHelp::Settings => renderer::render_settings(),
        BuiltinHelp::SettingsShow => renderer::render_settings_show(),
        BuiltinHelp::SettingsSet => renderer::render_settings_set(),
        BuiltinHelp::Up => renderer::render_up(),
        BuiltinHelp::Init => renderer::render_init(),
        BuiltinHelp::Doctor => renderer::render_doctor(),
        BuiltinHelp::Completions => renderer::render_completions(),
    }
}

/// Render the version string.
pub(crate) fn render_version() -> String {
    format!("creft {}", env!("CARGO_PKG_VERSION"))
}

// ── Private per-command formatters ───────────────────────────────────────────
//
// These helpers are called exclusively from `render()`, which is wired to the
// dispatch table in Stage 3. The dead_code lint fires now because the clap-based
// cli.rs (Stage 3 target) is still in place. The allow attribute is removed once
// Stage 3 connects the renderer.
mod renderer {
    use yansi::Paint;

    use super::{
        ADD_LONG_ABOUT, ADD_SHORT_ABOUT, COMPLETIONS_LONG_ABOUT, COMPLETIONS_SHORT_ABOUT,
        DOCTOR_LONG_ABOUT, DOCTOR_SHORT_ABOUT, INIT_LONG_ABOUT, INIT_SHORT_ABOUT, LIST_LONG_ABOUT,
        LIST_SHORT_ABOUT, PLUGIN_ACTIVATE_LONG_ABOUT, PLUGIN_ACTIVATE_SHORT_ABOUT,
        PLUGIN_DEACTIVATE_LONG_ABOUT, PLUGIN_DEACTIVATE_SHORT_ABOUT, PLUGIN_INSTALL_LONG_ABOUT,
        PLUGIN_INSTALL_SHORT_ABOUT, PLUGIN_LIST_LONG_ABOUT, PLUGIN_LIST_SHORT_ABOUT,
        PLUGIN_LONG_ABOUT, PLUGIN_SEARCH_LONG_ABOUT, PLUGIN_SEARCH_SHORT_ABOUT, PLUGIN_SHORT_ABOUT,
        PLUGIN_UNINSTALL_LONG_ABOUT, PLUGIN_UNINSTALL_SHORT_ABOUT, PLUGIN_UPDATE_LONG_ABOUT,
        PLUGIN_UPDATE_SHORT_ABOUT, REMOVE_LONG_ABOUT, REMOVE_SHORT_ABOUT, SETTINGS_LONG_ABOUT,
        SETTINGS_SET_SHORT_ABOUT, SETTINGS_SHORT_ABOUT, SETTINGS_SHOW_SHORT_ABOUT, SHOW_LONG_ABOUT,
        SHOW_SHORT_ABOUT, UP_LONG_ABOUT, UP_SHORT_ABOUT,
    };

    /// Format a help page with a short description, usage line, and long about.
    pub fn page(short_desc: &str, usage: &str, long_about: &str) -> String {
        format!(
            "{short_desc}\n\n{}{usage}\n\n{long_about}\n",
            "Usage: ".bold(),
        )
    }

    /// Format a help page that includes a column-aligned Options section.
    pub fn page_with_options(
        short_desc: &str,
        usage: &str,
        long_about: &str,
        options: &[(&str, &str)],
    ) -> String {
        let mut out = page(short_desc, usage, long_about);
        if !options.is_empty() {
            out.push_str(&format!("\n{}\n", "Options:".bold()));
            let max_label = options
                .iter()
                .map(|(label, _)| label.len())
                .max()
                .unwrap_or(0);
            for (label, desc) in options {
                let pad = " ".repeat(max_label - label.len());
                out.push_str(&format!("  {}{pad}  {desc}\n", label.bold()));
            }
        }
        out
    }

    pub fn render_add() -> String {
        page_with_options(
            "Save a skill from stdin",
            "creft add [OPTIONS]",
            ADD_LONG_ABOUT,
            &[
                ("--name <name>", "Override the skill name from frontmatter"),
                ("--description <desc>", "Override the skill description"),
                (
                    "--arg <name:desc>",
                    "Add or override an argument definition",
                ),
                ("--tag <tag>", "Add a tag to the skill"),
                ("--force", "Overwrite an existing skill without prompting"),
                ("--no-validate", "Skip validation checks"),
                ("--global, -g", "Save to global ~/.creft/ storage"),
            ],
        )
    }

    pub fn render_list() -> String {
        page_with_options(
            "List available skills",
            "creft list [NAMESPACE...] [OPTIONS]",
            LIST_LONG_ABOUT,
            &[
                ("--tag <tag>", "Filter by tag"),
                ("--all", "Flat list without namespace grouping"),
            ],
        )
    }

    pub fn render_show() -> String {
        page_with_options(
            "Show a skill's full definition",
            "creft show <skill> [OPTIONS]",
            SHOW_LONG_ABOUT,
            &[("--blocks", "Print only the executable code blocks")],
        )
    }

    pub fn render_remove() -> String {
        page_with_options(
            "Delete a skill",
            "creft remove <skill> [OPTIONS]",
            REMOVE_LONG_ABOUT,
            &[("--global, -g", "Remove from global ~/.creft/ storage")],
        )
    }

    pub fn render_plugin() -> String {
        page(
            "Manage skill collections",
            "creft plugin <subcommand> [OPTIONS]",
            PLUGIN_LONG_ABOUT,
        )
    }

    pub fn render_plugin_install() -> String {
        page_with_options(
            "Install a plugin from a git repository",
            "creft plugin install <source> [OPTIONS]",
            PLUGIN_INSTALL_LONG_ABOUT,
            &[(
                "--plugin <name>",
                "Install only a specific plugin from a multi-plugin repo",
            )],
        )
    }

    pub fn render_plugin_update() -> String {
        page(
            "Update installed plugins",
            "creft plugin update [name]",
            PLUGIN_UPDATE_LONG_ABOUT,
        )
    }

    pub fn render_plugin_uninstall() -> String {
        page(
            "Remove an installed plugin",
            "creft plugin uninstall <name>",
            PLUGIN_UNINSTALL_LONG_ABOUT,
        )
    }

    pub fn render_plugin_activate() -> String {
        page_with_options(
            "Make commands from an installed plugin available in a scope",
            "creft plugin activate <target> [OPTIONS]",
            PLUGIN_ACTIVATE_LONG_ABOUT,
            &[(
                "--global, -g",
                "Activate globally (~/.creft/plugins/settings.json)",
            )],
        )
    }

    pub fn render_plugin_deactivate() -> String {
        page_with_options(
            "Remove plugin commands from a scope",
            "creft plugin deactivate <target> [OPTIONS]",
            PLUGIN_DEACTIVATE_LONG_ABOUT,
            &[("--global, -g", "Deactivate from global scope")],
        )
    }

    pub fn render_plugin_list() -> String {
        page(
            "List installed plugins or commands in a plugin",
            "creft plugin list [name]",
            PLUGIN_LIST_LONG_ABOUT,
        )
    }

    pub fn render_plugin_search() -> String {
        page(
            "Search for commands across installed plugins",
            "creft plugin search <query...>",
            PLUGIN_SEARCH_LONG_ABOUT,
        )
    }

    pub fn render_settings() -> String {
        page(
            "Manage settings",
            "creft settings <subcommand>",
            SETTINGS_LONG_ABOUT,
        )
    }

    pub fn render_settings_show() -> String {
        page(
            "Show current settings",
            "creft settings show",
            "Prints all current configuration values.",
        )
    }

    pub fn render_settings_set() -> String {
        page(
            "Set a configuration value",
            "creft settings set <key> <value>",
            "Sets a configuration key to the specified value.\n\nExample:\n  creft settings set shell zsh\n  creft settings set shell none    Disable shell preference",
        )
    }

    pub fn render_up() -> String {
        page_with_options(
            "Install creft for your coding AI",
            "creft up [system] [OPTIONS]",
            UP_LONG_ABOUT,
            &[(
                "--global, -g",
                "Install globally (~/.claude/skills/creft/ etc.)",
            )],
        )
    }

    pub fn render_init() -> String {
        page(
            "Initialize local skill storage",
            "creft init",
            INIT_LONG_ABOUT,
        )
    }

    pub fn render_doctor() -> String {
        page(
            "Check environment and skill health",
            "creft doctor [skill]",
            DOCTOR_LONG_ABOUT,
        )
    }

    pub fn render_completions() -> String {
        page(
            "Generate shell completions",
            "creft completions <shell>",
            COMPLETIONS_LONG_ABOUT,
        )
    }

    // ── Short-form renderers (--help) ────────────────────────────────────────

    pub fn render_add_short() -> String {
        page_with_options(
            "Save a skill from stdin",
            "creft add [OPTIONS]",
            ADD_SHORT_ABOUT,
            &[
                ("--force", "Overwrite an existing skill without prompting"),
                ("--no-validate", "Skip validation checks"),
                ("--global, -g", "Save to global ~/.creft/ storage"),
            ],
        )
    }

    pub fn render_list_short() -> String {
        page_with_options(
            "List available skills",
            "creft list [NAMESPACE...] [OPTIONS]",
            LIST_SHORT_ABOUT,
            &[
                ("--tag <tag>", "Filter by tag"),
                ("--all", "Flat list without namespace grouping"),
            ],
        )
    }

    pub fn render_show_short() -> String {
        page_with_options(
            "Show a skill's full definition",
            "creft show <skill> [OPTIONS]",
            SHOW_SHORT_ABOUT,
            &[("--blocks", "Print only the executable code blocks")],
        )
    }

    pub fn render_remove_short() -> String {
        page_with_options(
            "Delete a skill",
            "creft remove <skill> [OPTIONS]",
            REMOVE_SHORT_ABOUT,
            &[("--global, -g", "Remove from global ~/.creft/ storage")],
        )
    }

    pub fn render_plugin_short() -> String {
        page(
            "Manage skill collections",
            "creft plugin <subcommand> [OPTIONS]",
            PLUGIN_SHORT_ABOUT,
        )
    }

    pub fn render_plugin_install_short() -> String {
        page(
            "Install a plugin from a git repository",
            "creft plugin install <source> [OPTIONS]",
            PLUGIN_INSTALL_SHORT_ABOUT,
        )
    }

    pub fn render_plugin_update_short() -> String {
        page(
            "Update installed plugins",
            "creft plugin update [name]",
            PLUGIN_UPDATE_SHORT_ABOUT,
        )
    }

    pub fn render_plugin_uninstall_short() -> String {
        page(
            "Remove an installed plugin",
            "creft plugin uninstall <name>",
            PLUGIN_UNINSTALL_SHORT_ABOUT,
        )
    }

    pub fn render_plugin_activate_short() -> String {
        page_with_options(
            "Make commands from an installed plugin available in a scope",
            "creft plugin activate <target> [OPTIONS]",
            PLUGIN_ACTIVATE_SHORT_ABOUT,
            &[(
                "--global, -g",
                "Activate globally (~/.creft/plugins/settings.json)",
            )],
        )
    }

    pub fn render_plugin_deactivate_short() -> String {
        page_with_options(
            "Remove plugin commands from a scope",
            "creft plugin deactivate <target> [OPTIONS]",
            PLUGIN_DEACTIVATE_SHORT_ABOUT,
            &[("--global, -g", "Deactivate from global scope")],
        )
    }

    pub fn render_plugin_list_short() -> String {
        page(
            "List installed plugins or commands in a plugin",
            "creft plugin list [name]",
            PLUGIN_LIST_SHORT_ABOUT,
        )
    }

    pub fn render_plugin_search_short() -> String {
        page(
            "Search for commands across installed plugins",
            "creft plugin search <query...>",
            PLUGIN_SEARCH_SHORT_ABOUT,
        )
    }

    pub fn render_settings_short() -> String {
        page(
            "Manage settings",
            "creft settings <subcommand>",
            SETTINGS_SHORT_ABOUT,
        )
    }

    pub fn render_settings_show_short() -> String {
        page(
            "Show current settings",
            "creft settings show",
            SETTINGS_SHOW_SHORT_ABOUT,
        )
    }

    pub fn render_settings_set_short() -> String {
        page(
            "Set a configuration value",
            "creft settings set <key> <value>",
            SETTINGS_SET_SHORT_ABOUT,
        )
    }

    pub fn render_up_short() -> String {
        page_with_options(
            "Install creft for your coding AI",
            "creft up [system] [OPTIONS]",
            UP_SHORT_ABOUT,
            &[(
                "--global, -g",
                "Install globally (~/.claude/skills/creft/ etc.)",
            )],
        )
    }

    pub fn render_init_short() -> String {
        page(
            "Initialize local skill storage",
            "creft init",
            INIT_SHORT_ABOUT,
        )
    }

    pub fn render_doctor_short() -> String {
        page(
            "Check environment and skill health",
            "creft doctor [skill]",
            DOCTOR_SHORT_ABOUT,
        )
    }

    pub fn render_completions_short() -> String {
        page(
            "Generate shell completions",
            "creft completions <shell>",
            COMPLETIONS_SHORT_ABOUT,
        )
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn builtins_returns_ten_entries() {
        assert_eq!(builtins().len(), 10);
    }

    #[test]
    fn builtins_sorted_alphabetically() {
        let names: Vec<&str> = builtins().iter().map(|e| e.name).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(
            names, sorted,
            "builtin entries must be alphabetically sorted"
        );
    }

    #[test]
    fn builtins_descriptions_under_50_chars() {
        for entry in builtins() {
            assert!(
                entry.description.len() < 50,
                "builtin '{}' description '{}' is {} chars, must be under 50",
                entry.name,
                entry.description,
                entry.description.len(),
            );
        }
    }

    #[test]
    fn builtins_descriptions_non_empty() {
        for entry in builtins() {
            assert!(
                !entry.description.is_empty(),
                "builtin '{}' must have a non-empty description",
                entry.name,
            );
        }
    }

    #[test]
    fn builtins_contains_expected_commands() {
        let names: Vec<&str> = builtins().iter().map(|e| e.name).collect();
        let expected = [
            "add",
            "completions",
            "doctor",
            "init",
            "list",
            "plugin",
            "remove",
            "settings",
            "show",
            "up",
        ];
        for cmd in &expected {
            assert!(
                names.contains(cmd),
                "builtins() must contain '{cmd}', got: {names:?}",
            );
        }
    }

    #[test]
    fn render_version_matches_cargo_pkg_version() {
        let version = render_version();
        assert!(
            version.starts_with("creft "),
            "version must start with 'creft ', got: {version:?}",
        );
        assert!(
            version.contains(env!("CARGO_PKG_VERSION")),
            "version must contain CARGO_PKG_VERSION",
        );
    }

    #[test]
    fn render_docs_add_contains_usage_line() {
        yansi::disable();
        let output = render_docs(BuiltinHelp::Add);
        yansi::enable();
        assert!(
            output.contains("Usage: creft add"),
            "render_docs(Add) must contain 'Usage: creft add', got: {output:?}",
        );
    }

    #[test]
    fn render_docs_show_contains_blocks_flag() {
        yansi::disable();
        let output = render_docs(BuiltinHelp::Show);
        yansi::enable();
        assert!(
            output.contains("--blocks"),
            "render_docs(Show) must document --blocks flag, got: {output:?}",
        );
    }

    const ALL_VARIANTS: &[BuiltinHelp] = &[
        BuiltinHelp::Add,
        BuiltinHelp::List,
        BuiltinHelp::Show,
        BuiltinHelp::Remove,
        BuiltinHelp::Plugin,
        BuiltinHelp::PluginInstall,
        BuiltinHelp::PluginUpdate,
        BuiltinHelp::PluginUninstall,
        BuiltinHelp::PluginActivate,
        BuiltinHelp::PluginDeactivate,
        BuiltinHelp::PluginList,
        BuiltinHelp::PluginSearch,
        BuiltinHelp::Settings,
        BuiltinHelp::SettingsShow,
        BuiltinHelp::SettingsSet,
        BuiltinHelp::Up,
        BuiltinHelp::Init,
        BuiltinHelp::Doctor,
        BuiltinHelp::Completions,
    ];

    #[test]
    fn all_render_docs_contain_usage_line() {
        yansi::disable();
        for &variant in ALL_VARIANTS {
            let output = render_docs(variant);
            assert!(
                output.contains("Usage:"),
                "render_docs({variant:?}) must contain a Usage: line, got: {output:?}",
            );
        }
        yansi::enable();
    }

    #[test]
    fn all_render_short_contain_usage_line() {
        yansi::disable();
        for &variant in ALL_VARIANTS {
            let output = render_short(variant);
            assert!(
                output.contains("Usage:"),
                "render_short({variant:?}) must contain a Usage: line, got: {output:?}",
            );
        }
        yansi::enable();
    }

    #[test]
    fn all_render_short_contain_docs_footer() {
        yansi::disable();
        for &variant in ALL_VARIANTS {
            let output = render_short(variant);
            assert!(
                output.contains("--docs"),
                "render_short({variant:?}) must contain '--docs' footer, got: {output:?}",
            );
        }
        yansi::enable();
    }

    #[test]
    fn all_render_short_under_20_lines() {
        yansi::disable();
        for &variant in ALL_VARIANTS {
            let output = render_short(variant);
            let line_count = output.lines().count();
            assert!(
                line_count <= 20,
                "render_short({variant:?}) must be at most 20 lines, got {line_count} lines",
            );
        }
        yansi::enable();
    }

    #[test]
    fn render_docs_disabled_produces_no_ansi() {
        yansi::disable();
        let output = render_docs(BuiltinHelp::Add);
        yansi::enable();
        assert!(
            !output.contains("\x1b["),
            "render_docs with yansi disabled must not contain ANSI escapes",
        );
    }

    #[test]
    fn render_short_disabled_produces_no_ansi() {
        yansi::disable();
        let output = render_short(BuiltinHelp::Add);
        yansi::enable();
        assert!(
            !output.contains("\x1b["),
            "render_short with yansi disabled must not contain ANSI escapes",
        );
    }

    #[test]
    fn render_docs_enabled_produces_ansi_bold_on_headers() {
        yansi::enable();
        let output = render_docs(BuiltinHelp::Add);
        assert!(
            output.contains("\x1b["),
            "render_docs with yansi enabled must contain ANSI escapes on section headers",
        );
    }

    #[test]
    fn no_docs_output_contains_built_in_tag() {
        yansi::disable();
        let variants = [
            BuiltinHelp::Add,
            BuiltinHelp::List,
            BuiltinHelp::Show,
            BuiltinHelp::Remove,
            BuiltinHelp::Plugin,
            BuiltinHelp::Settings,
            BuiltinHelp::Up,
            BuiltinHelp::Init,
            BuiltinHelp::Doctor,
            BuiltinHelp::Completions,
        ];
        for variant in variants {
            let output = render_docs(variant);
            assert!(
                !output.contains("(built-in)"),
                "render_docs({variant:?}) must not contain '(built-in)', got: {output:?}",
            );
        }
        yansi::enable();
    }
}
