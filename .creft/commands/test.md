---
name: test
description: Run tests with markdown output
tags:
  - dev
  - testing
args:
  - name: filter
    description: Filter tests by name or file (e.g. runner, test_pipe)
flags:
  - name: only-failed
    short: f
    type: bool
    description: Only show failed tests
  - name: summary
    short: s
    type: bool
    description: Show summary only, no individual test results
---

```docs
## Output

Produces a markdown report with a summary line and sections for failures, passes, and ignored tests.

Failures include captured stdout (assertion messages, panic traces).

Exit code 1 when any test fails.

## Filter syntax

`filter` matches against test name substrings. Examples:
- `runner`       — all tests whose name contains "runner"
- `test_pipe`    — a single test by partial name
- `integration`  — all integration tests
```

```bash
export NEXTEST_EXPERIMENTAL_LIBTEST_JSON=1
filter="{{filter|}}"
if [ -n "$filter" ]; then
    cargo nextest run --no-fail-fast -E "test($filter)" --message-format libtest-json 2>/dev/null || true
else
    cargo nextest run --no-fail-fast --message-format libtest-json 2>/dev/null || true
fi
```

```python
import sys
import json

only_failed = "{{only-failed}}" == "true"
summary_only = "{{summary}}" == "true"

passed = []
failed = []
ignored = []

for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    try:
        event = json.loads(line)
    except json.JSONDecodeError:
        continue

    if event.get("type") == "test" and event.get("event") in ("ok", "failed", "ignored"):
        name = event.get("name", "unknown")
        ev = event["event"]
        if ev == "ok":
            passed.append(name)
        elif ev == "failed":
            stdout = event.get("stdout", "")
            failed.append({"name": name, "stdout": stdout})
        elif ev == "ignored":
            ignored.append(name)

total = len(passed) + len(failed) + len(ignored)

print("# Test Results\n")
print(f"**{total} tests:** {len(passed)} passed, {len(failed)} failed, {len(ignored)} ignored\n")

if failed:
    print("## Failures\n")
    for f in failed:
        print(f"### `{f['name']}`\n")
        if f["stdout"].strip():
            print("```")
            print(f["stdout"].rstrip())
            print("```\n")

if not summary_only and not only_failed and passed:
    print("## Passed\n")
    for name in sorted(passed):
        print(f"- `{name}`")
    print()

if not summary_only and ignored:
    print("## Ignored\n")
    for name in sorted(ignored):
        print(f"- `{name}`")
    print()

if failed:
    sys.exit(1)
```
