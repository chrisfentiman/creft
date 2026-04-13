//! Tests for root help and subcommand short descriptions.

mod helpers;

use helpers::{creft_env, creft_with};
use predicates::prelude::*;
use pretty_assertions::assert_eq;

// ── Root help content ──────────────────────────────────────────────────────────

/// `creft --help` shows the new tagline.
#[test]
fn test_root_long_help_contains_tagline() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Executable skills for AI agents"));
}

/// `creft --help` shows the quick-start examples from ROOT_LONG_ABOUT.
#[test]
fn test_root_long_help_contains_examples() {
    let dir = creft_env();
    let output = creft_with(&dir)
        .args(["--help"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(output).unwrap();

    assert!(
        stdout.contains("creft cmd add") || stdout.contains("creft cmd list"),
        "root --help should contain quick-start examples; got: {stdout:?}"
    );
}

/// `creft --help` contains all subcommand names.
#[test]
fn test_root_long_help_contains_all_subcommands() {
    let dir = creft_env();
    let output = creft_with(&dir)
        .args(["--help"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(output).unwrap();

    let subcommands = ["cmd", "plugins", "settings", "up", "init", "doctor"];
    for cmd in &subcommands {
        assert!(
            stdout.contains(cmd),
            "root --help should contain subcommand '{cmd}'; got: {stdout:?}"
        );
    }
}

/// `-h` short help contains the tagline.
#[test]
fn test_root_short_help_contains_tagline() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["-h"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Executable skills for AI agents"));
}

// ── Subcommand description length ─────────────────────────────────────────────

/// No subcommand description line in `creft --help` exceeds 60 characters.
///
/// Clap renders subcommand descriptions in the Commands section. Each entry
/// looks like "  <name>  <description>". We check that the description portion
/// (after stripping the name) is at most 60 characters.
#[test]
fn test_subcommand_descriptions_within_60_chars() {
    let dir = creft_env();
    let output = creft_with(&dir)
        .args(["--help"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(output).unwrap();

    // Extract the Commands section lines.
    let mut in_commands = false;
    for line in stdout.lines() {
        if line.trim_start().starts_with("Commands:") {
            in_commands = true;
            continue;
        }
        if in_commands {
            // A blank line or a new section header ends the Commands block.
            if line.is_empty() || (line.starts_with(char::is_alphabetic) && line.ends_with(':')) {
                break;
            }
            // Each command entry is "  <name>  <description>".
            // Find the description by splitting on two or more spaces after the name.
            let trimmed = line.trim_start();
            if let Some(desc_start) = trimmed.find("  ") {
                let desc = trimmed[desc_start..].trim();
                assert!(
                    desc.chars().count() <= 60,
                    "subcommand description exceeds 60 chars ({} chars): {:?}",
                    desc.chars().count(),
                    desc
                );
            }
        }
    }
}

// ── no-args shows short help ──────────────────────────────────────────────────

/// `creft` with no arguments shows the short help (same as `creft -h`).
///
/// The short help must NOT contain text unique to ROOT_LONG_ABOUT, such as
/// the storage model note "Skills are stored in .creft/".
#[test]
fn test_no_args_shows_short_help_not_long() {
    let dir = creft_env();

    creft_with(&dir)
        .assert()
        .success()
        // This string appears in ROOT_LONG_ABOUT but not ROOT_ABOUT.
        .stdout(predicate::str::contains("Skills are stored in .creft/").not());
}

/// `creft` with no arguments and `creft -h` produce identical output.
#[test]
fn test_no_args_same_as_short_flag() {
    let dir = creft_env();

    let no_args = creft_with(&dir)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let short_flag = creft_with(&dir)
        .args(["-h"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    assert_eq!(
        String::from_utf8_lossy(&no_args),
        String::from_utf8_lossy(&short_flag),
        "creft with no args and creft -h should produce identical output"
    );
}

/// `creft --help` still shows the long help content (ROOT_LONG_ABOUT).
#[test]
fn test_long_help_flag_shows_long_about() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["--help"])
        .assert()
        .success()
        // This string appears in ROOT_LONG_ABOUT only, not in the short ROOT_ABOUT.
        .stdout(predicate::str::contains(
            "creft cmd list                 list available skills",
        ));
}

// ── Terminology: "skill" not "command" for user-authored skills ────────────────

// ── skill --help clap-style format ────────────────────────────────────────────

/// A user-defined skill's `--help` output matches clap conventions:
/// - First line is description only (no `name — description` prefix)
/// - Contains a `Usage:` line
/// - Uses `Arguments:` not `ARGS:`
/// - Uses `Options:` not `FLAGS:`
#[test]
fn test_skill_help_matches_clap_format() {
    let dir = creft_env();

    // Add a skill with args and flags.
    let skill_md = concat!(
        "---\n",
        "name: lint\n",
        "description: Run clippy with markdown output\n",
        "args:\n",
        "  - name: file\n",
        "    description: Filter findings to a specific file\n",
        "    required: false\n",
        "flags:\n",
        "  - name: fix\n",
        "    short: f\n",
        "    description: Auto-fix what clippy can\n",
        "    type: bool\n",
        "tags:\n",
        "  - dev\n",
        "  - quality\n",
        "---\n",
        "\n",
        "```bash\n",
        "cargo clippy\n",
        "```\n"
    );

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(skill_md)
        .assert()
        .success();

    let output = creft_with(&dir)
        .args(["lint", "--help"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(output).unwrap();

    // First line is description only
    let first_line = stdout.lines().next().unwrap_or("");
    assert_eq!(
        first_line, "Run clippy with markdown output",
        "first line should be description only; got: {stdout:?}"
    );

    // Usage: line is present
    assert!(
        stdout.contains("Usage:"),
        "skill --help should contain a Usage: line; got: {stdout:?}"
    );

    // [OPTIONS] appears because there are flags
    assert!(
        stdout.contains("[OPTIONS]"),
        "skill with flags should show [OPTIONS] in usage; got: {stdout:?}"
    );

    // Arguments: section (not ARGS:)
    assert!(
        stdout.contains("Arguments:"),
        "skill --help should use 'Arguments:' not 'ARGS:'; got: {stdout:?}"
    );
    assert!(
        !stdout.contains("ARGS:"),
        "ARGS: must not appear in clap-style help; got: {stdout:?}"
    );

    // Options: section (not FLAGS:)
    assert!(
        stdout.contains("Options:"),
        "skill --help should use 'Options:' not 'FLAGS:'; got: {stdout:?}"
    );
    assert!(
        !stdout.contains("FLAGS:"),
        "FLAGS: must not appear in clap-style help; got: {stdout:?}"
    );

    // Tags: section (not TAGS:)
    assert!(
        stdout.contains("Tags:"),
        "skill --help should use 'Tags:' not 'TAGS:'; got: {stdout:?}"
    );
    assert!(
        !stdout.contains("TAGS:"),
        "TAGS: must not appear in clap-style help; got: {stdout:?}"
    );
}

// ── Terminology: "skill" not "command" for user-authored skills ────────────────

/// `creft add --help` uses "skill" not "command" for user-authored skills.
#[test]
fn test_add_help_uses_skill_terminology() {
    let dir = creft_env();
    let output = creft_with(&dir)
        .args(["cmd", "add", "--help"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(output).unwrap();

    assert!(
        stdout.contains("skill"),
        "add --help should use 'skill' terminology; got: {stdout:?}"
    );
    // "Saves a new skill" should be present (from the short doc comment or long_about)
    assert!(
        stdout.contains("Saves a new skill") || stdout.contains("Save a new skill"),
        "add --help should describe saving a skill; got: {stdout:?}"
    );
}

// ── `creft help <skill>` dispatch ─────────────────────────────────────────────

/// `creft help <user-skill>` shows the skill's help (same as `creft <skill> --help`).
#[test]
fn test_help_subcommand_with_user_skill() {
    let dir = creft_env();

    // Add a user skill named `mutants`.
    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin(
            "---\nname: mutants\ndescription: Run mutation testing\n---\n\n```bash\necho mutants\n```\n",
        )
        .assert()
        .success();

    let output = creft_with(&dir)
        .args(["help", "mutants"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(output).unwrap();

    assert!(
        stdout.contains("Run mutation testing"),
        "creft help mutants should show the skill description; got: {stdout:?}"
    );
}

/// `creft help <namespace>` shows namespace listing when only sub-skills exist.
#[test]
fn test_help_subcommand_with_namespace() {
    let dir = creft_env();

    // Add two skills under the `tavily` namespace but no skill named exactly `tavily`.
    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin("---\nname: tavily search\ndescription: Search the web\n---\n\n```bash\necho search\n```\n")
        .assert()
        .success();
    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin("---\nname: tavily crawl\ndescription: Crawl a website\n---\n\n```bash\necho crawl\n```\n")
        .assert()
        .success();

    let output = creft_with(&dir)
        .args(["help", "tavily"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(output).unwrap();

    // Namespace listing shows the namespace header and at least one sub-skill name.
    assert!(
        stdout.contains("tavily"),
        "creft help tavily should show namespace listing; got: {stdout:?}"
    );
    assert!(
        stdout.contains("search") || stdout.contains("crawl"),
        "creft help tavily should list sub-skills; got: {stdout:?}"
    );
}

/// `creft help cmd` shows clap's built-in help for the `cmd` subcommand.
#[test]
fn test_help_subcommand_with_builtin() {
    let dir = creft_env();

    let output = creft_with(&dir)
        .args(["help", "cmd"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(output).unwrap();

    // Clap renders the `cmd` subcommand description in its help output.
    assert!(
        stdout.contains("skill") || stdout.contains("cmd"),
        "creft help cmd should show built-in cmd help; got: {stdout:?}"
    );
}

/// `creft help` with no trailing args shows root help content.
#[test]
fn test_help_subcommand_no_args() {
    let dir = creft_env();

    let output = creft_with(&dir)
        .args(["help"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(output).unwrap();

    // Root help always contains the tagline and the list of subcommands.
    assert!(
        stdout.contains("Executable skills for AI agents"),
        "creft help should show root help tagline; got: {stdout:?}"
    );
}

/// `creft help nonexistent` produces an error (clap's "unrecognized subcommand" behavior).
#[test]
fn test_help_subcommand_nonexistent_skill() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["help", "nonexistent"])
        .assert()
        .failure();
}

/// `creft help <ns> <skill>` works for multi-word user skills.
#[test]
fn test_help_subcommand_multiword_skill() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["cmd", "add"])
        .write_stdin("---\nname: tavily search\ndescription: Search the web with Tavily\n---\n\n```bash\necho search\n```\n")
        .assert()
        .success();

    let output = creft_with(&dir)
        .args(["help", "tavily", "search"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(output).unwrap();

    assert!(
        stdout.contains("Search the web with Tavily"),
        "creft help tavily search should show skill description; got: {stdout:?}"
    );
}

// ── built-in --help standardization ───────────────────────────────────────────

/// `creft add --help` contains a structured `Usage:` section.
#[test]
fn test_add_help_has_usage_section() {
    let dir = creft_env();
    let output = creft_with(&dir)
        .args(["cmd", "add", "--help"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(output).unwrap();

    assert!(
        stdout.contains("Usage:"),
        "creft add --help should contain 'Usage:' section; got: {stdout:?}"
    );
}

/// `creft add --help` uses Title Case `Frontmatter Fields:` not ALL CAPS.
#[test]
fn test_add_help_has_frontmatter_section() {
    let dir = creft_env();
    let output = creft_with(&dir)
        .args(["cmd", "add", "--help"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(output).unwrap();

    assert!(
        stdout.contains("Frontmatter Fields:"),
        "creft add --help should contain 'Frontmatter Fields:' (Title Case); got: {stdout:?}"
    );
    assert!(
        !stdout.contains("FRONTMATTER FIELDS:"),
        "creft add --help must not contain 'FRONTMATTER FIELDS:' (ALL CAPS); got: {stdout:?}"
    );
}

/// `creft doctor --help` contains an `Exit Codes:` section.
#[test]
fn test_doctor_help_has_exit_codes() {
    let dir = creft_env();
    let output = creft_with(&dir)
        .args(["doctor", "--help"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(output).unwrap();

    assert!(
        stdout.contains("Exit Codes:"),
        "creft doctor --help should contain 'Exit Codes:' section; got: {stdout:?}"
    );
}

/// `creft up --help` contains a `Supported Systems:` section.
#[test]
fn test_up_help_has_supported_systems() {
    let dir = creft_env();
    let output = creft_with(&dir)
        .args(["up", "--help"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(output).unwrap();

    assert!(
        stdout.contains("Supported Systems:"),
        "creft up --help should contain 'Supported Systems:' section; got: {stdout:?}"
    );
}

/// Every built-in command's `--help` output contains a `Usage:` section.
#[test]
fn test_all_builtin_help_has_usage() {
    let dir = creft_env();

    // Top-level builtins: invoke directly.
    let top_level = ["up", "doctor", "init", "cmd", "plugins", "settings"];
    for cmd in &top_level {
        let output = creft_with(&dir)
            .args([cmd, "--help"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let stdout = String::from_utf8(output).unwrap();
        assert!(
            stdout.contains("Usage:"),
            "creft {cmd} --help should contain 'Usage:' section; got: {stdout:?}"
        );
    }

    // `cmd` sub-commands: invoke as `creft cmd <sub> --help`.
    let cmd_subs = ["add", "list", "show", "cat", "rm"];
    for sub in &cmd_subs {
        let output = creft_with(&dir)
            .args(["cmd", sub, "--help"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let stdout = String::from_utf8(output).unwrap();
        assert!(
            stdout.contains("Usage:"),
            "creft cmd {sub} --help should contain 'Usage:' section; got: {stdout:?}"
        );
    }
}

/// No built-in command's `--help` output contains ALL CAPS section headers
/// (e.g., `FRONTMATTER FIELDS:`, `CODE BLOCKS:`, `HOW TO WRITE A SKILL DEFINITION:`).
///
/// Section headers must use Title Case to match clap's convention.
#[test]
fn test_no_allcaps_sections_in_help() {
    let dir = creft_env();

    // Regex: a line that is all uppercase letters and spaces (4+ chars) ending with ':'
    // This catches headers like "FRONTMATTER FIELDS:" or "CODE BLOCKS:" but not
    // normal prose that might happen to have an uppercase word.
    let allcaps_re = regex::Regex::new(r"(?m)^[A-Z ]{4,}:$").unwrap();

    // Top-level builtins.
    let top_level = ["up", "doctor", "init", "cmd", "plugins", "settings"];
    for cmd in &top_level {
        let output = creft_with(&dir)
            .args([cmd, "--help"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let stdout = String::from_utf8(output).unwrap();
        assert!(
            !allcaps_re.is_match(&stdout),
            "creft {cmd} --help must not contain ALL CAPS section headers; got: {stdout:?}"
        );
    }

    // `cmd` sub-commands.
    let cmd_subs = ["add", "list", "show", "cat", "rm"];
    for sub in &cmd_subs {
        let output = creft_with(&dir)
            .args(["cmd", sub, "--help"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let stdout = String::from_utf8(output).unwrap();
        assert!(
            !allcaps_re.is_match(&stdout),
            "creft cmd {sub} --help must not contain ALL CAPS section headers; got: {stdout:?}"
        );
    }
}
