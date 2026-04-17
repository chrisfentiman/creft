---
name: _creft session start
description: Output creft context for session initialization. Used by hook-based harness integrations.
---

````bash
# Output creft context for the current session.
# Harnesses that support hooks call this at session start
# and inject the output into the agent's context.

# Static section: what creft is and when to reach for it.
cat <<'CREFT_CONTEXT'
# creft

CLI that runs markdown-defined commands as subcommands. Commands persist
between sessions -- create once, use everywhere.

## When to use creft

  Reusable workflow       creft <name> [args] [--flags]
  Check what exists       creft list
  Drill into namespace    creft list <namespace>
  Understand a command    creft <name> --help
  See full definition     creft show <name>
  Save a new command      creft add --help (for format reference)

## Decision triggers

  Want to run a project task?     Check `creft list` first -- it may exist
  Repeating a shell recipe?       Save it: `creft add <<'EOF' ... EOF`
  Need a command's syntax?        Run `creft <name> --help`, not memory
CREFT_CONTEXT

# Dynamic listing: show what commands are actually available.
if command -v creft >/dev/null 2>&1; then
  echo ""
  echo "## Installed Commands"
  echo ""
  echo "The following commands are available in this environment."
  echo "Run 'creft <command> --help' for usage details."
  echo ""
  # Strip ANSI escapes so the output is clean in agent context.
  creft list 2>/dev/null | sed 's/\x1b\[[0-9;]*m//g'
fi
````
