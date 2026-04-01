/// One-line tagline shown in the root help header.
pub const ROOT_ABOUT: &str = "Executable skills for Agents";

/// Extended description shown by `creft --help`, including usage examples and storage overview.
pub const ROOT_LONG_ABOUT: &str = "\
Executable skills for Agents

Saves reusable commands as markdown and runs them as subcommands.
Skills are .md files with YAML frontmatter and fenced code blocks.

  creft add <<'EOF'              save a skill from stdin
  creft hello World              run a skill directly
  creft hello World --verbose    show rendered blocks, then run
  creft hello World --dry-run    show rendered blocks, do not run
  creft list                     see available skills
  creft add --help               learn how to create skills

Skills are stored in .creft/ (project-local) or ~/.creft/ (global).
Local skills shadow global ones with the same name.

Global Flags (available on every skill):
  --dry-run       Show rendered template blocks, do not execute
  --verbose, -v   Show rendered template blocks on stderr, then execute";

/// Extended description shown by `creft add --help`, covering skill format, frontmatter fields, and validation.
pub const ADD_LONG_ABOUT: &str = "\
Saves a new skill to the registry

Use this when you have a shell recipe, API call, or multi-step workflow
you want to reuse. Pipe the skill definition as markdown to stdin.

Examples:
  creft add <<'EOF'                          Save from stdin (recommended)
  creft add --name hello --description \"...\" Create from flags (quick one-liners)
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

Frontmatter Fields:
  name          Required. Spaces create namespaces: 'gh issue-body' -> 'creft gh issue-body'
  description   Required. One line: what it does and when to use it
  args          Positional arguments. Each has: name, description, default, required, validation
  flags         Named --flags. Each has: name, short, type (bool/string), default, validation
  env           Environment variables. Each has: name, required (default true)
  tags          List of tags for filtering with 'creft list --tag'
  pipe          When true, stdout pipes between blocks. Default false: uses $CREFT_PREV

Code Blocks:
  Each fenced block is an executable step. Language tag sets the interpreter.
  Blocks run in order; if one fails, execution stops.

  Exit codes:
    0     Success, continue to the next block
    1-98  Error, stop the pipeline and propagate the exit code
    99    Early successful return — stop the pipeline, creft exits 0
    100+  Error, stop the pipeline and propagate the exit code

  Interpreters: bash, python, node, zsh, ruby, docs (not executed -- shown in --help)

  LLM Blocks:
    Use ```llm to send prompts to AI CLI tools. Add a YAML header before
    --- to configure the provider:

      ```llm
      provider: claude
      model: haiku
      params: \"--max-tokens 500\"
      ---
      Summarize this: {{prev}}
      ```

    Providers: claude (default), gemini, codex, ollama, or any CLI tool name.
    The provider handles authentication (API keys, config files).
    Template placeholders ({{prev}}, {{name}}) work in the prompt body.
    Skills with llm blocks always run sequentially, even with pipe: true.

  Dependencies (first line comment):
    # deps: requests, pandas          Python (uses uv run --with)
    // deps: lodash, chalk            Node (uses npx --package=)
    # deps: jq, yq                    Shell (warns if not on PATH)

Template Placeholders:
  {{name}}            Positional arg or flag value
  {{prev}}            Stdout of previous code block
  {{name|default}}    Value with fallback

Output Chaining:
  Default (pipe: false): $CREFT_PREV, $CREFT_BLOCK_1, $CREFT_BLOCK_2, etc.
  Pipeline (pipe: true): stdout pipes directly as stdin to next block

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

Examples:
  creft show hello
  creft show gh issue-body";

/// Extended description shown by `creft cat --help`, explaining code-only output mode.
pub const CAT_LONG_ABOUT: &str = "\
Prints just the executable code blocks

Strips frontmatter and docs, showing only the runnable scripts. Use this
to pipe skill code to another tool, inspect the raw script, or copy it.

Examples:
  creft cat hello
  creft cat gh issue-body";

/// Extended description shown by `creft rm --help`, including namespace cleanup behavior.
pub const RM_LONG_ABOUT: &str = "\
Deletes a skill from the registry

Removes the skill file. Empty namespace directories are cleaned up
automatically.

Examples:
  creft rm hello
  creft rm gh issue-body";

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
are refreshed; non-creft content is preserved. Some systems (Cursor,
Windsurf) don't support global rules via files.";

/// Extended description shown by `creft edit --help`, covering both pipe-replace and `$EDITOR` modes.
pub const EDIT_LONG_ABOUT: &str = "\
Opens a skill for editing

Two modes: pipe new content to stdin (for scripts and AI agents), or
open in $EDITOR (for humans).

Examples:
  creft edit hello                         Open in $EDITOR
  echo \"$content\" | creft edit hello       Replace content from stdin
  EDITOR=\"code --wait\" creft edit hello    Use VS Code as editor
  creft edit gh issue-body                 Edit a namespaced skill

Pipe Mode:
  Pipe valid creft markdown to stdin. The skill file is replaced.
  Content is validated before writing. Use --no-validate to skip.

Editor Mode:
  Without piped input, opens in $EDITOR (defaults to vi).
  Multi-word editors like 'code --wait' are supported.";

/// Extended description shown by `creft install --help`, covering package manifest format and namespacing.
pub const INSTALL_LONG_ABOUT: &str = "\
Installs a skill package from a git repository

A skill package is a git repo with a creft.yaml manifest. Any .md file
with valid creft frontmatter becomes an available skill, namespaced
under the package name.

Examples:
  creft install https://github.com/someone/k8s-tools
  creft install git@github.com:someone/k8s-tools.git
  creft install /path/to/local/repo

After installing, skills are available as subcommands:
  creft k8s-tools deploy production

Manifest Format (creft.yaml):
  name: k8s-tools
  version: 0.1.0
  description: Kubernetes deployment skills

The package name in the manifest determines the namespace, not the
repo name. Packages install to nearest .creft/packages/ or ~/.creft/packages/.
Use --global to always install globally.";

/// Extended description shown by `creft update --help`, explaining single-package and all-packages modes.
pub const UPDATE_LONG_ABOUT: &str = "\
Updates installed skill packages

Runs git pull on the package's cloned repository.

Examples:
  creft update k8s-tools    Update a specific package
  creft update              Update all installed packages";

/// Extended description shown by `creft uninstall --help`, clarifying that local skills are unaffected.
pub const UNINSTALL_LONG_ABOUT: &str = "\
Removes an installed skill package

Deletes the package directory and all its skills. Local (user-created)
skills are not affected.

Examples:
  creft uninstall k8s-tools";

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
  uv, npx), AI providers (claude, gemini, codex, ollama -- all optional),
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
