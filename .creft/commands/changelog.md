---
name: changelog
description: Generate changelog from git history
tags:
  - dev
  - release
args:
  - name: range
    description: Git revision range (e.g. v0.1.0..HEAD, or a number like 20 for last N commits)
    default: "20"
    validation: "^[a-zA-Z0-9_.^~/-]+$"
flags:
  - name: all
    short: a
    type: bool
    description: Show all commits since last tag (ignores range arg)
---

```docs
Generates a changelog from git history by grouping commits into conventional
commit type sections.

Output format:
  Commits following the Conventional Commits spec (type(scope): desc) are
  grouped under labelled sections: Features, Bug Fixes, Refactoring, etc.
  Commits that do not match the pattern appear under an "Other" section.
  Each entry includes the short SHA for traceability.

Range syntax:
  20              Last 20 commits (default)
  v0.1.0..HEAD    All commits since tag v0.1.0
  main..HEAD      All commits ahead of main branch
  abc123..def456  Commits between two SHAs

Examples:
  creft changelog              Last 20 commits
  creft changelog 50           Last 50 commits
  creft changelog v0.2.0..HEAD Since the v0.2.0 tag
  creft changelog --all        Since last tag (full release notes)
```

```bash
range="{{range}}"
show_all="{{all}}"

if [ "$show_all" = "true" ]; then
    last_tag=$(git describe --tags --abbrev=0 2>/dev/null || echo "")
    if [ -n "$last_tag" ]; then
        git log "${last_tag}..HEAD" --pretty=format:'%H|%s|%an|%ai' --no-merges
    else
        git log --pretty=format:'%H|%s|%an|%ai' --no-merges
    fi
elif echo "$range" | grep -qE '^[0-9]+$'; then
    git log -n "$range" --pretty=format:'%H|%s|%an|%ai' --no-merges
else
    git log "${range}" --pretty=format:'%H|%s|%an|%ai' --no-merges
fi
```

```python
import sys
import re
from collections import defaultdict

lines = sys.stdin.read().strip().splitlines()
if not lines or not lines[0]:
    print("# Changelog\n\nNo commits found.")
    sys.exit(0)

TYPE_LABELS = {
    "feat": "Features",
    "fix": "Bug Fixes",
    "docs": "Documentation",
    "style": "Style",
    "refactor": "Refactoring",
    "perf": "Performance",
    "test": "Tests",
    "build": "Build",
    "ci": "CI/CD",
    "chore": "Chores",
    "revert": "Reverts",
}

pattern = re.compile(r'^(\w+)(?:\(([^)]+)\))?\s*:\s*(.+)$')

groups = defaultdict(list)
other = []

for line in lines:
    parts = line.split("|", 3)
    if len(parts) < 4:
        continue
    sha, subject, author, date = parts
    sha_short = sha[:7]
    date_short = date[:10]

    m = pattern.match(subject)
    if m:
        ctype, scope, desc = m.groups()
        scope_str = f"**{scope}:** " if scope else ""
        entry = f"- {scope_str}{desc} (`{sha_short}`)"
        groups[ctype].append(entry)
    else:
        other.append(f"- {subject} (`{sha_short}`)")

print("# Changelog\n")

# Print in conventional order
for ctype in ["feat", "fix", "refactor", "perf", "test", "docs", "build", "ci", "style", "chore", "revert"]:
    if ctype in groups:
        label = TYPE_LABELS.get(ctype, ctype.title())
        print(f"## {label}\n")
        for entry in groups[ctype]:
            print(entry)
        print()
        del groups[ctype]

# Any remaining non-standard types
for ctype, entries in sorted(groups.items()):
    print(f"## {ctype.title()}\n")
    for entry in entries:
        print(entry)
    print()

if other:
    print("## Other\n")
    for entry in other:
        print(entry)
    print()
```
