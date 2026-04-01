---
name: linecount
description: Lines of code by file
args:
- name: file
  description: Specific file to count
---

```bash
file="{{file|}}"
if [ -n "$file" ]; then
    wc -l "src/$file"
else
    find src -name '*.rs' -exec wc -l {} +
fi
```

```python
import sys
for line in sys.stdin.read().strip().splitlines():
    parts = line.split()
    if len(parts) == 2 and parts[1] != "total":
        print(f"  {parts[1].split('/')[-1]:20s} {parts[0]:>6s}")
    elif "total" in line:
        print(f"\n  {'total':20s} {parts[0]:>6s}")
```
