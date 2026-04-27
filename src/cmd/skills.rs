//! Entry point for `creft skills test` and output formatting.
//!
//! This module owns:
//! - Fixture discovery and scenario dispatch.
//! - Console output formatting (per-scenario lines, final summary).
//! - `FixtureError` rendering to stderr; both parse errors and test failures
//!   exit with code 1 via [`CreftError::Setup`].
//!
//! Structured `ScenarioOutcome`s come back from [`scenario::run`]; this module
//! decides how to render them. Output formatting does not live in
//! `skill_test/` because the framework returns values, the CLI decides how to
//! present them.

use crate::error::CreftError;
use crate::model::AppContext;
use crate::skill_test::fixture::{self, FixtureError, Scenario};
use crate::skill_test::match_pattern;
use crate::skill_test::scenario::{RunOpts, ScenarioOutcome, ScenarioStatus, run};

// ── Output formatting constants ───────────────────────────────────────────────

const PREFIX_PASS: &str = "PASS";
const PREFIX_FAIL: &str = "FAIL";
const PREFIX_TIME: &str = "TIME";
const PREFIX_SETUP: &str = "SETUP";

// ── Entry point ───────────────────────────────────────────────────────────────

/// Run `creft skills test [SKILL] [SCENARIO] [OPTIONS]`.
///
/// Discovers fixtures under the local root's `.creft/commands/`, optionally
/// filtered by `skill` (basename) and `scenario` (name). Runs every matching
/// scenario, prints per-scenario output, and returns `Ok(())` when all pass or
/// `Err(CreftError::Setup(...))` when any fail.
///
/// On non-Unix platforms this returns a setup error immediately; the
/// scenario runner itself also guards for non-Unix, but checking here avoids
/// allocating sandboxes before the OS constraint is surfaced.
pub fn cmd_skills_test(
    ctx: &AppContext,
    skill: Option<String>,
    scenario: Option<String>,
    keep: bool,
    detail: bool,
    where_: bool,
) -> Result<(), CreftError> {
    #[cfg(not(unix))]
    return Err(CreftError::Setup(
        "`creft skills test` is currently supported on Unix only (macOS, Linux); \
         Windows support is not yet implemented."
            .to_owned(),
    ));

    #[cfg(unix)]
    cmd_skills_test_unix(ctx, skill, scenario, keep, detail, where_)
}

#[cfg(unix)]
fn cmd_skills_test_unix(
    ctx: &AppContext,
    skill: Option<String>,
    scenario_filter: Option<String>,
    keep: bool,
    detail: bool,
    where_: bool,
) -> Result<(), CreftError> {
    // Require a local root — fixtures only exist in project skill trees.
    let local_root = ctx.find_local_root().ok_or_else(|| {
        CreftError::Setup(
            "no .creft/ directory found in this or any parent directory; \
             run from a project root or after `creft init`"
                .to_owned(),
        )
    })?;

    let commands_dir = local_root.join("commands");

    // Compile the SKILL pattern once before the filesystem walk. When the
    // caller supplied no skill filter, the walk returns every fixture.
    let skill_matcher = skill
        .as_deref()
        .map(match_pattern::compile)
        .transpose()
        .map_err(|e| CreftError::Setup(e.to_string()))?;

    // Discover fixture files, applying the skill basename matcher at the
    // filesystem level (before any file is opened).
    let fixture_paths = fixture::discover(&commands_dir, skill_matcher.as_ref())
        .map_err(|e| CreftError::Setup(e.to_string()))?;

    // Parse every fixture file, collecting parse errors to report before running.
    let mut all_scenarios: Vec<Scenario> = Vec::new();
    let mut parse_errors: Vec<FixtureError> = Vec::new();
    for path in &fixture_paths {
        match fixture::load_file(path) {
            Ok(scenarios) => all_scenarios.extend(scenarios),
            Err(e) => parse_errors.push(e),
        }
    }

    if !parse_errors.is_empty() {
        for e in &parse_errors {
            eprintln!("error: {e}");
        }
        return Err(CreftError::Setup(format!(
            "{} fixture file(s) failed to parse",
            parse_errors.len()
        )));
    }

    // Apply the scenario-name pattern filter (post-parse, because the name is
    // inside the file). Compile once and apply per-name via the matcher.
    if let Some(ref pattern) = scenario_filter {
        let scenario_matcher =
            match_pattern::compile(pattern).map_err(|e| CreftError::Setup(e.to_string()))?;
        all_scenarios.retain(|s| scenario_matcher.matches(&s.name));
    }

    // --where: list discovered fixtures and scenarios, then exit.
    if where_ {
        print_where_listing(&fixture_paths, &all_scenarios);
        return Ok(());
    }

    if all_scenarios.is_empty() {
        if fixture_paths.is_empty() {
            println!("0 scenarios: no fixtures found");
        } else {
            println!(
                "0 scenarios: filter matched no scenarios in {} fixture(s)",
                fixture_paths.len()
            );
        }
        return Ok(());
    }

    // Build RunOpts once; passed to every scenario::run call.
    let opts = RunOpts {
        creft_binary: None, // resolve via current_exe() inside scenario::run
        default_timeout: std::time::Duration::from_secs(60),
        keep_on_failure: keep,
    };

    // Run every scenario, accumulate outcomes.
    let mut outcomes: Vec<(Scenario, ScenarioOutcome)> = Vec::new();
    for scenario in all_scenarios {
        let outcome = run(&scenario, ctx, &opts);
        print_scenario_line(&scenario, &outcome, detail);
        outcomes.push((scenario, outcome));
    }

    print_summary(&outcomes);

    let any_failed = outcomes
        .iter()
        .any(|(_, o)| !matches!(o.status, ScenarioStatus::Pass));
    if any_failed {
        Err(CreftError::Setup("one or more scenarios failed".to_owned()))
    } else {
        Ok(())
    }
}

// ── Output helpers ────────────────────────────────────────────────────────────

/// Print the `--where` listing: one line per scenario, then a footer.
fn print_where_listing(fixture_paths: &[std::path::PathBuf], scenarios: &[Scenario]) {
    // Group scenarios by source file for the listing.
    let mut by_file: std::collections::BTreeMap<&std::path::Path, Vec<&Scenario>> =
        std::collections::BTreeMap::new();
    for s in scenarios {
        by_file.entry(&s.source_file).or_default().push(s);
    }

    // Also include fixture files that loaded zero scenarios.
    for path in fixture_paths {
        by_file.entry(path.as_path()).or_default();
    }

    for (path, file_scenarios) in &by_file {
        println!("{} ({} scenario(s))", path.display(), file_scenarios.len());
        for s in file_scenarios {
            println!("  - {}", s.name);
        }
    }

    let fixture_count = by_file.len();
    let scenario_count = scenarios.len();
    println!("\n{fixture_count} fixture(s), {scenario_count} scenario(s)");
}

/// Print one result line for a scenario.
///
/// On failure, print the failure detail block (assertion failures or error
/// message). When `detail` is true, also print the full stdout/stderr and any
/// `notes` for passing scenarios.
fn print_scenario_line(scenario: &Scenario, outcome: &ScenarioOutcome, detail: bool) {
    let skill_label = scenario
        .source_file
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("?");

    match &outcome.status {
        ScenarioStatus::Pass => {
            let block_count = outcome.trace.len();
            let prim_count: u32 = outcome
                .trace
                .iter()
                .map(|r| r.primitives.values().sum::<u32>())
                .sum();
            println!(
                "{PREFIX_PASS}  {skill_label} / {}       ({block_count} block(s), {prim_count} primitive(s))",
                scenario.name
            );
            if detail {
                print_detail_section(scenario, outcome);
            }
        }
        ScenarioStatus::Fail(failures) => {
            println!("{PREFIX_FAIL}  {skill_label} / {}", scenario.name);
            for f in failures {
                if let Some(ref loc) = f.locator {
                    println!("        {}: {}", f.kind, loc);
                } else {
                    println!("        {}:", f.kind);
                }
                println!("          expected: {}", f.expected);
                println!("          actual:   {}", f.actual);
            }
            if let Some(ref path) = outcome.kept_path {
                eprintln!("        sandbox: {}", path.display());
            }
            if detail {
                print_detail_section(scenario, outcome);
            }
        }
        ScenarioStatus::Timeout => {
            let timeout_secs = scenario.when.timeout_seconds.unwrap_or(60);
            println!(
                "{PREFIX_TIME}  {skill_label} / {}       (after {timeout_secs}s)",
                scenario.name
            );
            if detail {
                print_detail_section(scenario, outcome);
            }
        }
        ScenarioStatus::SetupError(msg) => {
            println!(
                "{PREFIX_SETUP}  {skill_label} / {}       ({})",
                scenario.name, msg
            );
            if detail {
                print_detail_section(scenario, outcome);
            }
        }
    }
}

/// Print the detail block for a scenario (stdout/stderr and optional notes).
fn print_detail_section(scenario: &Scenario, outcome: &ScenarioOutcome) {
    if let Some(ref notes) = scenario.notes {
        println!("      notes:");
        for line in notes.lines() {
            println!("        {line}");
        }
    }
    if !outcome.stdout.is_empty() {
        println!("      stdout:");
        for line in outcome.stdout.lines() {
            println!("        {line}");
        }
    }
    if !outcome.stderr.is_empty() {
        println!("      stderr:");
        for line in outcome.stderr.lines() {
            println!("        {line}");
        }
    }
}

/// Print the final summary line.
fn print_summary(outcomes: &[(Scenario, ScenarioOutcome)]) {
    let total = outcomes.len();
    let passed = outcomes
        .iter()
        .filter(|(_, o)| matches!(o.status, ScenarioStatus::Pass))
        .count();
    let failed = outcomes
        .iter()
        .filter(|(_, o)| matches!(o.status, ScenarioStatus::Fail(_)))
        .count();
    let timeout = outcomes
        .iter()
        .filter(|(_, o)| matches!(o.status, ScenarioStatus::Timeout))
        .count();
    let setup_errors = outcomes
        .iter()
        .filter(|(_, o)| matches!(o.status, ScenarioStatus::SetupError(_)))
        .count();

    let mut parts = vec![format!("{passed} passed")];
    if failed > 0 {
        parts.push(format!("{failed} failed"));
    }
    if timeout > 0 {
        parts.push(format!("{timeout} timeout"));
    }
    if setup_errors > 0 {
        parts.push(format!("{setup_errors} setup error(s)"));
    }

    println!("\n{total} scenario(s): {}", parts.join(", "));
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::error::CreftError;
    use crate::model::AppContext;

    // ── Helpers ───────────────────────────────────────────────────────────────

    /// Build a project root with `.creft/commands/` and a separate home dir.
    ///
    /// Returns `(home_tmp, project_tmp, ctx)`. The home dir is separate from the
    /// project dir so `find_local_root()` does not mistake it for `~/.creft/`.
    fn project_with_commands_dir() -> (tempfile::TempDir, tempfile::TempDir, AppContext) {
        let home_tmp = tempfile::TempDir::new().expect("home tmp");
        let project_tmp = tempfile::TempDir::new().expect("project tmp");
        std::fs::create_dir_all(project_tmp.path().join(".creft/commands"))
            .expect("create commands dir");
        let ctx = AppContext::for_test(
            home_tmp.path().to_path_buf(),
            project_tmp.path().to_path_buf(),
        );
        (home_tmp, project_tmp, ctx)
    }

    /// Write a fixture file with one or more scenarios to a project's commands dir.
    fn write_fixture(project: &tempfile::TempDir, skill_name: &str, yaml: &str) {
        let path = project
            .path()
            .join(".creft/commands")
            .join(format!("{skill_name}.test.yaml"));
        std::fs::write(&path, yaml).expect("write fixture");
    }

    // ── No-local-root tests ───────────────────────────────────────────────────

    #[test]
    fn cmd_skills_test_no_local_root_returns_setup_error() {
        let tmp = tempfile::TempDir::new().unwrap();
        // home == cwd, so find_local_root() sees global root and returns None.
        let ctx = AppContext::for_test(tmp.path().to_path_buf(), tmp.path().to_path_buf());
        let result = cmd_skills_test(&ctx, None, None, false, false, false);
        assert!(
            matches!(result, Err(CreftError::Setup(ref msg)) if msg.contains("no .creft/ directory found")),
            "no local root must return Setup error containing 'no .creft/ directory found'; got: {result:?}",
        );
    }

    #[test]
    fn cmd_skills_test_no_local_root_with_where_returns_setup_error() {
        let tmp = tempfile::TempDir::new().unwrap();
        let ctx = AppContext::for_test(tmp.path().to_path_buf(), tmp.path().to_path_buf());
        let result = cmd_skills_test(&ctx, None, None, false, false, true);
        assert!(
            matches!(result, Err(CreftError::Setup(ref msg)) if msg.contains("no .creft/ directory found")),
            "--where without local root must return Setup error; got: {result:?}",
        );
    }

    // ── Happy-path tests ──────────────────────────────────────────────────────

    /// A fixture with two passing scenarios using `sh -c` (no creft binary needed).
    const TWO_PASS_SCENARIOS: &str = r#"
- name: first-pass
  when:
    argv: ["sh", "-c", "exit 0"]
  then:
    exit_code: 0
- name: second-pass
  when:
    argv: ["sh", "-c", "exit 0"]
  then:
    exit_code: 0
"#;

    #[test]
    fn cmd_skills_test_returns_ok_when_all_scenarios_pass() {
        let (_home, project, ctx) = project_with_commands_dir();
        write_fixture(&project, "setup", TWO_PASS_SCENARIOS);

        let result = cmd_skills_test(&ctx, None, None, false, false, false);
        assert!(
            result.is_ok(),
            "two passing scenarios must return Ok(()); got: {result:?}",
        );
    }

    #[test]
    fn cmd_skills_test_skill_filter_restricts_to_matching_fixture() {
        let (_home, project, ctx) = project_with_commands_dir();
        write_fixture(&project, "setup", TWO_PASS_SCENARIOS);
        // Write a second fixture that would fail if run.
        write_fixture(
            &project,
            "other",
            r#"
- name: would-fail
  when:
    argv: ["sh", "-c", "exit 1"]
  then:
    exit_code: 0
"#,
        );

        // Filtering to "setup" loads exactly TWO_PASS_SCENARIOS and skips "other".
        let result = cmd_skills_test(&ctx, Some("setup".to_owned()), None, false, false, false);
        assert!(
            result.is_ok(),
            "skill filter 'setup' must run only setup.test.yaml and return Ok(()); got: {result:?}",
        );
    }

    #[test]
    fn cmd_skills_test_scenario_filter_restricts_to_named_scenario() {
        let (_home, project, ctx) = project_with_commands_dir();
        write_fixture(
            &project,
            "setup",
            r#"
- name: first-pass
  when:
    argv: ["sh", "-c", "exit 0"]
  then:
    exit_code: 0
- name: would-fail
  when:
    argv: ["sh", "-c", "exit 1"]
  then:
    exit_code: 0
"#,
        );

        // Filtering to scenario "first-pass" must skip "would-fail".
        let result = cmd_skills_test(
            &ctx,
            Some("setup".to_owned()),
            Some("first-pass".to_owned()),
            false,
            false,
            false,
        );
        assert!(
            result.is_ok(),
            "scenario filter 'first-pass' must run at most one scenario; got: {result:?}",
        );
    }

    #[test]
    fn cmd_skills_test_with_where_returns_ok_without_running_scenarios() {
        let (_home, project, ctx) = project_with_commands_dir();
        // Write a fixture that would fail if actually executed.
        write_fixture(
            &project,
            "setup",
            r#"
- name: would-fail
  when:
    argv: ["sh", "-c", "exit 1"]
  then:
    exit_code: 0
"#,
        );

        // --where must exit 0 even though the scenario would fail if run.
        let result = cmd_skills_test(&ctx, None, None, false, false, true);
        assert!(
            result.is_ok(),
            "--where must return Ok(()) without executing scenarios; got: {result:?}",
        );
    }

    #[test]
    fn cmd_skills_test_where_with_skill_filter_restricts_listing() {
        let (_home, project, ctx) = project_with_commands_dir();
        write_fixture(&project, "setup", TWO_PASS_SCENARIOS);
        write_fixture(
            &project,
            "other",
            r#"
- name: other-scenario
  when:
    argv: ["sh", "-c", "exit 0"]
  then: {}
"#,
        );

        // --where with SKILL filter must return Ok(()) for a fixture that exists.
        let result = cmd_skills_test(&ctx, Some("setup".to_owned()), None, false, false, true);
        assert!(
            result.is_ok(),
            "--where with skill filter must return Ok(()); got: {result:?}",
        );
    }

    // ── Filter-empties diagnostic test ────────────────────────────────────────

    #[test]
    fn cmd_skills_test_filter_that_empties_scenarios_returns_ok_not_error() {
        let (_home, project, ctx) = project_with_commands_dir();
        write_fixture(
            &project,
            "setup",
            r#"
- name: alpha
  when:
    argv: ["sh", "-c", "exit 0"]
  then: {}
"#,
        );

        // Filter to a nonexistent scenario name — setup.test.yaml exists but
        // no scenario is named "nonexistent".
        let result = cmd_skills_test(
            &ctx,
            Some("setup".to_owned()),
            Some("nonexistent".to_owned()),
            false,
            false,
            false,
        );
        assert!(
            result.is_ok(),
            "filter that empties scenarios must return Ok(()); got: {result:?}",
        );
    }
}
