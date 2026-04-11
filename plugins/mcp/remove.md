---
name: mcp remove
description: Remove an MCP server from a Claude settings file. Optionally purge the vault entry and keychain secrets.
args:
- name: name
  description: MCP server name to remove
flags:
- name: global
  short: g
  type: bool
  description: Remove from user scope (~/.claude.json top-level mcpServers). Default is local scope (per-project).
- name: claude-path
  type: string
  default: ''
  description: Remove from a specific Claude settings file. Overrides --global.
- name: purge
  type: bool
  description: Also delete the vault entry at ~/.creft/mcp/vault/<name>.json and any keychain entries for this MCP. Without --purge, only the settings file entry is removed.
tags:
- mcp
---

```python
"""
Remove an MCP server from a Claude settings file.

By default removes only the settings entry for the given scope, leaving
the vault entry and keychain secrets in place so you can re-install later
without re-entering anything. Use --purge to do a full teardown.

Scope resolution mirrors `creft mcp add`:
  --claude-path <file>  → that file's top-level mcpServers
  --global              → ~/.claude.json top-level mcpServers
  (default)             → ~/.claude.json projects[<cwd>].mcpServers (local)
"""
import json
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path


NAME_PATTERN = re.compile(r"^[a-zA-Z0-9_-]+$")
KEYCHAIN_SERVICE = "creft.mcp"


def die(msg):
    print(f"error: {msg}", file=sys.stderr)
    sys.exit(1)


def keychain_delete(account):
    """Delete a keychain entry. Returns True on success, False if not found."""
    result = subprocess.run(
        [
            "security", "delete-generic-password",
            "-s", KEYCHAIN_SERVICE,
            "-a", account,
        ],
        capture_output=True,
        text=True,
    )
    return result.returncode == 0


def atomic_write_json(path, data):
    """Write JSON atomically, preserving original file permissions."""
    original_mode = os.stat(path).st_mode if path.exists() else 0o600
    fd, tmp_path = tempfile.mkstemp(
        dir=str(path.parent),
        prefix=path.name + ".",
        suffix=".tmp",
    )
    try:
        with os.fdopen(fd, "w") as f:
            json.dump(data, f, indent=2)
            f.write("\n")
        os.chmod(tmp_path, original_mode)
        os.replace(tmp_path, str(path))
    except Exception:
        if os.path.exists(tmp_path):
            os.unlink(tmp_path)
        raise


# ── Parse args ────────────────────────────────────────────────────
name = "{{name}}"
use_global = "{{global|}}".strip().lower() in ("true", "1", "yes")
claude_path_arg = r'''{{claude-path|}}'''.strip()
purge = "{{purge|}}".strip().lower() in ("true", "1", "yes")

if not name:
    die("name is required")
if not NAME_PATTERN.match(name):
    die(f"name must match [a-zA-Z0-9_-]+, got {name!r}")


# ── Target resolution ────────────────────────────────────────────
project_dir = os.environ.get("CLAUDE_PROJECT_DIR") or os.getcwd()
project_key = str(Path(project_dir).resolve())

if claude_path_arg:
    settings_path = Path(claude_path_arg).expanduser()
    scope = "explicit"
elif use_global:
    settings_path = Path.home() / ".claude.json"
    scope = "global"
else:
    settings_path = Path.home() / ".claude.json"
    scope = "local"

if not settings_path.exists():
    die(f"settings file does not exist: {settings_path}")


# ── Remove from settings file ────────────────────────────────────
with open(settings_path) as f:
    settings = json.load(f)

if scope == "local":
    projects = settings.get("projects", {}) or {}
    project_entry = projects.get(project_key, {}) or {}
    servers = project_entry.get("mcpServers", {}) or {}
    target_desc = f"{settings_path} → projects[{project_key!r}].mcpServers"
else:
    servers = settings.get("mcpServers", {}) or {}
    target_desc = f"{settings_path} → mcpServers ({scope})"

if name in servers:
    del servers[name]
    # For local scope, make sure the delete lands in the actual nested dict
    if scope == "local":
        settings["projects"][project_key]["mcpServers"] = servers
    else:
        settings["mcpServers"] = servers
    atomic_write_json(settings_path, settings)
    print(f"removed {name} from {target_desc}")
    removed_from_settings = True
else:
    print(f"{name} not found in {target_desc}")
    removed_from_settings = False


# ── Purge vault + keychain if requested ──────────────────────────
if purge:
    vault_dir = Path.home() / ".creft" / "mcp" / "vault"
    entry_path = vault_dir / f"{name}.json"

    # Read the vault entry first so we know which keychain accounts to delete.
    # If the vault entry doesn't exist, we can't know — warn and skip keychain purge.
    keychain_accounts = []
    if entry_path.exists():
        try:
            with open(entry_path) as f:
                meta = json.load(f)
            for secret in meta.get("secrets") or []:
                key = secret.get("key")
                if key:
                    keychain_accounts.append(f"{name}.{key}")
        except Exception as e:
            print(f"warning: failed to read vault entry for keychain enumeration: {e}", file=sys.stderr)
    else:
        print(f"note: no vault entry at {entry_path} — skipping keychain purge", file=sys.stderr)

    # Delete keychain entries
    for account in keychain_accounts:
        if keychain_delete(account):
            print(f"deleted keychain entry {account}")
        else:
            print(f"no keychain entry for {account}")

    # Delete the vault entry
    if entry_path.exists():
        entry_path.unlink()
        print(f"deleted vault entry {entry_path}")

if not removed_from_settings and not purge:
    sys.exit(1)
```
