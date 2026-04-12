---
name: schedule ls
description: List all creft-managed scheduled jobs with status, schedule, and command.
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
    launch_agents = Path.home() / "Library" / "LaunchAgents"
    if not launch_agents.exists():
        print("no creft-managed schedules found (no ~/Library/LaunchAgents directory)")
        return

    plists = sorted(launch_agents.glob("creft.*.plist"))
    if not plists:
        print("no creft-managed schedules found")
        return

    # Get loaded status from launchctl
    loaded = set()
    result = subprocess.run(["launchctl", "list"], capture_output=True, text=True)
    if result.returncode == 0:
        for line in result.stdout.splitlines():
            parts = line.split("\t")
            if len(parts) >= 3 and parts[2].startswith("creft."):
                loaded.add(parts[2])

    print(f"{len(plists)} creft-managed schedule(s):\n")
    for p in plists:
        try:
            data = read_plist(p)
        except Exception as e:
            print(f"  {p.name} [unreadable: {e}]")
            continue

        label = data.get("Label", p.stem)
        name = label.removeprefix("creft.")
        status = "loaded" if label in loaded else "unloaded"

        print(f"  {name} [{status}]")

        interval = data.get("StartCalendarInterval")
        run_at_load = data.get("RunAtLoad", False)
        if interval is None and run_at_load:
            print(f"    schedule: at reboot (RunAtLoad)")
        elif isinstance(interval, dict):
            print(f"    schedule: {format_interval(interval)}")
        elif isinstance(interval, list):
            if len(interval) == 1:
                print(f"    schedule: {format_interval(interval[0])}")
            else:
                print(f"    schedule: ({len(interval)} time slots)")
                for iv in interval[:5]:
                    print(f"              {format_interval(iv)}")
                if len(interval) > 5:
                    print(f"              ... and {len(interval) - 5} more")
        else:
            print(f"    schedule: (no schedule)")

        cmd = data.get("ProgramArguments", [])
        if len(cmd) >= 3 and cmd[0] == "/bin/zsh":
            cmd_str = cmd[2]
        elif cmd:
            cmd_str = " ".join(cmd)
        else:
            cmd_str = "(none)"
        if len(cmd_str) > 80:
            cmd_str = cmd_str[:77] + "..."
        print(f"    command:  {cmd_str}")

        workdir = data.get("WorkingDirectory", "")
        if workdir:
            print(f"    workdir:  {workdir}")

        log = data.get("StandardOutPath", "")
        if log:
            print(f"    log:      {log}")
        print()


if __name__ == "__main__":
    main()
```
