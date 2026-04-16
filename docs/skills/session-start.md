---
name: _creft session start
description: Output creft context for session initialization. Used by hook-based harness integrations.
---

````bash
# Output creft context for the current session.
# Harnesses that support hooks call this at session start
# and inject the output into the agent's context.

cat <<'CREFT_CONTEXT'
# creft -- Executable Skills for AI Agents

creft saves reusable commands as markdown files and runs them as CLI
subcommands. Skills persist between sessions -- create once, use from any
directory, on any machine. LLM blocks call AI providers (claude, gemini,
codex, ollama) as pipeline steps.

## When to create a skill

  Run once, never again:              run inline, don't make a skill
  Run again, or share with the team:  make it a skill
  Pipe output to an LLM:              use an LLM block in the skill

## Discovering skills

  creft list                      Show all skills, grouped by namespace
  creft list <namespace>          Drill into a namespace
  creft <skill> --help            See what a skill does and what it accepts
  creft show <skill>              Read the full skill definition

## Running skills

  creft <name> [args...] [--flags...]

  --dry-run     Preview rendered blocks without executing
  --verbose     Print rendered blocks to stderr before execution

## Creating skills

  creft add <<'EOF'
  ---
  name: deploy
  description: Deploys the app to staging or production.
  args:
    - name: env
  ---

  ```bash
  echo "Deploying to {{env}}..."
  ```

  ```llm
  Confirm deployment to {{env}} succeeded.
  ```
  EOF

Run `creft add --help` for the complete format reference.

## Managing skills

  creft show <name>               View full definition
  creft show --blocks <name>      View code blocks only
  creft remove <name>             Remove a skill
  creft add --force <<'EOF'       Update an existing skill

## Plugins

  creft plugin install <git-url>            Install a plugin
  creft plugin activate <plugin>/<cmd>      Activate a command
  creft plugin list                         List installed plugins

## Skill storage

  Local:   .creft/ in the project directory (travels with the repo)
  Global:  ~/.creft/ (available everywhere)

Local skills shadow global ones with the same name.
CREFT_CONTEXT
````
