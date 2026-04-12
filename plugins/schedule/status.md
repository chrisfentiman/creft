---
name: schedule status
description: Show detailed status of a creft-managed scheduled job including recent log tail.
args:
- name: name
  description: Job name (without the 'creft.' prefix)
---

```python
import json
import subprocess
import sys
from pathlib import Path


def read_plist(path):
    result = subprocess.run(
        ["plutil", "-convert", "json", "-o", "-", str(path)],
        capture_output=True, text=True,
    )
    if result.returncode != 0:
        raise OSError(f"plutil failed: {result.stderr.strip()}")
    return json.loads(result.stdout)


WEEKDAYS = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"]
MONTHS = ["", "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"]


def format_interval(d):
    parts = []
    if "Weekday" in d:
        parts.append(WEEKDAYS[d["Weekday"]])
    if "Month" in d:
        parts.append(MONTHS[d["Month"]])
    if "Day" in d:
        parts.append(f"day-of-month {d['Day']}")
    if "Hour" in d and "Minute" in d:
        parts.append(f"{d['Hour']:02d}:{d['Minute']:02d}")
    elif "Hour" in d:
        parts.append(f"{d['Hour']:02d}:** (any minute)")
    elif "Minute" in d:
        parts.append(f"**:{d['Minute']:02d} (every hour)")
    return " ".join(parts) if parts else "(no time constraint)"


def main():
    name = "{{name}}"
    if not name:
        print("error: name is required", file=sys.stderr)
        sys.exit(1)

    label = f"creft.{name}"
    plist_path = Path.home() / "Library" / "LaunchAgents" / f"{label}.plist"

    if not plist_path.exists():
        print(f"error: {label} is not installed.", file=sys.stderr)
        sys.exit(1)

    data = read_plist(plist_path)

    print(f"{label}")
    print(f"  plist:    {plist_path}")
    print()

    # Schedule
    interval = data.get("StartCalendarInterval")
    run_at_load = data.get("RunAtLoad", False)
    print("  schedule:")
    if interval is None and run_at_load:
        print(f"    at reboot (RunAtLoad)")
    elif isinstance(interval, dict):
        print(f"    {format_interval(interval)}")
    elif isinstance(interval, list):
        for iv in interval:
            print(f"    {format_interval(iv)}")
    else:
        print(f"    (no schedule)")
    print()

    # Command
    cmd = data.get("ProgramArguments", [])
    if len(cmd) >= 3 and cmd[0] == "/bin/zsh":
        print(f"  command:  {cmd[2]}")
    elif cmd:
        print(f"  command:  {' '.join(cmd)}")

    workdir = data.get("WorkingDirectory", "")
    if workdir:
        print(f"  workdir:  {workdir}")
    print()

    # launchctl state
    result = subprocess.run(
        ["launchctl", "list", label],
        capture_output=True,
        text=True,
    )
    if result.returncode == 0 and result.stdout.strip():
        print("  launchctl state:")
        # launchctl list <label> returns a plist-like string; print it indented
        for line in result.stdout.strip().splitlines():
            print(f"    {line}")
    else:
        print("  launchctl state: unloaded or not found")
    print()

    # Log tail
    log = data.get("StandardOutPath", "")
    if log:
        log_path = Path(log)
        if log_path.exists():
            size = log_path.stat().st_size
            print(f"  log:      {log_path} ({size} bytes)")
            with open(log_path) as f:
                lines = f.readlines()
            tail = lines[-20:] if len(lines) > 20 else lines
            if tail:
                print(f"  recent output (last {len(tail)} lines):")
                for line in tail:
                    print(f"    {line.rstrip()}")
            else:
                print(f"  (log is empty)")
        else:
            print(f"  log:      {log_path} (does not exist yet)")


if __name__ == "__main__":
    main()
```
