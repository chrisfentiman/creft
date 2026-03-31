//! Tests for `creft add`, `creft rm`, and validation behavior.

mod helpers;

use helpers::{creft_env, creft_with, tool_available};
use predicates::prelude::*;
use rstest::rstest;

// Maps a language name to the tool used to validate it.
fn lang_to_tool(lang: &str) -> &str {
    match lang {
        "python" => "python3",
        other => other,
    }
}

// ── add tests ─────────────────────────────────────────────────────────────────

/// Piping a valid markdown command definition to `creft add` succeeds.
#[test]
fn test_add_from_stdin() {
    let dir = creft_env();
    creft_with(&dir)
        .args(["add"])
        .write_stdin("---\nname: hello\ndescription: greet\n---\n\n```bash\necho hello\n```\n")
        .assert()
        .success()
        .stderr(predicate::str::contains("added: hello"));
}

/// `creft add --name <name> --description <desc>` with piped frontmatter applies
/// the flag overrides. assert_cmd always pipes stdin so the is_terminal() branch
/// that builds entirely from flags is not reachable; this test exercises the
/// override path, which is the meaningful flag behavior.
#[test]
fn test_add_from_flags() {
    let dir = creft_env();
    // Pipe a base document; --name and --description override the frontmatter values.
    creft_with(&dir)
        .args(["add", "--name", "hello", "--description", "greet"])
        .write_stdin(
            "---\nname: placeholder\ndescription: placeholder\n---\n\n```bash\necho hello\n```\n",
        )
        .assert()
        .success()
        .stderr(predicate::str::contains("added: hello"));
}

/// Adding the same command twice without --force is rejected.
#[test]
fn test_add_duplicate_rejected() {
    let dir = creft_env();
    let markdown = "---\nname: hello\ndescription: greet\n---\n\n```bash\necho hello\n```\n";

    // First add succeeds.
    creft_with(&dir)
        .args(["add"])
        .write_stdin(markdown)
        .assert()
        .success();

    // Second add without --force fails.
    creft_with(&dir)
        .args(["add"])
        .write_stdin(markdown)
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("already exists"));
}

/// Adding the same command a second time with --force succeeds.
#[test]
fn test_add_force_overwrites() {
    let dir = creft_env();
    let markdown = "---\nname: hello\ndescription: greet\n---\n\n```bash\necho hello\n```\n";

    creft_with(&dir)
        .args(["add"])
        .write_stdin(markdown)
        .assert()
        .success();

    creft_with(&dir)
        .args(["add", "--force"])
        .write_stdin(markdown)
        .assert()
        .success();
}

// ── add validation tests ──────────────────────────────────────────────────────

/// Adding a skill that uses `{{undeclared}}` — a placeholder not listed in
/// `args` or `flags` — succeeds (exit code 0) but writes a warning to stderr.
/// The warning must mention "warning:" and the phrase "not declared in args or flags".
#[test]
fn test_add_with_undeclared_placeholder_warns() {
    let dir = creft_env();

    // The template references `{{undeclared}}` which is not listed in `args`.
    let markdown = "---\nname: warn-skill\ndescription: skill with undeclared placeholder\n---\n\n```bash\necho \"{{undeclared}}\"\n```\n";

    creft_with(&dir)
        .args(["add"])
        .write_stdin(markdown)
        // Save must succeed — validation warnings do not block the write.
        .assert()
        .success()
        .stderr(predicate::str::contains("warning:"))
        .stderr(predicate::str::contains("not declared in args or flags"));
}

/// Adding a skill where every placeholder matches a declared arg produces no
/// validation warnings. Exit code must be 0 and stderr must not contain "warning:".
#[test]
fn test_add_with_valid_placeholders_no_warnings() {
    let dir = creft_env();

    // `{{who}}` is declared in `args`, so no warning should be emitted.
    let markdown = "---\nname: valid-skill\ndescription: skill with valid placeholder\nargs:\n  - name: who\n    description: who to greet\n---\n\n```bash\necho \"Hello, {{who}}!\"\n```\n";

    creft_with(&dir)
        .args(["add"])
        .write_stdin(markdown)
        .assert()
        .success()
        // "added:" confirmation must appear.
        .stderr(predicate::str::contains("added: valid-skill"))
        // No validation warnings expected.
        .stderr(predicate::str::contains("warning:").not());
}

// ── syntax validation integration tests ──────────────────────────────────────

/// Adding a code block with a syntax error is rejected for each supported language.
#[rstest]
#[case::bash("bash", "if true; then\n  echo broken")]
#[case::python("python", "def foo():\nprint('bad indent')")]
#[case::node("node", "function foo() {\n  console.log('unclosed'")]
fn test_add_syntax_error_rejected(#[case] lang: &str, #[case] broken_body: &str) {
    if !tool_available(lang_to_tool(lang)) {
        println!("{} not available -- skipping", lang);
        return;
    }
    let dir = creft_env();
    let markdown = format!(
        "---\nname: bad-{lang}\ndescription: broken {lang}\n---\n\n```{lang}\n{broken_body}\n```\n"
    );
    creft_with(&dir)
        .args(["add"])
        .write_stdin(markdown.as_str())
        .assert()
        .failure()
        .stderr(predicate::str::contains("error:"));
}

/// Adding a code block with valid syntax succeeds for each supported language.
#[rstest]
#[case::bash("bash", "if true; then\n  echo ok\nfi")]
#[case::python("python", "def foo():\n    print('hello')")]
fn test_add_valid_syntax_succeeds(#[case] lang: &str, #[case] valid_body: &str) {
    if !tool_available(lang_to_tool(lang)) {
        println!("{} not available -- skipping", lang);
        return;
    }
    let dir = creft_env();
    let markdown = format!(
        "---\nname: good-{lang}\ndescription: valid {lang}\n---\n\n```{lang}\n{valid_body}\n```\n"
    );
    creft_with(&dir)
        .args(["add"])
        .write_stdin(markdown.as_str())
        .assert()
        .success()
        .stderr(predicate::str::contains(&format!("added: good-{lang}")));
}

/// Passing a skip-validation flag accepts broken bash syntax.
#[rstest]
#[case::force("--force", "force-bad-bash")]
#[case::no_validate("--no-validate", "novalidate-bad-bash")]
fn test_add_skip_validation_flag(#[case] flag: &str, #[case] skill_name: &str) {
    if !tool_available("bash") {
        println!("bash not available -- skipping");
        return;
    }
    let dir = creft_env();
    let markdown = format!(
        "---\nname: {skill_name}\ndescription: broken bash with skip flag\n---\n\n```bash\nif true; then\n  echo broken\n```\n"
    );
    creft_with(&dir)
        .args(["add", flag])
        .write_stdin(markdown.as_str())
        .assert()
        .success()
        .stderr(predicate::str::contains(&format!("added: {skill_name}")));
}

/// Piped `creft edit` with a syntax error in the new content is rejected.
/// Lives here (not in edit.rs) because it tests validation behavior.
#[test]
fn test_edit_piped_validates() {
    if !tool_available("bash") {
        println!("bash not available — skipping test_edit_piped_validates");
        return;
    }
    let dir = creft_env();

    // First add a valid skill.
    let valid_markdown =
        "---\nname: edit-target\ndescription: to be edited\n---\n\n```bash\necho original\n```\n";
    creft_with(&dir)
        .args(["add"])
        .write_stdin(valid_markdown)
        .assert()
        .success();

    // Now pipe broken content via edit — should be rejected.
    let broken_markdown = "---\nname: edit-target\ndescription: broken edit\n---\n\n```bash\nif true; then\n  echo broken\n```\n";
    creft_with(&dir)
        .args(["edit", "edit-target"])
        .write_stdin(broken_markdown)
        .assert()
        .failure()
        .stderr(predicate::str::contains("error:"));
}

/// Piped `creft edit --no-validate` accepts broken content.
/// Lives here (not in edit.rs) because it tests validation behavior.
#[test]
fn test_edit_piped_no_validate() {
    if !tool_available("bash") {
        println!("bash not available — skipping test_edit_piped_no_validate");
        return;
    }
    let dir = creft_env();

    // First add a valid skill.
    let valid_markdown = "---\nname: edit-novalidate-target\ndescription: to be edited\n---\n\n```bash\necho original\n```\n";
    creft_with(&dir)
        .args(["add"])
        .write_stdin(valid_markdown)
        .assert()
        .success();

    // Now pipe broken content via edit --no-validate — should succeed.
    let broken_markdown = "---\nname: edit-novalidate-target\ndescription: broken edit\n---\n\n```bash\nif true; then\n  echo broken\n```\n";
    creft_with(&dir)
        .args(["edit", "edit-novalidate-target", "--no-validate"])
        .write_stdin(broken_markdown)
        .assert()
        .success()
        .stderr(predicate::str::contains("edited: edit-novalidate-target"));
}

/// A skill block whose language interpreter is not available is accepted silently.
/// We simulate this by using a fictional language tag that maps to no checker.
#[test]
fn test_add_missing_interpreter_skips_check() {
    let dir = creft_env();
    // "cobol" is not in the supported set — check is silently skipped.
    // The code would be a syntax error in COBOL, but creft doesn't validate it.
    let markdown = "---\nname: cobol-skill\ndescription: a cobol skill\n---\n\n```cobol\nTHIS IS DEFINITELY NOT VALID COBOL SYNTAX !!!@#$\n```\n";

    creft_with(&dir)
        .args(["add"])
        .write_stdin(markdown)
        .assert()
        .success()
        .stderr(predicate::str::contains("added: cobol-skill"));
}

/// A bash block containing template placeholders does not cause false syntax
/// errors — the placeholders are replaced with `__CREFT_PH__` before checking.
#[test]
fn test_add_placeholders_dont_cause_false_syntax_errors() {
    if !tool_available("bash") {
        println!(
            "bash not available — skipping test_add_placeholders_dont_cause_false_syntax_errors"
        );
        return;
    }
    let dir = creft_env();
    let markdown = "---\nname: ph-bash\ndescription: bash with placeholder\nargs:\n  - name: repo\n    description: repo name\n---\n\n```bash\necho \"hello {{repo}}\"\n```\n";

    creft_with(&dir)
        .args(["add"])
        .write_stdin(markdown)
        .assert()
        .success()
        .stderr(predicate::str::contains("added: ph-bash"));
}

/// `docs` blocks are not syntax-checked. A docs block with arbitrary content
/// (including things that look like broken code) must not cause failures.
#[test]
fn test_add_docs_blocks_not_validated() {
    let dir = creft_env();
    // The docs block contains markdown and code-like text that would fail if
    // validated as bash, but it should be skipped entirely.
    let markdown = "---\nname: docs-skill\ndescription: skill with docs block\n---\n\n```docs\n# Usage\n\nRun it like: `if broken then`\n```\n\n```bash\necho hello\n```\n";

    creft_with(&dir)
        .args(["add"])
        .write_stdin(markdown)
        .assert()
        .success()
        .stderr(predicate::str::contains("added: docs-skill"));
}

// ── description length warning integration tests ─────────────────────────────

/// `creft add` with a description over 80 chars warns on stderr but still succeeds.
#[test]
fn test_add_long_description_warns() {
    let dir = creft_env();
    // 100-character description — over the 80-char threshold.
    let long_desc = "a".repeat(100);
    let markdown = format!(
        "---\nname: long-desc-skill\ndescription: {long_desc}\n---\n\n```bash\necho hello\n```\n"
    );

    creft_with(&dir)
        .args(["add"])
        .write_stdin(markdown.as_str())
        // Operation must succeed — the warning doesn't block.
        .assert()
        .success()
        .stderr(predicate::str::contains("added: long-desc-skill"))
        .stderr(predicate::str::contains("description is long"));
}

/// `creft add` with a description under 80 chars does NOT emit a length warning.
#[test]
fn test_add_short_description_no_length_warning() {
    let dir = creft_env();
    let markdown = "---\nname: short-desc-skill\ndescription: Short description\n---\n\n```bash\necho hi\n```\n";

    creft_with(&dir)
        .args(["add"])
        .write_stdin(markdown)
        .assert()
        .success()
        .stderr(predicate::str::contains("description is long").not());
}

/// `creft add --no-validate` with a long description does NOT warn (validation skipped).
#[test]
fn test_add_no_validate_skips_description_warning() {
    let dir = creft_env();
    let long_desc = "a".repeat(100);
    let markdown = format!(
        "---\nname: novalidate-long\ndescription: {long_desc}\n---\n\n```bash\necho hi\n```\n"
    );

    creft_with(&dir)
        .args(["add", "--no-validate"])
        .write_stdin(markdown.as_str())
        .assert()
        .success()
        .stderr(predicate::str::contains("description is long").not());
}

/// `creft add --force` with a long description does NOT warn (validation skipped).
#[test]
fn test_add_force_skips_description_warning() {
    let dir = creft_env();
    let long_desc = "a".repeat(100);
    let markdown = format!(
        "---\nname: force-long-desc\ndescription: {long_desc}\n---\n\n```bash\necho hi\n```\n"
    );

    creft_with(&dir)
        .args(["add", "--force"])
        .write_stdin(markdown.as_str())
        .assert()
        .success()
        .stderr(predicate::str::contains("description is long").not());
}

// ── error case tests ──────────────────────────────────────────────────────────

/// Trying to add a command named after a reserved built-in is rejected.
#[test]
fn test_reserved_name_rejected() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["add"])
        .write_stdin("---\nname: add\ndescription: shadow add\n---\n\n```bash\necho oops\n```\n")
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("reserved"));
}

// ── rm tests ──────────────────────────────────────────────────────────────────

/// Adding a command and then removing it succeeds. After removal, list is empty.
#[test]
fn test_rm_command() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["add"])
        .write_stdin("---\nname: hello\ndescription: greet\n---\n\n```bash\necho hello\n```\n")
        .assert()
        .success();

    creft_with(&dir).args(["rm", "hello"]).assert().success();

    // After removal the list is empty.
    creft_with(&dir)
        .args(["list"])
        .assert()
        .success()
        .stderr(predicate::str::contains("no commands found"));
}

/// `creft rm nonexistent` exits with code 2 (CommandNotFound).
#[test]
fn test_rm_not_found() {
    let dir = creft_env();
    creft_with(&dir)
        .args(["rm", "nonexistent"])
        .assert()
        .failure()
        .code(2);
}

// ── sub-skill existence warning tests ────────────────────────────────────────

/// `creft add` warns when a shell block references a creft sub-skill that doesn't exist.
/// The warning must appear on stderr but the skill must still be added (exit 0).
#[test]
fn test_add_warns_on_missing_sub_skill() {
    let dir = creft_env();
    creft_with(&dir)
        .args(["add"])
        .write_stdin(
            "---\nname: sub-skill-caller\ndescription: calls a missing sub-skill\n---\n\n\
             ```bash\ncreft nonexistent-skill-xyzzy\n```\n",
        )
        .assert()
        .success()
        .stderr(
            predicate::str::contains("nonexistent-skill-xyzzy").and(predicate::str::contains(
                "not found (referenced as creft sub-skill)",
            )),
        );
}

/// `creft add --force` with a missing sub-skill produces no sub-skill warning.
#[test]
fn test_add_no_sub_skill_warn_with_force() {
    let dir = creft_env();
    creft_with(&dir)
        .args(["add", "--force"])
        .write_stdin(
            "---\nname: sub-skill-force\ndescription: calls a missing sub-skill\n---\n\n\
             ```bash\ncreft nonexistent-skill-xyzzy\n```\n",
        )
        .assert()
        .success()
        .stderr(predicate::str::contains("not found (referenced as creft sub-skill)").not());
}

/// Add skill A, then add skill B that references `creft A`. No sub-skill warning for B.
#[test]
fn test_add_sub_skill_found_no_warn() {
    let dir = creft_env();
    // Add skill A first.
    creft_with(&dir)
        .args(["add"])
        .write_stdin(
            "---\nname: skill-a-exists\ndescription: skill a\n---\n\n```bash\necho a\n```\n",
        )
        .assert()
        .success();

    // Add skill B that references creft skill-a-exists.
    creft_with(&dir)
        .args(["add"])
        .write_stdin(
            "---\nname: skill-b-caller\ndescription: calls skill a\n---\n\n\
             ```bash\ncreft skill-a-exists\n```\n",
        )
        .assert()
        .success()
        .stderr(predicate::str::contains("not found (referenced as creft sub-skill)").not());
}

// ── dependency resolution integration tests ───────────────────────────────────

/// A skill with `# deps: requests` in a python block produces no dep warning —
/// `requests` is a real, stable PyPI package. Network-gated.
#[test]
fn test_add_python_dep_found() {
    if !helpers::network_available() {
        eprintln!("skipping test_add_python_dep_found: no network");
        return;
    }
    let dir = creft_env();
    let skill = "---\nname: py-dep-found\ndescription: python with valid dep\n---\n\n\
                 ```python\n# deps: requests\nimport requests\n```\n";
    creft_with(&dir)
        .args(["add"])
        .write_stdin(skill)
        .assert()
        .success()
        .stderr(predicate::str::contains("not found on PyPI").not());
}

/// A skill with `# deps: zzz-nonexistent-pkg-12345` in a python block produces
/// a dep warning on stderr. Still exits 0. Network-gated.
#[test]
fn test_add_python_dep_not_found() {
    if !helpers::network_available() {
        eprintln!("skipping test_add_python_dep_not_found: no network");
        return;
    }
    let dir = creft_env();
    let skill = "---\nname: py-dep-missing\ndescription: python with bad dep\n---\n\n\
                 ```python\n# deps: zzz-nonexistent-pkg-12345\npass\n```\n";
    creft_with(&dir)
        .args(["add"])
        .write_stdin(skill)
        .assert()
        .success()
        .stderr(predicate::str::contains("not found on PyPI"));
}

/// A skill with `# deps: lodash` in a node block produces no dep warning —
/// `lodash` is a real, stable npm package. Network-gated.
#[test]
fn test_add_node_dep_found() {
    if !helpers::network_available() {
        eprintln!("skipping test_add_node_dep_found: no network");
        return;
    }
    let dir = creft_env();
    let skill = "---\nname: node-dep-found\ndescription: node with valid dep\n---\n\n\
                 ```node\n// deps: lodash\nconst _ = require('lodash');\n```\n";
    creft_with(&dir)
        .args(["add"])
        .write_stdin(skill)
        .assert()
        .success()
        .stderr(predicate::str::contains("not found on npm").not());
}

/// A skill with `# deps: zzz-nonexistent-pkg-12345` in a node block produces
/// a dep warning on stderr. Still exits 0. Network-gated.
#[test]
fn test_add_node_dep_not_found() {
    if !helpers::network_available() {
        eprintln!("skipping test_add_node_dep_not_found: no network");
        return;
    }
    let dir = creft_env();
    let skill = "---\nname: node-dep-missing\ndescription: node with bad dep\n---\n\n\
                 ```node\n// deps: zzz-nonexistent-pkg-12345\nconsole.log('hi');\n```\n";
    creft_with(&dir)
        .args(["add"])
        .write_stdin(skill)
        .assert()
        .success()
        .stderr(predicate::str::contains("not found on npm"));
}

/// A skill with a shell block and a declared dep not on PATH produces a warning
/// on stderr but still exits 0.
#[test]
fn test_add_warns_on_missing_shell_dep() {
    let dir = creft_env();
    let skill = "---\nname: shell-dep-missing\ndescription: shell with bad dep\n---\n\n\
                 ```bash\n# deps: __nonexistent_dep_xyzzy__\necho hello\n```\n";
    creft_with(&dir)
        .args(["add"])
        .write_stdin(skill)
        .assert()
        .success()
        .stderr(predicate::str::contains("not found on PATH"));
}

/// With `--no-validate`, dep warnings are skipped entirely.
#[test]
fn test_add_dep_warn_skipped_with_no_validate() {
    let dir = creft_env();
    let skill = "---\nname: dep-novalidate\ndescription: shell dep no-validate\n---\n\n\
                 ```bash\n# deps: __nonexistent_dep_xyzzy__\necho hello\n```\n";
    creft_with(&dir)
        .args(["add", "--no-validate"])
        .write_stdin(skill)
        .assert()
        .success()
        .stderr(predicate::str::contains("not found on PATH").not());
}
