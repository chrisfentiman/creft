# mcp

Manage MCP server configurations without editing JSON.

## The problem

MCP server configurations live in `~/.claude.json` as raw JSON. Adding a server means finding the right key, constructing the config object, pasting it in, and hoping you didn't break the surrounding structure. Tokens and API keys go straight into the file, sitting next to everything else, one accidental `git add` away from a credential leak.

Sharing a setup between machines means copying JSON around. Rotating a token means finding every place it appears and updating it. There's no audit trail of what's installed where, no way to verify that a server's credentials are still valid, and no way to reinstall from scratch after a machine wipe without re-entering everything from memory.

The `mcp` plugin solves this with a vault. Server configurations are stored as templates in `~/.creft/mcp/vault/`. Secrets are stored in the macOS keychain. The settings file is assembled from the vault at install or sync time. Rotating a secret means updating the keychain and running `creft mcp sync`.

## Commands

### `creft mcp add <name>`

Register an MCP server and install it into a Claude settings file.

```
# HTTP server with authentication
creft mcp add arxiv \
  --http https://mcp.arxiv.org \
  --requires-auth

# stdio server
creft mcp add sqlite \
  --stdio "uvx mcp-server-sqlite --db ~/data.db"

# Install globally (user scope) instead of per-project
creft mcp add arxiv --http https://mcp.arxiv.org --global
```

When `--requires-auth` is set, a native password dialog appears before anything is written to disk. If the user cancels, nothing changes. On success, the token goes to the macOS keychain at service `creft.mcp`, account `<name>.authorization`.

**Flags:**

| Flag | Description |
|---|---|
| `--http <url>` | HTTP transport URL |
| `--stdio <cmd>` | Full command line for a stdio server, parsed with shlex |
| `--requires-auth` | (HTTP only) Prompt for a Bearer token; store in keychain |
| `--global`, `-g` | Install to `~/.claude.json` top-level `mcpServers` |
| `--claude-path <file>` | Install to a specific settings file |

Default scope (no flags) installs to `~/.claude.json` under `projects[<cwd>].mcpServers`, keeping per-project configs isolated in your home directory where they can't be accidentally committed.

### `creft mcp ls`

List vault entries and show installation status.

```
creft mcp ls           # local scope (current project)
creft mcp ls --global  # user scope
creft mcp ls --all     # both scopes
```

Output shows each server's transport, source URL or command, and whether its keychain secrets are present:

```
vault: /Users/you/.creft/mcp/vault
local: /Users/you/.claude.json → projects['/your/project']

  arxiv  [vault + installed]
    transport: http
    source:    https://mcp.arxiv.org
    secret:    arxiv.authorization  [OK]
```

### `creft mcp remove <name>`

Remove a server from the active settings file.

```
creft mcp remove arxiv           # remove from current scope
creft mcp remove arxiv --purge   # also delete vault entry and keychain secrets
```

Without `--purge`, the vault entry stays intact. Run `creft mcp sync` to reinstall later without re-entering credentials.

### `creft mcp sync`

Rebuild settings file entries from vault templates and keychain values. Idempotent.

```
creft mcp sync              # sync all vault entries to local scope
creft mcp sync --global     # sync to user scope
creft mcp sync --only arxiv # sync one server
```

Run this after rotating a secret (update the keychain entry directly, then sync) or after setting up a new machine (the vault entries travel with your dotfiles; the keychain entry is re-entered once).

## Scope model

Three scopes, in precedence order:

| Scope | Flag | Location in `~/.claude.json` |
|---|---|---|
| local (default) | none | `projects[<cwd>].mcpServers` |
| global | `--global` | top-level `mcpServers` |
| explicit | `--claude-path <file>` | top-level `mcpServers` in that file |

Local scope keeps project-specific MCP configs separate. Servers you install for one project don't appear in another.

## Secret storage

Secrets never appear in vault files. The vault stores only the template — which header to set, which keychain account to read, how to format the value. When `sync` runs, it reads from the keychain and assembles the final config.

Keychain entries use:
- Service: `creft.mcp`
- Account: `<server-name>.<key>` (e.g., `arxiv.authorization`)
