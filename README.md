# creft

[![CI](https://github.com/chrisfentiman/creft/actions/workflows/ci.yml/badge.svg)](https://github.com/chrisfentiman/creft/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/creft.svg)](https://crates.io/crates/creft)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

**Executable skills for AI agents.**

Save agent workflows as markdown. Run them as CLI commands.

![demo](assets/demo.gif)

## Install

```sh
cargo install creft
```

Or: `brew install chrisfentiman/creft/creft` · [Binary releases](https://github.com/chrisfentiman/creft/releases)

## Quick start

Save and run a skill:

````sh
creft cmd add <<'EOF'
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

Add an LLM block — pipe `git diff` to Claude for a one-line summary:

````sh
creft cmd add <<'EOF'
---
name: summarize-diff
description: Summarize recent git changes in one sentence.
---
```bash
git diff HEAD~1
```
```llm
---
Summarize these changes in one sentence.
```
EOF
````

```
$ creft summarize-diff
Adds retry logic to the HTTP client with configurable backoff.
```

For args, flags, env vars, multi-block pipes, LLM provider options, and more: [Skill Authoring Reference](docs/skill-reference.md) · `creft cmd add --help`

## Agent integration

```sh
creft up
```

Auto-detects and installs instruction files for: Claude Code, Cursor, Windsurf, Aider, GitHub Copilot, Codex, Gemini CLI.

After setup, agents discover skills with `creft cmd list`, run them with `creft <name>`, and author new ones with `creft cmd add`. The instruction files teach the agent the full workflow without human prompting.

| System | File |
|---|---|
| Claude Code | `.claude/skills/creft/SKILL.md` |
| Cursor | `.cursor/rules/creft.mdc` |
| Windsurf | `.windsurf/rules/creft.md` |

Run `creft up --help` for all systems and the `--global` flag.

## What creft can do

### The bidirectional loop

Agents both author and consume skills in the same format. `creft cmd add` to save a workflow, `creft cmd list` to see what exists, `creft <name>` to execute. The agent that creates a skill and the agent that runs it three weeks later on a different machine need no shared context — the skill is the context.

| Without creft | With creft |
|---|---|
| Agent workflows die when the session ends | Agent workflows persist as executable skills |
| You re-explain the same procedures every session | The agent discovers existing skills and builds on them |
| Complex pipelines require glue code and configuration | `bash \| python \| llm` in one markdown file |
| Sharing a workflow means sharing a README and hoping | `creft plugins install` and it works |

### Multi-language pipelines with LLM blocks

`bash | python | llm` in one file. Blocks connect as a pipeline — each block's stdout feeds the next block's stdin via OS file descriptors. All blocks run concurrently, like Unix pipes.

````markdown
```bash
curl -s -H "Authorization: token ${GITHUB_TOKEN}" \
  "https://api.github.com/repos/{{repo}}/issues?state=open"
```
```python
import sys, json
issues = json.load(sys.stdin)
for i in issues[:10]:
    print(f"#{i['number']} {i['title']}")
```
```llm
---
Summarize these GitHub issues. Group by theme. Be concise:

{{prev}}
```
````

The LLM block buffers upstream input, sends it to the provider CLI with the prompt, and pipes the response downstream. Providers: `claude` (default), `gemini`, `codex`, `ollama`, or any CLI tool name. Authentication is handled by the provider's own CLI — no API keys in creft.

Supported languages: `bash`, `sh`, `zsh`, `python`, `node`, `ruby`, and any interpreter on PATH.

### Args, flags, and env vars

Declare a typed CLI interface in YAML frontmatter. Args support regex validation. Flags support bool and string types. Required env vars are checked before the skill runs.

```yaml
args:
  - name: env
    description: Target environment
    required: true
    validation: "^(staging|production)$"
flags:
  - name: dry-run
    short: d
    type: bool
    description: Show what would happen
env:
  - name: AWS_PROFILE
    required: true
```

### Validation at save time

Skills can't break because they're checked before they exist. `creft cmd add` runs syntax checks (`bash -n`, `python ast`, `node --check`), shellcheck on bash blocks, PATH verification for commands, and dependency resolution against PyPI/npm before saving the file.

Use `--force` to skip all checks, or `--no-validate` to skip validation only.

### Plugins

Install skill collections from any git repo. Activate per-project or globally.

```sh
creft plugins install https://github.com/example/k8s-tools
creft plugins activate k8s-tools
creft k8s-tools deploy production
```

Activate a single command instead of the whole plugin:

```sh
creft plugins activate k8s-tools/deploy
```

## Commands

### Skill commands

| | |
|---|---|
| `creft cmd add` | Save a skill from stdin |
| `creft cmd rm <name>` | Delete a skill |
| `creft cmd list` | List skills |
| `creft cmd show <name>` | Print a skill's full definition |
| `creft cmd cat <name>` | Print code blocks only |

### Plugin commands

| | |
|---|---|
| `creft plugins install <url>` | Install a plugin from a git repo |
| `creft plugins update [name]` | Update installed plugins |
| `creft plugins uninstall <name>` | Remove a plugin |
| `creft plugins activate <target>` | Make plugin commands available in a scope |
| `creft plugins deactivate <target>` | Remove plugin commands from a scope |
| `creft plugins list [name]` | List installed plugins or commands in a plugin |
| `creft plugins search <query>` | Search commands across installed plugins |

`<target>` is `plugin-name` to activate all commands or `plugin-name/command` to activate one. Pass `--global` to activate at the user level instead of the nearest `.creft/`.

### Other commands

| | |
|---|---|
| `creft up [system]` | Set up AI agent integration |
| `creft init` | Initialize local `.creft/` |
| `creft doctor [name]` | Check environment or skill health |
| `creft settings` | Manage configuration |

## Bundled plugins

creft ships four plugins. Install from `creft/<plugin>`, activate what you need.

| Plugin | What it does | Example |
|---|---|---|
| **fetch** | Pull dependency source into `workbench/code/` for agent reading | `creft fetch serde` |
| **ask** | Query another project's Claude env or collect user input | `creft ask infra-repo "how does the router work?"` |
| **mcp** | Manage MCP server configs with keychain secrets | `creft mcp add arxiv --http https://mcp.arxiv.org` |
| **schedule** | Schedule recurring agent tasks via launchd | `creft schedule add daily-brief --schedule "0 7 * * *"` |

```sh
creft plugins install creft/fetch
creft plugins activate fetch
```

See [Bundled Plugins Reference](docs/bundled-plugins.md) for full usage.

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
- `creft cmd add --help` — quick reference accessible from the terminal.
- `creft doctor` — check whether your environment can run skills.

## Contributing

Pull requests welcome. Open an issue first for significant changes.

## License

[MIT](LICENSE)

