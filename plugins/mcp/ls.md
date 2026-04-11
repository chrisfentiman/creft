---
name: mcp ls
description: List MCP vault entries and show which are installed in the current scope. Default scope is local (per-project); use --global to show user-scope or --claude-path for a specific file.
flags:
- name: global
  short: g
  type: bool
  description: Show user-scope MCPs (~/.claude.json top-level mcpServers) instead of the per-project local scope.
- name: all
  short: a
  type: bool
  description: Show all scopes (local for current project + global) together.
- name: claude-path
  type: string
  default: ''
  description: Show MCPs from a specific Claude settings file (top-level mcpServers). Overrides --global and --all.
tags:
- mcp
---

```python
"""
List MCP vault entries and their installed/keychain state.

Scope resolution mirrors `creft mcp add`:
  --claude-path <file>  → that file's top-level mcpServers
  --global              → ~/.claude.json top-level mcpServers
  --all                 → both local (current project) and global
  (default)             → ~/.claude.json projects[<cwd>].mcpServers (local)
"""
import json
import os
import subprocess
import sys
from pathlib import Path


KEYCHAIN_SERVICE = "creft.mcp"

vault_dir = Path.home() / ".creft" / "mcp" / "vault"

claude_path_arg = r'''{{claude-path|}}'''.strip()
use_global = "{{global|}}".strip().lower() in ("true", "1", "yes")
show_all = "{{all|}}".strip().lower() in ("true", "1", "yes")

project_dir = os.environ.get("CLAUDE_PROJECT_DIR") or os.getcwd()
project_key = str(Path(project_dir).resolve())


def keychain_present(account):
    result = subprocess.run(
        [
            "security", "find-generic-password",
            "-s", KEYCHAIN_SERVICE,
            "-a", account,
        ],
        capture_output=True,
        text=True,
    )
    return result.returncode == 0


def load_installed(path, scope):
    """Return a dict of name -> server config for the given scope."""
    if not path.exists():
        return {}
    try:
        with open(path) as f:
            data = json.load(f)
    except json.JSONDecodeError as e:
        print(f"warning: failed to parse {path}: {e}", file=sys.stderr)
        return {}

    if scope == "global" or scope == "explicit":
        return data.get("mcpServers", {}) or {}
    if scope == "local":
        projects = data.get("projects", {}) or {}
        return (projects.get(project_key) or {}).get("mcpServers", {}) or {}
    return {}


# ── Resolve target(s) ────────────────────────────────────────────
targets = []  # list of (scope_name, path, installed_dict)

if claude_path_arg:
    path = Path(claude_path_arg).expanduser()
    targets.append(("explicit", path, load_installed(path, "explicit")))
elif show_all:
    home_settings = Path.home() / ".claude.json"
    targets.append(("local", home_settings, load_installed(home_settings, "local")))
    targets.append(("global", home_settings, load_installed(home_settings, "global")))
elif use_global:
    path = Path.home() / ".claude.json"
    targets.append(("global", path, load_installed(path, "global")))
else:
    path = Path.home() / ".claude.json"
    targets.append(("local", path, load_installed(path, "local")))


# ── Load vault entries ───────────────────────────────────────────
vault_entries = {}
if vault_dir.exists():
    for entry_file in sorted(vault_dir.glob("*.json")):
        try:
            with open(entry_file) as f:
                meta = json.load(f)
            name = meta.get("name", entry_file.stem)
            vault_entries[name] = meta
        except Exception as e:
            print(f"warning: failed to parse {entry_file}: {e}", file=sys.stderr)


# ── Header ───────────────────────────────────────────────────────
print(f"vault: {vault_dir}")
for scope_name, path, _ in targets:
    if scope_name == "local":
        print(f"{scope_name}: {path} → projects[{project_key!r}]")
    else:
        print(f"{scope_name}: {path}")
print()


# ── Per-scope listing ────────────────────────────────────────────
all_empty = True

for scope_name, path, installed in targets:
    all_names = sorted(set(vault_entries) | set(installed))

    if show_all:
        print(f"── {scope_name} scope ─────────────────────────")

    if not all_names:
        print(f"  (no MCP servers in {scope_name} scope)")
        print()
        continue

    all_empty = False

    for name in all_names:
        in_vault = name in vault_entries
        in_settings = name in installed

        tags = []
        if in_vault:
            tags.append("vault")
        if in_settings:
            tags.append("installed")
        status = " + ".join(tags)

        print(f"  {name}  [{status}]")

        if in_vault:
            meta = vault_entries[name]
            transport = meta.get("transport", "?")
            source = meta.get("url") or meta.get("command", "")
            print(f"    transport: {transport}")
            if source:
                print(f"    source:    {source}")

            for secret in meta.get("secrets") or []:
                key = secret.get("key", "?")
                account = f"{name}.{key}"
                marker = "OK" if keychain_present(account) else "MISSING"
                print(f"    secret:    {account}  [{marker}]")
        elif in_settings:
            cfg = installed[name]
            print(f"    transport: {cfg.get('type', '?')} (not in vault)")
            print(f"    hint:      run `creft mcp save {name}` to pull into the vault")

        print()

if all_empty and not vault_entries:
    print("No MCP servers found. Register one with `creft mcp add <name> ...`.")
```
