---
name: test mutants
description: Run mutation testing
tags:
  - dev
  - testing
pipe: true
args:
  - name: file
    description: Filter mutations to a specific file (e.g. error.rs, runner.rs)
    validation: "^[a-zA-Z0-9_.-]+$"
flags:
  - name: list
    short: l
    type: bool
    description: List possible mutations without running them
  - name: diff
    short: d
    type: bool
    description: Only test mutations in files changed since main
  - name: timeout
    short: t
    type: string
    description: Timeout per mutant in seconds
    default: "30"
    validation: "^[0-9]+$"
  - name: jobs
    short: j
    type: string
    description: Number of parallel jobs
    default: "4"
    validation: "^[0-9]+$"
---

```docs
Mutation testing introduces small code changes (mutants) — such as replacing `+`
with `-`, flipping a boolean, or removing a return value — then runs your test suite
against each one.

If tests catch the change and fail, the mutant is "caught" (good).
If tests still pass despite the broken code, the mutant "survives" (bad).

A surviving mutant means your tests do not verify that behavior. It points to a gap
in behavioral coverage, not just line coverage.

**Mutation score** = caught / (caught + missed). Aim for 80%+ on critical paths.

  Caught     — tests detected this change. Tests are working.
  Survived   — tests did not catch this change. A test gap exists here.
  Timeout    — the mutant caused tests to hang. May indicate an infinite loop.
  Unviable   — mutant produced code that does not compile; skipped.

Use --list first to preview mutations before running the full suite.
```

```bash
file_filter="{{file|}}"
list_only="{{list}}"
diff_mode="{{diff}}"
timeout="{{timeout}}"
jobs="{{jobs}}"

args=""

# Build file filter from args or --diff
if [ -n "$file_filter" ]; then
    args="$args -f $file_filter"
elif [ "$diff_mode" = "true" ]; then
    changed=$(git diff --name-only main -- '*.rs' 2>/dev/null | tr '\n' ' ')
    if [ -z "$changed" ]; then
        echo "No .rs files changed since main."
        echo "---CREFT_MUTANTS_MODE---empty"
        exit 0
    fi
    for f in $changed; do
        args="$args -f $(basename $f)"
    done
fi

if [ "$list_only" = "true" ]; then
    cargo mutants --list $args 2>/dev/null
    echo "---CREFT_MUTANTS_MODE---list"
else
    cargo mutants --timeout "$timeout" --jobs "$jobs" $args 2>&1
    echo "---CREFT_MUTANTS_MODE---run"
    if [ -f mutants.out/outcomes.json ]; then
        echo "---CREFT_MUTANTS_JSON---"
        cat mutants.out/outcomes.json
    fi
fi
```

```python
import sys
import json
from collections import defaultdict

raw = sys.stdin.read()

if "---CREFT_MUTANTS_MODE---empty" in raw:
    print("# Mutation Testing\n")
    print("No .rs files changed since main.")
    sys.exit(0)

if "---CREFT_MUTANTS_MODE---list" in raw:
    content = raw.split("---CREFT_MUTANTS_MODE---list")[0].strip()
    lines = [l for l in content.splitlines() if l.strip()]

    print("# Mutation Candidates\n")
    print(f"**{len(lines)} possible mutations**\n")

    by_file = defaultdict(list)
    for line in lines:
        if ":" in line:
            parts = line.split(":", 2)
            if len(parts) >= 3:
                filename = parts[0].strip().split("/")[-1]
                by_file[filename].append(line.strip())
            else:
                by_file["other"].append(line.strip())
        else:
            by_file["other"].append(line.strip())

    for filename in sorted(by_file.keys()):
        mutations = by_file[filename]
        print(f"## {filename} ({len(mutations)})\n")
        for m in mutations:
            print(f"- `{m}`")
        print()

else:
    content = raw.split("---CREFT_MUTANTS_MODE---run")[0].strip()
    json_data = None

    if "---CREFT_MUTANTS_JSON---" in raw:
        json_str = raw.split("---CREFT_MUTANTS_JSON---")[1].strip()
        try:
            json_data = json.loads(json_str)
        except json.JSONDecodeError:
            pass

    print("# Mutation Testing Results\n")

    if json_data and "outcomes" in json_data:
        outcomes = json_data["outcomes"]

        caught = []
        missed = []
        timeout_list = []
        unviable = []

        for o in outcomes:
            scenario = o.get("scenario", "")
            summary = o.get("summary", "")

            # Skip baseline
            if isinstance(scenario, str):
                continue

            mutant_data = scenario.get("Mutant")
            if not mutant_data:
                continue

            if summary == "CaughtMutant":
                caught.append(mutant_data)
            elif summary == "MissedMutant":
                missed.append(mutant_data)
            elif summary == "Timeout":
                timeout_list.append(mutant_data)
            elif summary == "Unviable":
                unviable.append(mutant_data)

        total = len(caught) + len(missed) + len(timeout_list) + len(unviable)
        denom = len(caught) + len(missed)
        score = f"{len(caught) / denom * 100:.1f}%" if denom > 0 else "N/A"

        print(f"**{total} mutants tested:** {len(caught)} caught, {len(missed)} missed, {len(timeout_list)} timeout, {len(unviable)} unviable")
        print(f"**Mutation score:** {score}\n")

        if missed:
            print("## Surviving Mutants\n")
            print("These mutations were NOT caught by tests:\n")
            for m in missed:
                filepath = m.get("file", "unknown")
                filename = filepath.split("/")[-1]
                func_info = m.get("function", {})
                func_name = func_info.get("function_name", "") if isinstance(func_info, dict) else ""
                name = m.get("name", "unknown mutation")
                span = m.get("span", {})
                start = span.get("start", {})
                line = start.get("line", 0)

                print(f"### `{filename}:{line}` — {name}\n")
                if func_name:
                    print(f"**Function:** `{func_name}`\n")

                if filepath and line > 0:
                    try:
                        with open(filepath) as f:
                            src_lines = f.readlines()
                        ctx_start = max(0, line - 3)
                        ctx_end = min(len(src_lines), line + 2)
                        print("```rust")
                        for n in range(ctx_start, ctx_end):
                            marker = ">" if n + 1 == line else " "
                            print(f"{marker}{n + 1:4d} | {src_lines[n].rstrip()}")
                        print("```\n")
                    except (FileNotFoundError, IndexError):
                        pass

        if not missed:
            print("All mutants were caught by tests.")

        # Summary stats from top-level
        print(f"\n---\n")
        print(f"Total: {json_data.get('total_mutants', 0)} | Caught: {json_data.get('caught', 0)} | Missed: {json_data.get('missed', 0)} | Timeout: {json_data.get('timeout', 0)} | Unviable: {json_data.get('unviable', 0)}")
    else:
        print("```")
        print(content)
        print("```")

    if json_data and json_data.get("missed", 0) > 0:
        sys.exit(1)
```
