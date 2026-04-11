---
name: schedule parse-cron
description: Parse a cron expression into a launchd-compatible schedule spec. Internal helper called by schedule add and schedule update.
args:
- name: expression
  description: Cron expression (5 fields) or special string (@daily, @hourly, @weekly, @monthly, @yearly, @reboot)
---

```python
"""
Parse a cron expression into launchd's schedule format.

Output on success: a single-line JSON object written to stdout, containing
one of:
  {"StartCalendarInterval": {...}}        — single time
  {"StartCalendarInterval": [{...}, ...]} — multiple times
  {"RunAtLoad": true}                     — @reboot

Exit 0 on success, exit 1 on error with a human-readable message on stderr.

Supports:
  - Standard 5-field cron: minute hour day month weekday
  - Wildcards: *
  - Lists: 1,3,5
  - Ranges: 1-5 (including with names: MON-FRI, JAN-JUN)
  - Named weekdays: sun mon tue wed thu fri sat (case-insensitive)
  - Named months: jan feb mar apr may jun jul aug sep oct nov dec (case-insensitive)
  - Special strings: @reboot @hourly @daily @midnight @weekly @monthly @yearly @annually

Rejects with a clear error:
  - Step syntax (*/N) — expand to explicit list instead
  - Reversed ranges (5-1)
  - Malformed ranges (1-2-3, 1-, -5)
  - Non-numeric values where numbers are expected
  - All-wildcard expressions (* * * * *)
  - Values outside the valid range for their field
  - Day/weekday names in fields that don't accept them
"""
import json
import sys
from itertools import product


WEEKDAY_NAMES = {
    "sun": 0, "mon": 1, "tue": 2, "wed": 3,
    "thu": 4, "fri": 5, "sat": 6,
}
MONTH_NAMES = {
    "jan": 1, "feb": 2, "mar": 3, "apr": 4, "may": 5, "jun": 6,
    "jul": 7, "aug": 8, "sep": 9, "oct": 10, "nov": 11, "dec": 12,
}

# @-prefixed special strings expand to standard 5-field cron expressions.
# @reboot is handled separately because launchd uses RunAtLoad, not a calendar interval.
SPECIALS = {
    "@hourly":   "0 * * * *",
    "@daily":    "0 0 * * *",
    "@midnight": "0 0 * * *",
    "@weekly":   "0 0 * * 0",
    "@monthly":  "0 0 1 * *",
    "@yearly":   "0 0 1 1 *",
    "@annually": "0 0 1 1 *",
}


def die(msg):
    print(f"error: {msg}", file=sys.stderr)
    sys.exit(1)


def parse_value(s, fname, name_map=None):
    """Parse one value token into an integer. Accepts names if name_map is given."""
    s = s.strip()
    if not s:
        die(f"{fname}: empty value")
    if name_map is not None:
        low = s.lower()
        if low in name_map:
            return name_map[low]
    try:
        return int(s)
    except ValueError:
        if name_map is not None:
            valid = ", ".join(sorted(name_map))
            die(f"{fname}: expected integer or one of [{valid}], got {s!r}")
        die(f"{fname}: expected integer, got {s!r}")


def parse_field(field, fname, lo, hi, name_map=None):
    """Parse one cron field into a sorted list of valid integers (or None for wildcard)."""
    field = field.strip()
    if not field:
        die(f"{fname}: empty field")
    if field == "*":
        return None
    if "/" in field:
        die(
            f"{fname}: step syntax (*/N) not supported. "
            f"Expand to explicit values, e.g. '*/2' in hour → '0,2,4,6,8,10,12,14,16,18,20,22'"
        )

    values = []
    for part in field.split(","):
        part = part.strip()
        if not part:
            die(f"{fname}: empty value in list")
        if "-" in part:
            # Range syntax: a-b
            range_parts = part.split("-")
            if len(range_parts) != 2:
                die(f"{fname}: malformed range {part!r} (expected A-B, got {len(range_parts)} parts)")
            a_str, b_str = range_parts
            if not a_str or not b_str:
                die(f"{fname}: malformed range {part!r} (missing endpoint)")
            a = parse_value(a_str, fname, name_map)
            b = parse_value(b_str, fname, name_map)
            if a > b:
                die(f"{fname}: reversed range {part!r} ({a} > {b}). Use {b}-{a} or a list.")
            values.extend(range(a, b + 1))
        else:
            values.append(parse_value(part, fname, name_map))

    for v in values:
        if v < lo or v > hi:
            die(f"{fname}: value {v} out of range [{lo}, {hi}]")
    return sorted(set(values))


def parse_cron(expr):
    """Parse a 5-field cron expression or @-special. Returns a dict suitable for json.dumps."""
    expr = expr.strip()

    if expr == "@reboot":
        return {"RunAtLoad": True}

    if expr.startswith("@"):
        if expr not in SPECIALS:
            valid = ", ".join(sorted(SPECIALS) + ["@reboot"])
            die(f"unknown special string {expr!r}. Valid: {valid}")
        expr = SPECIALS[expr]

    fields = expr.split()
    if len(fields) != 5:
        die(
            f"cron expression must have 5 fields (minute hour day month weekday), "
            f"got {len(fields)}: {expr!r}"
        )

    minute, hour, day, month, weekday = fields

    mins = parse_field(minute, "minute", 0, 59)
    hrs = parse_field(hour, "hour", 0, 23)
    days = parse_field(day, "day", 1, 31)
    months = parse_field(month, "month", 1, 12, MONTH_NAMES)
    wds = parse_field(weekday, "weekday", 0, 7, WEEKDAY_NAMES)

    # cron tradition: 7 and 0 both mean Sunday. Normalize to 0.
    if wds is not None:
        wds = sorted(set(0 if w == 7 else w for w in wds))

    field_lists = []
    field_keys = []
    for values, key in [(mins, "Minute"), (hrs, "Hour"), (days, "Day"), (months, "Month"), (wds, "Weekday")]:
        if values is not None:
            field_lists.append(values)
            field_keys.append(key)

    if not field_lists:
        die(
            "all-wildcard expression (* * * * *) would run every minute. "
            "Specify at least one field."
        )

    intervals = [dict(zip(field_keys, combo)) for combo in product(*field_lists)]
    if len(intervals) == 1:
        return {"StartCalendarInterval": intervals[0]}
    return {"StartCalendarInterval": intervals}


def main():
    expr = "{{expression}}"
    if not expr:
        die("expression is required")
    result = parse_cron(expr)
    print(json.dumps(result))


if __name__ == "__main__":
    main()
```
