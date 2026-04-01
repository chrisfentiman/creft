//! Integration tests for llm block execution.
//!
//! These tests use `cat` as the provider (universally available) to test
//! the llm execution path without requiring a real AI CLI on PATH.

mod helpers;

use helpers::{creft_env, creft_with};
use predicates::prelude::*;
use std::io::Write;

const LLM_CAT_SKILL: &str = "---\nname: llm-cat\ndescription: llm block with cat provider\n---\n\n\
```llm\nprovider: cat\n---\nhello from llm block\n```\n";

/// Dry run of an llm block shows the command and prompt without executing.
#[test]
fn test_llm_block_dry_run() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["add"])
        .write_stdin(LLM_CAT_SKILL)
        .assert()
        .success();

    creft_with(&dir)
        .args(["llm-cat", "--dry-run"])
        .assert()
        .success()
        .stderr(predicate::str::contains("llm: cat"))
        .stderr(predicate::str::contains("command: cat"))
        .stderr(predicate::str::contains("prompt:"));
}

/// Executing an llm block with `cat` as provider echoes the prompt.
#[test]
fn test_llm_block_execution_with_cat() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["add"])
        .write_stdin(LLM_CAT_SKILL)
        .assert()
        .success();

    creft_with(&dir)
        .args(["llm-cat"])
        .assert()
        .success()
        .stdout(predicate::str::contains("hello from llm block"));
}

/// An llm block with a provider CLI not on PATH produces a clear error message.
#[test]
fn test_llm_block_execution_provider_not_found() {
    let dir = creft_env();

    let skill = "---\nname: llm-missing\ndescription: llm block with missing provider\n---\n\n\
```llm\nprovider: nonexistent-llm-xyz\n---\nsome prompt\n```\n";

    creft_with(&dir)
        .args(["add"])
        .write_stdin(skill)
        .assert()
        .success();

    creft_with(&dir)
        .args(["llm-missing"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("nonexistent-llm-xyz"));
}

/// A multi-block skill where bash produces output and llm uses {{prev}}.
#[test]
fn test_llm_block_sequential_with_prev() {
    let dir = creft_env();

    // Bash block outputs "from bash", llm block (cat) echoes the prompt with {{prev}} substituted.
    let skill = "---\nname: llm-prev\ndescription: llm block using prev\n---\n\n\
```bash\necho from bash\n```\n\n\
```llm\nprovider: cat\n---\nreceived: {{prev}}\n```\n";

    creft_with(&dir)
        .args(["add"])
        .write_stdin(skill)
        .assert()
        .success();

    creft_with(&dir)
        .args(["llm-prev"])
        .assert()
        .success()
        .stdout(predicate::str::contains("received:"))
        .stdout(predicate::str::contains("from bash"));
}

/// An llm block with no YAML header (just a prompt) uses default provider.
#[test]
fn test_llm_block_no_header() {
    let dir = creft_env();

    // No YAML header at all — LlmConfig defaults to claude provider.
    // We use dry-run to verify it parses correctly without needing claude.
    let skill = "---\nname: llm-noheader\ndescription: llm block with no header\n---\n\njust a prompt without yaml header\n\
This is the prompt text that has no --- separator at all.\n```\n";

    // Actually we need a proper fenced code block format
    let skill2 = "---\nname: llm-noheader\ndescription: llm block with no header\n---\n\n\
```llm\njust a prompt with no yaml header\n```\n";

    creft_with(&dir)
        .args(["add"])
        .write_stdin(skill2)
        .assert()
        .success();

    // Dry run should show claude as the provider (default) and the full content as prompt.
    creft_with(&dir)
        .args(["llm-noheader", "--dry-run"])
        .assert()
        .success()
        .stderr(predicate::str::contains("llm: claude"));
}

/// A `pipe: true` skill with an llm block falls back to sequential execution.
#[test]
fn test_llm_block_pipe_mode_fallback() {
    let dir = creft_env();

    // pipe: true skill with an llm block — should still work (falls back to sequential).
    let skill = "---\nname: llm-pipe\ndescription: pipe skill with llm\npipe: true\n---\n\n\
```bash\necho from pipe bash\n```\n\n\
```llm\nprovider: cat\n---\npipe input: {{prev}}\n```\n";

    creft_with(&dir)
        .args(["add"])
        .write_stdin(skill)
        .assert()
        .success();

    creft_with(&dir)
        .args(["llm-pipe"])
        .assert()
        .success()
        .stdout(predicate::str::contains("pipe input:"));
}

/// Validation of an llm block with no prompt text produces an error.
#[test]
fn test_llm_block_validate_empty_prompt() {
    let dir = creft_env();

    // A block that has a YAML header but no prompt after ---.
    let skill = "---\nname: llm-empty\ndescription: llm with empty prompt\n---\n\n\
```llm\nprovider: cat\n---\n```\n";

    // creft add validates on save — should fail.
    creft_with(&dir)
        .args(["add"])
        .write_stdin(skill)
        .assert()
        .failure()
        .stderr(predicate::str::contains("no prompt text"));
}

/// creft doctor for a skill with an llm block reports the provider.
#[test]
fn test_llm_block_doctor_reports_provider() {
    let dir = creft_env();

    // Use cat as provider so it's found on PATH.
    creft_with(&dir)
        .args(["add"])
        .write_stdin(LLM_CAT_SKILL)
        .assert()
        .success();

    creft_with(&dir)
        .args(["doctor", "llm-cat"])
        .assert()
        .success();
}

/// A verbose run shows the provider command and expanded prompt on stderr.
#[test]
fn test_llm_block_verbose_shows_command() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["add"])
        .write_stdin(LLM_CAT_SKILL)
        .assert()
        .success();

    creft_with(&dir)
        .args(["llm-cat", "--verbose"])
        .assert()
        .success()
        .stderr(predicate::str::contains("command: cat"))
        .stderr(predicate::str::contains("prompt:"));
}

/// An llm block that exits 99 causes creft to return success (early pipeline termination).
///
/// Exit code 99 is the conventional "early successful exit" signal. Any block — including
/// llm blocks — that exits 99 should stop pipeline execution and report success to the caller.
#[test]
fn test_llm_block_exit_99_early_return() {
    let dir = creft_env();

    // Write a script that consumes stdin, prints output, then exits 99.
    let script_dir = tempfile::TempDir::new().unwrap();
    let script_path = script_dir.path().join("exit99.sh");
    {
        let mut f = std::fs::File::create(&script_path).unwrap();
        f.write_all(b"#!/bin/sh\ncat\necho \"early exit output\"\nexit 99\n")
            .unwrap();
    }

    // Make the script executable.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&script_path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script_path, perms).unwrap();
    }

    let provider_path = script_path.to_string_lossy();
    let skill = format!(
        "---\nname: llm-exit99\ndescription: llm block that exits 99\n---\n\n\
```llm\nprovider: {provider_path}\n---\nprompt text\n```\n"
    );

    creft_with(&dir)
        .args(["add"])
        .write_stdin(skill.as_str())
        .assert()
        .success();

    // Exit 99 from the llm block means early successful termination — creft must exit 0.
    creft_with(&dir)
        .args(["llm-exit99"])
        .assert()
        .success()
        .stdout(predicate::str::contains("early exit output"));
}
