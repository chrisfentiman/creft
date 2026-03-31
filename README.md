# creft

[![CI](https://github.com/chrisfentiman/creft/actions/workflows/ci.yml/badge.svg)](https://github.com/chrisfentiman/creft/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/creft.svg)](https://crates.io/crates/creft)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

A skill system that turns markdown instructions into executable commands.

![demo](assets/demo.gif)

- **Agents author repeatable workflows.** Write a markdown file, `creft add` makes it a command. Next session, next machine — it just runs. No LLM needed.
- **Skills run deterministically.** Same input, same output. No interpretation, no token cost.
- **Skills validate themselves.** Syntax, PATH commands, PyPI/npm deps — checked before saving.
- **Skills distribute as packages.** `creft install <git-url>`. Share workflows like code.

## Install

```sh
cargo install creft
```

Or: `brew install chrisfentiman/creft/creft` · [Binary releases](https://github.com/chrisfentiman/creft/releases)

## Write a skill

````sh
creft add <<'EOF'
---
name: hello
description: Greet someone
args:
  - name: who
---
```bash
echo "Hello, {{who}}!"
```
EOF
````

```
$ creft hello World
Hello, World!
```

## Mix languages

````sh
creft add <<'EOF'
---
name: linecount
description: Lines of code by file
pipe: true
---
```bash
find src -name '*.rs' -exec wc -l {} +
```
```python
import sys
for line in sys.stdin.read().strip().splitlines():
    parts = line.split()
    if len(parts) == 2:
        print(f"  {parts[1].split('/')[-1]:20s} {parts[0]:>6s}")
```
EOF
````

```
$ creft linecount
  store.rs                  1737
  runner.rs                 1917
  doctor.rs                 1883
```

`pipe: true` connects blocks with OS file descriptors. Concurrent, not buffered.

## Add structure

````markdown
---
name: deploy
description: Deploy to production
args:
  - name: env
    validation: "^(staging|production)$"
flags:
  - name: dry-run
    short: d
    type: bool
env:
  - name: AWS_PROFILE
    required: true
---

```docs
Deploys the current branch. Requires AWS credentials.
```

```bash
echo "Deploying to {{env}}..."
git rev-parse --short HEAD
```

```bash
# deps: awscli
aws ecs update-service --cluster {{env}} --service app --force-new-deployment
```
````

Args with regex validation. Typed flags. Required env vars. Docs blocks for `--help`. Dependencies installed on the fly via `uv`/`npx`. All declared in one file.

## Teach your agent

```sh
creft up              # auto-detect: Claude Code, Cursor, Windsurf, Copilot, Codex, Gemini
```

After setup, agents discover skills (`creft list`), run them (`creft <name>`), and author new ones (`creft add`).

## Share as packages

```sh
creft install https://github.com/example/k8s-tools
creft update k8s-tools
creft uninstall k8s-tools
```

A `creft.yaml` manifest at the repo root. Skills namespaced under the package name.

## Commands

| | |
|---|---|
| `creft add` | Save a skill from stdin |
| `creft list` | List skills |
| `creft show <name>` | Print a skill definition |
| `creft edit <name>` | Edit in `$EDITOR` or from stdin |
| `creft rm <name>` | Delete a skill |
| `creft cat <name>` | Print code blocks only |
| `creft install <url>` | Install from git |
| `creft update [name]` | Update packages |
| `creft uninstall <name>` | Remove a package |
| `creft up [system]` | Set up AI integration |
| `creft init` | Initialize local `.creft/` |
| `creft doctor [name]` | Check environment or skill health |

## This repo runs on creft

| | |
|---|---|
| `creft test` | Run tests, markdown output |
| `creft test mutants` | Mutation testing |
| `creft lint` | Clippy, markdown output |
| `creft coverage` | Code coverage with source context |
| `creft check` | All quality gates (calls test, lint, coverage) |
| `creft bench` | Compile time, test time, binary size |
| `creft changelog` | Changelog from git history |

## Contributing

Pull requests welcome. Open an issue first for significant changes.

## License

[MIT](LICENSE)
