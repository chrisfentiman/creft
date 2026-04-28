---
name: _test arg_path_no_clobber
description: Regression fixture — arg named `path` must not clobber inherited PATH
args:
  - name: path
    description: A path value passed to the skill (stored as CREFT_ARG_PATH, not PATH)
---

```bash
python3 --version
```
