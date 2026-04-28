---
name: _test pipe creft_exit_with_llm
description: Regression for the LLM-sponge cancellation contract on creft_exit
pipe: true
---

```python
# deps: none
import json
print(json.dumps({"deny": "regex match"}))
creft_exit()
```

```llm
provider: nonexistent_llm_binary_xyz
---
Echo this back: {{prev}}
```

```python
# deps: none
import sys
print("downstream:", sys.stdin.read())
```
