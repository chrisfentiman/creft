# fetch

Pull dependency source code into your project for AI agent context.

## The problem

Web search creates a bad game of telephone. The Vercel team discovered this while building v0: a smaller model summarizes search results, the main model acts on those summaries, and hallucinations compound at each step. Their answer was [opensrc](https://github.com/vercel-labs/opensrc) — inject actual source files directly into agents' read-only filesystems. When v0 needed to use the AI SDK, it searched hand-curated directories with real code patterns. Web search went away entirely.

`creft fetch` extends that idea beyond npm. Agents make mistakes when they call functions they haven't read. They guess signatures, miss error variants, assume APIs that changed two major versions ago. The fix is obvious: read the source. The problem is getting the source at the exact version you're using, without history and test fixtures burning context.

`creft fetch` resolves a package name to its repository, shallow-clones the exact release tag, strips `.git` history, and drops the source in `workbench/code/` where agents can read it directly.

## How it differs from Context7 and DeepWiki

Context7 (Upstash) injects live documentation — the right solution for "which version of the API am I using?" but it gives you docs, not source. DeepWiki (Cognition) generates wiki documentation with architecture diagrams — the right solution for "how is this repo structured?" but it's AI-generated understanding, not the actual code.

`creft fetch` gives you the source at the exact version tag. These are complementary layers, not competitors: Context7 for correct API usage, `creft fetch` for implementation details, DeepWiki for architectural understanding.

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

For GitHub shorthands and URLs, creft tries the version string as a tag or branch, then tries it with a `v` prefix. If neither matches, it falls back to the default branch.

## Already cached

If the destination directory already exists, fetch skips the clone and reports the cached path. To force a re-fetch, run `--clean <name>` first.
