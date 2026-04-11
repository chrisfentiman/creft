---
name: schedule run
description: Manually trigger a creft-managed scheduled job, regardless of schedule.
args:
- name: name
  description: Job name (without the 'creft.' prefix)
---

```python
import subprocess
import sys
from pathlib import Path


def main():
    name = "{{name}}"
    if not name:
        print("error: name is required", file=sys.stderr)
        sys.exit(1)

    label = f"creft.{name}"
    plist_path = Path.home() / "Library" / "LaunchAgents" / f"{label}.plist"

    if not plist_path.exists():
        print(
            f"error: {label} is not installed. "
            f"Run `creft schedule add {name} --schedule ... --command ...` first.",
            file=sys.stderr,
        )
        sys.exit(1)

    result = subprocess.run(
        ["launchctl", "start", label],
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        print(f"error: launchctl start failed (exit {result.returncode})", file=sys.stderr)
        if result.stderr:
            print(result.stderr.strip(), file=sys.stderr)
        sys.exit(1)

    print(f"started {label}")
    print()
    print(f"watch progress:  creft schedule status {name}")
    print(f"tail log:        tail -f ~/Library/Logs/{label}.log")


if __name__ == "__main__":
    main()
```
