---
name: schedule add
description: Install a macOS launchd scheduled job. Cron-format schedules, zsh -lic wrapper.
args:
- name: name
  description: Job name (becomes the label suffix, e.g. 'daily-brief' → 'creft.daily-brief')
flags:
- name: schedule
  description: Cron expression (5 fields) or special string (@daily, @hourly, @weekly, @monthly, @yearly, @reboot)
- name: command
  description: Shell command to run at the scheduled time
- name: workdir
  description: Working directory (defaults to current directory)
  default: ''
- name: log
  description: Log file path (defaults to ~/Library/Logs/creft.<name>.log)
  default: ''
- name: preview
  short: p
  type: bool
  description: Print what would be installed without writing the plist or calling launchctl
---

```python
import json
import os
import re
import subprocess
import sys
from pathlib import Path


def _plist_value(v, indent=2):
    prefix = "  " * indent
    if isinstance(v, bool):
        return f"{prefix}<{'true' if v else 'false'}/>"
    if isinstance(v, int):
        return f"{prefix}<integer>{v}</integer>"
    if isinstance(v, str):
        return f"{prefix}<string>{v}</string>"
    if isinstance(v, list):
        lines = [f"{prefix}<array>"]
        for item in v:
            lines.append(_plist_value(item, indent + 1))
        lines.append(f"{prefix}</array>")
        return "\n".join(lines)
    if isinstance(v, dict):
        lines = [f"{prefix}<dict>"]
        for k, val in v.items():
            lines.append(f"{prefix}  <key>{k}</key>")
            lines.append(_plist_value(val, indent + 1))
        lines.append(f"{prefix}</dict>")
        return "\n".join(lines)
    return f"{prefix}<string>{v}</string>"


def write_plist(data, path):
    lines = [
        '<?xml version="1.0" encoding="UTF-8"?>',
        '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">',
        '<plist version="1.0">',
        '<dict>',
    ]
    for k, v in data.items():
        lines.append(f"  <key>{k}</key>")
        lines.append(_plist_value(v, 1))
    lines.append('</dict>')
    lines.append('</plist>')
    lines.append('')
    with open(path, "w") as f:
        f.write("\n".join(lines))


NAME_PATTERN = re.compile(r"^[a-zA-Z0-9_-]+$")

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


def die(msg):
    print(f"error: {msg}", file=sys.stderr)
    sys.exit(1)


def main():
    name = "{{name}}"
    schedule = r'''{{schedule}}'''
    command = r'''{{command}}'''
    workdir = r'''{{workdir|}}'''
    log = r'''{{log|}}'''
    preview = "{{preview|}}".strip().lower() in ("true", "1", "yes")

    if not name:
        die("name is required")
    if not NAME_PATTERN.match(name):
        die(f"name must match [a-zA-Z0-9_-]+, got {name!r}")
    if not schedule:
        die("--schedule is required")
    if not command:
        die("--command is required")

    label = f"creft.{name}"

    if not workdir:
        workdir = os.getcwd()
    workdir = os.path.abspath(os.path.expanduser(workdir))
    if not os.path.isdir(workdir):
        die(f"workdir does not exist: {workdir}")

    if not log:
        log_dir = Path.home() / "Library" / "Logs"
        log = str(log_dir / f"{label}.log")
    log = os.path.abspath(os.path.expanduser(log))

    # Delegate cron parsing to the shared helper.
    result = subprocess.run(
        ["creft", "schedule", "parse-cron", schedule],
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        # parse-cron already wrote a human-readable error to stderr.
        sys.stderr.write(result.stderr)
        sys.exit(result.returncode)
    try:
        parsed = json.loads(result.stdout)
    except json.JSONDecodeError as e:
        die(f"parse-cron returned invalid JSON: {e}\noutput was: {result.stdout!r}")

    # Build the plist. parse-cron returns either {"StartCalendarInterval": ...}
    # or {"RunAtLoad": true} (for @reboot). Merge it into the base dict.
    plist = {
        "Label": label,
        "ProgramArguments": ["/bin/zsh", "-lic", command],
        "WorkingDirectory": workdir,
        "StandardOutPath": log,
        "StandardErrorPath": log,
        "RunAtLoad": False,
        **parsed,
    }

    launch_agents = Path.home() / "Library" / "LaunchAgents"
    plist_path = launch_agents / f"{label}.plist"

    if preview:
        print(f"would install {label}")
        print(f"plist:     {plist_path}")
        print()
        print("schedule:")
        if "RunAtLoad" in parsed and parsed["RunAtLoad"]:
            print("  at reboot (RunAtLoad)")
        else:
            interval = parsed.get("StartCalendarInterval")
            if isinstance(interval, dict):
                print(f"  {format_interval(interval)}")
            elif isinstance(interval, list):
                for iv in interval:
                    print(f"  {format_interval(iv)}")
        print()
        print(f"command:   {command}")
        print(f"workdir:   {workdir}")
        print(f"log:       {log}")
        print()
        print("(preview — nothing installed)")
        return

    if plist_path.exists():
        die(
            f"{plist_path} already exists. "
            f"Run `creft schedule update {name} ...` to modify, "
            f"or `creft schedule remove {name}` to delete first."
        )

    launch_agents.mkdir(parents=True, exist_ok=True)
    Path(log).parent.mkdir(parents=True, exist_ok=True)

    write_plist(plist, plist_path)

    # Load atomically: if load fails, remove the plist so we're not left
    # with a file on disk that isn't actually scheduled.
    load_result = subprocess.run(
        ["launchctl", "load", str(plist_path)],
        capture_output=True,
        text=True,
    )
    if load_result.returncode != 0:
        plist_path.unlink()
        print(f"error: launchctl load failed (exit {load_result.returncode})", file=sys.stderr)
        if load_result.stderr:
            print(load_result.stderr.strip(), file=sys.stderr)
        print("plist removed; nothing installed", file=sys.stderr)
        sys.exit(load_result.returncode)

    print(f"installed {label}")
    print(f"plist:     {plist_path}")
    print()
    print("schedule:")
    if "RunAtLoad" in parsed and parsed["RunAtLoad"]:
        print("  at reboot (RunAtLoad)")
    else:
        interval = parsed.get("StartCalendarInterval")
        if isinstance(interval, dict):
            print(f"  {format_interval(interval)}")
        elif isinstance(interval, list):
            for iv in interval:
                print(f"  {format_interval(iv)}")
    print()
    print(f"command:   {command}")
    print(f"workdir:   {workdir}")
    print(f"log:       {log}")
    print()
    print(f"manual run: creft schedule run {name}")
    print(f"status:     creft schedule status {name}")
    print(f"remove:     creft schedule remove {name}")


if __name__ == "__main__":
    main()
```
