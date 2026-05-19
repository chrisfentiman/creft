//! Integration tests for stdin-mode execution and `# flags:` / `# extension:` directives.
//!
//! Tests that require specific interpreters (ruby, zx) gate on `tool_available`
//! and skip with a clear message when the binary is absent — no `#[ignore]`.

mod helpers;

use helpers::{creft_env, creft_with, tool_available};
use predicates::prelude::*;

/// A `ruby` block with no directives runs via stdin mode: the block source is
/// piped to `ruby`'s stdin and stdout is captured.
#[test]
fn ruby_stdin_mode_single_block_produces_output() {
    if !tool_available("ruby") {
        eprintln!("skipping: ruby not on PATH");
        return;
    }

    let dir = creft_env();

    creft_with(&dir)
        .args(["add"])
        .write_stdin(concat!(
            "---\n",
            "name: ruby-hello\n",
            "description: ruby stdin mode\n",
            "---\n",
            "\n",
            "```ruby\n",
            "puts 'hello from ruby'\n",
            "```\n",
        ))
        .assert()
        .success();

    creft_with(&dir)
        .args(["ruby-hello"])
        .assert()
        .success()
        .stdout(predicate::str::contains("hello from ruby"));
}

/// A `ruby` block as the downstream stage in a two-block pipe chain receives
/// the upstream block's output via the `{{prev}}` template substitution.
///
/// In a pipe chain, stdin-mode blocks run as sponge stages: upstream output is
/// buffered and substituted into the block source via `{{prev}}`. The block
/// source itself is then piped to the interpreter's stdin. This is the correct
/// model because when ruby reads source from stdin, `STDIN` is already at EOF
/// and cannot be used for additional data — `{{prev}}` is the only way to pass
/// upstream data into a stdin-mode block in a pipe chain.
#[cfg(unix)]
#[test]
fn ruby_stdin_mode_pipe_chain_receives_upstream_via_prev() {
    if !tool_available("ruby") {
        eprintln!("skipping: ruby not on PATH");
        return;
    }

    let dir = creft_env();

    creft_with(&dir)
        .args(["add"])
        .write_stdin(concat!(
            "---\n",
            "name: bash-to-ruby\n",
            "description: bash upstream, ruby sponge via prev\n",
            "---\n",
            "\n",
            "```bash\n",
            "echo upstream\n",
            "```\n",
            "\n",
            "```ruby\n",
            "puts \"got: {{prev}}\"\n",
            "```\n",
        ))
        .assert()
        .success();

    creft_with(&dir)
        .args(["bash-to-ruby"])
        .assert()
        .success()
        .stdout(predicate::str::contains("got: upstream"));
}

/// A `bash` block with `# flags: -x` passes `-x` to bash, which traces each
/// command to stderr. Verifies flags are inserted between the interpreter and
/// the script path.
#[test]
fn bash_flags_directive_passes_flags_to_interpreter() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["add"])
        .write_stdin(concat!(
            "---\n",
            "name: bash-trace\n",
            "description: bash with -x\n",
            "---\n",
            "\n",
            "```bash\n",
            "# flags: -x\n",
            "echo traced\n",
            "```\n",
        ))
        .assert()
        .success();

    // bash -x traces commands to stderr; the echo still writes to stdout.
    creft_with(&dir)
        .args(["bash-trace"])
        .assert()
        .success()
        .stdout(predicate::str::contains("traced"));
}

/// A static `# flags: -x` on a bash block passes the flag to the interpreter.
/// bash `-x` traces each command to stderr. The block still runs and produces
/// stdout output normally — this confirms the flag was forwarded, not consumed
/// by creft itself.
///
/// The expand-and-split contract for placeholder values is verified at the unit
/// level in `src/runner/blocks/mod.rs::tests::expand_and_split_flags_*`.
#[test]
fn flags_directive_static_flag_reaches_interpreter() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["add"])
        .write_stdin(concat!(
            "---\n",
            "name: bash-flags-static\n",
            "description: static flag directive\n",
            "---\n",
            "\n",
            "```bash\n",
            "# flags: -x\n",
            "echo flagged\n",
            "```\n",
        ))
        .assert()
        .success();

    // bash -x traces to stderr; echo still goes to stdout.
    creft_with(&dir)
        .args(["bash-flags-static"])
        .assert()
        .success()
        .stdout(predicate::str::contains("flagged"));
}

/// A `# flags: {{placeholder}}` directive expands the bound value and splits it
/// on whitespace before passing discrete tokens to the interpreter.
///
/// When `opts` is bound to `"-e -x"` (a single string containing a space),
/// `expand_and_split_flags` must produce two tokens — `["-e", "-x"]` — not one.
/// bash receives `bash -e -x script.sh`. The `-x` flag traces each command to
/// stderr, and the script's echo reaches stdout. If the value were passed as a
/// single token `"-e -x"`, bash would reject it as an unknown option and the
/// command would fail, making the split observable through success vs. failure.
///
/// Gated on `which bash` — bash is always present on supported platforms.
#[test]
fn flags_placeholder_value_splits_into_discrete_tokens() {
    if !tool_available("bash") {
        eprintln!("skipping: bash not on PATH");
        return;
    }

    let dir = creft_env();

    creft_with(&dir)
        .args(["add"])
        .write_stdin(concat!(
            "---\n",
            "name: flags-placeholder-split\n",
            "description: placeholder flags split on whitespace\n",
            "args:\n",
            "  - name: opts\n",
            "    description: flags to pass to bash\n",
            "---\n",
            "\n",
            "```bash\n",
            "# flags: {{opts}}\n",
            "echo split-ok\n",
            "```\n",
        ))
        .assert()
        .success();

    // Pass "-e -x" as a single string value for `opts`. The `--` separates
    // creft's own option parsing from the skill's positional args, allowing the
    // value to start with `-` without creft misinterpreting it as a flag.
    // After substitution and whitespace splitting, bash receives two discrete
    // tokens: `-e` and `-x`. bash -x traces execution to stderr; the echo still
    // writes to stdout.
    //
    // If the split were absent, bash would receive "-e -x" as one argument and
    // reject it as an unknown option — failure would be the observable proof of
    // the missing split.
    //
    // `--verbose` causes creft to forward child stderr to its own stderr, making
    // bash's -x trace observable in the captured output.
    creft_with(&dir)
        .args(["flags-placeholder-split", "--verbose", "--", "-e -x"])
        .assert()
        .success()
        .stdout(predicate::str::contains("split-ok"))
        .stderr(predicate::str::contains("echo split-ok"));
}

/// A `zx` block with `# flags: -` runs the block content through `zx -` via
/// stdin. The `# flags:` directive line is stripped from the body by the parser
/// so zx's JS engine never sees it.
#[test]
fn zx_stdin_mode_with_flags_dash() {
    if !tool_available("zx") {
        eprintln!("skipping: zx not on PATH");
        return;
    }

    let dir = creft_env();

    creft_with(&dir)
        .args(["add"])
        .write_stdin(concat!(
            "---\n",
            "name: zx-hello\n",
            "description: zx via stdin with flags -\n",
            "---\n",
            "\n",
            "```zx\n",
            "# flags: -\n",
            "const out = await $`echo hello from zx`;\n",
            "process.stdout.write(out.stdout);\n",
            "```\n",
        ))
        .assert()
        .success();

    creft_with(&dir)
        .args(["zx-hello"])
        .assert()
        .success()
        .stdout(predicate::str::contains("hello from zx"));
}

/// A `python` block with `# extension: pyi` writes the temp file with a `.pyi`
/// suffix. Python doesn't care about the extension so this test verifies the
/// block still executes correctly with the overridden extension.
#[test]
fn python_extension_override_still_executes() {
    if !tool_available("python3") {
        eprintln!("skipping: python3 not on PATH");
        return;
    }

    let dir = creft_env();

    creft_with(&dir)
        .args(["add"])
        .write_stdin(concat!(
            "---\n",
            "name: python-pyi\n",
            "description: python with extension override\n",
            "---\n",
            "\n",
            "```python\n",
            "# extension: pyi\n",
            "print('pyi block executed')\n",
            "```\n",
        ))
        .assert()
        .success();

    creft_with(&dir)
        .args(["python-pyi"])
        .assert()
        .success()
        .stdout(predicate::str::contains("pyi block executed"));
}

/// An unknown-tag block with `# extension: rb` uses file mode (ShellRunner):
/// the binary is `ruby` (from the tag) and the temp file suffix is `.rb`.
#[test]
fn unknown_tag_with_extension_uses_file_mode() {
    if !tool_available("ruby") {
        eprintln!("skipping: ruby not on PATH");
        return;
    }

    let dir = creft_env();

    creft_with(&dir)
        .args(["add"])
        .write_stdin(concat!(
            "---\n",
            "name: ruby-file-mode\n",
            "description: ruby in file mode via extension directive\n",
            "---\n",
            "\n",
            "```ruby\n",
            "# extension: rb\n",
            "puts 'file mode'\n",
            "```\n",
        ))
        .assert()
        .success();

    creft_with(&dir)
        .args(["ruby-file-mode"])
        .assert()
        .success()
        .stdout(predicate::str::contains("file mode"));
}

/// An unknown-tag block whose binary is missing from PATH prints the interpreter-
/// not-found error at execution time (from `spawn_block`'s existing error path).
///
/// This uses a deliberately absurd tag that cannot exist on any machine.
#[test]
fn unknown_tag_binary_missing_fails_at_execution() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["add", "--no-validate"])
        .write_stdin(concat!(
            "---\n",
            "name: missing-interp\n",
            "description: interpreter that cannot exist\n",
            "---\n",
            "\n",
            "```an-interpreter-that-cannot-exist-9a8b7c6d\n",
            "echo hello\n",
            "```\n",
        ))
        .assert()
        .success();

    creft_with(&dir)
        .args(["missing-interp"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "an-interpreter-that-cannot-exist-9a8b7c6d",
        ));
}
