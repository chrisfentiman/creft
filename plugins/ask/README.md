# ask

Ask questions across project boundaries, or ask the user directly from inside an agent.

## Two problems, one command

### Querying another project

You're working in one repo and need to understand something about another. You could grep through the foreign codebase yourself, but you'd be doing it without that project's CLAUDE.md, its custom rules, its specialized agents. You'd be applying your current context to a codebase that has its own context.

`creft ask <project> "question"` spawns Claude Code in the registered project's directory. The target project's full environment — CLAUDE.md, rules files, agents — becomes the context for answering your question. The answer comes back to stdout. Your agent reads it.

This is different from grepping a codebase or reading its source directly. When you ask a project, you get answers shaped by that project's own understanding of itself.

### Asking the user

Agents sometimes need human input: a decision, a secret, a choice between options. Without a dialog mechanism, agents have to halt and print to stdout, which gets buried in conversation history or lost in a pipeline.

`creft ask "question"` opens a native tkinter dialog on the user's screen and returns the answer on stdout. The agent continues when the user responds.

## Usage

```
# Query a registered project
creft ask <project-name> "your question"

# Ask the user (text input)
creft ask "What should I call this?"

# Ask with a choice
creft ask "Deploy to staging or production?" \
  --type choice --options "staging,production"

# Ask yes/no
creft ask "Continue with the refactor?" --type confirm

# Ask multiple questions from a JSON spec
creft ask --json '{"from": "rust-engineer", "questions": [...]}'

# List registered projects
creft ask --list
```

## Project mode

Projects are registered in `~/.creft/projects.json`. Create or edit this file directly:

```json
{
  "weft": {
    "path": "/path/to/weft"
  },
  "api": {
    "path": "/path/to/api",
    "cli": "claude",
    "agent": "rust-engineer"
  }
}
```

The `path` key is required. `cli` defaults to `claude`. `agent` is optional — when set, it passes `--agent <name>` to the Claude invocation.

Then:

```
creft ask weft "how does the router handle auth failures?"
```

This runs `claude -p --permission-mode bypassPermissions "how does..."` in the `weft` project's directory. The answer prints to stdout. Exit code 0 on success, 1 on error.

## Dialog mode

When the first argument doesn't match a registered project, `creft ask` opens a dialog.

**Question types:**

| Type | Input | Returns |
|---|---|---|
| `text` | Free text field | The typed string |
| `password` | Masked field | The typed string |
| `choice` | Radio buttons | The selected option |
| `multi` | Checkboxes | Comma-separated selections |
| `confirm` | Yes / No radio | `yes` or `no` |

**Flags for dialog mode:**

| Flag | Short | Description |
|---|---|---|
| `--type` | `-t` | Question type (default: `text`) |
| `--options` | `-o` | Comma-separated options for `choice` and `multi` |
| `--context` | `-c` | Background text shown above the question |
| `--from` | `-f` | Agent name shown in the dialog title (default: `Agent`) |
| `--json` | `-j` | Full survey spec for multi-question mode |

## Multi-question mode

Pass a JSON spec to ask multiple questions in one dialog:

```
creft ask --json '{
  "from": "rust-architect",
  "context": "Choosing the async runtime for this project.",
  "questions": [
    {"id": "runtime", "type": "choice", "prompt": "Which runtime?", "options": ["tokio", "async-std"]},
    {"id": "notes", "type": "text", "prompt": "Any constraints to keep in mind?"}
  ]
}'
```

Returns a JSON object on stdout: `{"runtime": "tokio", "notes": "must support macOS and Linux"}`.

## Exit codes

| Code | Meaning |
|---|---|
| `0` | Answer on stdout |
| `1` | User cancelled (dialog) or Claude errored (project query) |
| `2` | Configuration error (missing project, bad JSON, invalid type) |
