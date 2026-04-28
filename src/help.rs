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
    1+    Error, stop the pipeline and propagate the exit code

  Early exit:
    Call creft_exit to stop the pipeline from inside a block:
      creft_exit          Stop with success (exit 0)
      creft_exit 0        Same as above
      creft_exit 1        Stop with failure (exit 1)

  Interpreters: bash, python, node, zsh, docs (not executed -- shown in --help)

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
  Checks syntax (bash -n, python ast, node --check),
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

The skill name is supplied via --skill; bare positional names are not
accepted. Quote multi-word names so the shell passes them as a single
argument: --skill \"gh issue-body\".

Examples:
  creft remove --skill hello
  creft remove --skill \"gh issue-body\"
  creft remove --skill hello --global";

/// Extended description shown by `creft up --docs`, listing supported AI coding systems and install locations.
pub const UP_LONG_ABOUT: &str = "\
Install creft into your coding AI tools.

For supported tools (Claude Code, Gemini), installs a session-start
hook that teaches the agent about creft. The hook runs automatically
and updates when creft updates -- no need to re-run `creft up`.

For other tools (Cursor, Windsurf, Aider, Copilot, Codex), writes a static
instruction file. Run `creft up` again after upgrading creft to refresh.

By default, installs globally (~/.claude/settings.json, etc.). Use --local
to install in the current project directory instead.

Examples:
  creft up                  Install globally for all supported systems
  creft up claude-code      Install globally for Claude Code only
  creft up --local          Auto-detect and install for systems in this project
  creft up -l cursor        Install Cursor in this project only

Supported Systems:
  claude-code    .claude/settings.json (session start hook)
  gemini         .gemini/settings.json (session start hook)
  cursor         .cursor/rules/creft.mdc
  windsurf       .windsurf/rules/creft.md
  aider          CONVENTIONS.md (appends)
  copilot        .github/copilot-instructions.md
  codex          AGENTS.md (appends)

Make sure `creft` is on your PATH for the hook to work.
Some systems (Cursor, Windsurf) don't support global rules via files.";

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
  Checks interpreters (bash, python3, node), tools (git, shellcheck,
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
  creft plugin install creft/ask
  creft plugin install https://github.com/user/my-plugin
  creft plugin install git@github.com:user/my-plugin.git
  creft plugin install /path/to/local/plugin-repo";

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

Examples:
  creft remove --skill hello
  creft remove --skill \"gh issue-body\"

Run 'creft remove --docs' for the full reference.";

/// Extended description shown by `creft remove test --docs`.
pub const REMOVE_TEST_LONG_ABOUT: &str = "\
Delete a single scenario from a skill's *.test.yaml fixture.

The named scenario is excised by direct byte slice; surrounding scenarios,
comments, and hand-formatting are preserved verbatim. Comments that appear
between the removed entry's `-` indicator and the next entry's `-` indicator
are removed with the entry — they are treated as belonging to it.

Required flags:
  --skill <name>   The target skill (e.g. `setup`, `hooks guard bash`).
                   The skill must exist in the local commands/ tree.
  --name <name>    The exact value of the scenario's `name:` field.

Errors:
  Skill not found              command not found: <skill>
  Fixture file missing         no test fixture for skill '<skill>': <path>
  Scenario not found           no test scenario named '<name>' in <skill>
  Malformed fixture            existing fixture is malformed: <reason>

Examples:
  creft remove test --skill setup --name 'fresh install succeeds'
  creft remove test --skill 'hooks guard bash' --name 'rejects rm -rf'";

/// Short description shown by `creft remove test --help`.
const REMOVE_TEST_SHORT_ABOUT: &str = "\
Delete a single scenario from a skill's *.test.yaml fixture.

Examples:
  creft remove test --skill setup --name 'fresh install succeeds'

Run 'creft remove test --docs' for the full reference.";

const UP_SHORT_ABOUT: &str = "\
Installs creft into your coding AI tools. Hook-based for Claude Code and
Gemini; static instruction file for Cursor, Windsurf, Aider, Copilot, Codex.

Examples:
  creft up                  Install globally for all supported systems
  creft up claude-code      Install globally for Claude Code only
  creft up --local          Auto-detect and install for systems in this project

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

const SKILLS_LONG_ABOUT: &str = "\
Manages authored skills

Subcommands:
  test  Run table-driven tests for skills

Options:
  -h, --help   Show this help.
  --docs       Show full documentation.

See 'creft skills <command> --help' for details.";

const SKILLS_SHORT_ABOUT: &str = "\
Subcommands: test

Example:
  creft skills test

Run 'creft skills --docs' for the full reference.";

const SKILLS_TEST_LONG_ABOUT: &str = "\
Skill tests are co-located with the skill they test. A skill at
`.creft/commands/foo.md` is tested by `.creft/commands/foo.test.yaml`,
which contains a YAML list of scenarios.

Each scenario describes:
  - given:    initial filesystem state in a hermetic sandbox
  - before:   optional shell mutation between given and when
  - when:     the `creft` invocation under test (argv, stdin, env)
  - then:     expected exit code, stdout, stderr, files, JSON shape,
              and coverage of the skill's blocks
  - after:    optional shell teardown that always runs

Placeholders `{sandbox}`, `{source}`, and `{home}` expand to paths
inside the sandbox.

Coverage assertions are evaluated against a trace the runner emits per
scenario. The trace records which blocks executed and which
side-channel primitives (creft_print, creft_search, creft_store_*) each
block invoked. The framework compares the trace against the scenario's
`then.coverage` block.

Example fixture:

  - name: setup populates target dir
    given:
      files:
        \"{source}/.claude/rules/x.md\": \"# rule\"
    when:
      argv: [creft, setup, --claude-dir, \"{home}/.claude\"]
    then:
      exit_code: 0
      files:
        \"{home}/.claude/rules/x.md\":
          contains: \"rule\"
      coverage:
        blocks: [0]

Filtering by name:
  SKILL patterns match skill basenames (the filename before `.test.yaml`).
  Plain text matches a basename exactly. Patterns containing `*` or `?`
  are anchored fnmatch globs.

  SCENARIO and --filter patterns match scenario names. A pattern with no
  `*` or `?` is a substring — any name containing it matches. A pattern
  with `*` or `?` is an anchored fnmatch glob.

  --filter <pattern> matches scenario names across every discovered
  fixture. SKILL is optional with --filter; when supplied, it narrows
  the discovered fixture set first and --filter then narrows scenarios
  within it. The SCENARIO positional always requires SKILL (positional
  grammar) and is mutually exclusive with --filter. Empty --filter
  pattern is rejected.

Examples:
  creft skills test setup                   # the 'setup' skill (exact basename match)
  creft skills test \"setup*\"                # all skills whose basename starts with \"setup\"
  creft skills test merge*                  # all skills whose basename starts with \"merge\"
  creft skills test setup fresh-install     # one scenario in the setup skill
  creft skills test --filter \"merge*\"       # every scenario starting with \"merge\" (any skill)
  creft skills test setup --filter \"fresh\"  # setup scenarios containing \"fresh\"";

const SKILLS_TEST_SHORT_ABOUT: &str = "\
Tests are YAML fixtures co-located with the skill they test.

Examples:
  creft skills test                       Run all fixture tests
  creft skills test setup                 Run tests for the 'setup' skill (exact basename)
  creft skills test \"setup*\"              Run tests for all skills starting with 'setup'
  creft skills test \"merge*\"              Run tests for skills starting with \"merge\"
  creft skills test --filter \"merge*\"     Run every scenario starting with \"merge\", across all skills
  creft skills test setup --filter fresh  Run setup scenarios whose name contains \"fresh\"
  creft skills test --where               List discovered fixtures

Run 'creft skills test --docs' for the full reference.";

const COMPLETIONS_SHORT_ABOUT: &str = "\
Supported shells: bash, zsh, fish

Examples:
  creft completions bash >> ~/.bashrc
  creft completions zsh > ~/.zfunc/_creft

Run 'creft completions --docs' for the full reference.";

const ADD_TEST_LONG_ABOUT: &str = "\
Author a new scenario from stdin, mirroring `creft add` for skills.

The stdin envelope uses YAML frontmatter (`---` delimiters) to supply the
target skill and scenario name, followed by the scenario YAML body:

  ---
  skill: setup
  name: fresh install succeeds
  ---
  when:
    argv: [creft, setup]
  then:
    exit_code: 0

Required frontmatter fields:
  skill   The target skill name (e.g. `setup`, `hooks guard bash`).
          The skill must exist in the local root.
  name    The new scenario's name. Must be unique within the fixture
          unless --force is supplied.

The scenario body uses the same shape as hand-authored `*.test.yaml` entries:
`given`, `before`, `when`, `then`, `after`, and `notes` keys.

The fixture file is `<local-root>/commands/<skill-path>.test.yaml`.
If the file does not exist it is created. The existing file is otherwise
preserved verbatim -- comments, blank lines, and hand-formatted YAML survive.

When --force is supplied and a scenario with the same name exists, the entire
file is re-emitted through the YAML emitter to perform the replacement. YAML
comments are not preserved by the emitter; the success message names this
trade-off so it is visible to the caller.

When --force is supplied but no matching scenario exists, the command writes a
warning to stderr and proceeds to append the new scenario:
  warning: --force given but no scenario named '<name>' exists in <path>; appending as a new scenario

Examples:
  creft add test <<'EOF'
  ---
  skill: setup
  name: fresh install succeeds
  ---
  when:
    argv: [creft, setup, --claude-dir, /tmp/target]
  then:
    exit_code: 0
  EOF

  creft add test --force <<'EOF'
  ---
  skill: setup
  name: fresh install succeeds
  ---
  when:
    argv: [creft, setup, --claude-dir, /tmp/target]
  then:
    exit_code: 1
  EOF

Errors:
  Missing piped stdin             creft add test requires piped stdin
  Missing skill: field            missing required frontmatter field 'skill'
  Missing name: field             missing required frontmatter field 'name'
  Skill not found                 command not found: <skill>
  Malformed scenario body         scenario validation failed: ...
  Name collision (no --force)     command already exists: test '<name>' ...";

const ADD_TEST_SHORT_ABOUT: &str = "\
Pipe a scenario envelope (frontmatter + YAML body) to stdin.

Examples:
  creft add test <<'EOF'          Append a new scenario
  creft add test --force <<'EOF'  Replace an existing scenario by name

Run 'creft add test --docs' for the full reference.";

// ── Alias built-in help text ─────────────────────────────────────────────────

const ALIAS_SHORT_ABOUT: &str = "\
Manage namespace aliases that rewrite path prefixes at dispatch.

Subcommands: add, remove, list

Run 'creft alias <subcommand> --help' for details.
Run 'creft alias --docs' for the full reference.";

const ALIAS_LONG_ABOUT: &str = "\
Manage namespace aliases that rewrite path prefixes at dispatch.

Usage: creft alias <subcommand> [args]

Subcommands:
  add     Add an alias from one path to another
  remove  Remove an alias by its 'from' path
  list    List all aliases with scope tags

Run 'creft alias <subcommand> --help' for details.";

const ALIAS_ADD_SHORT_ABOUT: &str = "\
Add a namespace alias: 'creft alias add bl backlog'.

The alias is stored in the same scope as <to> (local or global).
Conflicts with built-ins, existing skills, and cycles are rejected.

Run 'creft alias add --docs' for the full reference.";

const ALIAS_ADD_LONG_ABOUT: &str = "\
Add a namespace alias.

Aliases rewrite path prefixes at dispatch. After 'creft alias add bl backlog',
'creft bl list' resolves to 'creft backlog list'. Multi-word prefixes use
quoting: 'creft alias add \"my new\" \"foo bar\"'.

The alias's storage scope is derived from <to>: if <to> is a local skill,
package, or plugin, the alias goes in <project>/.creft/aliases.yaml; if
global, in ~/.creft/aliases.yaml.

Conflicts with built-in commands, existing skills, and existing namespaces
are rejected. Cycles among aliases are rejected.

Aliases rewrite argv only at the position of a direct skill invocation:
'creft bl --help' rewrites to 'creft backlog --help'. Built-in arguments
are not rewritten — 'creft help bl' does NOT show backlog's help; it shows
the root listing because 'help' is the built-in's name and 'bl' is its
literal argument.";

const ALIAS_REMOVE_SHORT_ABOUT: &str = "\
Remove an alias: 'creft alias remove bl'.

Searches local then global; removes from the first scope that contains it.
Multi-word <from> requires shell quoting: 'creft alias remove \"my new\"'.

Run 'creft alias remove --docs' for the full reference.";

const ALIAS_REMOVE_LONG_ABOUT: &str = "\
Remove an alias.

Searches local then global; removes from the first scope that contains
the alias. Errors if no scope contains it.

Multi-word <from> requires shell quoting: 'creft alias remove \"my new\"'.
Unquoted multi-token forms error as 'unexpected argument'.";

const ALIAS_LIST_SHORT_ABOUT: &str = "\
List all aliases with scope tags.

Prints aliases sorted by 'from', one per line, with '[local]' or '[global]'
tags. 'no aliases defined' when empty.

Run 'creft alias list --docs' for the full reference.";

const ALIAS_LIST_LONG_ABOUT: &str = "\
List all aliases.

Prints aliases sorted by 'from', one per line, with the scope tag in
brackets. Aliases are loaded from <project>/.creft/aliases.yaml (when
present) and ~/.creft/aliases.yaml.";

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
        name: "alias",
        description: "Manage namespace aliases",
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
        name: "skills",
        description: "Manage authored skills",
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
    AddTest,
    Alias,
    AliasAdd,
    AliasRemove,
    AliasList,
    List,
    Show,
    Remove,
    RemoveTest,
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
    Skills,
    SkillsTest,
    Up,
    Init,
    Doctor,
    Completions,
}

impl BuiltinHelp {
    /// The CLI command name for this built-in, matching the entry name stored in `_builtin.idx`.
    ///
    /// Used by `DocsSearch` dispatch to filter index results to the specific
    /// built-in the user queried, and by `rebuild_builtin_index` to name
    /// entries in the index.
    pub(crate) fn cli_name(self) -> &'static str {
        match self {
            Self::Add => "add",
            Self::AddTest => "add test",
            Self::Alias => "alias",
            Self::AliasAdd => "alias add",
            Self::AliasRemove => "alias remove",
            Self::AliasList => "alias list",
            Self::List => "list",
            Self::Show => "show",
            Self::Remove => "remove",
            Self::RemoveTest => "remove test",
            Self::Plugin => "plugin",
            Self::PluginInstall => "plugin install",
            Self::PluginUpdate => "plugin update",
            Self::PluginUninstall => "plugin uninstall",
            Self::PluginActivate => "plugin activate",
            Self::PluginDeactivate => "plugin deactivate",
            Self::PluginList => "plugin list",
            Self::PluginSearch => "plugin search",
            Self::Settings => "settings",
            Self::SettingsShow => "settings show",
            Self::SettingsSet => "settings set",
            Self::Skills => "skills",
            Self::SkillsTest => "skills test",
            Self::Up => "up",
            Self::Init => "init",
            Self::Doctor => "doctor",
            Self::Completions => "completions",
        }
    }

    /// Look up a variant by its CLI name.
    ///
    /// Returns `None` if the name doesn't match any variant.
    pub(crate) fn from_cli_name(name: &str) -> Option<BuiltinHelp> {
        Self::all_variants()
            .iter()
            .find(|v| v.cli_name() == name)
            .copied()
    }

    /// All `BuiltinHelp` variants in declaration order.
    ///
    /// Used by the index lifecycle to iterate over all built-in commands
    /// when building `_builtin.idx`.
    pub(crate) fn all_variants() -> &'static [BuiltinHelp] {
        &[
            Self::Add,
            Self::AddTest,
            Self::Alias,
            Self::AliasAdd,
            Self::AliasRemove,
            Self::AliasList,
            Self::List,
            Self::Show,
            Self::Remove,
            Self::RemoveTest,
            Self::Plugin,
            Self::PluginInstall,
            Self::PluginUpdate,
            Self::PluginUninstall,
            Self::PluginActivate,
            Self::PluginDeactivate,
            Self::PluginList,
            Self::PluginSearch,
            Self::Settings,
            Self::SettingsShow,
            Self::SettingsSet,
            Self::Skills,
            Self::SkillsTest,
            Self::Up,
            Self::Init,
            Self::Doctor,
            Self::Completions,
        ]
    }
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
        BuiltinHelp::AddTest => renderer::render_add_test_short(),
        BuiltinHelp::Alias => renderer::render_alias_short(),
        BuiltinHelp::AliasAdd => renderer::render_alias_add_short(),
        BuiltinHelp::AliasRemove => renderer::render_alias_remove_short(),
        BuiltinHelp::AliasList => renderer::render_alias_list_short(),
        BuiltinHelp::List => renderer::render_list_short(),
        BuiltinHelp::Show => renderer::render_show_short(),
        BuiltinHelp::Remove => renderer::render_remove_short(),
        BuiltinHelp::RemoveTest => renderer::render_remove_test_short(),
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
        BuiltinHelp::Skills => renderer::render_skills_short(),
        BuiltinHelp::SkillsTest => renderer::render_skills_test_short(),
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
        BuiltinHelp::AddTest => renderer::render_add_test(),
        BuiltinHelp::Alias => renderer::render_alias(),
        BuiltinHelp::AliasAdd => renderer::render_alias_add(),
        BuiltinHelp::AliasRemove => renderer::render_alias_remove(),
        BuiltinHelp::AliasList => renderer::render_alias_list(),
        BuiltinHelp::List => renderer::render_list(),
        BuiltinHelp::Show => renderer::render_show(),
        BuiltinHelp::Remove => renderer::render_remove(),
        BuiltinHelp::RemoveTest => renderer::render_remove_test(),
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
        BuiltinHelp::Skills => renderer::render_skills(),
        BuiltinHelp::SkillsTest => renderer::render_skills_test(),
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

    use crate::wrap::{MAX_WIDTH, wrap_description, wrap_text};

    use super::{
        ADD_LONG_ABOUT, ADD_SHORT_ABOUT, ADD_TEST_LONG_ABOUT, ADD_TEST_SHORT_ABOUT,
        ALIAS_ADD_LONG_ABOUT, ALIAS_ADD_SHORT_ABOUT, ALIAS_LIST_LONG_ABOUT, ALIAS_LIST_SHORT_ABOUT,
        ALIAS_LONG_ABOUT, ALIAS_REMOVE_LONG_ABOUT, ALIAS_REMOVE_SHORT_ABOUT, ALIAS_SHORT_ABOUT,
        COMPLETIONS_LONG_ABOUT, COMPLETIONS_SHORT_ABOUT, DOCTOR_LONG_ABOUT, DOCTOR_SHORT_ABOUT,
        INIT_LONG_ABOUT, INIT_SHORT_ABOUT, LIST_LONG_ABOUT, LIST_SHORT_ABOUT,
        PLUGIN_ACTIVATE_LONG_ABOUT, PLUGIN_ACTIVATE_SHORT_ABOUT, PLUGIN_DEACTIVATE_LONG_ABOUT,
        PLUGIN_DEACTIVATE_SHORT_ABOUT, PLUGIN_INSTALL_LONG_ABOUT, PLUGIN_INSTALL_SHORT_ABOUT,
        PLUGIN_LIST_LONG_ABOUT, PLUGIN_LIST_SHORT_ABOUT, PLUGIN_LONG_ABOUT,
        PLUGIN_SEARCH_LONG_ABOUT, PLUGIN_SEARCH_SHORT_ABOUT, PLUGIN_SHORT_ABOUT,
        PLUGIN_UNINSTALL_LONG_ABOUT, PLUGIN_UNINSTALL_SHORT_ABOUT, PLUGIN_UPDATE_LONG_ABOUT,
        PLUGIN_UPDATE_SHORT_ABOUT, REMOVE_LONG_ABOUT, REMOVE_SHORT_ABOUT, REMOVE_TEST_LONG_ABOUT,
        REMOVE_TEST_SHORT_ABOUT, SETTINGS_LONG_ABOUT, SETTINGS_SET_SHORT_ABOUT,
        SETTINGS_SHORT_ABOUT, SETTINGS_SHOW_SHORT_ABOUT, SHOW_LONG_ABOUT, SHOW_SHORT_ABOUT,
        SKILLS_LONG_ABOUT, SKILLS_SHORT_ABOUT, SKILLS_TEST_LONG_ABOUT, SKILLS_TEST_SHORT_ABOUT,
        UP_LONG_ABOUT, UP_SHORT_ABOUT,
    };

    /// Format a help page with a short description, usage line, and long about.
    pub fn page(short_desc: &str, usage: &str, long_about: &str) -> String {
        let wrapped = wrap_text(long_about, MAX_WIDTH, 0);
        format!("{short_desc}\n\n{}{usage}\n\n{wrapped}\n", "Usage: ".bold(),)
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
            let desc_col = 2 + max_label + 2;
            let desc_budget = MAX_WIDTH.saturating_sub(desc_col);
            for (label, desc) in options {
                let pad = " ".repeat(max_label - label.len());
                let wrapped = wrap_description(desc, desc_budget, desc_col);
                out.push_str(&format!("  {}{pad}  {wrapped}\n", label.bold()));
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

    pub fn render_add_test() -> String {
        page_with_options(
            "Append or replace a test scenario from stdin",
            "creft add test [OPTIONS]",
            ADD_TEST_LONG_ABOUT,
            &[(
                "--force",
                "Replace an existing scenario with the same name. Without a \
                 collision, --force is a no-op (the scenario is appended) and \
                 a warning is printed. Note: replacement re-emits the file via \
                 the YAML emitter; comments may be lost.",
            )],
        )
    }

    pub fn render_alias() -> String {
        page(
            "Manage namespace aliases",
            "creft alias <subcommand> [args]",
            ALIAS_LONG_ABOUT,
        )
    }

    pub fn render_alias_add() -> String {
        page(
            "Add a namespace alias",
            "creft alias add <from> <to>",
            ALIAS_ADD_LONG_ABOUT,
        )
    }

    pub fn render_alias_remove() -> String {
        page(
            "Remove an alias",
            "creft alias remove <from>",
            ALIAS_REMOVE_LONG_ABOUT,
        )
    }

    pub fn render_alias_list() -> String {
        page(
            "List all aliases",
            "creft alias list",
            ALIAS_LIST_LONG_ABOUT,
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
            "creft remove --skill <name> [OPTIONS]",
            REMOVE_LONG_ABOUT,
            &[("--global, -g", "Remove from global ~/.creft/ storage")],
        )
    }

    pub fn render_remove_test() -> String {
        page_with_options(
            "Delete one scenario from a skill's *.test.yaml fixture",
            "creft remove test --skill <name> --name <scenario>",
            REMOVE_TEST_LONG_ABOUT,
            &[
                ("--skill <name>", "Target skill"),
                ("--name <name>", "Exact value of the scenario's name field"),
            ],
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
        page(
            "Install a plugin from a git repository",
            "creft plugin install <source>",
            PLUGIN_INSTALL_LONG_ABOUT,
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

    pub fn render_skills() -> String {
        page(
            "Manage authored skills",
            "creft skills <command> [OPTIONS]",
            SKILLS_LONG_ABOUT,
        )
    }

    pub fn render_skills_test() -> String {
        page_with_options(
            "Run table-driven tests for skills",
            "creft skills test [SKILL] [SCENARIO] [OPTIONS]",
            SKILLS_TEST_LONG_ABOUT,
            &[
                (
                    "SKILL",
                    "Pattern matching skill basenames (the part of the filename before \
                     `.test.yaml`). Plain text matches a basename exactly. \
                     Patterns containing `*` or `?` are anchored fnmatch globs.",
                ),
                (
                    "SCENARIO",
                    "Pattern matching scenario names within the supplied SKILL. Plain text \
                     matches any scenario whose name contains it. Patterns containing `*` or \
                     `?` are anchored fnmatch globs. Cannot be combined with `--filter`. \
                     Requires SKILL to precede it (positional grammar).",
                ),
                (
                    "--filter <PATTERN>",
                    "Pattern matching scenario names across every discovered fixture. Same \
                     pattern shape as SCENARIO. SKILL is optional; when supplied, narrows \
                     the discovered fixture set first. Cannot be combined with the SCENARIO \
                     positional. Cannot be empty.",
                ),
                (
                    "--keep",
                    "Preserve sandbox directories for failed scenarios; their \
                     paths are printed on stderr.",
                ),
                (
                    "--detail",
                    "Show stdout/stderr for every scenario, not just failures.",
                ),
                (
                    "--where",
                    "List discovered fixtures and scenarios, then exit.",
                ),
            ],
        )
    }

    pub fn render_up() -> String {
        page_with_options(
            "Install creft for your coding AI",
            "creft up [system] [OPTIONS]",
            UP_LONG_ABOUT,
            &[(
                "--local, -l",
                "Install in this project only (auto-detect systems in CWD)",
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

    pub fn render_add_test_short() -> String {
        page_with_options(
            "Append or replace a test scenario from stdin",
            "creft add test [OPTIONS]",
            ADD_TEST_SHORT_ABOUT,
            &[(
                "--force",
                "Replace an existing scenario by name (warns if no collision)",
            )],
        )
    }

    pub fn render_alias_short() -> String {
        page(
            "Manage namespace aliases",
            "creft alias <subcommand> [args]",
            ALIAS_SHORT_ABOUT,
        )
    }

    pub fn render_alias_add_short() -> String {
        page(
            "Add a namespace alias",
            "creft alias add <from> <to>",
            ALIAS_ADD_SHORT_ABOUT,
        )
    }

    pub fn render_alias_remove_short() -> String {
        page(
            "Remove an alias",
            "creft alias remove <from>",
            ALIAS_REMOVE_SHORT_ABOUT,
        )
    }

    pub fn render_alias_list_short() -> String {
        page(
            "List all aliases",
            "creft alias list",
            ALIAS_LIST_SHORT_ABOUT,
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
            "creft remove --skill <name> [OPTIONS]",
            REMOVE_SHORT_ABOUT,
            &[("--global, -g", "Remove from global ~/.creft/ storage")],
        )
    }

    pub fn render_remove_test_short() -> String {
        page_with_options(
            "Delete one scenario from a skill's *.test.yaml fixture",
            "creft remove test --skill <name> --name <scenario>",
            REMOVE_TEST_SHORT_ABOUT,
            &[
                ("--skill <name>", "Target skill"),
                ("--name <name>", "Exact value of the scenario's name field"),
            ],
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
            "creft plugin install <source>",
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

    pub fn render_skills_short() -> String {
        page(
            "Manage authored skills",
            "creft skills <command> [OPTIONS]",
            SKILLS_SHORT_ABOUT,
        )
    }

    pub fn render_skills_test_short() -> String {
        page_with_options(
            "Run table-driven tests for skills",
            "creft skills test [SKILL] [SCENARIO] [OPTIONS]",
            SKILLS_TEST_SHORT_ABOUT,
            &[
                (
                    "--filter <PATTERN>",
                    "Run only scenarios matching this pattern, across every \
                     discovered fixture. Plain text matches any name \
                     containing it; patterns with `*` or `?` are anchored \
                     fnmatch globs. SKILL is optional; when supplied, it \
                     narrows the discovered fixture set first. Cannot be \
                     combined with the SCENARIO positional. Cannot be empty.",
                ),
                (
                    "--keep",
                    "Preserve sandbox directories for failed scenarios",
                ),
                ("--detail", "Show stdout/stderr for every scenario"),
                (
                    "--where",
                    "List discovered fixtures and scenarios, then exit",
                ),
            ],
        )
    }

    pub fn render_up_short() -> String {
        page_with_options(
            "Install creft for your coding AI",
            "creft up [system] [OPTIONS]",
            UP_SHORT_ABOUT,
            &[(
                "--local, -l",
                "Install in this project only (auto-detect systems in CWD)",
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
    use serial_test::serial;

    use super::*;

    #[test]
    fn builtins_returns_twelve_entries() {
        assert_eq!(builtins().len(), 12);
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
            "skills",
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
    #[serial]
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
    #[serial]
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
    #[serial]
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
    #[serial]
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
    #[serial]
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
    #[serial]
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
    #[serial]
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
    #[serial]
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
    #[serial]
    fn render_docs_enabled_produces_ansi_bold_on_headers() {
        yansi::enable();
        let output = render_docs(BuiltinHelp::Add);
        assert!(
            output.contains("\x1b["),
            "render_docs with yansi enabled must contain ANSI escapes on section headers",
        );
    }

    #[test]
    #[serial]
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
