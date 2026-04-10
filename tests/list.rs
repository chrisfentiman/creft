//! Tests for `creft list`, `creft show`, `creft cat`, grouped output, drill-in, and namespace help.

mod helpers;

use helpers::{create_test_package, creft_env, creft_with};
use predicates::prelude::*;

// ── list tests ────────────────────────────────────────────────────────────────

/// `creft list` with an empty store reports that no commands exist.
#[test]
fn test_list_empty() {
    let dir = creft_env();
    creft_with(&dir)
        .args(["list"])
        .assert()
        .success()
        .stderr(predicate::str::contains("no commands found"));
}

/// After adding two commands, `creft list` shows both names.
#[test]
fn test_list_shows_commands() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["add"])
        .write_stdin("---\nname: alpha\ndescription: first\n---\n\n```bash\necho alpha\n```\n")
        .assert()
        .success();

    creft_with(&dir)
        .args(["add"])
        .write_stdin("---\nname: beta\ndescription: second\n---\n\n```bash\necho beta\n```\n")
        .assert()
        .success();

    creft_with(&dir)
        .args(["list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("alpha"))
        .stdout(predicate::str::contains("beta"));
}

/// `creft list --tag <tag>` shows only commands whose frontmatter includes that tag.
///
/// Tags are stored in YAML frontmatter, not via a CLI flag — write_stdin carries
/// the frontmatter with the `tags:` field already populated.
#[test]
fn test_list_filter_by_tag() {
    let dir = creft_env();

    // Command with ops tag.
    creft_with(&dir)
        .args(["add"])
        .write_stdin(
            "---\nname: deploy\ndescription: Deploy the app\ntags:\n  - ops\n  - deploy\n---\n\n```bash\necho deploying\n```\n",
        )
        .assert()
        .success();

    // Command with dev tag (different).
    creft_with(&dir)
        .args(["add"])
        .write_stdin(
            "---\nname: lint\ndescription: Run linter\ntags:\n  - dev\n  - test\n---\n\n```bash\necho linting\n```\n",
        )
        .assert()
        .success();

    // Filtering by "ops" should show only "deploy".
    creft_with(&dir)
        .args(["list", "--tag", "ops"])
        .assert()
        .success()
        .stdout(predicate::str::contains("deploy"))
        .stdout(predicate::str::contains("lint").not());
}

// ── show tests ────────────────────────────────────────────────────────────────

/// `creft show <name>` prints the raw markdown file content including frontmatter.
#[test]
fn test_show_command() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["add"])
        .write_stdin("---\nname: hello\ndescription: greet\n---\n\n```bash\necho hello\n```\n")
        .assert()
        .success();

    creft_with(&dir)
        .args(["show", "hello"])
        .assert()
        .success()
        .stdout(predicate::str::contains("name: hello"))
        .stdout(predicate::str::contains("description: greet"));
}

/// `creft show nonexistent` exits with code 2 (CommandNotFound).
#[test]
fn test_show_not_found() {
    let dir = creft_env();
    creft_with(&dir)
        .args(["show", "nonexistent"])
        .assert()
        .failure()
        .code(2);
}

// ── cat tests ─────────────────────────────────────────────────────────────────

/// `creft cat <name>` prints the code block content without frontmatter.
#[test]
fn test_cat_command() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["add"])
        .write_stdin("---\nname: hello\ndescription: greet\n---\n\n```bash\necho hello\n```\n")
        .assert()
        .success();

    creft_with(&dir)
        .args(["cat", "hello"])
        .assert()
        .success()
        .stdout(predicate::str::contains("echo hello"))
        // Frontmatter should not appear in cat output.
        .stdout(predicate::str::contains("description:").not());
}

// ── grouped creft list output tests ───────────────────────────────────────────

/// `creft list` groups namespaced skills under their namespace.
/// The namespace shows a count; individual namespaced skills do not appear at top level.
#[test]
fn test_list_grouped_output() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["add"])
        .write_stdin(
            "---\nname: hello\ndescription: Greets someone\n---\n\n```bash\necho hello\n```\n",
        )
        .assert()
        .success();

    creft_with(&dir)
        .args(["add"])
        .write_stdin("---\nname: tavily search\ndescription: Search the web\n---\n\n```bash\necho search\n```\n")
        .assert()
        .success();

    creft_with(&dir)
        .args(["add"])
        .write_stdin("---\nname: tavily crawl\ndescription: Crawl a website\n---\n\n```bash\necho crawl\n```\n")
        .assert()
        .success();

    let output = creft_with(&dir)
        .args(["list"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8_lossy(&output);

    // tavily namespace appears with skill count.
    assert!(
        stdout.contains("tavily"),
        "tavily namespace should appear; got: {stdout:?}"
    );
    assert!(
        stdout.contains("2 skills"),
        "tavily should show '2 skills'; got: {stdout:?}"
    );

    // hello (non-namespaced) appears with its description.
    assert!(
        stdout.contains("hello"),
        "hello should appear; got: {stdout:?}"
    );
    assert!(
        stdout.contains("Greets someone"),
        "hello description should appear; got: {stdout:?}"
    );

    // Individual namespaced skills must NOT appear at top level.
    assert!(
        !stdout.contains("tavily search"),
        "tavily search should not appear at top level; got: {stdout:?}"
    );
    assert!(
        !stdout.contains("tavily crawl"),
        "tavily crawl should not appear at top level; got: {stdout:?}"
    );
}

/// `creft list --all` shows the flat list identical to old behavior.
#[test]
fn test_list_all_flag() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["add"])
        .write_stdin(
            "---\nname: hello\ndescription: Greets someone\n---\n\n```bash\necho hello\n```\n",
        )
        .assert()
        .success();

    creft_with(&dir)
        .args(["add"])
        .write_stdin("---\nname: tavily search\ndescription: Search the web\n---\n\n```bash\necho search\n```\n")
        .assert()
        .success();

    creft_with(&dir)
        .args(["add"])
        .write_stdin("---\nname: tavily crawl\ndescription: Crawl a website\n---\n\n```bash\necho crawl\n```\n")
        .assert()
        .success();

    // --all shows every skill by full name with description.
    creft_with(&dir)
        .args(["list", "--all"])
        .assert()
        .success()
        .stdout(predicate::str::contains("hello"))
        .stdout(predicate::str::contains("tavily crawl"))
        .stdout(predicate::str::contains("tavily search"));
}

/// `creft list nonexistent` prints an error message to stderr and exits 0.
#[test]
fn test_list_namespace_not_found() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["add"])
        .write_stdin(
            "---\nname: hello\ndescription: Greets someone\n---\n\n```bash\necho hello\n```\n",
        )
        .assert()
        .success();

    creft_with(&dir)
        .args(["list", "nonexistent"])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "no skills found under 'nonexistent'",
        ));
}

/// `creft list --tag <tag>` with namespaced skills groups the filtered result.
#[test]
fn test_list_tag_with_namespace() {
    let dir = creft_env();

    // tavily search tagged with 'api'
    creft_with(&dir)
        .args(["add"])
        .write_stdin("---\nname: tavily search\ndescription: Search the web\ntags:\n  - api\n---\n\n```bash\necho search\n```\n")
        .assert()
        .success();

    // tavily crawl NOT tagged with 'api'
    creft_with(&dir)
        .args(["add"])
        .write_stdin("---\nname: tavily crawl\ndescription: Crawl a website\ntags:\n  - crawl\n---\n\n```bash\necho crawl\n```\n")
        .assert()
        .success();

    // Filtering by 'api' shows tavily namespace (1 skill matched).
    creft_with(&dir)
        .args(["list", "--tag", "api"])
        .assert()
        .success()
        .stdout(predicate::str::contains("tavily"))
        .stdout(predicate::str::contains("1 skill"));
}

/// `creft list <namespace> --tag <nonexistent>` shows generic empty message, not
/// "no skills found under '...'" — the namespace exists, the tag just matched nothing.
#[test]
fn test_list_namespace_with_nonexistent_tag() {
    let dir = creft_env();

    // Add tavily skills without the 'nonexistent' tag.
    creft_with(&dir)
        .args(["add"])
        .write_stdin("---\nname: tavily search\ndescription: Search the web\ntags:\n  - api\n---\n\n```bash\necho search\n```\n")
        .assert()
        .success();

    creft_with(&dir)
        .args(["add"])
        .write_stdin("---\nname: tavily crawl\ndescription: Crawl a website\ntags:\n  - crawl\n---\n\n```bash\necho crawl\n```\n")
        .assert()
        .success();

    // The tavily namespace exists but no skill has the 'nonexistent' tag.
    // Flags must precede namespace positional args (trailing_var_arg captures everything after first
    // positional). Should show generic empty message, not "no skills found under 'tavily'".
    let output = creft_with(&dir)
        .args(["list", "--tag", "nonexistent", "tavily"])
        .assert()
        .success();

    let stderr = String::from_utf8_lossy(&output.get_output().stderr);
    assert!(
        stderr.contains("no commands found. use 'creft add' to create one."),
        "expected generic empty message; got: {stderr:?}"
    );
    assert!(
        !stderr.contains("no skills found under"),
        "must not show namespace-not-found message when namespace exists; got: {stderr:?}"
    );
}

/// `creft list` shows [package] annotation on a package namespace.
#[test]
fn test_list_package_annotation() {
    let pkg_repo = create_test_package(
        "annotated-pkg",
        &[
            (
                "search.md",
                "---\nname: search\ndescription: Search\n---\n\n```bash\necho search\n```\n",
            ),
            (
                "crawl.md",
                "---\nname: crawl\ndescription: Crawl\n---\n\n```bash\necho crawl\n```\n",
            ),
        ],
    );
    let creft_home = creft_env();

    creft_with(&creft_home)
        .args(["install", pkg_repo.path().to_str().unwrap()])
        .assert()
        .success();

    creft_with(&creft_home)
        .args(["list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("annotated-pkg"))
        .stdout(predicate::str::contains("[package]"))
        .stdout(predicate::str::contains("2 skills"));
}

// ── creft list <namespace> drill-in ───────────────────────────────────────────

/// `creft list tavily` shows individual skills inside the tavily namespace,
/// each with their full name (including namespace prefix) and description.
#[test]
fn test_list_drill_into_namespace() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["add"])
        .write_stdin(
            "---\nname: hello\ndescription: Greets someone\n---\n\n```bash\necho hello\n```\n",
        )
        .assert()
        .success();

    creft_with(&dir)
        .args(["add"])
        .write_stdin("---\nname: tavily search\ndescription: Search the web\n---\n\n```bash\necho search\n```\n")
        .assert()
        .success();

    creft_with(&dir)
        .args(["add"])
        .write_stdin("---\nname: tavily crawl\ndescription: Crawl a website\n---\n\n```bash\necho crawl\n```\n")
        .assert()
        .success();

    let output = creft_with(&dir)
        .args(["list", "tavily"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8_lossy(&output);

    // Full skill names appear with their descriptions.
    assert!(
        stdout.contains("tavily search"),
        "tavily search should appear in drill-in output; got: {stdout:?}"
    );
    assert!(
        stdout.contains("Search the web"),
        "tavily search description should appear; got: {stdout:?}"
    );
    assert!(
        stdout.contains("tavily crawl"),
        "tavily crawl should appear in drill-in output; got: {stdout:?}"
    );
    assert!(
        stdout.contains("Crawl a website"),
        "tavily crawl description should appear; got: {stdout:?}"
    );

    // Non-tavily skills must NOT appear.
    assert!(
        !stdout.contains("hello"),
        "hello should not appear when drilling into tavily; got: {stdout:?}"
    );

    // The namespace entry itself ('tavily' with count) must NOT appear — we are inside it.
    assert!(
        !stdout.contains("2 skills"),
        "namespace count line should not appear inside drill-in; got: {stdout:?}"
    );
}

/// `creft list aws` shows sub-namespaces. `creft list aws s3` shows leaf skills.
/// Verifies drill-in at two levels of depth.
#[test]
fn test_list_deep_namespace() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["add"])
        .write_stdin("---\nname: aws s3 copy\ndescription: Copy objects between S3 buckets\n---\n\n```bash\necho copy\n```\n")
        .assert()
        .success();

    creft_with(&dir)
        .args(["add"])
        .write_stdin("---\nname: aws s3 sync\ndescription: Sync a local directory to S3\n---\n\n```bash\necho sync\n```\n")
        .assert()
        .success();

    creft_with(&dir)
        .args(["add"])
        .write_stdin("---\nname: aws ec2 list\ndescription: List EC2 instances\n---\n\n```bash\necho list\n```\n")
        .assert()
        .success();

    // creft list aws — should show sub-namespaces aws s3 and aws ec2 as collapsed entries.
    let aws_output = creft_with(&dir)
        .args(["list", "aws"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let aws_stdout = String::from_utf8_lossy(&aws_output);

    assert!(
        aws_stdout.contains("aws s3"),
        "aws s3 sub-namespace should appear when drilling into aws; got: {aws_stdout:?}"
    );
    assert!(
        aws_stdout.contains("aws ec2"),
        "aws ec2 sub-namespace should appear when drilling into aws; got: {aws_stdout:?}"
    );
    // Leaf skills should not appear at this level.
    assert!(
        !aws_stdout.contains("aws s3 copy"),
        "aws s3 copy leaf skill should not appear at the aws level; got: {aws_stdout:?}"
    );
    assert!(
        !aws_stdout.contains("aws s3 sync"),
        "aws s3 sync leaf skill should not appear at the aws level; got: {aws_stdout:?}"
    );
    assert!(
        !aws_stdout.contains("aws ec2 list"),
        "aws ec2 list leaf skill should not appear at the aws level; got: {aws_stdout:?}"
    );

    // creft list aws s3 — should show leaf skills aws s3 copy and aws s3 sync.
    let s3_output = creft_with(&dir)
        .args(["list", "aws", "s3"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let s3_stdout = String::from_utf8_lossy(&s3_output);

    assert!(
        s3_stdout.contains("aws s3 copy"),
        "aws s3 copy should appear in aws s3 drill-in; got: {s3_stdout:?}"
    );
    assert!(
        s3_stdout.contains("Copy objects between S3 buckets"),
        "aws s3 copy description should appear; got: {s3_stdout:?}"
    );
    assert!(
        s3_stdout.contains("aws s3 sync"),
        "aws s3 sync should appear in aws s3 drill-in; got: {s3_stdout:?}"
    );
    assert!(
        s3_stdout.contains("Sync a local directory to S3"),
        "aws s3 sync description should appear; got: {s3_stdout:?}"
    );
    // ec2 skills must not leak into the s3 drill-in.
    assert!(
        !s3_stdout.contains("aws ec2"),
        "aws ec2 should not appear when drilling into aws s3; got: {s3_stdout:?}"
    );
}

// ── list output truncation and footer ─────────────────────────────────────────

/// `creft list` with a skill that has a 100-char description shows "..." truncation.
#[test]
fn test_list_long_description_truncated() {
    let dir = creft_env();
    // 100-character description — over the 60-char display limit.
    let long_desc = "a".repeat(100);
    let markdown = format!(
        "---\nname: truncated-skill\ndescription: {long_desc}\n---\n\n```bash\necho hi\n```\n"
    );

    creft_with(&dir)
        .args(["add", "--no-validate"])
        .write_stdin(markdown.as_str())
        .assert()
        .success();

    creft_with(&dir)
        .args(["list"])
        .assert()
        .success()
        // The truncated output should contain "..." (ellipsis suffix).
        .stdout(predicate::str::contains("..."))
        // The full 100-char description must NOT appear verbatim.
        .stdout(predicate::str::contains(long_desc).not());
}

/// `creft list` with a skill that has a short description shows it in full (no "...").
#[test]
fn test_list_short_description_not_truncated() {
    let dir = creft_env();
    let short_desc = "Short description here";
    let markdown = format!(
        "---\nname: short-skill\ndescription: {short_desc}\n---\n\n```bash\necho hi\n```\n"
    );

    creft_with(&dir)
        .args(["add"])
        .write_stdin(markdown.as_str())
        .assert()
        .success();

    creft_with(&dir)
        .args(["list"])
        .assert()
        .success()
        .stdout(predicate::str::contains(short_desc))
        // No truncation suffix for short descriptions.
        .stdout(predicate::str::contains("...").not());
}

// ── hidden command tests ───────────────────────────────────────────────────────

/// A top-level command whose name starts with `_` is excluded from `creft list`.
#[test]
fn hidden_top_level_command_excluded_from_list() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["add"])
        .write_stdin(
            "---\nname: _internal\ndescription: private\n---\n\n```bash\necho internal\n```\n",
        )
        .assert()
        .success();

    creft_with(&dir)
        .args(["add"])
        .write_stdin("---\nname: visible\ndescription: public\n---\n\n```bash\necho visible\n```\n")
        .assert()
        .success();

    creft_with(&dir)
        .args(["list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("visible"))
        .stdout(predicate::str::contains("_internal").not());
}

/// A hidden subcommand is excluded from `creft list <namespace>`, but the namespace
/// itself still appears at top level when it has at least one visible command.
/// The skill count for the namespace reflects only visible commands.
#[test]
fn hidden_subcommand_excluded_from_namespace_drill_in() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["add"])
        .write_stdin("---\nname: hooks _guard\ndescription: private guard\n---\n\n```bash\necho guard\n```\n")
        .assert()
        .success();

    creft_with(&dir)
        .args(["add"])
        .write_stdin(
            "---\nname: hooks deploy\ndescription: deploy hook\n---\n\n```bash\necho deploy\n```\n",
        )
        .assert()
        .success();

    // Top-level list: hooks namespace appears with count of 1 (only visible command).
    let top_output = creft_with(&dir)
        .args(["list"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let top_stdout = String::from_utf8_lossy(&top_output);
    assert!(
        top_stdout.contains("hooks"),
        "hooks namespace should appear in top-level list; got: {top_stdout:?}"
    );
    assert!(
        top_stdout.contains("1 skill"),
        "hooks namespace should show 1 skill (hidden guard excluded); got: {top_stdout:?}"
    );

    // Drill-in: hooks deploy appears, hooks _guard does not.
    creft_with(&dir)
        .args(["list", "hooks"])
        .assert()
        .success()
        .stdout(predicate::str::contains("hooks deploy"))
        .stdout(predicate::str::contains("hooks _guard").not());
}

/// A namespace whose own token starts with `_` is entirely excluded from `creft list`.
#[test]
fn hidden_namespace_excluded_from_list() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["add"])
        .write_stdin("---\nname: _private mycommand\ndescription: secret\n---\n\n```bash\necho secret\n```\n")
        .assert()
        .success();

    creft_with(&dir)
        .args(["list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("_private").not());
}

/// `creft list _private` (explicit hidden prefix) shows hidden commands under that namespace.
#[test]
fn explicit_hidden_prefix_shows_hidden_commands() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["add"])
        .write_stdin("---\nname: _private mycommand\ndescription: secret\n---\n\n```bash\necho secret\n```\n")
        .assert()
        .success();

    creft_with(&dir)
        .args(["list", "_private"])
        .assert()
        .success()
        .stdout(predicate::str::contains("_private mycommand"));
}

/// A hidden command executes normally when called by name.
#[test]
fn hidden_command_executes_normally() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["add"])
        .write_stdin(
            "---\nname: _internal\ndescription: private\n---\n\n```bash\necho hidden-output\n```\n",
        )
        .assert()
        .success();

    creft_with(&dir)
        .args(["_internal"])
        .assert()
        .success()
        .stdout(predicate::str::contains("hidden-output"));
}

/// `creft show _internal` works normally on a hidden command.
#[test]
fn show_works_on_hidden_command() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["add"])
        .write_stdin(
            "---\nname: _internal\ndescription: private\n---\n\n```bash\necho hidden-output\n```\n",
        )
        .assert()
        .success();

    creft_with(&dir)
        .args(["show", "_internal"])
        .assert()
        .success()
        .stdout(predicate::str::contains("name: _internal"));
}

/// `creft list --tag ops` excludes hidden commands even when they match the tag.
#[test]
fn tag_filter_excludes_hidden_commands() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["add"])
        .write_stdin("---\nname: _internal\ndescription: private\ntags:\n  - ops\n---\n\n```bash\necho internal\n```\n")
        .assert()
        .success();

    creft_with(&dir)
        .args(["add"])
        .write_stdin("---\nname: visible\ndescription: public\ntags:\n  - ops\n---\n\n```bash\necho visible\n```\n")
        .assert()
        .success();

    creft_with(&dir)
        .args(["list", "--tag", "ops"])
        .assert()
        .success()
        .stdout(predicate::str::contains("visible"))
        .stdout(predicate::str::contains("_internal").not());
}

/// `creft list` with namespaced skills shows the help footer at root level.
#[test]
fn test_list_shows_footer_with_namespaces() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["add"])
        .write_stdin("---\nname: tavily search\ndescription: Search the web\n---\n\n```bash\necho search\n```\n")
        .assert()
        .success();

    creft_with(&dir)
        .args(["add"])
        .write_stdin("---\nname: tavily crawl\ndescription: Crawl a website\n---\n\n```bash\necho crawl\n```\n")
        .assert()
        .success();

    creft_with(&dir)
        .args(["list"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "See 'creft <skill> --help' for details.",
        ));
}

/// `creft list <namespace>` (drill-in) does NOT show the footer.
#[test]
fn test_list_drill_in_no_footer() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["add"])
        .write_stdin("---\nname: tavily search\ndescription: Search the web\n---\n\n```bash\necho search\n```\n")
        .assert()
        .success();

    creft_with(&dir)
        .args(["add"])
        .write_stdin("---\nname: tavily crawl\ndescription: Crawl a website\n---\n\n```bash\necho crawl\n```\n")
        .assert()
        .success();

    creft_with(&dir)
        .args(["list", "tavily"])
        .assert()
        .success()
        .stdout(predicate::str::contains("See 'creft <skill> --help' for details.").not());
}

/// `creft list` with only non-namespaced skills still shows the footer at root level.
#[test]
fn test_list_footer_always_shown_at_root() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["add"])
        .write_stdin(
            "---\nname: hello\ndescription: Greets someone\n---\n\n```bash\necho hello\n```\n",
        )
        .assert()
        .success();

    creft_with(&dir)
        .args(["add"])
        .write_stdin("---\nname: world\ndescription: Says world\n---\n\n```bash\necho world\n```\n")
        .assert()
        .success();

    creft_with(&dir)
        .args(["list"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "See 'creft <skill> --help' for details.",
        ));
}

// ── creft list --all hidden command tests ─────────────────────────────────────

/// `creft list --all` shows hidden `_`-prefixed commands alongside visible ones.
#[test]
fn list_all_includes_hidden_top_level_commands() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["add"])
        .write_stdin(
            "---\nname: _internal\ndescription: private\n---\n\n```bash\necho internal\n```\n",
        )
        .assert()
        .success();

    creft_with(&dir)
        .args(["add"])
        .write_stdin("---\nname: visible\ndescription: public\n---\n\n```bash\necho visible\n```\n")
        .assert()
        .success();

    creft_with(&dir)
        .args(["list", "--all"])
        .assert()
        .success()
        .stdout(predicate::str::contains("visible"))
        .stdout(predicate::str::contains("_internal"));
}

/// `creft list --all` shows hidden namespaced commands that are normally suppressed.
#[test]
fn list_all_includes_hidden_namespaced_commands() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["add"])
        .write_stdin("---\nname: hooks _guard\ndescription: private guard\n---\n\n```bash\necho guard\n```\n")
        .assert()
        .success();

    creft_with(&dir)
        .args(["add"])
        .write_stdin(
            "---\nname: hooks deploy\ndescription: deploy hook\n---\n\n```bash\necho deploy\n```\n",
        )
        .assert()
        .success();

    creft_with(&dir)
        .args(["list", "--all"])
        .assert()
        .success()
        .stdout(predicate::str::contains("hooks deploy"))
        .stdout(predicate::str::contains("hooks _guard"));
}

/// `creft list <namespace> --all` shows hidden commands within the namespace.
#[test]
fn list_namespace_all_includes_hidden_commands() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["add"])
        .write_stdin("---\nname: hooks _guard\ndescription: private guard\n---\n\n```bash\necho guard\n```\n")
        .assert()
        .success();

    creft_with(&dir)
        .args(["add"])
        .write_stdin(
            "---\nname: hooks deploy\ndescription: deploy hook\n---\n\n```bash\necho deploy\n```\n",
        )
        .assert()
        .success();

    // --all must precede the namespace positional arg — trailing_var_arg captures
    // everything after the first positional, so flags placed after are treated as
    // namespace tokens rather than parsed as options.
    creft_with(&dir)
        .args(["list", "--all", "hooks"])
        .assert()
        .success()
        .stdout(predicate::str::contains("hooks deploy"))
        .stdout(predicate::str::contains("hooks _guard"));
}

/// `creft list --all` shows hidden commands even when a tag filter is applied.
#[test]
fn list_all_with_tag_includes_hidden_matching_commands() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["add"])
        .write_stdin("---\nname: _internal\ndescription: private\ntags:\n  - ops\n---\n\n```bash\necho internal\n```\n")
        .assert()
        .success();

    creft_with(&dir)
        .args(["add"])
        .write_stdin("---\nname: visible\ndescription: public\ntags:\n  - ops\n---\n\n```bash\necho visible\n```\n")
        .assert()
        .success();

    creft_with(&dir)
        .args(["list", "--all", "--tag", "ops"])
        .assert()
        .success()
        .stdout(predicate::str::contains("visible"))
        .stdout(predicate::str::contains("_internal"));
}

/// `creft list --all` does NOT show the footer (flat mode only).
#[test]
fn test_list_all_no_footer() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["add"])
        .write_stdin("---\nname: tavily search\ndescription: Search the web\n---\n\n```bash\necho search\n```\n")
        .assert()
        .success();

    creft_with(&dir)
        .args(["add"])
        .write_stdin("---\nname: tavily crawl\ndescription: Crawl a website\n---\n\n```bash\necho crawl\n```\n")
        .assert()
        .success();

    creft_with(&dir)
        .args(["list", "--all"])
        .assert()
        .success()
        .stdout(predicate::str::contains("See 'creft <skill> --help' for details.").not());
}

// ── creft <namespace> --help ───────────────────────────────────────────────────

/// `creft tavily --help` shows namespace listing header and skills when `tavily` is a namespace.
#[test]
fn test_namespace_help() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["add"])
        .write_stdin("---\nname: tavily search\ndescription: Search the web\n---\n\n```bash\necho search\n```\n")
        .assert()
        .success();

    creft_with(&dir)
        .args(["add"])
        .write_stdin("---\nname: tavily crawl\ndescription: Crawl a website from a starting URL\n---\n\n```bash\necho crawl\n```\n")
        .assert()
        .success();

    creft_with(&dir)
        .args(["add"])
        .write_stdin("---\nname: tavily extract\ndescription: Extract content from URLs\n---\n\n```bash\necho extract\n```\n")
        .assert()
        .success();

    let output = creft_with(&dir)
        .args(["tavily", "--help"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8_lossy(&output);

    // Header line: "tavily — 3 skills"
    assert!(
        stdout.contains("tavily"),
        "namespace help should show namespace name; got: {stdout:?}"
    );
    assert!(
        stdout.contains("3 skills"),
        "namespace help header should show skill count; got: {stdout:?}"
    );

    // Individual skills should be listed with descriptions.
    assert!(
        stdout.contains("tavily search"),
        "tavily search should appear in namespace help; got: {stdout:?}"
    );
    assert!(
        stdout.contains("Search the web"),
        "tavily search description should appear; got: {stdout:?}"
    );
    assert!(
        stdout.contains("tavily crawl"),
        "tavily crawl should appear in namespace help; got: {stdout:?}"
    );
    assert!(
        stdout.contains("tavily extract"),
        "tavily extract should appear in namespace help; got: {stdout:?}"
    );
}

/// `creft nonexistent --help` still returns an error when nothing matches.
#[test]
fn test_namespace_help_nonexistent() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["add"])
        .write_stdin("---\nname: hello\ndescription: greet\n---\n\n```bash\necho hello\n```\n")
        .assert()
        .success();

    creft_with(&dir)
        .args(["nonexistent", "--help"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("nonexistent"));
}

/// When a skill and namespace share the same name prefix, skill help takes priority.
/// E.g., if `tavily` is both a skill name and a namespace prefix, `creft tavily --help`
/// shows the SKILL help, not the namespace listing.
#[test]
fn test_namespace_help_skill_takes_priority() {
    let dir = creft_env();

    // Add a skill literally named "tavily" (single token).
    creft_with(&dir)
        .args(["add"])
        .write_stdin(
            "---\nname: tavily\ndescription: The Tavily skill\n---\n\n```bash\necho tavily\n```\n",
        )
        .assert()
        .success();

    // Also add namespace-prefixed skills.
    creft_with(&dir)
        .args(["add"])
        .write_stdin("---\nname: tavily search\ndescription: Search the web\n---\n\n```bash\necho search\n```\n")
        .assert()
        .success();

    let output = creft_with(&dir)
        .args(["tavily", "--help"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8_lossy(&output);

    // Should show skill help for "tavily" (not namespace listing).
    // Skill help uses the description from frontmatter, not a "N skills" header.
    assert!(
        stdout.contains("The Tavily skill"),
        "skill help should take priority over namespace listing; got: {stdout:?}"
    );
    // Namespace listing would show "2 skills" — that must NOT appear.
    assert!(
        !stdout.contains("2 skills"),
        "namespace listing should not appear when skill takes priority; got: {stdout:?}"
    );
}

// ── list UX improvements ──────────────────────────────────────────────────────

/// `creft list` at root level prints "Skills:" header.
#[test]
fn test_list_has_skills_header() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["add"])
        .write_stdin("---\nname: alpha\ndescription: first\n---\n\n```bash\necho alpha\n```\n")
        .assert()
        .success();

    creft_with(&dir)
        .args(["add"])
        .write_stdin("---\nname: beta\ndescription: second\n---\n\n```bash\necho beta\n```\n")
        .assert()
        .success();

    let output = creft_with(&dir)
        .args(["list"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8_lossy(&output);
    assert!(
        stdout.contains("Skills:\n"),
        "root list should print 'Skills:' header; got: {stdout:?}"
    );
}

/// `creft list <namespace>` drill-in prints "Skills in '<namespace>':" header.
#[test]
fn test_list_drill_in_has_scoped_header() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["add"])
        .write_stdin("---\nname: tavily search\ndescription: Search the web\n---\n\n```bash\necho search\n```\n")
        .assert()
        .success();

    creft_with(&dir)
        .args(["add"])
        .write_stdin("---\nname: tavily crawl\ndescription: Crawl a website\n---\n\n```bash\necho crawl\n```\n")
        .assert()
        .success();

    let output = creft_with(&dir)
        .args(["list", "tavily"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8_lossy(&output);
    assert!(
        stdout.contains("Skills in 'tavily':"),
        "drill-in should print scoped header; got: {stdout:?}"
    );
}

/// `creft list --all` prints "Skills:" header.
#[test]
fn test_list_all_has_header() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["add"])
        .write_stdin("---\nname: hello\ndescription: greet\n---\n\n```bash\necho hello\n```\n")
        .assert()
        .success();

    let output = creft_with(&dir)
        .args(["list", "--all"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8_lossy(&output);
    assert!(
        stdout.contains("Skills:\n"),
        "--all should print 'Skills:' header; got: {stdout:?}"
    );
}

/// `creft list` output does NOT contain `(local)` or `(global)` scope annotations.
#[test]
fn test_list_no_scope_annotation() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["add"])
        .write_stdin("---\nname: hello\ndescription: greet\n---\n\n```bash\necho hello\n```\n")
        .assert()
        .success();

    let output = creft_with(&dir)
        .args(["list"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8_lossy(&output);
    assert!(
        !stdout.contains("(local)"),
        "list should not show (local) annotation; got: {stdout:?}"
    );
    assert!(
        !stdout.contains("(global)"),
        "list should not show (global) annotation; got: {stdout:?}"
    );
}

/// When skill `test` exists AND `test mutants` / `test integration` exist,
/// the root list shows `test` with `[2 subskills]`, no duplicate "test  2 skills" line.
#[test]
fn test_list_subskill_count_on_leaf_with_children() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["add"])
        .write_stdin("---\nname: test\ndescription: Run tests\n---\n\n```bash\necho test\n```\n")
        .assert()
        .success();

    creft_with(&dir)
        .args(["add"])
        .write_stdin("---\nname: test mutants\ndescription: Run mutation testing\n---\n\n```bash\necho mutants\n```\n")
        .assert()
        .success();

    creft_with(&dir)
        .args(["add"])
        .write_stdin("---\nname: test integration\ndescription: Run integration tests\n---\n\n```bash\necho integration\n```\n")
        .assert()
        .success();

    let output = creft_with(&dir)
        .args(["list"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8_lossy(&output);

    // The leaf `test` should show the subskill count.
    assert!(
        stdout.contains("[2 subskills]"),
        "test line should show [2 subskills]; got: {stdout:?}"
    );

    // No separate namespace-only line for "test" showing "2 skills".
    assert!(
        !stdout.contains("2 skills"),
        "namespace-only 'test  2 skills' line should be suppressed; got: {stdout:?}"
    );

    // test mutants must NOT appear at root level.
    assert!(
        !stdout.contains("test mutants"),
        "test mutants should not appear at root level; got: {stdout:?}"
    );
}

/// When skill `deploy` exists and `deploy canary` exists, shows `[1 subskill]` (singular).
#[test]
fn test_list_subskill_count_singular() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["add"])
        .write_stdin(
            "---\nname: deploy\ndescription: Deploy the app\n---\n\n```bash\necho deploy\n```\n",
        )
        .assert()
        .success();

    creft_with(&dir)
        .args(["add"])
        .write_stdin("---\nname: deploy canary\ndescription: Canary deploy\n---\n\n```bash\necho canary\n```\n")
        .assert()
        .success();

    let output = creft_with(&dir)
        .args(["list"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8_lossy(&output);
    assert!(
        stdout.contains("[1 subskill]"),
        "singular should show [1 subskill]; got: {stdout:?}"
    );
    // Must NOT say "1 subskills" (wrong plural).
    assert!(
        !stdout.contains("[1 subskills]"),
        "singular must not say '1 subskills'; got: {stdout:?}"
    );
}

/// A plain skill with no children shows no subskill annotation.
#[test]
fn test_list_no_subskill_count_plain_skill() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["add"])
        .write_stdin("---\nname: hello\ndescription: greet\n---\n\n```bash\necho hello\n```\n")
        .assert()
        .success();

    let output = creft_with(&dir)
        .args(["list"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8_lossy(&output);
    assert!(
        !stdout.contains("subskill"),
        "plain skill should show no subskill annotation; got: {stdout:?}"
    );
}

/// When a namespace exists with no same-named leaf, it renders with "N skills" (not subskills).
#[test]
fn test_list_namespace_without_leaf_no_subskill_annotation() {
    let dir = creft_env();

    creft_with(&dir)
        .args(["add"])
        .write_stdin("---\nname: tavily search\ndescription: Search the web\n---\n\n```bash\necho search\n```\n")
        .assert()
        .success();

    creft_with(&dir)
        .args(["add"])
        .write_stdin("---\nname: tavily crawl\ndescription: Crawl a website\n---\n\n```bash\necho crawl\n```\n")
        .assert()
        .success();

    let output = creft_with(&dir)
        .args(["list"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8_lossy(&output);
    // Namespace entry with no same-named leaf shows "2 skills".
    assert!(
        stdout.contains("2 skills"),
        "namespace without leaf should show '2 skills'; got: {stdout:?}"
    );
    // The namespace line must not show "[N subskill]" inline annotation.
    // (The footer may contain "subskills" — that is correct.)
    assert!(
        !stdout.contains("[1 subskill]") && !stdout.contains("[2 subskills]"),
        "namespace without leaf must not show [N subskill] annotation; got: {stdout:?}"
    );
}

/// `creft list` on empty store shows no "Skills:" header — early return fires first.
#[test]
fn test_list_empty_no_header() {
    let dir = creft_env();

    let output = creft_with(&dir)
        .args(["list"])
        .assert()
        .success()
        .get_output()
        .clone();

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        stderr.contains("no commands found"),
        "empty list should print 'no commands found' to stderr; got: {stderr:?}"
    );
    assert!(
        !stdout.contains("Skills:"),
        "empty list should not print Skills: header; got: {stdout:?}"
    );
}
