---
name: _test pipe creft_exit_python_next
description: Regression for creft_exit stdout recovery with a non-sponge downstream block
pipe: true
---

```python
# deps: none
import json
print(json.dumps({"deny": "regex match"}))
creft_exit()
```

```python
# deps: none
import time
time.sleep(60)
import sys
sys.stdin.read()
```
