---
name: lint
description: Run clippy with markdown output
tags:
  - dev
  - quality
pipe: true
args:
  - name: file
    description: Filter findings to a specific file (e.g. runner.rs)
    validation: "^[a-zA-Z0-9_.-]+$"
flags:
  - name: fix
    short: f
    type: bool
    description: Auto-fix what clippy can (runs cargo clippy --fix)
  - name: pedantic
    short: p
    type: bool
    description: Include pedantic lints
---

```docs
## What clippy checks

Runs `cargo clippy` across the whole workspace. By default enforces `-D warnings` (all warnings are errors). With `--pedantic` also enables `clippy::pedantic` for stricter style checks.

## Output format

Produces a markdown Lint Report with a summary count and one section per finding. Each finding shows:
- File path and line number
- Lint code (e.g. `clippy::needless_pass_by_value`)
- The offending code snippet
- A suggested fix when clippy can provide one

Exit code 1 when any errors are found.
```

```bash
fix="{{fix}}"
pedantic="{{pedantic}}"

if [ "$fix" = "true" ]; then
    cargo clippy --fix --allow-dirty --allow-staged -- -D warnings 2>&1
    echo '{"__creft_fix_mode": true}'
else
    args="-D warnings"
    if [ "$pedantic" = "true" ]; then
        args="$args -W clippy::pedantic"
    fi
    cargo clippy --message-format=json -- $args 2>/dev/null || true
fi
```

```python
import sys
import json

target_file = "{{file|}}"

lines = sys.stdin.read().strip().splitlines()

# Check for fix mode
if lines and lines[-1].strip().startswith('{"__creft_fix_mode"'):
    print("# Lint Fix\n")
    output = "\n".join(lines[:-1]).strip()
    if output:
        print("```")
        print(output)
        print("```")
    else:
        print("No output — all fixes applied.")
    sys.exit(0)

findings = []
for line in lines:
    try:
        msg = json.loads(line)
    except json.JSONDecodeError:
        continue

    if msg.get("reason") != "compiler-message":
        continue

    message = msg.get("message", {})
    level = message.get("level", "")
    if level not in ("warning", "error"):
        continue

    text = message.get("message", "")
    code = message.get("code", {})
    code_id = code.get("code", "") if code else ""

    # Get primary span
    spans = message.get("spans", [])
    primary = None
    for span in spans:
        if span.get("is_primary"):
            primary = span
            break
    if not primary and spans:
        primary = spans[0]

    if not primary:
        continue

    filename = primary.get("file_name", "")
    line_start = primary.get("line_start", 0)
    line_end = primary.get("line_end", 0)
    snippet = primary.get("text", [])

    # Filter by file if specified
    if target_file and target_file not in filename:
        continue

    finding = {
        "level": level,
        "code": code_id,
        "message": text,
        "file": filename,
        "line_start": line_start,
        "line_end": line_end,
        "snippet": snippet,
        "suggestion": "",
    }

    # Extract suggested fix
    children = message.get("children", [])
    for child in children:
        if child.get("level") == "help" and child.get("spans"):
            for cspan in child["spans"]:
                if cspan.get("suggested_replacement") is not None:
                    finding["suggestion"] = cspan["suggested_replacement"]
                    break

    findings.append(finding)

print("# Lint Report\n")

if not findings:
    print("No warnings or errors found.")
    sys.exit(0)

errors = [f for f in findings if f["level"] == "error"]
warnings = [f for f in findings if f["level"] == "warning"]

print(f"**{len(findings)} findings:** {len(errors)} errors, {len(warnings)} warnings\n")

for f in findings:
    icon = "E" if f["level"] == "error" else "W"
    code_suffix = f" ({f['code']})" if f["code"] else ""
    print(f"## [{icon}] {f['file']}:{f['line_start']}{code_suffix}\n")
    print(f"**{f['message']}**\n")

    if f["snippet"]:
        print("```rust")
        for s in f["snippet"]:
            text = s.get("text", "")
            print(f"{f['line_start']:4d} | {text}")
        print("```\n")

    if f["suggestion"]:
        print(f"**Fix:** `{f['suggestion']}`\n")

if errors:
    sys.exit(1)
```
