/// One-line tagline shown in the root help header.
pub const ROOT_ABOUT: &str = "Executable skills for AI agents";

/// Extended description shown by `creft --help`, including usage examples and storage overview.
pub const ROOT_LONG_ABOUT: &str = "\
creft — Executable skills for AI agents

Save agent workflows as markdown. Run them as CLI commands.

Run:
  creft <name> [args] [--flags]

Discover:
  creft cmd list                 list available skills
  creft <name> --help            show a skill's args and description

Create:
  creft cmd add <<'EOF'          save a skill from stdin
  creft cmd add --help           full format reference

Setup:
  creft plugins                  manage skill collections
  creft up                       install for your coding AI
  creft init                     create local .creft/ for this project
  creft doctor                   check environment and skill health

Global Flags:
  --dry-run       show rendered blocks, do not execute
  --verbose, -v   show rendered blocks on stderr, then execute";

/// Extended description shown by `creft cmd --help`, listing skill management subcommands.
pub const CMD_LONG_ABOUT: &str = "\
Manages local and global skills

Subcommands:
  add     Save a new skill from stdin
  list    List available skills
  show    Show a skill's full definition
  cat     Print a skill's code blocks
  rm      Delete a skill

Examples:
  creft cmd add <<'EOF'              Save a skill from stdin
  creft cmd list                     List all skills
  creft cmd list gh                  List skills in the 'gh' namespace
  creft cmd show hello               Show a skill's definition
  creft cmd cat hello                Print a skill's code blocks
  creft cmd rm hello                 Delete a skill

Bare 'creft cmd' with no subcommand runs 'creft cmd list'.
'creft command' is an alias for 'creft cmd'.";

/// Extended description shown by `creft cmd add --help`, covering skill format, frontmatter fields, and validation.
pub const ADD_LONG_ABOUT: &str = "\
Saves a new skill to the registry

Use this when you have a shell recipe, API call, or multi-step workflow
you want to reuse. Pipe the skill definition as markdown to stdin.

Examples:
  creft cmd add <<'EOF'                          Save from stdin (recommended)
  creft cmd add --force <<'EOF'                  Overwrite existing skill
  creft cmd add --no-validate <<'EOF'            Skip validation only

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
  tags          List of tags for filtering with 'creft cmd list --tag'

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
    99    Early successful return — stop the pipeline, creft exits 0
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
    (sponge pattern — all stdin is collected before the prompt is sent).
    Use {{prev}} in the prompt to reference the buffered input.
    On non-Unix systems, multi-block skills with LLM blocks are not supported.

  Dependencies (first line comment):
    # deps: requests, pandas          Python (uses uv run --with)
    // deps: lodash, chalk            Node (uses npm install + NODE_PATH)
    # deps: jq, yq                    Shell (warns if not on PATH)

Template Placeholders:
  {{name}}            Positional arg or flag value
  {{name|default}}    Value with fallback
  {{prev}}            Buffered output from previous block (LLM blocks only —
                      other blocks receive previous output via stdin)

Storage:
  Skills save to nearest .creft/ directory, or ~/.creft/ if none exists.
  Use --global to always save to ~/.creft/.

Validation:
  Checks syntax (bash -n, python ast, node --check, ruby -c),
  shellcheck warnings, command availability, dependency resolution,
  and sub-skill references. Use --force to skip all checks,
  or --no-validate to skip validation only (keeps overwrite check).";

/// Extended description shown by `creft cmd list --help`, covering namespace grouping and filtering options.
pub const LIST_LONG_ABOUT: &str = "\
Shows available skills, grouped by namespace

Namespaces are collapsed by default -- each shows the number of skills
inside. Drill into a namespace to see its skills.

Examples:
  creft cmd list              All skills, grouped by namespace
  creft cmd list tavily       Skills in the 'tavily' namespace
  creft cmd list aws s3       Skills in the 'aws s3' sub-namespace
  creft cmd list --tag api    Only skills tagged 'api' (grouped)
  creft cmd list --all        Flat list without grouping

Use 'creft <skill> --help' as a shortcut for 'creft cmd list <skill>'.";

/// Extended description shown by `creft cmd show --help`, explaining full markdown output mode.
pub const SHOW_LONG_ABOUT: &str = "\
Prints a skill's full markdown definition

Shows frontmatter, docs, and code blocks. Use this to understand what a
skill does before running it, or to review an existing skill's implementation.

Examples:
  creft cmd show hello
  creft cmd show gh issue-body";

/// Extended description shown by `creft cmd cat --help`, explaining code-only output mode.
pub const CAT_LONG_ABOUT: &str = "\
Prints just the executable code blocks

Strips frontmatter and docs, showing only the runnable scripts. Use this
to pipe skill code to another tool, inspect the raw script, or copy it.

Examples:
  creft cmd cat hello
  creft cmd cat gh issue-body";

/// Extended description shown by `creft cmd rm --help`, including namespace cleanup behavior.
pub const RM_LONG_ABOUT: &str = "\
Deletes a skill from the registry

Removes the skill file. Empty namespace directories are cleaned up
automatically.

Examples:
  creft cmd rm hello
  creft cmd rm gh issue-body";

/// Extended description shown by `creft up --help`, listing supported AI coding systems and install locations.
pub const UP_LONG_ABOUT: &str = "\
Installs creft instructions for your coding AI

Detects which AI coding systems are present and installs the appropriate
instruction file so the LLM knows how to discover and use creft skills.
The installed instructions reflect the v0.3.0 CLI structure (creft cmd,
creft plugins).

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
  creft cmd add --name hello  New skills now save locally

Safe to run multiple times -- if .creft/ already exists, prints a message
and exits successfully.";

/// Extended description shown by `creft plugins --help`, listing plugin management subcommands.
pub const PLUGINS_LONG_ABOUT: &str = "\
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
  creft plugins install https://github.com/user/my-plugin
  creft plugins update
  creft plugins uninstall my-plugin

Bare 'creft plugins' with no subcommand runs 'creft plugins list'.";

/// Extended description shown by `creft plugins install --help`.
pub const PLUGIN_INSTALL_LONG_ABOUT: &str = "\
Installs a plugin from a git repository into the global plugin cache

Plugin installs are always global (~/.creft/plugins/). Activate commands
in a project scope with 'creft plugins activate'.

A plugin is a git repo with a .creft/catalog.json manifest.
Any .md file with valid creft frontmatter
becomes an available command, namespaced under the plugin name.

Examples:
  creft plugins install https://github.com/user/my-plugin
  creft plugins install git@github.com:user/my-plugin.git
  creft plugins install /path/to/local/plugin-repo
  creft plugins install https://github.com/org/multi-plugin --plugin fetch";

/// Extended description shown by `creft plugins update --help`.
pub const PLUGIN_UPDATE_LONG_ABOUT: &str = "\
Updates installed plugins

Runs git pull on the plugin's cloned repository.

Examples:
  creft plugins update my-plugin    Update a specific plugin
  creft plugins update              Update all installed plugins";

/// Extended description shown by `creft plugins uninstall --help`.
pub const PLUGIN_UNINSTALL_LONG_ABOUT: &str = "\
Removes an installed plugin from the global cache

Deletes the plugin directory and all its commands.

Examples:
  creft plugins uninstall my-plugin";

/// Extended description shown by `creft plugins activate --help`.
pub const PLUGIN_ACTIVATE_LONG_ABOUT: &str = "\
Makes commands from an installed plugin available in a scope

Writes activation state to .creft/plugins/settings.json (local scope,
default) or ~/.creft/plugins/settings.json (global scope, --global).

Examples:
  creft plugins activate my-plugin            Activate all commands
  creft plugins activate my-plugin/fetch      Activate a single command
  creft plugins activate my-plugin --global   Activate globally";

/// Extended description shown by `creft plugins deactivate --help`.
pub const PLUGIN_DEACTIVATE_LONG_ABOUT: &str = "\
Removes plugin commands from a scope

Examples:
  creft plugins deactivate my-plugin           Deactivate all commands
  creft plugins deactivate my-plugin/fetch     Deactivate a single command
  creft plugins deactivate my-plugin --global  Deactivate from global scope";

/// Extended description shown by `creft plugins list --help`.
pub const PLUGIN_LIST_LONG_ABOUT: &str = "\
Lists installed plugins, or commands in a specific plugin

Examples:
  creft plugins list               Show all installed plugins
  creft plugins list my-plugin     Show commands in my-plugin";

/// Extended description shown by `creft plugins search --help`.
pub const PLUGIN_SEARCH_LONG_ABOUT: &str = "\
Searches for commands across installed plugins

Matches against command name, description, and tags.

Examples:
  creft plugins search deploy
  creft plugins search kubernetes deploy";

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
