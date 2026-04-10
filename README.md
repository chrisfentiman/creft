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
- **Skills distribute as packages.** `creft install <git-url>` shares workflows across teams.

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

### Packages

Install a collection of skills from any public git repo. Skills are namespaced under the package name.

```sh
creft install https://github.com/example/k8s-tools
creft k8s-tools deploy production
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

| | |
|---|---|
| `creft add` | Save a skill from stdin |
| `creft edit <name>` | Edit in `$EDITOR` or from stdin |
| `creft rm <name>` | Delete a skill |
| `creft list` | List skills |
| `creft show <name>` | Print a skill's full definition |
| `creft cat <name>` | Print code blocks only |
| `creft install <url>` | Install a skill package from git |
| `creft update [name]` | Update installed packages |
| `creft uninstall <name>` | Remove a package |
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
