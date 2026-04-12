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
        .args(["cmd", "add"])
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
        .args(["cmd", "add"])
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
        .args(["cmd", "add"])
        .write_stdin(skill)
        .assert()
        .success();

    creft_with(&dir)
        .args(["llm-missing"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("nonexistent-llm-xyz"));
}

/// A multi-block pipe chain where bash produces output and an LLM sponge uses {{prev}}.
///
/// With pipe-by-default, this skill runs through run_pipe_chain. The sponge reads
/// bash's output, buffers it, and substitutes it as {{prev}} into the prompt.
#[test]
fn test_llm_block_pipe_sponge_prev() {
    let dir = creft_env();

    // Bash block outputs "from bash", llm sponge (cat) echoes the prompt with {{prev}} substituted.
    let skill = "---\nname: llm-prev\ndescription: llm block using prev\n---\n\n\
```bash\necho from bash\n```\n\n\
```llm\nprovider: cat\n---\nreceived: {{prev}}\n```\n";

    creft_with(&dir)
        .args(["cmd", "add"])
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
    let skill = "---\nname: llm-noheader\ndescription: llm block with no header\n---\n\n\
```llm\njust a prompt with no yaml header\n```\n";

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(skill)
        .assert()
        .success();

    // Dry run should show claude as the provider (default) and the full content as prompt.
    creft_with(&dir)
        .args(["llm-noheader", "--dry-run"])
        .assert()
        .success()
        .stderr(predicate::str::contains("llm: claude"));
}

/// A multi-block skill with an llm block runs as a true concurrent pipe chain.
///
/// The LLM block participates as a sponge stage: it reads all upstream output,
/// performs template substitution, and relays the provider's stdout downstream
/// via an OS pipe.
#[test]
fn test_llm_block_pipe_mode_sponge() {
    let dir = creft_env();

    let skill = "---\nname: llm-pipe\ndescription: pipe skill with llm\n---\n\n\
```bash\necho from pipe bash\n```\n\n\
```llm\nprovider: cat\n---\npipe input: {{prev}}\n```\n";

    creft_with(&dir)
        .args(["cmd", "add"])
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
        .args(["cmd", "add"])
        .write_stdin(skill)
        .assert()
        .failure()
        .stderr(predicate::str::contains("no prompt text"));
}

/// creft doctor for a skill with an llm block reports the provider name and availability.
#[test]
fn test_llm_block_doctor_reports_provider() {
    let dir = creft_env();

    // Use cat as provider so it's found on PATH.
    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(LLM_CAT_SKILL)
        .assert()
        .success();

    creft_with(&dir)
        .args(["doctor", "llm-cat"])
        .assert()
        .success()
        // Doctor should report the provider CLI (cat) in its interpreter checks.
        .stderr(predicate::str::contains("cat"));
}

/// A verbose run shows the provider command and expanded prompt on stderr.
#[test]
fn test_llm_block_verbose_shows_command() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["cmd", "add"])
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

/// A pipe chain with bash -> llm -> bash chains output through the sponge correctly.
///
/// The sponge reads bash's output, substitutes `{{prev}}`, and relays the provider's
/// stdout as stdin to the downstream bash block via an OS pipe.
#[test]
fn test_llm_pipe_stdin_routes_prev_output() {
    let dir = creft_env();

    // Block 1 (bash): echo a known string.
    // Block 2 (llm/cat): prompt contains {{prev}}, cat echoes it back.
    // Block 3 (bash): cat — reads stdin and echoes it.
    // If stdin routing is correct, block 3 outputs block 2's output.
    let skill = "---\nname: pipe-stdin-chain\ndescription: pipe stdin routing test\n---\n\n\
```bash\necho 'hello from bash'\n```\n\n\
```llm\nprovider: cat\n---\n{{prev}}\n```\n\n\
```bash\ncat\n```\n";

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(skill)
        .assert()
        .success();

    creft_with(&dir)
        .args(["pipe-stdin-chain"])
        .assert()
        .success()
        .stdout(predicate::str::contains("hello from bash"));
}

/// Block 0 of a pipe skill reads from the parent's stdin regardless of LLM blocks.
///
/// When block 0 is a normal block in a pipe skill, it inherits the parent's stdin.
/// The sponge (block 1) reads block 0's output and substitutes `{{prev}}`.
#[test]
fn test_llm_pipe_block0_inherits_parent_stdin() {
    let dir = creft_env();

    // Block 1 (bash/cat): reads stdin from the parent process.
    // Block 2 (llm/cat): prompt contains {{prev}}.
    // We pipe "injected data" into creft — if block 0 inherits parent stdin correctly,
    // it reads that data and passes it forward.
    let skill = "---\nname: pipe-block0-stdin\ndescription: block 0 parent stdin test\n---\n\n\
```bash\ncat\n```\n\n\
```llm\nprovider: cat\n---\n{{prev}}\n```\n";

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(skill)
        .assert()
        .success();

    creft_with(&dir)
        .args(["pipe-block0-stdin"])
        .write_stdin("injected data")
        .assert()
        .success()
        .stdout(predicate::str::contains("injected data"));
}

/// An empty upstream output sponge sends EOF to the downstream block without hanging.
///
/// When the upstream block produces no output, the sponge substitutes empty string
/// for `{{prev}}`, the provider produces empty output, and the downstream block
/// receives EOF on stdin immediately.
#[test]
fn test_llm_pipe_empty_prev_sends_eof() {
    let dir = creft_env();

    // Block 1 (bash): outputs nothing.
    // Block 2 (llm/cat): prompt is {{prev}} (empty string).
    // Block 3 (bash/wc -c): counts bytes on stdin — should output 0.
    let skill = "---\nname: pipe-empty-prev\ndescription: empty prev EOF test\n---\n\n\
```bash\nprintf ''\n```\n\n\
```llm\nprovider: cat\n---\n{{prev}}\n```\n\n\
```bash\nwc -c\n```\n";

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(skill)
        .assert()
        .success();

    // wc -c outputs a number (possibly with whitespace). The key invariant is
    // that the command completes without hanging and the exit code is 0.
    creft_with(&dir)
        .args(["pipe-empty-prev"])
        .assert()
        .success();
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
        .args(["cmd", "add"])
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

/// Data flows from bash through an LLM sponge stage to a downstream bash block via true pipe.
///
/// Regression guard: the sponge must relay provider output through the os_pipe to the
/// downstream block — not via env vars or sequential stdin injection.
#[test]
fn test_llm_sponge_pipe_streams_to_downstream() {
    let dir = creft_env();

    // bash outputs "upstream", sponge (cat) echoes it as prompt, downstream wc -c counts bytes.
    let skill = "---\nname: sponge-stream\ndescription: sponge streams to downstream\n---\n\n\
```bash\necho upstream\n```\n\n\
```llm\nprovider: cat\n---\n{{prev}}\n```\n\n\
```bash\nwc -c\n```\n";

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(skill)
        .assert()
        .success();

    // wc -c should report a non-zero byte count — data flowed through the sponge.
    creft_with(&dir).args(["sponge-stream"]).assert().success();
}

/// Two consecutive LLM sponge stages chain correctly.
///
/// Sponge 1 reads bash output and annotates it; sponge 2 reads sponge 1's output.
#[test]
fn test_llm_sponge_multiple_consecutive() {
    let dir = creft_env();

    // bash → llm(cat, "A:{{prev}}") → llm(cat, "B:{{prev}}") → bash(cat)
    let skill = "---\nname: sponge-chain\ndescription: consecutive sponge stages\n---\n\n\
```bash\necho start\n```\n\n\
```llm\nprovider: cat\n---\nA:{{prev}}\n```\n\n\
```llm\nprovider: cat\n---\nB:{{prev}}\n```\n\n\
```bash\ncat\n```\n";

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(skill)
        .assert()
        .success();

    creft_with(&dir)
        .args(["sponge-chain"])
        .assert()
        .success()
        .stdout(predicate::str::contains("B:"))
        .stdout(predicate::str::contains("A:"))
        .stdout(predicate::str::contains("start"));
}

/// A missing LLM provider in a pipe chain produces a non-zero exit and a useful error message.
#[test]
fn test_llm_sponge_provider_not_found_in_pipe() {
    let dir = creft_env();

    let skill = "---\nname: sponge-missing\ndescription: pipe with missing provider\n---\n\n\
```bash\necho hello\n```\n\n\
```llm\nprovider: nonexistent-xyz\n---\n{{prev}}\n```\n\n\
```bash\ncat\n```\n";

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(skill)
        .assert()
        .success();

    creft_with(&dir)
        .args(["sponge-missing"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("nonexistent-xyz"));
}

/// Exit 99 from an LLM provider in a pipe chain suppresses relay output.
#[test]
fn test_llm_sponge_exit_99_in_pipe() {
    let dir = creft_env();

    let script_dir = tempfile::TempDir::new().unwrap();
    let script_path = script_dir.path().join("exit99.sh");
    {
        let mut f = std::fs::File::create(&script_path).unwrap();
        f.write_all(b"#!/bin/sh\ncat\nexit 99\n").unwrap();
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&script_path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script_path, perms).unwrap();
    }

    let provider_path = script_path.to_string_lossy();
    let skill = format!(
        "---\nname: sponge-exit99\ndescription: sponge exit 99 suppression\n---\n\n\
```bash\necho data\n```\n\n\
```llm\nprovider: {provider_path}\n---\n{{{{prev}}}}\n```\n\n\
```bash\necho should-not-appear\n```\n"
    );

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(skill.as_str())
        .assert()
        .success();

    creft_with(&dir)
        .args(["sponge-exit99"])
        .assert()
        .success()
        .stdout(predicate::str::contains("should-not-appear").not());
}

/// When the first block in a pipe chain is an LLM sponge, it reads from parent stdin.
#[test]
fn test_llm_sponge_first_block() {
    let dir = creft_env();

    // block 0: llm(cat, {{prev}}) — sponge reads parent stdin as "prev"
    // block 1: bash(cat) — echoes the sponge's output
    let skill = "---\nname: sponge-first\ndescription: llm as first pipe block\n---\n\n\
```llm\nprovider: cat\n---\n{{prev}}\n```\n\n\
```bash\ncat\n```\n";

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(skill)
        .assert()
        .success();

    creft_with(&dir)
        .args(["sponge-first"])
        .write_stdin("hello from parent")
        .assert()
        .success()
        .stdout(predicate::str::contains("hello from parent"));
}

/// When the last block in a pipe chain is an LLM sponge, its output feeds the relay thread.
#[test]
fn test_llm_sponge_last_block() {
    let dir = creft_env();

    let skill = "---\nname: sponge-last\ndescription: llm as last pipe block\n---\n\n\
```bash\necho upstream\n```\n\n\
```llm\nprovider: cat\n---\ngot: {{prev}}\n```\n";

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(skill)
        .assert()
        .success();

    creft_with(&dir)
        .args(["sponge-last"])
        .assert()
        .success()
        .stdout(predicate::str::contains("got:"))
        .stdout(predicate::str::contains("upstream"));
}

/// Frontmatter args are available in LLM sponge templates in pipe chains.
#[test]
fn test_llm_sponge_with_template_args() {
    let dir = creft_env();

    let skill = "---\nname: sponge-args\ndescription: sponge with template args\n\
args:\n  - name: greeting\n    required: true\n---\n\n\
```bash\necho data\n```\n\n\
```llm\nprovider: cat\n---\n{{greeting}}: {{prev}}\n```\n";

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(skill)
        .assert()
        .success();

    creft_with(&dir)
        .args(["sponge-args", "hello"])
        .assert()
        .success()
        .stdout(predicate::str::contains("hello:"))
        .stdout(predicate::str::contains("data"));
}

/// When an upstream bash block exits 99, the downstream LLM sponge must NOT
/// spawn its provider. The provider should never run.
///
/// Regression guard for Bug 1: before the fix, the sponge would read EOF and
/// unconditionally spawn the provider with an empty prompt.
#[test]
#[cfg(unix)]
fn test_pipe_exit_99_prevents_sponge_spawn() {
    use std::os::unix::fs::PermissionsExt;

    let dir = creft_env();
    let script_dir = tempfile::TempDir::new().unwrap();
    let sentinel = script_dir.path().join("sentinel.txt");
    let script_path = script_dir.path().join("mock-provider.sh");

    // The mock provider creates a sentinel file when it runs, then reads stdin.
    let script_content = format!("#!/bin/sh\ntouch {}\ncat\n", sentinel.to_string_lossy());
    {
        let mut f = std::fs::File::create(&script_path).unwrap();
        f.write_all(script_content.as_bytes()).unwrap();
        let mut perms = f.metadata().unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script_path, perms).unwrap();
    }

    let provider_path = script_path.to_string_lossy();
    let skill = format!(
        "---\nname: exit99-no-sponge\ndescription: upstream exit 99 prevents sponge spawn\n---\n\n\
```bash\nexit 99\n```\n\n\
```llm\nprovider: {provider_path}\n---\n{{{{prev}}}}\n```\n"
    );

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(skill.as_str())
        .assert()
        .success();

    let start = std::time::Instant::now();
    creft_with(&dir)
        .args(["exit99-no-sponge"])
        .assert()
        .success();
    let elapsed = start.elapsed();

    assert!(
        !sentinel.exists(),
        "provider must not have been spawned (sentinel file found)",
    );
    assert!(
        elapsed.as_secs() < 5,
        "must complete quickly when upstream exits 99 (took {:?})",
        elapsed
    );
}

/// Exit 99 with output followed by an LLM sponge: the sponge consumes the upstream
/// output via read_to_end before the cancel token fires. The sponge capture channel
/// must recover the output and the main thread must write it to stdout.
///
/// Regression guard for the sponge-consumes-exit-99-output gap.
#[test]
#[cfg(unix)]
fn test_pipe_exit_99_sponge_captures_upstream_output() {
    use std::os::unix::fs::PermissionsExt;

    let dir = creft_env();
    let script_dir = tempfile::TempDir::new().unwrap();
    let sentinel = script_dir.path().join("sentinel.txt");
    let script_path = script_dir.path().join("mock-provider.sh");

    // The mock provider creates a sentinel file when it runs, then reads stdin.
    // If the sponge is correctly cancelled before spawning, this file will not exist.
    let script_content = format!("#!/bin/sh\ntouch {}\ncat\n", sentinel.to_string_lossy());
    {
        let mut f = std::fs::File::create(&script_path).unwrap();
        f.write_all(script_content.as_bytes()).unwrap();
        let mut perms = f.metadata().unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script_path, perms).unwrap();
    }

    let provider_path = script_path.to_string_lossy();
    // Block 0 writes output then exits 99. Block 1 (sponge) reads that output via
    // read_to_end before cancel fires — the capture channel recovers it.
    let skill = format!(
        "---\nname: sponge-captures-exit99\ndescription: sponge capture channel recovery\n---\n\n\
```bash\necho captured-by-sponge; exit 99\n```\n\n\
```llm\nprovider: {provider_path}\n---\n{{{{prev}}}}\n```\n"
    );

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(skill.as_str())
        .assert()
        .success();

    let start = std::time::Instant::now();
    creft_with(&dir)
        .args(["sponge-captures-exit99"])
        .assert()
        .success()
        .stdout(predicate::str::contains("captured-by-sponge"));
    let elapsed = start.elapsed();

    assert!(
        !sentinel.exists(),
        "provider must not have been spawned (sentinel file found)",
    );
    assert!(
        elapsed.as_secs() < 2,
        "must complete quickly (took {:?})",
        elapsed
    );
}

/// Exit 99 from an upstream bash block must not spawn either downstream LLM sponge.
///
/// Regression guard for the sponge-to-sponge cancel propagation gap: before the
/// fix, the first sponge had no way to send the exit-99 determination to the
/// second sponge. The second sponge's `cancel_rx` was `None`, so it fell back to
/// `ctx.is_cancelled()`, which may not be set before `read_to_end` completes and
/// the sponge tries to spawn its provider.
#[test]
#[cfg(unix)]
fn test_pipe_exit_99_propagates_through_sponge_chain() {
    use std::os::unix::fs::PermissionsExt;

    let dir = creft_env();
    let script_dir = tempfile::TempDir::new().unwrap();

    let sentinel1 = script_dir.path().join("sentinel1.txt");
    let sentinel2 = script_dir.path().join("sentinel2.txt");
    let script1_path = script_dir.path().join("mock-provider1.sh");
    let script2_path = script_dir.path().join("mock-provider2.sh");

    let make_script = |path: &std::path::Path, sentinel: &std::path::Path| {
        let content = format!("#!/bin/sh\ntouch {}\ncat\n", sentinel.to_string_lossy());
        let mut f = std::fs::File::create(path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        let mut perms = f.metadata().unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms).unwrap();
    };
    make_script(&script1_path, &sentinel1);
    make_script(&script2_path, &sentinel2);

    let p1 = script1_path.to_string_lossy();
    let p2 = script2_path.to_string_lossy();

    // Block 0 (bash): exits 99 immediately.
    // Block 1 (llm): first sponge — must not spawn mock-provider1.
    // Block 2 (llm): second sponge — must not spawn mock-provider2.
    let skill = format!(
        "---\nname: sponge-chain-exit99\ndescription: exit 99 propagates through sponge chain\n---\n\n\
```bash\nexit 99\n```\n\n\
```llm\nprovider: {p1}\n---\n{{{{prev}}}}\n```\n\n\
```llm\nprovider: {p2}\n---\n{{{{prev}}}}\n```\n"
    );

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(skill.as_str())
        .assert()
        .success();

    let start = std::time::Instant::now();
    creft_with(&dir)
        .args(["sponge-chain-exit99"])
        .assert()
        .success();
    let elapsed = start.elapsed();

    assert!(
        !sentinel1.exists(),
        "first sponge provider must not have been spawned (sentinel1 found)",
    );
    assert!(
        !sentinel2.exists(),
        "second sponge provider must not have been spawned (sentinel2 found)",
    );
    assert!(
        elapsed.as_secs() < 5,
        "must complete quickly when upstream exits 99 (took {:?})",
        elapsed
    );
}

/// Normal (non-exit-99) sponge chains still work after the cancel propagation fix.
///
/// The upstream sponge sends `false` through its downstream cancel channel.
/// The downstream sponge receives `false`, proceeds, and its output is visible.
#[test]
#[cfg(unix)]
fn test_pipe_sponge_chain_normal_exit_proceeds() {
    let dir = creft_env();

    // bash → llm(cat, "A:{{prev}}") → llm(cat, "B:{{prev}}")
    // Both sponges must execute and their output must contain both prefixes.
    let skill = "---\nname: sponge-chain-normal\ndescription: consecutive sponge normal exit\n---\n\n\
```bash\necho start\n```\n\n\
```llm\nprovider: cat\n---\nA:{{prev}}\n```\n\n\
```llm\nprovider: cat\n---\nB:{{prev}}\n```\n";

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(skill)
        .assert()
        .success();

    creft_with(&dir)
        .args(["sponge-chain-normal"])
        .assert()
        .success()
        .stdout(predicate::str::contains("B:"))
        .stdout(predicate::str::contains("A:"))
        .stdout(predicate::str::contains("start"));
}

/// Three-block chain: bash → bash(exit 99 with output) → LLM sponge.
///
/// Block 1 reads block 0's output, prints "mid-result", exits 99.
/// Block 2 (sponge) consumed block 1's output via read_to_end before cancel fired.
/// The sponge capture channel must recover "mid-result" for the main thread.
#[test]
#[cfg(unix)]
fn test_pipe_exit_99_three_block_sponge_captures() {
    use std::os::unix::fs::PermissionsExt;

    let dir = creft_env();
    let script_dir = tempfile::TempDir::new().unwrap();
    let sentinel = script_dir.path().join("sentinel.txt");
    let script_path = script_dir.path().join("mock-provider.sh");

    let script_content = format!("#!/bin/sh\ntouch {}\ncat\n", sentinel.to_string_lossy());
    {
        let mut f = std::fs::File::create(&script_path).unwrap();
        f.write_all(script_content.as_bytes()).unwrap();
        let mut perms = f.metadata().unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script_path, perms).unwrap();
    }

    let provider_path = script_path.to_string_lossy();
    let skill = format!(
        "---\nname: three-block-sponge-exit99\ndescription: three-block sponge capture\n---\n\n\
```bash\necho input\n```\n\n\
```bash\ncat; echo mid-result; exit 99\n```\n\n\
```llm\nprovider: {provider_path}\n---\n{{{{prev}}}}\n```\n"
    );

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(skill.as_str())
        .assert()
        .success();

    let start = std::time::Instant::now();
    creft_with(&dir)
        .args(["three-block-sponge-exit99"])
        .assert()
        .success()
        .stdout(predicate::str::contains("mid-result"));
    let elapsed = start.elapsed();

    assert!(
        !sentinel.exists(),
        "provider must not have been spawned (sentinel file found)",
    );
    assert!(
        elapsed.as_secs() < 2,
        "must complete quickly (took {:?})",
        elapsed
    );
}

/// A sponge whose own LLM provider exits 99 propagates that cancel to the downstream sponge.
///
/// `bash | llm(exits 99) | llm(sentinel)` — the first sponge's child exits 99, so the
/// origination path at pipe.rs:519-523 sends `true` through `cancel_tx_downstream`. The
/// downstream sponge must not spawn its provider. This is distinct from the forwarding path
/// tested by `test_pipe_exit_99_propagates_through_sponge_chain`, which exercises cancel
/// forwarded from an upstream non-sponge (bash exits 99).
#[test]
#[cfg(unix)]
fn test_pipe_sponge_originated_exit_99_cancels_downstream() {
    use std::os::unix::fs::PermissionsExt;

    let dir = creft_env();
    let script_dir = tempfile::TempDir::new().unwrap();

    let exit99_path = script_dir.path().join("provider-exit99.sh");
    let sentinel = script_dir.path().join("sentinel.txt");
    let sentinel_provider_path = script_dir.path().join("sentinel-provider.sh");

    // First provider: reads stdin to satisfy the sponge's read_to_end, then exits 99.
    {
        let content = "#!/bin/sh\ncat > /dev/null\nexit 99\n";
        let mut f = std::fs::File::create(&exit99_path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        let mut perms = f.metadata().unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&exit99_path, perms).unwrap();
    }

    // Second provider (sentinel): creates a file when spawned, proving it ran.
    {
        let content = format!("#!/bin/sh\ntouch {}\ncat\n", sentinel.to_string_lossy());
        let mut f = std::fs::File::create(&sentinel_provider_path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        let mut perms = f.metadata().unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&sentinel_provider_path, perms).unwrap();
    }

    let p1 = exit99_path.to_string_lossy();
    let p2 = sentinel_provider_path.to_string_lossy();

    let skill = format!(
        "---\nname: sponge-exit99-originated\ndescription: sponge-originated exit-99 cancels downstream\n---\n\n\
```bash\necho input\n```\n\n\
```llm\nprovider: {p1}\n---\n{{{{prev}}}}\n```\n\n\
```llm\nprovider: {p2}\n---\n{{{{prev}}}}\n```\n"
    );

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(skill.as_str())
        .assert()
        .success();

    let start = std::time::Instant::now();
    creft_with(&dir)
        .args(["sponge-exit99-originated"])
        .assert()
        .success();
    let elapsed = start.elapsed();

    assert!(
        !sentinel.exists(),
        "downstream sponge provider must not have been spawned (sentinel file found)",
    );
    assert!(
        elapsed.as_secs() < 5,
        "must complete quickly when upstream sponge exits 99 (took {:?})",
        elapsed
    );
}
