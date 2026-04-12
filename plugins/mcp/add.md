---
name: mcp add
description: Register an MCP server in the vault and install it into the Claude Code settings file. Secrets are prompted for via a native dialog and stored in the macOS keychain.
args:
- name: name
  description: MCP server name (used as the key in mcpServers and the vault filename)
flags:
- name: http
  description: HTTP transport URL (for remote MCPs). Mutually exclusive with --stdio.
  default: ''
- name: stdio
  description: Full command line for a stdio MCP server (e.g. "uv tool run arxiv-mcp-server --storage-path ~/papers"). Parsed with shlex — quote arguments that contain spaces. Mutually exclusive with --http.
  default: ''
- name: requires-auth
  type: bool
  description: (HTTP only) Prompt for an Authorization Bearer token and store it in keychain
- name: global
  short: g
  type: bool
  description: Install to user scope (~/.claude.json top-level mcpServers). Default is local scope (per-project personal config).
- name: claude-path
  type: string
  default: ''
  description: Install to a specific Claude settings file at top-level mcpServers. Overrides --global and the local default.
tags:
- mcp
---

```python
"""
Register an MCP server in the vault and install it.

Flow (cancel-safe order — nothing is written until the secret is collected):
1. Validate name + flags
2. Prompt for required secrets via a native tkinter password dialog
3. Store secrets in macOS keychain (service: creft.mcp, account: <name>.<key>)
4. Write the vault entry JSON at ~/.creft/mcp/vault/<name>.json
5. Read the target settings file, inject the resolved server config, atomic write

If the user cancels the dialog, nothing is written to disk or keychain.
"""
import json
import os
import re
import shlex
import subprocess
import sys
import tempfile
from pathlib import Path


NAME_PATTERN = re.compile(r"^[a-zA-Z0-9_-]+$")
KEYCHAIN_SERVICE = "creft.mcp"


def die(msg):
    print(f"error: {msg}", file=sys.stderr)
    sys.exit(1)


def prompt_secret_gui(prompt_text, title, context=""):
    """Show a native password dialog via tkinter.

    Returns the entered value, or None if the user cancelled the dialog.
    """
    try:
        import tkinter as tk
        from tkinter import ttk
        import tkinter.font as tkfont
    except ImportError:
        die("tkinter is not available in this Python environment")

    result = {"value": None, "cancelled": True}

    root = tk.Tk()
    root.title(f"{title} needs your input")
    root.minsize(520, 220)

    default_font = tkfont.nametofont("TkDefaultFont")
    heading_font = default_font.copy()
    heading_font.configure(size=default_font.cget("size") + 4, weight="bold")

    import platform
    if platform.system() == "Darwin":
        root.lift()
        root.attributes("-topmost", True)
        root.after(150, lambda: root.attributes("-topmost", False))
        root.focus_force()

    main = ttk.Frame(root, padding=24)
    main.pack(fill=tk.BOTH, expand=True)

    ttk.Label(main, text=f"{title} needs your input", font=heading_font).pack(
        anchor=tk.W, pady=(0, 14)
    )

    if context:
        ttk.Label(main, text=context, wraplength=480, justify=tk.LEFT).pack(
            anchor=tk.W, pady=(0, 12)
        )
        ttk.Separator(main, orient=tk.HORIZONTAL).pack(fill=tk.X, pady=(0, 12))

    ttk.Label(main, text=prompt_text, wraplength=480, justify=tk.LEFT).pack(
        anchor=tk.W, pady=(0, 6)
    )

    entry = ttk.Entry(main, show="*")
    entry.pack(fill=tk.X, pady=(0, 12))
    entry.focus()

    def submit():
        result["value"] = entry.get()
        result["cancelled"] = False
        root.destroy()

    def cancel():
        result["cancelled"] = True
        root.destroy()

    btns = ttk.Frame(main)
    btns.pack(fill=tk.X, pady=(8, 0))
    ttk.Button(btns, text="Cancel", command=cancel).pack(side=tk.LEFT)
    ttk.Button(btns, text="Submit", command=submit).pack(side=tk.RIGHT)

    root.bind("<Return>", lambda e: submit())
    root.bind("<Escape>", lambda e: cancel())
    root.protocol("WM_DELETE_WINDOW", cancel)

    root.mainloop()

    if result["cancelled"]:
        return None
    return result["value"]


def keychain_set(account, value):
    """Store (or update) a value in the macOS keychain."""
    result = subprocess.run(
        [
            "security", "add-generic-password",
            "-U",
            "-s", KEYCHAIN_SERVICE,
            "-a", account,
            "-w", value,
        ],
        capture_output=True,
        text=True,
    )
    return result.returncode == 0


def keychain_get(account):
    """Read a value from the macOS keychain. Returns None if not present."""
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
    """Write JSON to a file atomically, preserving the original file permissions."""
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
http_url = r'''{{http|}}'''.strip()
stdio_cmd = r'''{{stdio|}}'''.strip()
requires_auth = "{{requires-auth|}}".strip().lower() in ("true", "1", "yes")
use_global = "{{global|}}".strip().lower() in ("true", "1", "yes")
claude_path_arg = r'''{{claude-path|}}'''.strip()

if not name:
    die("name is required")
if not NAME_PATTERN.match(name):
    die(f"name must match [a-zA-Z0-9_-]+, got {name!r}")

# Transport resolution: exactly one of --http or --stdio.
if http_url and stdio_cmd:
    die("--http and --stdio are mutually exclusive")
if not http_url and not stdio_cmd:
    die("one of --http <url> or --stdio <command> is required")

transport = "http" if http_url else "stdio"

if transport == "stdio" and requires_auth:
    die(
        "--requires-auth is only valid with --http. "
        "For stdio servers that need secret env vars, set them directly in your shell or use a wrapper."
    )

# Parse the stdio command line into command + args.
stdio_command = None
stdio_args = []
if transport == "stdio":
    try:
        parts = shlex.split(stdio_cmd)
    except ValueError as e:
        die(f"--stdio value failed to parse as a shell command: {e}")
    if not parts:
        die("--stdio value is empty after parsing")
    stdio_command = parts[0]
    stdio_args = parts[1:]


# ── Target resolution ────────────────────────────────────────────
# Scopes (in precedence order):
#   --claude-path <file>  → explicit file, top-level mcpServers
#   --global              → ~/.claude.json, top-level mcpServers (user scope)
#   (default)             → ~/.claude.json, projects[<cwd>].mcpServers (local scope)
#
# The local-scope default writes per-project personal MCPs into the user's
# ~/.claude.json under a project-keyed sub-object. Secrets never leave the
# user's home directory and are never a candidate for accidental `git add`.
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

vault_dir = Path.home() / ".creft" / "mcp" / "vault"
entry_path = vault_dir / f"{name}.json"

if not settings_path.exists():
    die(f"settings file does not exist: {settings_path}")


# ── Build the vault entry (in memory, not yet written) ──────────
if transport == "http":
    metadata = {
        "name": name,
        "transport": "http",
        "url": http_url,
    }
else:
    metadata = {
        "name": name,
        "transport": "stdio",
        "command": stdio_command,
        "args": stdio_args,
    }

secrets_to_collect = []

if requires_auth:
    metadata["secrets"] = [
        {
            "key": "authorization",
            "header": "Authorization",
            "template": "Bearer {value}",
            "prompt": "Authorization token (paste just the token; 'Bearer ' prefix is added at sync time)",
        }
    ]
    secrets_to_collect.append(
        (f"{name}.authorization", metadata["secrets"][0]["prompt"])
    )


# ── Collect secrets FIRST — cancel = nothing written anywhere ────
collected = {}
for account, prompt in secrets_to_collect:
    existing = keychain_get(account)
    context = (
        f"Installing MCP server {name!r} at {http_url}.\n"
        f"Secret will be stored in macOS keychain at service "
        f"{KEYCHAIN_SERVICE!r}, account {account!r}."
    )
    if existing is not None:
        context += "\n\nA value already exists for this account. Submitting will overwrite it."
    value = prompt_secret_gui(
        prompt_text=prompt,
        title=f"creft mcp add {name}",
        context=context,
    )
    if value is None:
        die("cancelled")
    if not value:
        die(f"no value provided for {account}")
    collected[account] = value


# ── Everything below mutates state ──────────────────────────────

# Write secrets to keychain
for account, value in collected.items():
    if not keychain_set(account, value):
        die(f"failed to write {account} to keychain")
    print(f"stored {account} in keychain")

# Write the vault entry as JSON
vault_dir.mkdir(parents=True, exist_ok=True)
with open(entry_path, "w") as f:
    json.dump(metadata, f, indent=2)
    f.write("\n")
print(f"vault entry: {entry_path}")

# Build the resolved server config
if transport == "http":
    server_config = {"type": "http", "url": http_url}
    if requires_auth:
        token = keychain_get(f"{name}.authorization")
        if not token:
            die(f"{name}.authorization missing from keychain after write — aborting")
        server_config["headers"] = {"Authorization": f"Bearer {token}"}
else:
    server_config = {"type": "stdio", "command": stdio_command, "args": stdio_args}

# Update the settings file atomically
with open(settings_path) as f:
    settings = json.load(f)

if scope == "local":
    # ~/.claude.json -> projects[<project_key>] -> mcpServers
    projects = settings.setdefault("projects", {})
    project_entry = projects.setdefault(project_key, {})
    servers = project_entry.setdefault("mcpServers", {})
    servers[name] = server_config
    target_desc = f"{settings_path} → projects[{project_key!r}].mcpServers"
else:
    # Top-level mcpServers for --global and --claude-path
    servers = settings.setdefault("mcpServers", {})
    servers[name] = server_config
    target_desc = f"{settings_path} → mcpServers ({scope})"

atomic_write_json(settings_path, settings)

print(f"installed {name} [{scope} scope]")
print(f"  target: {target_desc}")
print()
print(f"verify: creft mcp ls" + ("" if scope == "local" else f" --{scope}" if scope == "global" else f" --claude-path {settings_path}"))
```
