---
name: check
description: Run all quality gates
tags:
  - dev
  - quality
supports:
  - dry-run
flags:
  - name: threshold
    short: t
    type: string
    description: Minimum coverage percentage
    default: "85"
  - name: skip-coverage
    short: s
    type: bool
    description: Skip the coverage gate (faster)
  - name: diff
    short: d
    type: bool
    description: Only check coverage for files changed since main
env:
  - name: CREFT_DRY_RUN
    required: false
---

```docs
Run tests, lint, and coverage as a single quality gate report.

Each gate delegates to the corresponding creft skill:

  creft test --summary     Run the full test suite (nextest), summary output
  creft lint               Run clippy with -D warnings
  creft coverage           Run llvm-cov coverage analysis

Gates:
  Tests     All tests must pass. Failures print individual test output.
  Lint      Zero clippy warnings or errors.
  Coverage  Total line coverage must meet --threshold (default: 85%).
            Skip with --skip-coverage for a faster check during development.

Dry-run:
  Set CREFT_DRY_RUN=1 (or pass --dry-run) to print the commands that
  would run without executing them.

Examples:
  creft check                        Run all gates with defaults
  creft check --skip-coverage        Skip coverage (faster)
  creft check --threshold 90         Require 90% line coverage
  creft check --diff                 Coverage only for changed files
  creft check --dry-run              Print commands, do not execute
```

```bash
threshold="{{threshold}}"
skip_coverage="{{skip-coverage}}"
diff_flag="{{diff}}"

# Dry-run: print what would run and exit without executing.
if [ "$CREFT_DRY_RUN" = "1" ]; then
    echo "Would run:"
    echo "  creft test --summary"
    echo "  creft lint"
    if [ "$skip_coverage" != "true" ]; then
        if [ "$diff_flag" = "true" ]; then
            echo "  creft coverage --threshold $threshold --diff"
        else
            echo "  creft coverage --threshold $threshold"
        fi
    fi
    exit 0
fi

any_failed=0

echo "# Quality Gate Report"
echo ""

# Gate 1: Tests
echo "## Tests"
echo ""
creft test --summary
test_exit=$?
if [ $test_exit -ne 0 ]; then
    any_failed=1
fi
echo ""

# Gate 2: Lint
echo "## Lint"
echo ""
creft lint
lint_exit=$?
if [ $lint_exit -ne 0 ]; then
    any_failed=1
fi
echo ""

# Gate 3: Coverage (optional)
if [ "$skip_coverage" != "true" ]; then
    echo "## Coverage"
    echo ""
    if [ "$diff_flag" = "true" ]; then
        creft coverage --threshold "$threshold" --diff
    else
        creft coverage --threshold "$threshold"
    fi
    cov_exit=$?
    if [ $cov_exit -ne 0 ]; then
        any_failed=1
    fi
    echo ""
fi

# Summary
echo "---"
echo ""
if [ $any_failed -eq 0 ]; then
    echo "**All gates passed.**"
else
    echo "**Some gates failed.** See above for details."
    exit 1
fi
```
