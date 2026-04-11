---
name: mcp sync
description: Rebuild MCP server entries in the settings file from vault templates and keychain values. Idempotent — run it after rotating a secret or installing on a new machine.
flags:
- name: global
  short: g
  type: bool
  description: Sync into user scope (~/.claude.json top-level mcpServers). Default is local scope (per-project).
- name: claude-path
  type: string
  default: ''
  description: Sync into a specific Claude settings file at top-level mcpServers. Overrides --global and the local default.
- name: only
  description: Sync only a specific vault entry by name (omit to sync all)
  default: ''
tags:
- mcp
---

```python
"""
Rebuild MCP entries in the settings file from vault + keychain.

Target resolution mirrors `creft mcp add`:
  --claude-path <file>  → that file's top-level mcpServers
  --global              → ~/.claude.json top-level mcpServers
  (default)             → ~/.claude.json projects[<cwd>].mcpServers (local)
"""
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path


KEYCHAIN_SERVICE = "creft.mcp"


def die(msg):
    print(f"error: {msg}", file=sys.stderr)
    sys.exit(1)


def keychain_get(account):
    result = subprocess.run(
        [
            "security", "find-generic-password",
            "-s", KEYCHAIN_SERVICE,
            "-a", account,
            "-w",
        ],
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        return None
    return result.stdout.strip()


def atomic_write_json(path, data):
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


def resolve_http(name, meta):
    config = {"type": "http", "url": meta["url"]}
    headers = {}
    for secret in meta.get("secrets") or []:
        key = secret.get("key")
        header_name = secret.get("header")
        template = secret.get("template", "{value}")
        if not (key and header_name):
            continue
        value = keychain_get(f"{name}.{key}")
        if value is None:
            print(f"  warning: {name}.{key} not in keychain — skipping {name}", file=sys.stderr)
            return None
        headers[header_name] = template.format(value=value)
    if headers:
        config["headers"] = headers
    return config


def resolve_stdio(name, meta):
    config = {"type": "stdio", "command": meta["command"]}
    if "args" in meta:
        config["args"] = meta["args"]
    env = dict(meta.get("env") or {})
    for secret in meta.get("secrets") or []:
        key = secret.get("key")
        env_name = secret.get("env")
        if not (key and env_name):
            continue
        value = keychain_get(f"{name}.{key}")
        if value is None:
            print(f"  warning: {name}.{key} not in keychain — skipping {name}", file=sys.stderr)
            return None
        env[env_name] = value
    if env:
        config["env"] = env
    return config


def resolve_entry(meta):
    name = meta["name"]
    transport = meta.get("transport", "http")
    if transport == "http":
        return resolve_http(name, meta)
    if transport == "stdio":
        return resolve_stdio(name, meta)
    print(f"  warning: unknown transport {transport!r} for {name}", file=sys.stderr)
    return None


# ── Parse args ────────────────────────────────────────────────────
claude_path_arg = r'''{{claude-path|}}'''.strip()
use_global = "{{global|}}".strip().lower() in ("true", "1", "yes")
only_name = r'''{{only|}}'''.strip()

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

vault_dir = Path.home() / ".creft" / "mcp" / "vault"
if not vault_dir.exists():
    print(f"no vault directory at {vault_dir}")
    sys.exit(0)


# ── Load vault entries ───────────────────────────────────────────
entries = []
for entry_file in sorted(vault_dir.glob("*.json")):
    try:
        with open(entry_file) as f:
            meta = json.load(f)
        meta.setdefault("name", entry_file.stem)
        if only_name and meta["name"] != only_name:
            continue
        entries.append(meta)
    except Exception as e:
        print(f"warning: failed to parse {entry_file}: {e}", file=sys.stderr)

if not entries:
    if only_name:
        die(f"no vault entry named {only_name!r}")
    print("no vault entries to sync")
    sys.exit(0)


# ── Load current settings ────────────────────────────────────────
with open(settings_path) as f:
    settings = json.load(f)

# Navigate to the right mcpServers dict based on scope.
if scope == "local":
    projects = settings.setdefault("projects", {})
    project_entry = projects.setdefault(project_key, {})
    servers = project_entry.setdefault("mcpServers", {})
    target_desc = f"projects[{project_key!r}].mcpServers"
else:
    servers = settings.setdefault("mcpServers", {})
    target_desc = "mcpServers"


# ── Resolve each entry and merge into settings ───────────────────
updated = 0
skipped = 0
for meta in entries:
    name = meta["name"]
    config = resolve_entry(meta)
    if config is None:
        skipped += 1
        continue
    servers[name] = config
    print(f"  synced {name}")
    updated += 1

if updated == 0:
    print(f"\nnothing to sync ({skipped} skipped)")
    sys.exit(0)


# ── Atomic write ─────────────────────────────────────────────────
atomic_write_json(settings_path, settings)

print(f"\n{updated} synced, {skipped} skipped")
print(f"target: {settings_path} → {target_desc}  [{scope} scope]")
```
