# creft

Turning agent skills into executable CLI commands.

The problem is you can't build a CLI for every little workflow you want
to automate — and skills burn tokens for what's mostly ceremony. With
creft, you write a markdown file and it runs:

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

That's it. A markdown file became a CLI command.

An agent can create these with `creft add`. It can discover them with `creft list`. It can run them with `creft <name>`. It can install collections of them with `creft plugin install`.

No shared context needed between sessions — the skill is the context.

## Get Started

```sh
curl creft.run | sh
```

Set up your agent:

```sh
creft up
```

That's it. Works with Claude Code, Cursor, Copilot, Windsurf, Codex, Gemini CLI.

---

`creft add --help` for the full skill format. Multi-language blocks, LLM pipes, typed args, validation, plugins — it's all there.

[Docs](docs/) · [Skill Reference](docs/skill-reference.md) · [Bundled Plugins](docs/bundled-plugins.md) · [MIT License](LICENSE)

---

## Telemetry

creft checks for new releases once per UTC day. The check is a single GET request to `https://creft.run/latest`. The request carries one piece of information — a User-Agent header of the form `creft/<version> (<os>; <arch>)` — and nothing else. No per-machine identifier, no IP-derived ID, no install UUID.

The same request serves two purposes:

1. **Useful to you.** When a new version is available, the next interactive `creft` command surfaces a one-line "update available" notice. Run `creft update` (or `brew upgrade creft` for Homebrew installs) to upgrade.
2. **Useful to the project.** The User-Agent tells creft.run that an install at version X on platform Y was active today. The project counts active days per `(version, OS, arch)` bucket; nothing else is queryable from it.

The check runs in a fire-and-forget child process, so it cannot block your command. To opt out:

```sh
creft settings set telemetry off
```

To re-enable:

```sh
creft settings set telemetry on
```

The disclosure is shown on the welcome screen the first time creft runs after install, and is reflected in `creft settings show`.

`creft update` (run manually) calls the same endpoint and is **not** gated by the telemetry setting — it is the explicit purpose of the command.

The daily background check is suppressed when `$CI` is set to `true` or `1`. CI environments run bot traffic that does not represent active installs and would inflate the volume signal; the carve-out keeps the count meaningful. To force the check on or off independent of `$CI`, use `creft settings set telemetry on|off`. The manual `creft update` command is not affected by the CI carve-out — running it inside a CI workflow upgrades creft as it does anywhere else.

If creft was installed via `cargo install creft`, run `cargo install creft` to upgrade. The `creft update` command refuses with a redirect message, the same way it does for Homebrew installs — package-manager-installed binaries should be upgraded through the package manager that owns them.

---

## Why I built this

MCPs were awesome until the token bloat really started to kill it. The workarounds help, but when you're working with local coding agents, the best interface I've seen them consistently work with is just a CLI. So I started building CLIs — one for Databricks queries, a Python step that analyzed the results, an LLM step that reasoned about them. Suddenly I had a workflow script gluing all these components together, and it was awesome.

But it made me want to build more, and that left me with a fundamental problem: do I spend all my time building tooling for my coding agent, away from actually doing productive work? Or do I get it to recreate the same thing every time, just for the next repository I'm working on? creft came from that. I needed a way to create skills and commands that scale — zero cost for creating them, zero cost for keeping them around. Now the agent builds them, and they just exist. Across repos, across sessions, across machines.