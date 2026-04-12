# creft

[![CI](https://github.com/chrisfentiman/creft/actions/workflows/ci.yml/badge.svg)](https://github.com/chrisfentiman/creft/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/creft.svg)](https://crates.io/crates/creft)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

A skill system that turns markdown instructions into executable commands.

![demo](assets/demo.gif)

AI coding agents generate useful workflows — deploy scripts, test runners, code analysis pipelines — and then lose them when the session ends. Creft captures those workflows as markdown files and runs them as CLI commands. Same input, same output, no LLM needed at runtime.

- **Agents author skills.** Write a markdown file with `creft add`. Next session, next machine, the command is there.
- **Skills run deterministically.** No interpretation, no token cost at execution time.
- **LLM blocks bring AI into the pipeline.** Send output to Claude, Gemini, Codex, or Ollama as a step in the chain.
- **Skills validate before saving.** Syntax, PATH commands, PyPI/npm dependencies — checked at `creft add` time.
- **Plugins extend creft.** Install collections of skills from any git repo and activate them per project.

## Install

```sh
cargo install creft
```

Or: `brew install chrisfentiman/creft/creft` · [Binary releases](https://github.com/chrisfentiman/creft/releases)

## Quick start

````sh
creft add <<'EOF'
---
name: hello
description: Greet someone
args:
  - name: who
---
```bash
echo "Hello, {{who}}!"
```
EOF
````

```
$ creft hello World
Hello, World!
```

For args, flags, env vars, validation, multi-block pipes, LLM blocks, and more: [Skill Authoring Reference](docs/skill-reference.md).

## What creft can do

### Multi-language pipelines

Skills pipe stdout between blocks using OS file descriptors. Blocks run concurrently, not sequentially. Supported languages: `bash`, `sh`, `zsh`, `python`, `node`, `ruby`, and any interpreter on PATH.

````
```bash
find src -name '*.rs' -exec wc -l {} +
```
```python
import sys
# reads stdin from the bash block above
for line in sys.stdin.read().strip().splitlines():
    ...
```
````

### LLM blocks

Send pipeline output to an AI provider as a step in the chain. The default provider is `claude`; `gemini`, `codex`, and `ollama` are also supported.

````
```bash
git diff HEAD~1
```
```llm
provider: claude
Summarize these changes in one sentence.
```
````

### Args, flags, and env vars

Declare a typed CLI interface in YAML frontmatter. Args support regex validation. Flags support bool and string types. Required env vars are checked before the skill runs.

```yaml
args:
  - name: env
    validation: "^(staging|production)$"
flags:
  - name: dry-run
    short: d
    type: bool
env:
  - name: AWS_PROFILE
    required: true
```

### Validation

`creft add` checks syntax, verifies template variables are declared, runs shellcheck on bash blocks, and resolves `# deps:` declarations against the PyPI/npm registry before saving the file.

### Plugins

Install a collection of skills from any public git repo. The plugin is cached globally; activate specific commands in a project.

```sh
creft plugin install https://github.com/example/k8s-tools
creft plugin activate k8s-tools
creft k8s-tools deploy production
```

Activate a single command instead of the whole plugin:

```sh
creft plugin activate k8s-tools/deploy
```

## Bundled plugins

Creft ships four plugins. Install them individually or search across all of them with `creft plugin search`.

### fetch

Pull dependency source code into `workbench/code/` so agents read the actual implementation instead of guessing signatures.

```sh
creft plugin install creft-bundled --plugin fetch
creft plugin activate fetch

creft fetch serde
creft fetch serde@1.0.196
creft fetch serde-rs/serde@v1.0.196
creft fetch --list
```

Ecosystem detection reads `Cargo.toml`, `package.json`, or `pyproject.toml` to resolve bare package names. Override with `--ecosystem crates`, `--ecosystem npm`, or `--ecosystem pypi`.

### ask

Query a registered project through its own Claude environment, or open a native dialog to collect input from the user.

```sh
creft plugin install creft-bundled --plugin ask
creft plugin activate ask

# Query another project
creft ask weft "how does the router handle auth failures?"

# Ask the user
creft ask "Which environment?" --type choice --options "staging,production"
creft ask "Continue?" --type confirm
```

Projects are registered in `~/.creft/projects.json`. `creft ask --list` shows registered projects.

### mcp

Manage MCP server configurations without editing JSON. Secrets go to the macOS keychain; the settings file is assembled from a vault at sync time.

```sh
creft plugin install creft-bundled --plugin mcp
creft plugin activate mcp

creft mcp add arxiv --http https://mcp.arxiv.org --requires-auth
creft mcp ls
creft mcp sync
creft mcp remove arxiv --purge
```

Default scope installs per-project (`projects[<cwd>].mcpServers` in `~/.claude.json`). Pass `--global` to install at the user level.

### schedule

Schedule recurring agent tasks on macOS via launchd. Jobs persist across reboots, log to a file, and run with your full shell environment.

```sh
creft plugin install creft-bundled --plugin schedule
creft plugin activate schedule

creft schedule add daily-brief \
  --schedule "0 7 * * *" \
  --command "creft run daily-brief" \
  --workdir ~/projects/my-repo

creft schedule ls
creft schedule status daily-brief
creft schedule run daily-brief
creft schedule remove daily-brief
```

## Agent integration

```sh
creft up
```

Auto-detects and installs instruction files for: Claude Code, Cursor, Windsurf, Aider, GitHub Copilot, Codex, Gemini CLI.

After setup, agents discover skills with `creft list`, run them with `creft <name>`, and author new ones with `creft add`. The instruction files teach the agent the full workflow without human prompting.

File locations (project-level by default):

| System | File |
|---|---|
| Claude Code | `.claude/skills/creft/SKILL.md` |
| Cursor | `.cursor/rules/creft.mdc` |
| Windsurf | `.windsurf/rules/creft.md` |

Run `creft up --help` for all systems and the `--global` flag.

## Commands

### Skill commands

| | |
|---|---|
| `creft add` | Save a skill from stdin |
| `creft edit <name>` | Edit in `$EDITOR` or from stdin |
| `creft rm <name>` | Delete a skill |
| `creft list` | List skills |
| `creft show <name>` | Print a skill's full definition |
| `creft cat <name>` | Print code blocks only |

### Plugin commands

| | |
|---|---|
| `creft plugin install <url>` | Install a plugin from a git repo |
| `creft plugin update [name]` | Update installed plugins |
| `creft plugin uninstall <name>` | Remove a plugin |
| `creft plugin activate <target>` | Make plugin commands available in a scope |
| `creft plugin deactivate <target>` | Remove plugin commands from a scope |
| `creft plugin list [name]` | List installed plugins or commands in a plugin |
| `creft plugin search <query>` | Search commands across installed plugins |

`<target>` is `plugin-name` to activate all commands or `plugin-name/command` to activate one. Pass `--global` to activate at the user level instead of the nearest `.creft/`.

### Other commands

| | |
|---|---|
| `creft up [system]` | Set up AI agent integration |
| `creft init` | Initialize local `.creft/` |
| `creft doctor [name]` | Check environment or skill health |

## This repo runs on creft

| | |
|---|---|
| `creft test` | Run tests, markdown output |
| `creft test mutants` | Mutation testing |
| `creft lint` | Clippy, markdown output |
| `creft coverage` | Code coverage with source context |
| `creft check` | All quality gates (calls test, lint, coverage) |
| `creft bench` | Compile time, test time, binary size |
| `creft changelog` | Changelog from git history |

## Documentation

- [Skill Authoring Reference](docs/skill-reference.md) — complete reference for the skill format: frontmatter schema, code blocks, exit codes, LLM blocks, dependencies, validation, and storage.
- `creft add --help` — quick reference accessible from the terminal.
- `creft doctor` — check whether your environment can run skills.

## Contributing

Pull requests welcome. Open an issue first for significant changes.

## License

[MIT](LICENSE)
