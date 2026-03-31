---
name: hello
description: Greet someone with style
args:
- name: who
pipe: true
---

```bash
echo "Hello, {{who}}!"
```

```python
# deps: pyfiglet
import sys, pyfiglet
text = sys.stdin.read().strip()
print(pyfiglet.figlet_format(text))
```
