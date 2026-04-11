---
name: schedule update
description: Atomically modify an existing creft-managed scheduled job. Change schedule, command, workdir, or log; leave anything you don't specify unchanged.
args:
- name: name
  description: Job name (without the 'creft.' prefix)
flags:
- name: schedule
  description: New cron expression or special string. Leave unset to keep the current schedule.
  default: ''
- name: command
  description: New shell command. Leave unset to keep the current command.
  default: ''
- name: workdir
  description: New working directory. Leave unset to keep the current workdir.
  default: ''
- name: log
  description: New log file path. Leave unset to keep the current log.
  default: ''
- name: preview
  short: p
  type: bool
  description: Print what would change without writing the plist or calling launchctl
---

```python
import json
import os
import re
import shutil
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


def describe_schedule(plist):
    """Human-readable summary of a plist's schedule."""
    if plist.get("RunAtLoad") and "StartCalendarInterval" not in plist:
        return ["at reboot (RunAtLoad)"]
    interval = plist.get("StartCalendarInterval")
    if isinstance(interval, dict):
        return [format_interval(interval)]
    if isinstance(interval, list):
        return [format_interval(iv) for iv in interval]
    return ["(no schedule)"]


def die(msg):
    print(f"error: {msg}", file=sys.stderr)
    sys.exit(1)


def main():
    name = "{{name}}"
    new_schedule = r'''{{schedule|}}'''
    new_command = r'''{{command|}}'''
    new_workdir = r'''{{workdir|}}'''
    new_log = r'''{{log|}}'''
    preview = "{{preview|}}".strip().lower() in ("true", "1", "yes")

    if not name:
        die("name is required")
    if not NAME_PATTERN.match(name):
        die(f"name must match [a-zA-Z0-9_-]+, got {name!r}")

    if not any([new_schedule, new_command, new_workdir, new_log]):
        die("at least one of --schedule, --command, --workdir, --log must be specified")

    label = f"creft.{name}"
    plist_path = Path.home() / "Library" / "LaunchAgents" / f"{label}.plist"

    if not plist_path.exists():
        die(
            f"{label} does not exist. "
            f"Run `creft schedule add {name} --schedule ... --command ...` to create it."
        )

    # Load the existing plist as our starting point.
    plist = read_plist(plist_path)

    # Merge each flag into the plist if it was provided.
    # Anything not specified stays at its existing value.

    if new_command:
        plist["ProgramArguments"] = ["/bin/zsh", "-lic", new_command]

    if new_workdir:
        workdir = os.path.abspath(os.path.expanduser(new_workdir))
        if not os.path.isdir(workdir):
            die(f"workdir does not exist: {workdir}")
        plist["WorkingDirectory"] = workdir

    if new_log:
        log = os.path.abspath(os.path.expanduser(new_log))
        plist["StandardOutPath"] = log
        plist["StandardErrorPath"] = log

    if new_schedule:
        result = subprocess.run(
            ["creft", "schedule", "parse-cron", new_schedule],
            capture_output=True,
            text=True,
        )
        if result.returncode != 0:
            sys.stderr.write(result.stderr)
            sys.exit(result.returncode)
        try:
            parsed = json.loads(result.stdout)
        except json.JSONDecodeError as e:
            die(f"parse-cron returned invalid JSON: {e}\noutput was: {result.stdout!r}")

        # Clear both keys before applying the new schedule so we don't leave
        # stale fields when switching between calendar-based and @reboot.
        plist.pop("StartCalendarInterval", None)
        plist["RunAtLoad"] = False
        plist.update(parsed)

    # Summarize what changes.
    changes = []
    if new_schedule:
        changes.append("schedule:")
        for line in describe_schedule(plist):
            changes.append(f"  {line}")
    if new_command:
        cmd = plist["ProgramArguments"][2] if len(plist["ProgramArguments"]) >= 3 else ""
        changes.append(f"command:   {cmd}")
    if new_workdir:
        changes.append(f"workdir:   {plist['WorkingDirectory']}")
    if new_log:
        changes.append(f"log:       {plist['StandardOutPath']}")

    if preview:
        print(f"would update {label}")
        print(f"plist:     {plist_path}")
        print()
        for line in changes:
            print(line)
        print()
        print("(preview — nothing changed)")
        return

    # Back up the existing plist before making any changes. If anything fails,
    # we restore from this backup so update is atomic.
    backup_path = plist_path.with_suffix(".plist.backup")
    shutil.copy2(plist_path, backup_path)

    def rollback(reason):
        """Restore the backup and attempt to reload the old plist."""
        shutil.copy2(backup_path, plist_path)
        backup_path.unlink(missing_ok=True)
        # Try to reload the old plist. Ignore errors; the operator will see
        # the rollback message and can investigate.
        subprocess.run(
            ["launchctl", "load", str(plist_path)],
            capture_output=True,
            text=True,
        )
        print(f"error: {reason}", file=sys.stderr)
        print("rolled back to previous plist", file=sys.stderr)
        sys.exit(1)

    # Unload the current job before replacing the plist. launchd caches the
    # loaded plist contents, so load-without-unload would silently keep the
    # old definition.
    unload_result = subprocess.run(
        ["launchctl", "unload", str(plist_path)],
        capture_output=True,
        text=True,
    )
    if unload_result.returncode != 0:
        # Not fatal — the job might not have been loaded. Log it but continue.
        print(
            f"(launchctl unload returned {unload_result.returncode}, likely not loaded)",
            file=sys.stderr,
        )

    # Write the new plist.
    try:
        write_plist(plist, plist_path)
    except OSError as e:
        rollback(f"failed to write new plist: {e}")

    # Load the new plist. If this fails, roll back to the backup.
    load_result = subprocess.run(
        ["launchctl", "load", str(plist_path)],
        capture_output=True,
        text=True,
    )
    if load_result.returncode != 0:
        err = load_result.stderr.strip() if load_result.stderr else ""
        rollback(f"launchctl load failed (exit {load_result.returncode}): {err}")

    # Success — remove the backup.
    backup_path.unlink(missing_ok=True)

    print(f"updated {label}")
    print(f"plist:   {plist_path}")
    print()
    for line in changes:
        print(line)


if __name__ == "__main__":
    main()
```
