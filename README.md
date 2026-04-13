# creft

Turning agent skills into executable CLI commands.

The problem is you can't build a CLI for every little workflow you want
to automate — and skills burn tokens for what's mostly ceremony. With
creft, you write a markdown file and it runs:

````sh
creft cmd add <<'EOF'
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

That's it. A markdown file became a CLI command.

An agent can create these with `creft cmd add`. It can discover them with `creft cmd list`. It can run them with `creft <name>`. It can install collections of them with `creft plugins install`.

No shared context needed between sessions — the skill is the context.

## Get Started

```sh
cargo install creft
```

Set up your agent:

```sh
creft up
```

That's it. Works with Claude Code, Cursor, Copilot, Windsurf, Codex, Gemini CLI.

---

`creft cmd add --help` for the full skill format. Multi-language blocks, LLM pipes, typed args, validation, plugins — it's all there.

[Docs](docs/) · [Skill Reference](docs/skill-reference.md) · [Bundled Plugins](docs/bundled-plugins.md) · [MIT License](LICENSE)

---

## Why I built this

MCPs were awesome until the token bloat really started to kill it. The workarounds help, but when you're working with local coding agents, the best interface I've seen them consistently work with is just a CLI. So I started building CLIs — one for Databricks queries, a Python step that analyzed the results, an LLM step that reasoned about them. Suddenly I had a workflow script gluing all these components together, and it was awesome.

But it made me want to build more, and that left me with a fundamental problem: do I spend all my time building tooling for my coding agent, away from actually doing productive work? Or do I get it to recreate the same thing every time, just for the next repository I'm working on? creft came from that. I needed a way to create skills and commands that scale — zero cost for creating them, zero cost for keeping them around. Now the agent builds them, and they just exist. Across repos, across sessions, across machines.