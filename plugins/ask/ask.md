---
name: ask
description: Ask a question and get an answer. If the first arg matches a registered project (see `creft project add`), spawns Claude Code in that project's directory and returns the answer. Otherwise, shows a native dialog to ask the user. Returns the answer on stdout, exits 1 on cancel.
args:
  - name: target
    description: A registered project name OR the question text. If it matches a project, the second arg is the question. If not, this IS the question for the user dialog.
    default: ""
  - name: question
    description: The question to ask the project (only used when target is a project name)
    default: ""
flags:
  - name: type
    short: t
    type: string
    default: text
    description: Question type for single-question mode (text, password, choice, multi, confirm)
  - name: options
    short: o
    type: string
    default: ""
    description: Comma-separated options for choice/multi types
  - name: context
    short: c
    type: string
    default: ""
    description: Background context shown above the question(s)
  - name: from
    short: f
    type: string
    default: Agent
    description: Name of the agent asking
  - name: json
    short: j
    type: string
    default: ""
    description: JSON survey spec for multi-question mode (overrides flag-based mode)
  - name: list
    short: l
    type: bool
    description: List registered projects available for cross-project queries
tags:
  - interaction
  - ui
---

```python
"""
Ask a question and get an answer.

Three modes:

1. Project query (first arg matches a registered project):
     creft ask weft "how does the router handle timeouts?"
   Spawns Claude Code in the target project's directory. The project's
   CLAUDE.md, rules, agents, and codebase become the context. Returns
   the answer on stdout.

2. Single question dialog (first arg is the prompt):
     creft ask "Should I evolve or wrap?" \\
       --type choice --options "Evolve,Wrap" \\
       --context "..." --from rust-architect
   Output (stdout): the answer string

3. Survey dialog (JSON-based, multi-question):
     creft ask --json '{...}'
   Output (stdout): JSON object mapping question id to answer

Question types (for dialog modes):
  text     - Free text input. Returns the typed string.
  password - Free text input masked with asterisks. Returns the typed string.
  choice   - Pick one from a list. Returns the chosen option.
  multi    - Pick zero or more. Returns comma-separated picks.
  confirm  - Yes/no. Returns "yes" or "no".

Exit codes:
  0 - Answer printed to stdout
  1 - User cancelled (dialog) or Claude errored (project query)
  2 - Configuration error

Register projects with: creft project add <name> /path
"""
import sys, platform, json, subprocess
from pathlib import Path

target = """{{target|}}"""
question = """{{question|}}"""
show_list = "{{list|}}".strip().lower() in ("true", "1", "yes")

# ── List mode ────────────────────────────────────────────────────
if show_list:
    _registry = Path.home() / ".creft" / "projects.json"
    _projects = {}
    if _registry.exists():
        try:
            with open(_registry) as _f:
                _projects = json.load(_f)
        except (json.JSONDecodeError, OSError):
            pass
    if not _projects:
        print("no projects registered")
        print("register one with: creft ask add <name> /path/to/project")
    else:
        print(f"{len(_projects)} project(s):\n")
        for _name, _entry in sorted(_projects.items()):
            _p = _entry.get("path", "?")
            _cli = _entry.get("cli", "claude")
            _agent = _entry.get("agent", "")
            _exists = Path(_p).is_dir()
            _status = "" if _exists else " [PATH MISSING]"
            print(f"  {_name}{_status}")
            print(f"    path:  {_p}")
            if _cli != "claude":
                print(f"    cli:   {_cli}")
            if _agent:
                print(f"    agent: {_agent}")
            print()
    sys.exit(0)

# ── Project query routing ────────────────────────────────────────
# If the first arg matches a registered project, spawn claude -p in
# that directory and return the answer. Everything below this block
# is the dialog path.
_registry = Path.home() / ".creft" / "projects.json"
if _registry.exists() and target:
    try:
        with open(_registry) as _f:
            _projects = json.load(_f)
    except (json.JSONDecodeError, OSError):
        _projects = {}

    if target in _projects:
        _proj = _projects[target]
        _path = _proj.get("path", "")
        _cli = _proj.get("cli", "claude")
        _agent = _proj.get("agent", "")

        if not question:
            print("error: question is required when asking a project", file=sys.stderr)
            print(f"usage: creft ask {target} \"your question here\"", file=sys.stderr)
            sys.exit(2)

        if not Path(_path).is_dir():
            print(f"error: project path does not exist: {_path}", file=sys.stderr)
            sys.exit(2)

        _cmd = [_cli, "-p", "--permission-mode", "bypassPermissions"]
        if _agent:
            _cmd.extend(["--agent", _agent])
        _cmd.append(question)

        _result = subprocess.run(_cmd, cwd=_path)
        sys.exit(_result.returncode)

# ── Dialog path (user query) ────────────────────────────────────
# First arg was not a project — treat it as the prompt text.
prompt_text = target
qtype_flag = "{{type|text}}".strip().lower()
options_raw = """{{options|}}"""
context_text = """{{context|}}"""
agent_from = """{{from|Agent}}"""
json_str = """{{json|}}""".strip()

# Build the survey spec
single_mode = False
spec = None

if json_str:
    try:
        spec = json.loads(json_str)
    except json.JSONDecodeError as e:
        print(f"Invalid --json: {e}", file=sys.stderr)
        sys.exit(2)
elif prompt_text:
    options = [o.strip() for o in options_raw.split(",") if o.strip()] if options_raw else []
    if qtype_flag in ("choice", "multi") and not options:
        print(f"--options required for {qtype_flag} type", file=sys.stderr)
        sys.exit(2)
    spec = {
        "from": agent_from,
        "context": context_text,
        "questions": [{
            "id": "answer",
            "type": qtype_flag,
            "prompt": prompt_text,
            "options": options,
        }],
    }
    single_mode = True
else:
    print("Provide either a prompt argument or --json", file=sys.stderr)
    sys.exit(2)

# Validate spec
if not isinstance(spec, dict) or "questions" not in spec:
    print("JSON spec must be an object with a 'questions' array", file=sys.stderr)
    sys.exit(2)

questions = spec["questions"]
if not isinstance(questions, list) or not questions:
    print("'questions' must be a non-empty array", file=sys.stderr)
    sys.exit(2)

VALID_TYPES = {"text", "password", "choice", "multi", "confirm"}
for i, q in enumerate(questions):
    if q.get("type") not in VALID_TYPES:
        print(f"Question {i}: type must be one of {sorted(VALID_TYPES)}", file=sys.stderr)
        sys.exit(2)
    if q["type"] in ("choice", "multi") and not q.get("options"):
        print(f"Question {i}: options required for type {q['type']}", file=sys.stderr)
        sys.exit(2)

try:
    import tkinter as tk
    from tkinter import ttk
    import tkinter.font as tkfont
except ImportError:
    print("tkinter is not available. Install: brew install python-tk (macOS) "
          "or apt install python3-tk (Linux).", file=sys.stderr)
    sys.exit(2)

# State the dialog writes to
result = {"answers": None, "cancelled": True}

root = tk.Tk()
root.title(f"{spec.get('from', 'Agent')} needs information")
root.minsize(640, 320)

# Native fonts: use TkDefaultFont and friends, never specify a family
default_font = tkfont.nametofont("TkDefaultFont")
heading_font = default_font.copy()
heading_font.configure(size=default_font.cget("size") + 4, weight="bold")
muted_font = default_font.copy()
muted_font.configure(size=max(default_font.cget("size") - 1, 10))

if platform.system() == "Darwin":
    root.lift()
    root.attributes("-topmost", True)
    root.after(150, lambda: root.attributes("-topmost", False))
    root.focus_force()

main = ttk.Frame(root, padding=24)
main.pack(fill=tk.BOTH, expand=True)

# Header: "<from> needs your input"
header = ttk.Label(
    main,
    text=f"{spec.get('from', 'Agent')} needs your input",
    font=heading_font,
)
header.pack(anchor=tk.W, pady=(0, 14))

# Shared context (above all questions, as flowing text)
shared_ctx = spec.get("context", "").strip()
if shared_ctx:
    ctx_label = ttk.Label(
        main, text=shared_ctx, wraplength=600, justify=tk.LEFT,
    )
    ctx_label.pack(anchor=tk.W, pady=(0, 4))
    ttk.Separator(main, orient=tk.HORIZONTAL).pack(fill=tk.X, pady=(12, 16))

# Render each question
question_widgets = []  # list of (id, getter_callable)

for i, q in enumerate(questions):
    qid = q.get("id", f"q{i}")
    qprompt = q["prompt"]
    qtype = q["type"]
    qopts = q.get("options", [])
    qctx = q.get("context", "").strip()

    qframe = ttk.Frame(main)
    qframe.pack(fill=tk.X, pady=(0, 16))

    # Per-question context (if provided)
    if qctx:
        qctx_label = ttk.Label(
            qframe, text=qctx, wraplength=600, justify=tk.LEFT, font=muted_font,
        )
        qctx_label.pack(anchor=tk.W, pady=(0, 4))

    # Question prompt
    qlabel = ttk.Label(qframe, text=qprompt, wraplength=600, justify=tk.LEFT)
    qlabel.pack(anchor=tk.W, pady=(0, 6))

    if qtype == "text":
        entry = ttk.Entry(qframe)
        entry.pack(fill=tk.X)
        if not question_widgets:
            entry.focus()
        question_widgets.append((qid, lambda e=entry: e.get()))

    elif qtype == "password":
        entry = ttk.Entry(qframe, show="*")
        entry.pack(fill=tk.X)
        if not question_widgets:
            entry.focus()
        question_widgets.append((qid, lambda e=entry: e.get()))

    elif qtype == "choice":
        var = tk.StringVar(value=qopts[0])
        for opt in qopts:
            ttk.Radiobutton(qframe, text=opt, value=opt, variable=var).pack(anchor=tk.W, pady=1)
        question_widgets.append((qid, lambda v=var: v.get()))

    elif qtype == "multi":
        vars_list = []
        for opt in qopts:
            v = tk.BooleanVar(value=False)
            ttk.Checkbutton(qframe, text=opt, variable=v).pack(anchor=tk.W, pady=1)
            vars_list.append((opt, v))
        question_widgets.append((qid, lambda vl=vars_list: ",".join(o for o, v in vl if v.get())))

    elif qtype == "confirm":
        var = tk.StringVar(value="yes")
        ttk.Radiobutton(qframe, text="Yes", value="yes", variable=var).pack(anchor=tk.W, pady=1)
        ttk.Radiobutton(qframe, text="No", value="no", variable=var).pack(anchor=tk.W, pady=1)
        question_widgets.append((qid, lambda v=var: v.get()))

# Buttons
def submit():
    answers = {}
    for qid, getter in question_widgets:
        answers[qid] = getter()
    result["answers"] = answers
    result["cancelled"] = False
    root.destroy()

def cancel():
    result["cancelled"] = True
    root.destroy()

btn_frame = ttk.Frame(main)
btn_frame.pack(fill=tk.X, pady=(8, 0))
ttk.Button(btn_frame, text="Cancel", command=cancel).pack(side=tk.LEFT)
ttk.Button(btn_frame, text="Submit", command=submit).pack(side=tk.RIGHT)

root.bind("<Escape>", lambda e: cancel())
root.protocol("WM_DELETE_WINDOW", cancel)

# Submit on Enter when there's only one text or password question
if single_mode and questions[0]["type"] in ("text", "password"):
    root.bind("<Return>", lambda e: submit())

root.mainloop()

if result["cancelled"]:
    print("CANCELLED", file=sys.stderr)
    sys.exit(1)

if single_mode:
    print(result["answers"]["answer"])
else:
    print(json.dumps(result["answers"]))
```
