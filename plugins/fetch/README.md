# fetch

Pull dependency source code into your project for AI agent context.

## The problem

AI agents make mistakes when they call functions they haven't read. They guess signatures, miss error variants, assume APIs that changed two major versions ago. The fix is obvious: read the source. The problem is getting the source without burning half your context window on a `git clone` that includes test fixtures, CI configs, and five years of history you don't need.

Browsing registry documentation doesn't help either. Docs lag behind the implementation. They omit edge cases. They describe what the function is supposed to do, not what it actually does. The only reliable source of truth is the source code at the exact version you're using.

`creft fetch` resolves a package name to its repository, shallow-clones the exact release tag, strips `.git` history, and drops the result in `workbench/code/` where agents can read it directly.

## Usage

```
creft fetch <package>[@version]
creft fetch <owner>/<repo>[@tag]
creft fetch <git-url>[@tag]
```

### Examples

```
# Fetch the latest release of a crate
creft fetch serde

# Fetch a specific version
creft fetch serde@1.0.196

# Fetch by GitHub shorthand
creft fetch serde-rs/serde

# Fetch by GitHub shorthand at a tag
creft fetch serde-rs/serde@v1.0.196

# Fetch a full git URL
creft fetch https://github.com/serde-rs/serde

# Manage the cache
creft fetch --list
creft fetch --clean serde
creft fetch --clean-all
```

### Ecosystem detection

For bare package names (not GitHub shorthands or URLs), `creft fetch` auto-detects the ecosystem from project files:

| File present | Ecosystem |
|---|---|
| `package.json` | npm |
| `Cargo.toml` | crates |
| `pyproject.toml`, `requirements.txt`, or `setup.py` | pypi |

Override with `--ecosystem npm`, `--ecosystem crates`, or `--ecosystem pypi`.

## What gets cached

Source lands in `workbench/code/<name>/`. Each fetch writes a `.fetch-meta.json` alongside the source:

```json
{
  "package": "serde",
  "ecosystem": "crates",
  "repo_url": "https://github.com/serde-rs/serde",
  "version": "1.0.196",
  "tag": "v1.0.196"
}
```

The `.git` directory is removed. You get the source tree, not the history.

## Flags

| Flag | Short | Description |
|---|---|---|
| `--ecosystem` | `-e` | Force ecosystem: `npm`, `crates`, or `pypi` |
| `--list` | `-l` | Show all cached packages |
| `--clean <name>` | `-c` | Remove one cached package |
| `--clean-all` | | Remove all cached packages |

## Version pinning

Append `@version` to any spec. For registry packages, creft tries common tag patterns in order: `v1.0.0`, `1.0.0`, `serde-v1.0.0`, `serde-1.0.0`. If none match, it falls back to the default branch.

For GitHub shorthands and URLs, the version string is used directly as a branch or tag ref.

## Already cached

If the destination directory already exists, fetch skips the clone and reports the cached path. To force a re-fetch, run `--clean <name>` first.
