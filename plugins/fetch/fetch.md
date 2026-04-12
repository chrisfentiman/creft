---
name: fetch
description: Pull source code into workbench/code/ for AI agent context. Accepts package names (resolved via npm/crates/pypi), git URLs, or GitHub shorthand (owner/repo). All forms accept @version for tag/branch pinning.
args:
- name: packages
  description: One or more specs. Package name (zod), GitHub shorthand (colinhacks/zod), full git URL (https://github.com/colinhacks/zod), or any of those with @version pinning.
  default: ""
flags:
- name: ecosystem
  short: e
  description: Force ecosystem (npm, crates, pypi). Auto-detected from project files if omitted
- name: list
  short: l
  description: List cached source packages
  type: bool
- name: clean
  short: c
  type: string
  default: ""
  description: Remove a cached package by name
- name: clean-all
  type: bool
  description: Remove all cached packages
tags:
- source
- context
---

```python
import sys, os, json, subprocess
try:
    from urllib.request import urlopen, Request
    from urllib.error import URLError, HTTPError
except ImportError:
    sys.exit(1)

project_dir = os.environ.get("CLAUDE_PROJECT_DIR", os.getcwd())
code_dir = os.path.join(project_dir, "workbench", "code")

# ── List mode ─────────────────────────────────────────────────────
if "{{list|}}".strip().lower() in ("true", "1", "yes"):
    if not os.path.isdir(code_dir):
        print("No packages cached.")
        sys.exit(0)
    for entry in sorted(os.listdir(code_dir)):
        path = os.path.join(code_dir, entry)
        if os.path.isdir(path):
            meta = os.path.join(path, ".fetch-meta.json")
            if os.path.isfile(meta):
                with open(meta) as f:
                    m = json.load(f)
                ver = m.get("version", "?")
                tag = m.get("tag", "")
                label = f"@{tag}" if tag and tag != "default" else f"({ver})"
                print(f"  {entry}  {label}  [{m.get('ecosystem', '?')}]  {m.get('repo_url', '')}")
            else:
                print(f"  {entry}")
    sys.exit(0)

# ── Clean mode ────────────────────────────────────────────────────
clean_all = "{{clean-all|}}".strip().lower() in ("true", "1", "yes")
if clean_all:
    import shutil
    if os.path.isdir(code_dir):
        shutil.rmtree(code_dir)
        print("Removed all cached packages.")
    else:
        print("Nothing to clean.")
    sys.exit(0)

clean_pkg = "{{clean|}}".strip()
if clean_pkg:
    import shutil
    target = os.path.join(code_dir, clean_pkg)
    if os.path.isdir(target):
        shutil.rmtree(target)
        print(f"Removed {clean_pkg}")
    else:
        print(f"Not found: {clean_pkg}", file=sys.stderr)
        sys.exit(1)
    sys.exit(0)

# ── Detect ecosystem ─────────────────────────────────────────────
def detect_ecosystem():
    if os.path.isfile(os.path.join(project_dir, "package.json")):
        return "npm"
    if os.path.isfile(os.path.join(project_dir, "Cargo.toml")):
        return "crates"
    if os.path.isfile(os.path.join(project_dir, "pyproject.toml")):
        return "pypi"
    if os.path.isfile(os.path.join(project_dir, "requirements.txt")):
        return "pypi"
    if os.path.isfile(os.path.join(project_dir, "setup.py")):
        return "pypi"
    return None

eco_flag = "{{ecosystem|}}".strip()
ecosystem = eco_flag if eco_flag else detect_ecosystem()

packages = "{{packages}}".strip().split()
if not packages:
    print("No packages specified.", file=sys.stderr)
    sys.exit(1)

# ── Registry lookup ──────────────────────────────────────────────
def fetch_json(url):
    req = Request(url, headers={"User-Agent": "creft-fetch/1.0", "Accept": "application/json"})
    with urlopen(req, timeout=15) as resp:
        return json.loads(resp.read())

def normalize_git_url(url):
    url = url.replace("git+", "").replace("git://", "https://")
    if url.startswith("ssh://"):
        url = url.replace("ssh://", "https://")
    if url.startswith("git@github.com:"):
        url = url.replace("git@github.com:", "https://github.com/")
    if url.endswith(".git"):
        url = url[:-4]
    return url

def repo_from_npm(pkg):
    data = fetch_json(f"https://registry.npmjs.org/{pkg}")
    repo = data.get("repository", {})
    url = repo if isinstance(repo, str) else repo.get("url", "")
    version = data.get("dist-tags", {}).get("latest", "")
    return normalize_git_url(url), version

def repo_from_crates(pkg):
    data = fetch_json(f"https://crates.io/api/v1/crates/{pkg}")
    url = data.get("crate", {}).get("repository", "")
    versions = data.get("versions", [])
    version = ""
    for v in versions:
        if not v.get("yanked", False):
            version = v.get("num", "")
            break
    return url, version

def repo_from_pypi(pkg):
    data = fetch_json(f"https://pypi.org/pypi/{pkg}/json")
    info = data.get("info", {})
    version = info.get("version", "")
    urls = info.get("project_urls") or {}
    for key in ("Source", "Source Code", "Repository", "GitHub", "Code", "Homepage"):
        url = urls.get(key, "")
        if "github.com" in url or "gitlab.com" in url or "bitbucket.org" in url:
            return url, version
    hp = info.get("home_page", "")
    if "github.com" in hp or "gitlab.com" in hp:
        return hp, version
    return "", version

REGISTRY = {
    "npm": repo_from_npm,
    "crates": repo_from_crates,
    "pypi": repo_from_pypi,
}

# ── Clone ────────────────────────────────────────────────────────
def try_clone_at_tag(repo_url, tag, dest):
    """Attempt a shallow clone at a specific git tag or branch. Returns True on success."""
    result = subprocess.run(
        ["git", "clone", "--depth", "1", "--branch", tag, "--single-branch", repo_url, dest],
        capture_output=True, text=True, timeout=60
    )
    return result.returncode == 0

def is_git_spec(spec):
    """Detect whether a spec is a direct git URL or GitHub shorthand (owner/repo)."""
    if spec.startswith("git@"):
        return True
    if "://" in spec:
        return True
    # GitHub shorthand: exactly one '/', no leading '@' (scoped npm pkgs start with @)
    if not spec.startswith("@") and spec.count("/") == 1:
        return True
    return False

def parse_git_spec(spec):
    """Parse a git spec into (clone_url, cache_name, version).

    Version is None if not specified. cache_name is owner-repo format for
    collision safety. clone_url is an https URL git can clone directly.
    """
    version = None

    # SSH URL: git@host:owner/repo[.git]
    if spec.startswith("git@"):
        clone_url = spec
        # SSH URLs contain '@' in the scheme itself, so we can't rsplit for version
        # If the user wants a version with SSH, they have to clone default and checkout manually
    elif "://" in spec:
        # Full URL. Look for @version at the end, but only if the part after @
        # is a single token (no '/', no ':') — otherwise it's part of the URL.
        if "@" in spec:
            head, tail = spec.rsplit("@", 1)
            if "/" not in tail and ":" not in tail and tail:
                clone_url = head
                version = tail
            else:
                clone_url = spec
        else:
            clone_url = spec
    else:
        # GitHub shorthand: owner/repo[@version]
        if "@" in spec:
            shorthand, version = spec.rsplit("@", 1)
        else:
            shorthand = spec
        clone_url = f"https://github.com/{shorthand}"

    # Derive cache name from the clone URL path.
    # Strip scheme, ssh prefix, and .git suffix, then use owner-repo format.
    name_source = clone_url
    if name_source.startswith("git@"):
        # git@host:owner/repo.git -> owner/repo
        _, _, path = name_source.partition(":")
        name_source = path
    elif "://" in name_source:
        # https://host/owner/repo -> owner/repo
        name_source = name_source.split("://", 1)[1]
        # Drop the host
        if "/" in name_source:
            name_source = name_source.split("/", 1)[1]

    if name_source.endswith(".git"):
        name_source = name_source[:-4]

    # Use last two path segments as owner-repo if possible
    parts = [p for p in name_source.strip("/").split("/") if p]
    if len(parts) >= 2:
        cache_name = f"{parts[-2]}-{parts[-1]}"
    elif parts:
        cache_name = parts[-1]
    else:
        cache_name = "unknown"

    return clone_url, cache_name, version

def clone_git(spec, explicit_version=None):
    """Clone a git repo directly, bypassing the registry lookup path."""
    try:
        clone_url, cache_name, spec_version = parse_git_spec(spec)
    except Exception as e:
        print(f"Failed to parse git spec {spec!r}: {e}", file=sys.stderr)
        return False

    version = explicit_version or spec_version
    dest = os.path.join(code_dir, cache_name)

    if os.path.isdir(dest):
        print(f"Already cached: {cache_name} -> {dest}")
        return True

    os.makedirs(code_dir, exist_ok=True)

    cloned_ref = None
    if version:
        # For git specs, the version string is used directly as branch/tag.
        # Also try common tag patterns in case the user gave a plain version.
        ref_candidates = [version, f"v{version}"]
        for ref in ref_candidates:
            print(f"Trying {cache_name}@{ref}...")
            if try_clone_at_tag(clone_url, ref, dest):
                cloned_ref = ref
                break
            if os.path.isdir(dest):
                import shutil
                shutil.rmtree(dest)

    if not cloned_ref:
        print(f"Cloning {cache_name} from {clone_url} (default branch)...")
        result = subprocess.run(
            ["git", "clone", "--depth", "1", "--single-branch", clone_url, dest],
            capture_output=True, text=True, timeout=60
        )
        if result.returncode != 0:
            print(f"Clone failed for {cache_name}: {result.stderr.strip()}", file=sys.stderr)
            return False

    # Remove .git to save space
    git_dir = os.path.join(dest, ".git")
    if os.path.isdir(git_dir):
        import shutil
        shutil.rmtree(git_dir)

    meta = {
        "package": cache_name,
        "ecosystem": "git",
        "repo_url": clone_url,
        "version": version or "latest",
        "tag": cloned_ref or "default",
    }
    with open(os.path.join(dest, ".fetch-meta.json"), "w") as f:
        json.dump(meta, f, indent=2)
        f.write("\n")

    label = f"@{cloned_ref}" if cloned_ref else "(default branch)"
    print(f"Cached: {cache_name} {label} -> {dest}")
    return True

def clone_package(pkg, eco, explicit_version=None):
    lookup = REGISTRY.get(eco)
    if not lookup:
        print(f"Unknown ecosystem: {eco}", file=sys.stderr)
        return False

    try:
        repo_url, registry_version = lookup(pkg)
    except (HTTPError, URLError, KeyError) as e:
        print(f"Registry lookup failed for {pkg}: {e}", file=sys.stderr)
        return False

    if not repo_url:
        print(f"No repository URL found for {pkg}", file=sys.stderr)
        return False

    version = explicit_version or registry_version
    dest = os.path.join(code_dir, pkg)
    if os.path.isdir(dest):
        print(f"Already cached: {pkg} -> {dest}")
        return True

    os.makedirs(code_dir, exist_ok=True)

    cloned = False
    cloned_tag = None

    if version:
        # Tag naming varies by ecosystem — try common patterns
        tag_candidates = [
            f"v{version}",
            version,
            f"{pkg}-v{version}",
            f"{pkg}-{version}",
        ]
        for tag in tag_candidates:
            print(f"Trying {pkg}@{tag}...")
            if try_clone_at_tag(repo_url, tag, dest):
                cloned = True
                cloned_tag = tag
                break
            # Clean up failed partial clone
            if os.path.isdir(dest):
                import shutil
                shutil.rmtree(dest)

    if not cloned:
        # Fall back to default branch
        print(f"Cloning {pkg} from {repo_url} (default branch)...")
        result = subprocess.run(
            ["git", "clone", "--depth", "1", "--single-branch", repo_url, dest],
            capture_output=True, text=True, timeout=60
        )
        if result.returncode != 0:
            print(f"Clone failed for {pkg}: {result.stderr.strip()}", file=sys.stderr)
            return False

    # Remove .git to save space
    git_dir = os.path.join(dest, ".git")
    if os.path.isdir(git_dir):
        import shutil
        shutil.rmtree(git_dir)

    meta = {
        "package": pkg,
        "ecosystem": eco,
        "repo_url": repo_url,
        "version": version or "latest",
        "tag": cloned_tag or "default",
    }
    with open(os.path.join(dest, ".fetch-meta.json"), "w") as f:
        json.dump(meta, f, indent=2)
        f.write("\n")

    label = f"@{cloned_tag}" if cloned_tag else "(default branch)"
    print(f"Cached: {pkg} {label} -> {dest}")
    return True

# ── Main ─────────────────────────────────────────────────────────
failed = []
for spec in packages:
    # Route: direct git clone, or registry lookup?
    if is_git_spec(spec):
        if not clone_git(spec):
            failed.append(spec)
        continue

    # Package-name path — needs an ecosystem for registry lookup.
    if not ecosystem:
        print(
            f"Could not detect ecosystem for {spec!r}. "
            f"Use --ecosystem (npm, crates, pypi), or pass a git URL / owner/repo shorthand.",
            file=sys.stderr,
        )
        failed.append(spec)
        continue

    # Parse name@version for package specs.
    if "@" in spec and not spec.startswith("@"):
        pkg, version = spec.rsplit("@", 1)
    elif spec.startswith("@") and "@" in spec[1:]:
        # Scoped npm packages: @scope/pkg@version
        pkg, version = spec.rsplit("@", 1)
    else:
        pkg, version = spec, None
    if not clone_package(pkg, ecosystem, version):
        failed.append(pkg)

if failed:
    print(f"\nFailed: {', '.join(failed)}", file=sys.stderr)
    sys.exit(1)
```
