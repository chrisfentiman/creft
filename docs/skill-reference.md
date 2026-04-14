# Skill Authoring Reference

Complete reference for the creft skill format. Run `creft add --help` for a shorter version accessible from the terminal.

---

## Frontmatter Schema

Every skill file starts with YAML frontmatter between `---` delimiters, followed by one or more fenced code blocks.

````
---
name: hello
description: Greets someone by name.
args:
  - name: who
    description: Name to greet
    required: true
---

```bash
echo "Hello, {{who}}!"
```
````

### `name`

**Required.** String. The skill's invocation name.

Spaces create namespaces: `"gh issue-body"` registers as `creft gh issue-body`. The name maps to a filesystem path: `"gh issue-body"` stores as `.creft/commands/gh/issue-body.md`.

Names cannot match any built-in creft subcommand: `cmd`, `command`, `plugins`, `settings`, `up`, `help`, `version`, `init`, `doctor`.

```yaml
name: deploy
```

```yaml
name: gh issue-body
```

### `description`

**Required.** String. One line: what the skill does and when to use it.

Keep descriptions under 80 characters. Longer descriptions degrade `creft list` output (a warning is emitted at save time).

```yaml
description: Deploys the current branch to staging.
```

### `args`

Optional. List of positional arguments. Provided on the command line in declaration order.

Each arg has:

| Field | Type | Default | Description |
|---|---|---|---|
| `name` | string | — | Required. Used as the template placeholder `{{name}}`. |
| `description` | string | `""` | Shown in `creft <skill> --help`. |
| `default` | string | none | Value used when the arg is not provided. |
| `required` | bool | `false` | Fail with exit 3 if not provided and no default. |
| `validation` | string | none | Regex. Applied to the final value; fails with exit 3 if no match. |

```yaml
args:
  - name: env
    description: Target environment (staging or production)
    required: true
    validation: "^(staging|production)$"
  - name: branch
    description: Branch to deploy
    default: main
```

### `flags`

Optional. List of named `--flag` options.

Each flag has:

| Field | Type | Default | Description |
|---|---|---|---|
| `name` | string | — | Required. Invoked as `--name`. |
| `short` | string | none | Single-character short form. `"v"` → `-v`. |
| `description` | string | `""` | Shown in `--help`. |
| `type` | string | `"string"` | `"bool"` for presence flags; `"string"` for flags that take a value. |
| `default` | string | none | Default value for string flags. |
| `validation` | string | none | Regex. Applied to string flag values only. |

```yaml
flags:
  - name: dry-run
    short: "n"
    description: Show what would happen without making changes
    type: bool
  - name: format
    description: Output format
    default: text
    validation: "^(text|json)$"
```

### `env`

Optional. List of required environment variables. Creft checks these before running and exits 3 if a required variable is missing.

| Field | Type | Default | Description |
|---|---|---|---|
| `name` | string | — | Required. The variable name, e.g. `GITHUB_TOKEN`. |
| `required` | bool | `true` | When `false`, the variable is not checked at runtime. |

```yaml
env:
  - name: GITHUB_TOKEN
  - name: DEPLOY_KEY
    required: false
```

### `tags`

Optional. List of strings. Used with `creft list --tag <tag>` to filter skills.

```yaml
tags:
  - deploy
  - aws
```

### `supports`

Optional. List of strings declaring runtime features this skill handles itself. The only current value is `"dry-run"`.

When `"dry-run"` is in `supports` and the caller passes `--dry-run`, creft sets `CREFT_DRY_RUN=1` in the environment and executes the skill normally. The skill checks `$CREFT_DRY_RUN` to detect dry-run mode.

```yaml
supports:
  - dry-run
```

```bash
if [ "$CREFT_DRY_RUN" = "1" ]; then
  echo "Would deploy — dry-run active"
  exit 0
fi
```

---

## Code Blocks

Each fenced code block in the skill body is an executable step. The language tag determines the interpreter.

### Supported Interpreters

| Language tag(s) | Interpreter | Notes |
|---|---|---|
| `bash` | `bash` | |
| `sh` | `sh` | |
| `zsh` | `zsh` | |
| `python`, `python3` | `python3` | With deps: `uv run --with` |
| `node`, `javascript`, `js` | `node` | With deps: `npm install` + `NODE_PATH` |
| `typescript`, `ts` | `npx tsx` | Requires `npx` and `tsx` |
| `perl` | `perl` | |
| `llm` | Provider CLI | See [LLM Blocks](#llm-blocks) |
| `docs` | Not executed | Shown only in `--help` output |
| Any other tag | Treated as interpreter name | Falls back to the tag as the command name |

### Multi-Block Piping

When a skill has more than one code block, creft pipes stdout of each block as stdin to the next. Blocks run concurrently via OS file descriptors — not sequentially with buffering. Block 2 starts reading from the pipe before block 1 finishes writing.

A single-block skill runs standalone with no piping.

````
---
name: summarize-log
description: Extract warnings and summarize them.
---

```bash
grep "WARN" /var/log/app.log
```

```llm
Summarize these warnings in one paragraph: {{prev}}
```
````

### Exit Codes

| Code | Meaning |
|---|---|
| `0` | Success. Continue to the next block. |
| `1`–`98` | Error. Stop the pipeline and propagate the exit code. |
| `99` | Early successful return. Stop the pipeline; creft exits 0. |
| `100+` | Error. Stop the pipeline and propagate the exit code. |

**Exit 99** is a controlled early return. Use it when a block decides no further work is needed and the caller should not see a failure. A common pattern: a guard block that exits 99 when a precondition is already satisfied, so the rest of the skill is skipped without error.

```bash
# Guard: skip the rest if already deployed
if deployed_already; then
  exit 99
fi
```

Exit 99 is an internal signal. Creft translates it to exit 0 before returning to the caller. The code never escapes to the parent shell.

---

## Template Placeholders

Placeholders in code block bodies are substituted with arg and flag values before execution.

```
{{name}}          Value of arg or flag named "name"
{{name|default}}  Value of "name", or "default" if not provided
```

Unmatched placeholders — those with no corresponding arg or flag, and no pipe-default — are left as literal text. They do not cause an error.

```bash
echo "Deploying {{branch|main}} to {{env}}"
```

---

## LLM Blocks

An `llm` block sends a prompt to an AI provider CLI. The prompt is the block's content after an optional YAML configuration header.

````
```llm
provider: claude
model: claude-haiku-4-5
params: "--max-tokens 500"
---
Summarize this in one sentence: {{prev}}
```
````

### YAML Header Fields

| Field | Default | Description |
|---|---|---|
| `provider` | `claude` | CLI tool to invoke. See [Provider Commands](#provider-commands) below. |
| `model` | (provider default) | Model name. Omitted from the command when empty. |
| `params` | `""` | Raw flags appended to the command, split on whitespace. |

The `---` line separates the YAML header from the prompt body. If the block has no YAML header, it uses the defaults.

### Provider Commands

| Provider | Command invoked |
|---|---|
| `claude` (default) | `claude -p [--model <model>]` |
| `gemini` | `gemini -p [-m <model>]` |
| `codex` | `codex exec -` |
| `ollama` | `ollama run [<model>]` |
| Any other string | `<provider> [--model <model>]` |

Each provider handles its own authentication (API keys, config files). Creft does not manage credentials.

### The Sponge Pattern

LLM blocks break the streaming pipe model. Most blocks start reading stdin before the upstream block finishes writing. LLM blocks cannot — the provider CLI requires the complete prompt before it can begin generating a response.

Creft buffers all upstream output before spawning the LLM provider. This is the sponge pattern: the LLM block absorbs everything upstream has written, then sends the full buffer to the provider.

Use `{{prev}}` in the prompt body to reference the buffered input from the previous block:

````
```bash
git diff HEAD~1
```

```llm
provider: claude
---
Write a commit message for this diff:

{{prev}}
```
````

LLM blocks in multi-block skills require Unix (the sponge mechanism uses OS file descriptors). On non-Unix systems, multi-block skills with LLM blocks are not supported.

---

## Dependencies

Declare package dependencies in the first line of a code block as a comment. Creft installs them at runtime to a temporary directory — not to the system.

### Python

Uses `uv run --with`. Requires `uv` on PATH.

```python
# deps: requests, pandas
import requests
import pandas as pd
```

Creft invokes: `uv run --with requests --with pandas -- python3 <script>`

### Node

Uses `npm install` into a temporary directory, then sets `NODE_PATH`. Requires `npm` on PATH.

```javascript
// deps: lodash, chalk
const _ = require('lodash')
const chalk = require('chalk')
```

Creft writes a stub `package.json`, runs `npm install <deps>` in a temp dir, then sets `NODE_PATH` to the resulting `node_modules` before running `node <script>`.

### Shell

Warns at runtime if listed commands are not found on PATH. No installation is attempted.

```bash
# deps: jq, yq
jq '.items[]' data.json | yq -
```

---

## Storage

Skills save to the nearest `.creft/commands/` directory found by walking up from the current directory. When no `.creft/` exists, skills save to `~/.creft/commands/`.

Use `--global` on `creft add` to always save to `~/.creft/` regardless of whether a local `.creft/` exists.

Run `creft init` in a project directory to create a local `.creft/commands/` for that project.

**Shadowing:** A local skill with the same name as a global skill takes precedence. The global skill is not deleted — it remains available when creft is run outside the project directory.

**CREFT_HOME:** Set `$CREFT_HOME` to override both local and global roots. All skills resolve to `$CREFT_HOME` when this variable is set.

**Packages:** Installed packages live under `.creft/packages/` or `~/.creft/packages/`. See `creft plugin install --help`.

---

## Validation

Creft validates skills at save time. Use `--force` to skip all checks, or `--no-validate` to skip validation but still enforce the overwrite check.

Validation runs:

- **Syntax:** `bash -n` for shell blocks, `python3 -c "import ast; ast.parse(...)"` for Python, `node --check` for Node.
- **Shellcheck:** Warnings (not errors) for shell blocks that pass syntax. Placeholders are replaced with valid tokens before shellcheck runs.
- **Command availability:** Shell blocks are scanned for `creft <name>` invocations; referenced skills are checked for existence.
- **Dependency resolution:** Python and Node packages declared in `# deps:` comments are checked against their package registries via HTTP. Shell commands are checked for presence on PATH.
- **LLM block structure:** Whether the YAML header parses and the provider field is recognized.

Errors block the save. Warnings are printed but do not block.

Run `creft doctor <skill>` to check a skill's requirements after saving, including interpreter availability, env vars, and LLM provider CLIs.

---

## Namespace Resolution

Spaces in `name` create a hierarchy: `"aws s3 sync"` becomes `creft aws s3 sync` and stores at `.creft/commands/aws/s3/sync.md`.

`creft list aws` shows all skills under the `aws` namespace. `creft list aws s3` drills into the `aws s3` sub-namespace.

When two skills share a prefix — for example `"gh"` and `"gh issue-body"` — creft uses longest-match resolution: `creft gh issue-body` invokes the `gh issue-body` skill, not the `gh` skill with `issue-body` as a positional arg.
