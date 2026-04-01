---
name: bench
description: Measure compile time, test time, binary size
tags:
  - dev
  - performance
flags:
  - name: compare
    short: c
    type: string
    description: Compare against a git ref (e.g. main, v0.1.0). Shows deltas.
    validation: "^[a-zA-Z0-9_./-]+$"
---

```docs
Measures six metrics for the current workspace and optionally compares them
against another git ref.

Metrics:
  build_time   Release build time in seconds (cargo build --release after clean)
  test_time    Test suite wall time in seconds (cargo test)
  test_count   Number of passing tests
  binary_size  Size of the release binary in MB and bytes
  dep_count    Direct dependency count (cargo tree --depth 0)
  loc          Lines of Rust source code under src/

Comparison mode (--compare):
  Stashes any uncommitted changes, checks out the ref, measures there, then
  restores the original branch. Outputs a three-column table with Current,
  Compare, and Delta columns. Numeric deltas show + or - relative to the
  compare ref (positive means current is larger/slower).

Examples:
  creft bench                   Snapshot of current state
  creft bench -c main           Compare HEAD against main
  creft bench -c v0.1.0         Compare against a version tag
```

```bash
compare_ref="{{compare|}}"

echo "---CREFT_BENCH_START---"

# Current measurements
echo "## current"

# Clean build time
cargo clean 2>/dev/null
start=$(date +%s%3N)
cargo build --release 2>/dev/null
end=$(date +%s%3N)
echo "build_time|$(( (end - start) ))ms"

# Test time
start=$(date +%s%3N)
cargo test 2>/dev/null
end=$(date +%s%3N)
echo "test_time|$(( (end - start) ))ms"

# Test count
test_count=$(cargo test 2>&1 | grep "^test result:" | grep -oE '[0-9]+ passed' | grep -oE '[0-9]+')
echo "test_count|${test_count:-0}"

# Binary size
binary=$(find target/release -maxdepth 1 -name "creft" -type f 2>/dev/null)
if [ -n "$binary" ]; then
    size_bytes=$(wc -c < "$binary" | tr -d ' ')
    echo "binary_size_bytes|${size_bytes}"
else
    echo "binary_size_bytes|0"
fi

# Dependency count
dep_count=$(cargo tree --depth 0 -e normal 2>/dev/null | wc -l | tr -d ' ')
echo "dep_count|$dep_count"

# Lines of code (src only)
loc=$(find src -name '*.rs' -exec cat {} + 2>/dev/null | wc -l | tr -d ' ')
echo "loc|$loc"

# Compare ref measurements (if requested)
if [ -n "$compare_ref" ]; then
    echo "## compare"

    current_branch=$(git branch --show-current)
    current_sha=$(git rev-parse HEAD)

    git stash -q 2>/dev/null
    git checkout "$compare_ref" -q 2>/dev/null

    cargo clean 2>/dev/null
    start=$(date +%s%3N)
    cargo build --release 2>/dev/null
    end=$(date +%s%3N)
    echo "build_time|$(( (end - start) ))ms"

    start=$(date +%s%3N)
    cargo test 2>/dev/null
    end=$(date +%s%3N)
    echo "test_time|$(( (end - start) ))ms"

    test_count=$(cargo test 2>&1 | grep "^test result:" | grep -oE '[0-9]+ passed' | grep -oE '[0-9]+')
    echo "test_count|${test_count:-0}"

    binary=$(find target/release -maxdepth 1 -name "creft" -type f 2>/dev/null)
    if [ -n "$binary" ]; then
        size_bytes=$(wc -c < "$binary" | tr -d ' ')
        echo "binary_size_bytes|${size_bytes}"
    else
        echo "binary_size_bytes|0"
    fi

    dep_count=$(cargo tree --depth 0 -e normal 2>/dev/null | wc -l | tr -d ' ')
    echo "dep_count|$dep_count"

    loc=$(find src -name '*.rs' -exec cat {} + 2>/dev/null | wc -l | tr -d ' ')
    echo "loc|$loc"

    git checkout "$current_branch" -q 2>/dev/null || git checkout "$current_sha" -q 2>/dev/null
    git stash pop -q 2>/dev/null
fi
```

```python
import sys

raw = sys.stdin.read()
start = raw.find("---CREFT_BENCH_START---")
if start >= 0:
    raw = raw[start + len("---CREFT_BENCH_START---"):]

sections = {}
current_section = None
for line in raw.strip().splitlines():
    line = line.strip()
    if line.startswith("## "):
        current_section = line[3:]
        sections[current_section] = {}
    elif "|" in line and current_section:
        key, val = line.split("|", 1)
        sections[current_section][key] = val

LABELS = {
    "build_time": "Build time (release)",
    "test_time": "Test time",
    "test_count": "Tests",
    "binary_size": "Binary size",
    "dep_count": "Dependencies",
    "loc": "Lines of code (src/)",
}

UNITS = {
    "build_time": "",
    "test_time": "",
}

def format_size(raw):
    """Convert raw bytes string to human-readable MB display."""
    try:
        b = int(raw)
        if b == 0:
            return "not found"
        mb = b / 1048576
        return f"{mb:.1f}MB ({b} bytes)"
    except (ValueError, TypeError):
        return raw

def parse_ms(raw):
    """Parse millisecond string (e.g. '1234ms') to float seconds."""
    raw = raw.strip()
    if raw.endswith("ms"):
        return float(raw[:-2]) / 1000.0
    return float(raw)

def format_time(raw):
    """Format millisecond timing to seconds display."""
    try:
        secs = parse_ms(raw)
        return f"{secs:.1f}s"
    except (ValueError, TypeError):
        return raw

def get_display(key, val):
    """Convert raw measurement value to display string."""
    if key in ("build_time", "test_time"):
        return format_time(val)
    if key == "binary_size_bytes":
        return format_size(val)
    return val

def get_numeric(key, val):
    """Extract numeric value for delta calculation."""
    if key in ("build_time", "test_time"):
        try:
            return parse_ms(val)
        except (ValueError, TypeError):
            return None
    if key == "binary_size_bytes":
        try:
            b = int(val)
            return b / 1048576  # compare in MB
        except (ValueError, TypeError):
            return None
    try:
        return float(val)
    except (ValueError, TypeError):
        return None

print("# Benchmark Report\n")

current = sections.get("current", {})
compare = sections.get("compare", {})

# Normalise keys: bash block emits binary_size_bytes; display as binary_size
DISPLAY_KEYS = ["build_time", "test_time", "test_count", "binary_size_bytes", "dep_count", "loc"]
DISPLAY_LABELS = {
    "build_time": "Build time (release)",
    "test_time": "Test time",
    "test_count": "Tests",
    "binary_size_bytes": "Binary size",
    "dep_count": "Dependencies",
    "loc": "Lines of code (src/)",
}

if compare:
    name_w = max(len(v) for v in DISPLAY_LABELS.values())
    print(f"| {'Metric':<{name_w}} | Current | Compare | Delta |")
    print(f"|{'-' * (name_w + 2)}|---------|---------|-------|")

    for key in DISPLAY_KEYS:
        label = DISPLAY_LABELS.get(key, key)
        cur_raw = current.get(key, "—")
        cmp_raw = compare.get(key, "—")

        cur_display = get_display(key, cur_raw) if cur_raw != "—" else "—"
        cmp_display = get_display(key, cmp_raw) if cmp_raw != "—" else "—"

        delta = "—"
        if cur_raw != "—" and cmp_raw != "—":
            cur_num = get_numeric(key, cur_raw)
            cmp_num = get_numeric(key, cmp_raw)
            if cur_num is not None and cmp_num is not None:
                diff = cur_num - cmp_num
                if key in ("build_time", "test_time"):
                    delta = f"+{diff:.1f}s" if diff > 0 else (f"{diff:.1f}s" if diff < 0 else "—")
                elif key == "binary_size_bytes":
                    delta = f"+{diff:.1f}MB" if diff > 0 else (f"{diff:.1f}MB" if diff < 0 else "—")
                else:
                    delta = f"+{diff:.0f}" if diff > 0 else (f"{diff:.0f}" if diff < 0 else "—")

        print(f"| {label:<{name_w}} | {cur_display:<7} | {cmp_display:<7} | {delta} |")
else:
    name_w = max(len(v) for v in DISPLAY_LABELS.values())
    print(f"| {'Metric':<{name_w}} | Value |")
    print(f"|{'-' * (name_w + 2)}|-------|")

    for key in DISPLAY_KEYS:
        label = DISPLAY_LABELS.get(key, key)
        val = current.get(key, "—")
        display = get_display(key, val) if val != "—" else "—"
        print(f"| {label:<{name_w}} | {display} |")

print()
```
