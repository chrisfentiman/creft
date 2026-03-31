---
name: coverage
description: Run code coverage analysis
tags:
  - dev
  - testing
pipe: true
args:
  - name: file
    description: Show detailed uncovered lines/functions for this file (e.g. error.rs)
    validation: "^[a-zA-Z0-9_.-]+$"
flags:
  - name: threshold
    short: t
    type: string
    description: Minimum acceptable line coverage percentage
    default: "85"
    validation: "^[0-9]+$"
  - name: ignore
    short: i
    type: string
    description: Comma-separated list of files to exclude (e.g. main.rs,lib.rs)
    default: "main.rs"
  - name: all
    short: a
    type: bool
    description: Include all files (disables --ignore)
  - name: diff
    short: d
    type: bool
    description: Only check files changed since main branch
  - name: only-below
    short: b
    type: bool
    description: Only show files below the threshold
---

```docs
Runs llvm-cov on the current Rust project and formats the results.

Output format:
  - Summary table: one row per file showing line% and function% coverage
  - Status column: "ok" if file meets threshold, "!!" if below
  - Total line at the bottom with project-wide percentages
  - File detail mode (with [file] arg): uncovered line ranges with source context

Examples:
  creft coverage
  creft coverage --threshold 90
  creft coverage --diff --only-below
  creft coverage error.rs
```

```bash
# Output JSON coverage data, then a separator, then changed files list
cargo llvm-cov --json 2>/dev/null
echo "---CREFT_DIFF_SEPARATOR---"
if [ "{{diff}}" = "true" ]; then
    git diff --name-only main -- '*.rs' 2>/dev/null | xargs -I{} basename {} | sort -u
fi
```

```python
import sys
import json

raw = sys.stdin.read()
parts = raw.split("---CREFT_DIFF_SEPARATOR---")
json_data = parts[0].strip()
diff_files_raw = parts[1].strip() if len(parts) > 1 else ""

threshold = float("{{threshold}}")
target_file = "{{file|}}"
diff_mode = "{{diff}}" == "true"
only_below = "{{only-below}}" == "true"
show_all = "{{all}}" == "true"

data = json.loads(json_data)

IGNORE = set() if show_all else {f.strip() for f in "{{ignore}}".split(",") if f.strip()}
DIFF_FILES = {f.strip() for f in diff_files_raw.splitlines() if f.strip()} if diff_mode else set()

all_files = data.get("data", [{}])[0].get("files", [])

def find_file(name):
    for file_data in all_files:
        if file_data["filename"].split("/")[-1] == name:
            return file_data
    return None

def get_uncovered_ranges(file_data):
    segments = file_data.get("segments", [])
    uncovered = []
    i = 0
    while i < len(segments):
        seg = segments[i]
        line, col, count, has_count, is_region, is_gap = seg[0], seg[1], seg[2], seg[3], seg[4], seg[5]
        if has_count and is_region and count == 0 and not is_gap:
            start = line
            j = i + 1
            end = start
            while j < len(segments):
                next_seg = segments[j]
                if next_seg[3] and next_seg[4] and not next_seg[5]:
                    end = next_seg[0] - 1 if next_seg[0] > start else start
                    break
                j += 1
            else:
                end = start
            uncovered.append((start, end))
            i = j
        else:
            i += 1
    # Merge overlapping/adjacent
    merged = []
    for start, end in sorted(uncovered):
        if merged and start <= merged[-1][1] + 1:
            merged[-1] = (merged[-1][0], max(merged[-1][1], end))
        else:
            merged.append((start, end))
    return merged

def print_detail(file_data, name):
    summary = file_data.get("summary", {})
    lines = summary.get("lines", {})
    functions = summary.get("functions", {})

    print(f"# Coverage: {name}\n")
    print(f"- **Lines:** {lines.get('percent', 0):.1f}% ({lines.get('covered', 0)}/{lines.get('count', 0)})")
    print(f"- **Functions:** {functions.get('percent', 0):.1f}% ({functions.get('covered', 0)}/{functions.get('count', 0)})")

    merged = get_uncovered_ranges(file_data)

    # Read source file
    src_path = file_data["filename"]
    src_lines = []
    try:
        with open(src_path) as f:
            src_lines = f.readlines()
    except FileNotFoundError:
        pass

    if merged and src_lines:
        print(f"\n## Uncovered Lines\n")
        for start, end in merged:
            ctx_start = max(1, start - 2)
            ctx_end = min(len(src_lines), end + 2)
            uncovered_set = set(range(start, end + 1))

            print(f"**Lines {start}-{end}:**\n")
            print("```rust")
            for n in range(ctx_start, ctx_end + 1):
                line = src_lines[n - 1].rstrip()
                if n in uncovered_set:
                    print(f">{n:4d} | {line}")
                else:
                    print(f" {n:4d} | {line}")
            print("```\n")
    elif merged:
        print(f"\n## Uncovered Lines\n")
        for start, end in merged:
            print(f"- Lines {start}-{end}")
    else:
        print(f"\nAll lines covered.")


if target_file:
    found = find_file(target_file)
    if not found:
        print(f"File not found: {target_file}")
        sys.exit(1)
    print_detail(found, target_file)

else:
    # Summary mode
    files = []
    for file_data in all_files:
        name = file_data["filename"].split("/")[-1]
        if name in IGNORE:
            continue
        if diff_mode and name not in DIFF_FILES:
            continue
        summary = file_data.get("summary", {})
        lines = summary.get("lines", {})
        functions = summary.get("functions", {})

        entry = {
            "name": name,
            "line_pct": lines.get("percent", 0),
            "line_count": lines.get("count", 0),
            "line_covered": lines.get("covered", 0),
            "func_pct": functions.get("percent", 0),
            "func_count": functions.get("count", 0),
            "func_covered": functions.get("covered", 0),
            "file_data": file_data,
        }

        if only_below and entry["line_pct"] >= threshold:
            continue

        files.append(entry)

    files.sort(key=lambda x: x["line_pct"])

    title = "# Coverage Report"
    if diff_mode:
        title += " (changed files only)"
    if only_below:
        title += f" (below {threshold}%)"
    print(f"{title}\n")

    if not files:
        if diff_mode:
            print("No changed files found (or all meet threshold).")
        elif only_below:
            print(f"All files meet {threshold}% threshold.")
        sys.exit(0)

    # Pre-compute column widths for aligned table
    name_w = max(len("File"), max(len(f["name"]) for f in files))
    lines_vals = [f'{f["line_pct"]:.1f}% ({f["line_covered"]}/{f["line_count"]})' for f in files]
    funcs_vals = [f'{f["func_pct"]:.1f}% ({f["func_covered"]}/{f["func_count"]})' for f in files]
    lines_w = max(len("Lines"), max(len(v) for v in lines_vals))
    funcs_w = max(len("Functions"), max(len(v) for v in funcs_vals))

    print(f'| {"St":2s} | {"File":<{name_w}s} | {"Lines":<{lines_w}s} | {"Functions":<{funcs_w}s} |')
    print(f'|{"-" * 4}|{"-" * (name_w + 2)}|{"-" * (lines_w + 2)}|{"-" * (funcs_w + 2)}|')

    any_below = False
    for f, lv, fv in zip(files, lines_vals, funcs_vals):
        status = "ok" if f["line_pct"] >= threshold else "!!"
        if f["line_pct"] < threshold:
            any_below = True
        print(f'| {status} | {f["name"]:<{name_w}s} | {lv:<{lines_w}s} | {fv:<{funcs_w}s} |')

    total = data.get("data", [{}])[0].get("totals", {})
    tl = total.get("lines", {})
    tf = total.get("functions", {})
    print(f'\n**Total:** lines {tl.get("percent", 0):.1f}% ({tl.get("covered", 0)}/{tl.get("count", 0)}) | funcs {tf.get("percent", 0):.1f}% ({tf.get("covered", 0)}/{tf.get("count", 0)})')

    # In only-below mode, show uncovered lines detail for each file
    if only_below and files:
        print("")
        for f in files:
            print("---\n")
            print_detail(f["file_data"], f["name"])

    if any_below:
        print(f"\nFiles below {threshold}% threshold found.")
        sys.exit(1)
    else:
        print(f"\nAll files meet {threshold}% threshold.")
```
