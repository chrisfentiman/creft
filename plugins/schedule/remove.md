---
name: schedule remove
description: Unload and delete a creft-managed scheduled job.
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
        print(f"error: {plist_path} does not exist. Nothing to remove.", file=sys.stderr)
        sys.exit(1)

    # Unload — ignore exit code because the job may not be loaded, which is fine
    unload_result = subprocess.run(
        ["launchctl", "unload", str(plist_path)],
        capture_output=True,
        text=True,
    )
    if unload_result.returncode == 0:
        print(f"unloaded {label}")
    else:
        # Not fatal — might just mean it wasn't loaded
        print(f"(launchctl unload returned {unload_result.returncode}, likely not loaded)")

    plist_path.unlink()
    print(f"deleted {plist_path}")


if __name__ == "__main__":
    main()
```
