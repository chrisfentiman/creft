use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use yansi::Paint;

use crate::cmd::skill::render_namespace_listing;
use crate::error::CreftError;
use crate::model::{AppContext, Scope, SkillSource};
use crate::namespace::skill_namespace;
use crate::search;
use crate::settings::Settings;
use crate::wrap::{MAX_WIDTH, wrap_description, wrap_text};
use crate::{frontmatter, runner, shell, store};

pub fn run_user_command(ctx: &AppContext, args: &[String]) -> Result<(), CreftError> {
    let has_help = args.iter().any(|a| a == "--help" || a == "-h");
    let has_docs = args
        .iter()
        .any(|a| a == "--docs" || a.starts_with("--docs="));
    let dry_run = args.iter().any(|a| a == "--dry-run");
    let verbose = args.iter().any(|a| a == "--verbose" || a == "-v");

    // Extract a query from `--docs <value>` or `--docs=<value>` before filtering.
    // The returned index (if any) is the position of the query value in `args` for
    // the space-separated `--docs value` form — that position must be excluded from
    // `filtered` or the query value would be mistaken for part of the command name.
    let (docs_query, docs_query_index) = extract_docs_query(args);

    // Filter out meta-flags before resolving command name so they are not
    // mistakenly matched as part of the command name or passed as remaining args.
    // If you add a filter here, update RESERVED_FLAG_NAMES in validate.rs.
    let filtered: Vec<String> = args
        .iter()
        .enumerate()
        .filter(|(i, a)| {
            *a != "--help"
                && *a != "-h"
                && !a.starts_with("--docs")
                && *a != "--dry-run"
                && *a != "--verbose"
                && *a != "-v"
                && Some(*i) != docs_query_index
        })
        .map(|(_, a)| a.clone())
        .collect();

    if has_docs {
        if filtered.is_empty() {
            return super::skill::cmd_list(ctx, None, false, false, vec![]);
        }
        match store::resolve_command(ctx, &filtered) {
            Ok((name, _, source)) => {
                // When a search query is present, search the namespace index
                // rather than rendering the full doc text.
                if let Some(ref query) = docs_query {
                    return search_skill_docs(ctx, &name, query);
                }

                let raw = store::read_raw_from(ctx, &name, &source)?;
                let rendered = render_skill_docs(&name, &raw);
                print!("{}", rendered);
                // Show subcommands for namespace skills, mirroring --help behavior.
                if store::has_subcommands(ctx, &name)? {
                    let all_subcommands = store::list_direct_subcommands(ctx, &name)?;
                    let subcommands: Vec<_> = all_subcommands
                        .into_iter()
                        .filter(|(def, _)| !def.is_hidden())
                        .collect();
                    if !subcommands.is_empty() {
                        let prefix_strip = format!("{} ", name);
                        let display_names: Vec<&str> = subcommands
                            .iter()
                            .map(|(def, _)| {
                                def.name
                                    .strip_prefix(prefix_strip.as_str())
                                    .unwrap_or(def.name.as_str())
                            })
                            .collect();
                        println!();
                        println!("{}", "Skills:".bold());
                        let max_name = display_names.iter().map(|n| n.len()).max().unwrap_or(0);
                        let desc_col = 2 + max_name + 2;
                        let desc_budget = MAX_WIDTH.saturating_sub(desc_col);
                        for ((def, _source), display) in subcommands.iter().zip(&display_names) {
                            let desc =
                                wrap_description(def.description.as_str(), desc_budget, desc_col);
                            let pad = " ".repeat(max_name - display.len());
                            println!("  {}{}  {}", display.bold(), pad, desc);
                        }
                        println!();
                        println!("Run 'creft {} <skill> --docs' for more information.", name);
                    }
                }
                return Ok(());
            }
            Err(_) => {
                let prefix: Vec<&str> = filtered.iter().map(|s| s.as_str()).collect();
                if store::namespace_exists(ctx, &prefix)? {
                    return cmd_namespace_help(ctx, &prefix);
                }
                store::resolve_command(ctx, &filtered)?;
            }
        }
        return Ok(());
    }

    if has_help {
        if filtered.is_empty() {
            return super::skill::cmd_list(ctx, None, false, false, vec![]);
        }
        match store::resolve_command(ctx, &filtered) {
            Ok((name, _, source)) => {
                let cmd = store::load_from(ctx, &name, &source)?;
                print!("{}", cmd.help_text());
                // If this command also acts as a namespace prefix, list its
                // direct subcommands so users can discover them from --help.
                if store::has_subcommands(ctx, &name)? {
                    let all_subcommands = store::list_direct_subcommands(ctx, &name)?;
                    let subcommands: Vec<_> = all_subcommands
                        .into_iter()
                        .filter(|(def, _)| !def.is_hidden())
                        .collect();
                    if !subcommands.is_empty() {
                        // Strip the parent namespace prefix from each child name so
                        // that `ask add` displays as just `add` under the `ask` help.
                        let prefix_strip = format!("{} ", name);
                        let display_names: Vec<&str> = subcommands
                            .iter()
                            .map(|(def, _)| {
                                def.name
                                    .strip_prefix(prefix_strip.as_str())
                                    .unwrap_or(def.name.as_str())
                            })
                            .collect();
                        println!();
                        println!("{}", "Skills:".bold());
                        let max_name = display_names.iter().map(|n| n.len()).max().unwrap_or(0);
                        let desc_col = 2 + max_name + 2;
                        let desc_budget = MAX_WIDTH.saturating_sub(desc_col);
                        for ((def, _source), display) in subcommands.iter().zip(&display_names) {
                            let desc =
                                wrap_description(def.description.as_str(), desc_budget, desc_col);
                            let pad = " ".repeat(max_name - display.len());
                            println!("  {}{}  {}", display.bold(), pad, desc);
                        }
                        println!();
                        println!("Run 'creft {} <skill> --help' for more information.", name);
                    }
                }
                return Ok(());
            }
            Err(_) => {
                // Skill resolution failed — fall back to namespace help, then propagate.
                let prefix: Vec<&str> = filtered.iter().map(|s| s.as_str()).collect();
                if store::namespace_exists(ctx, &prefix)? {
                    return cmd_namespace_help(ctx, &prefix);
                }
                store::resolve_command(ctx, &filtered)?;
            }
        }
        return Ok(());
    }

    let (name, remaining, source) = match store::resolve_command(ctx, &filtered) {
        Ok(result) => result,
        Err(e) => {
            // Bare namespace invocation: `creft <ns>` lists the namespace
            // instead of erroring, matching the behaviour of `creft <ns> --help`.
            let prefix: Vec<&str> = filtered.iter().map(|s| s.as_str()).collect();
            if store::namespace_exists(ctx, &prefix)? {
                return cmd_namespace_help(ctx, &prefix);
            }
            return Err(e);
        }
    };
    let cwd = ctx.derive_cwd(&source);
    let cwd_str = cwd.to_string_lossy().to_string();
    let cmd = store::load_from(ctx, &name, &source)?;

    let mut extra_env: Vec<(String, String)> = Vec::new();
    if store::is_local_source(&source) {
        // Local-scope skills receive their project root so they can reference
        // project-relative paths without hard-coding the directory.
        extra_env.push(("CREFT_PROJECT_ROOT".to_string(), cwd_str));
    }

    let cancel = Arc::new(AtomicBool::new(false));
    // Register the cancel flag with the SIGINT handler. Failure is intentionally
    // ignored — worst case the cancel token is never set, and cancellation falls
    // back to pipe closure (the existing behavior).
    #[cfg(unix)]
    let _ = signal_hook::flag::register(signal_hook::consts::SIGINT, Arc::clone(&cancel));

    // Load settings to resolve the persistent shell preference. A corrupt or
    // missing settings file falls back gracefully — skill execution continues
    // using the $SHELL env var.
    let settings_shell_pref = ctx
        .settings_path()
        .ok()
        .and_then(|p| Settings::load(&p).ok())
        .and_then(|s| s.get("shell").map(str::to_string));
    let plugin_name = match &source {
        SkillSource::Plugin(name) => Some(name.clone()),
        _ => None,
    };
    let store_dir = ctx
        .store_dir_for(Scope::Global)
        .unwrap_or_else(|_| std::path::PathBuf::new());

    // Resolve the optional trace writer from CREFT_TRACE_FD. The framework dup'd
    // a writeable pipe end into this process at the named fd via pre_exec. When
    // the var is unset (every terminal-driven invocation), no writer is attached
    // and trace emission is skipped with no overhead.
    //
    // Construction goes through RunContext::trace_writer_from_fd so the shape of
    // TraceWriter stays owned by the runner module — this site only plumbs the fd.
    #[cfg(unix)]
    let trace_writer = std::env::var("CREFT_TRACE_FD")
        .ok()
        .and_then(|s| s.parse::<std::os::unix::io::RawFd>().ok())
        // Reject stdin/stdout/stderr: taking ownership of fd 0/1/2 would close
        // those streams for the process when the TraceWriter is dropped.
        .filter(|&fd| fd > 2)
        .map(|fd| {
            // SAFETY: CREFT_TRACE_FD names an fd the framework dup'd into this
            // process via pre_exec before exec; ownership transferred to this
            // process at exec time. We are the unique owner of the fd from this
            // point until the returned TraceWriter is dropped at end-of-run.
            // The fd > 2 guard above ensures stdin/stdout/stderr are never claimed.
            unsafe { runner::RunContext::trace_writer_from_fd(fd) }
        });
    #[cfg(not(unix))]
    let trace_writer: Option<runner::TraceWriter> = None;

    let run_ctx = runner::RunContext::new(Arc::clone(&cancel), cwd, extra_env, verbose, dry_run)
        .with_shell_preference(shell::detect(settings_shell_pref.as_deref()))
        .with_skill_name(name.clone())
        .with_plugin(plugin_name)
        .with_store_dir(store_dir)
        .with_trace_writer(trace_writer);

    if run_ctx.is_verbose() || run_ctx.is_dry_run() {
        // Bind args first so render_blocks can substitute them.
        let (bound, _) = runner::parse_and_bind(&cmd, &remaining)?;
        let bound_refs: Vec<(&str, &str)> = bound
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();

        if run_ctx.is_verbose() {
            runner::render_blocks(&cmd, &bound_refs)?;
        }

        if run_ctx.is_dry_run() && !run_ctx.is_verbose() {
            // Pure dry-run path: either delegate to native dry-run or print-only.
            if cmd.def.supports_feature("dry-run") {
                // Skill handles dry-run natively — inject the env var and execute.
                let mut env = run_ctx.env().to_vec();
                env.push(("CREFT_DRY_RUN".to_string(), "1".to_string()));
                let native_ctx = runner::RunContext::new(
                    Arc::clone(&cancel),
                    run_ctx.cwd().to_path_buf(),
                    env,
                    false,
                    true,
                )
                .with_shell_preference(run_ctx.shell_preference().map(String::from));
                return runner::run(&cmd, &remaining, &native_ctx);
            } else {
                return runner::dry_run(&cmd, &remaining, &run_ctx);
            }
        }

        if run_ctx.is_dry_run() {
            // --verbose --dry-run: rendered above, do not execute.
            return Ok(());
        }
    }

    // --verbose only (render done above) or no flags: execute normally.
    let result = runner::run(&cmd, &remaining, &run_ctx);

    // When a skill's block exits non-zero, the runner is quiet (the child
    // already wrote to stderr). Print a creft-level summary so the caller
    // knows which skill failed and with what code.
    if let Err(CreftError::ExecutionFailed { code, .. }) = &result {
        eprintln!("error: '{}' exited with code {}", name, code);
    }

    result
}

/// Extract the search query from `--docs <value>` or `--docs=<value>` in the raw arg list.
///
/// Returns `(query, consumed_index)` where:
/// - `query` is `None` when `--docs` is bare or the value following `--docs` starts with `-`.
/// - `consumed_index` is `Some(i)` for the space-separated form (`--docs value`), where `i` is
///   the index of the query value in `args`. This index must be excluded from the filtered arg
///   list so the query value is not mistaken for part of the command name. For the equals form
///   (`--docs=value`), the value is part of the `--docs=` arg itself and `consumed_index` is
///   `None`.
fn extract_docs_query(args: &[String]) -> (Option<String>, Option<usize>) {
    for (i, arg) in args.iter().enumerate() {
        // `--docs=value` form: value is embedded in the same arg, no separate index to skip.
        if let Some(query) = arg.strip_prefix("--docs=")
            && !query.is_empty()
        {
            return (Some(query.to_owned()), None);
        }
        // `--docs value` form: the query is the next arg; record its index so the caller
        // can exclude it from command resolution.
        if arg == "--docs"
            && let Some(next) = args.get(i + 1)
            && !next.starts_with('-')
        {
            return (Some(next.clone()), Some(i + 1));
        }
    }
    (None, None)
}

/// Search the namespace index for a skill and render matching entries.
///
/// Derives the namespace from the skill name, loads the corresponding index
/// file, extracts content snippets for each matching entry, and prints them.
/// Prints a no-match message to stderr if nothing is found.
fn search_skill_docs(ctx: &AppContext, skill_name: &str, query: &str) -> Result<(), CreftError> {
    let ns = skill_namespace(skill_name);

    // Try global scope first, then local scope.
    let index = load_namespace_index(ctx, ns);

    let terms: Vec<&str> = query.split_whitespace().collect();

    match index {
        Some(idx) => {
            let matches = idx.search(query);
            let results: Vec<search::snippet::SnippetResult> = matches
                .into_iter()
                .map(|e| {
                    let snippets = search::load_skill_text(ctx, &e.name)
                        .map(|text| search::snippet::extract_snippets(&text, &terms, 2))
                        .unwrap_or_default();
                    search::snippet::SnippetResult {
                        name: e.name.clone(),
                        namespace: ns.to_owned(),
                        description: e.description.clone(),
                        snippets,
                    }
                })
                .collect();

            if search::snippet::render_snippet_results(&results, &terms, false)
                .map(|rendered| print!("{}", rendered))
                .is_none()
            {
                // Exact search found no snippets — try fuzzy fallback.
                let fuzzy_matches = idx.search_fuzzy(query);
                let mut scored: Vec<(f64, search::snippet::SnippetResult)> = fuzzy_matches
                    .into_iter()
                    .filter_map(|e| {
                        let text = search::load_skill_text(ctx, &e.name)?;
                        let score = search::tokenize::score_query(query, &text);
                        if score < search::FUZZY_THRESHOLD {
                            return None;
                        }
                        let snippets = search::snippet::extract_snippets(&text, &terms, 2);
                        Some((
                            score,
                            search::snippet::SnippetResult {
                                name: e.name.clone(),
                                namespace: ns.to_owned(),
                                description: e.description.clone(),
                                snippets,
                            },
                        ))
                    })
                    .collect();

                scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
                let fuzzy_results: Vec<search::snippet::SnippetResult> =
                    scored.into_iter().map(|(_, r)| r).collect();

                match search::snippet::render_snippet_results(&fuzzy_results, &terms, false) {
                    Some(rendered) => print!("{}", rendered),
                    None => {
                        eprintln!("no documentation matches for \"{}\"", query);
                    }
                }
            }
        }
        None => {
            eprintln!(
                "no documentation matches for \"{}\" (run 'creft up' to build the search index)",
                query
            );
        }
    }
    Ok(())
}

/// Load a namespace index, trying global scope then local scope.
///
/// Returns `None` if neither scope has a readable, valid index.
fn load_namespace_index(ctx: &AppContext, namespace: &str) -> Option<search::index::SearchIndex> {
    for scope in &[Scope::Global, Scope::Local] {
        if let Ok(Some(idx)) = search::store::load_index(ctx, namespace, *scope) {
            return Some(idx);
        }
    }
    None
}

/// Render skill documentation from the raw markdown file content.
///
/// Strips the YAML frontmatter (replaced with a bold name + description header),
/// removes all executable fenced code blocks, and applies ANSI bold to markdown
/// `#` headers. The content of ````docs` blocks is preserved — only their fence
/// delimiters are stripped. The result is ready for direct printing to stdout.
pub(crate) fn render_skill_docs(skill_name: &str, raw_content: &str) -> String {
    // Try to parse frontmatter for the name/description header. Fall back to
    // the resolved skill name when frontmatter is malformed or missing.
    let (header_name, description, body) = match frontmatter::parse(raw_content) {
        Ok((def, body)) => (def.name, def.description, body),
        Err(_) => {
            // Use whatever content follows any leading `---` block, or the whole file.
            let body = raw_content
                .trim_start_matches("---")
                .trim_start()
                .to_string();
            (skill_name.to_string(), String::new(), body)
        }
    };

    let mut out = String::new();
    out.push_str(&format!("{}\n", header_name.bold()));
    if !description.is_empty() {
        out.push_str(&wrap_text(&description, MAX_WIDTH, 0));
        out.push('\n');
    }
    out.push('\n');

    // Walk body lines: strip executable fenced blocks, preserve `docs` block
    // content (dropping only its fence delimiters), bold `#` headers.
    let stripped = strip_code_blocks(&body);

    // Collapse runs of 3+ blank lines to 2.
    let collapsed = collapse_blank_lines(&stripped);

    // Wrap prose paragraphs while leaving pre-formatted (indented) and header
    // lines unchanged.
    out.push_str(&wrap_text(&collapsed, MAX_WIDTH, 0));
    if !collapsed.is_empty() {
        out.push('\n');
    }
    out
}

/// Strip fenced code blocks from markdown body text.
///
/// - Executable blocks (bash, python, etc.) are dropped entirely including their content.
/// - `docs` blocks have their fence delimiters dropped but their content preserved.
/// - Lines outside any fence are passed through unchanged.
fn strip_code_blocks(body: &str) -> String {
    let mut out = String::new();
    // Track whether we're inside a fence and its parameters.
    let mut in_fence = false;
    let mut fence_backtick_count = 0usize;
    let mut fence_is_docs = false;

    for line in body.lines() {
        let trimmed = line.trim_start();

        if !in_fence {
            // Look for an opening fence: 3+ backticks followed by a lang tag.
            if trimmed.starts_with("```") {
                let count = trimmed.chars().take_while(|c| *c == '`').count();
                if count >= 3 {
                    let lang = trimmed[count..].trim();
                    in_fence = true;
                    fence_backtick_count = count;
                    fence_is_docs = lang == "docs";
                    // Drop the opening fence delimiter; never emit it.
                    continue;
                }
            }
            // Regular prose line — emit with optional header bolding.
            if trimmed.starts_with('#') {
                // Verify it's a proper ATX header: `#` characters followed by a space.
                let hashes = trimmed.chars().take_while(|c| *c == '#').count();
                if trimmed.as_bytes().get(hashes) == Some(&b' ') {
                    out.push_str(&format!("{}\n", line.bold()));
                    continue;
                }
            }
            out.push_str(line);
            out.push('\n');
        } else {
            // Inside a fence: look for the matching closing line.
            let closing = "`".repeat(fence_backtick_count);
            if trimmed.starts_with(closing.as_str())
                && trimmed[fence_backtick_count..].trim().is_empty()
            {
                // Closing fence found. Drop the delimiter; exit fence mode.
                in_fence = false;
                fence_backtick_count = 0;
                fence_is_docs = false;
                continue;
            }
            // Inside a docs block: emit content lines.
            // Inside any other block: drop content lines.
            if fence_is_docs {
                out.push_str(line);
                out.push('\n');
            }
        }
    }

    out
}

/// Collapse runs of 3 or more consecutive blank lines down to 2.
fn collapse_blank_lines(text: &str) -> String {
    let mut out = String::new();
    let mut blank_run = 0usize;

    for line in text.lines() {
        if line.trim().is_empty() {
            blank_run += 1;
            if blank_run <= 2 {
                out.push('\n');
            }
        } else {
            blank_run = 0;
            out.push_str(line);
            out.push('\n');
        }
    }

    out
}

/// Show namespace help: a header line followed by the grouped skill listing.
///
/// Called when `creft <namespace> --help` is used and the name resolves to a
/// namespace prefix rather than an individual skill.
pub fn cmd_namespace_help(ctx: &AppContext, prefix: &[&str]) -> Result<(), CreftError> {
    let all_skills = store::list_namespace_skills(ctx, prefix)?;

    // Suppress hidden skills unless the user explicitly named a hidden prefix.
    let explicit_hidden = prefix.iter().any(|p| p.starts_with('_'));
    let skills: Vec<_> = if explicit_hidden {
        all_skills
    } else {
        all_skills
            .into_iter()
            .filter(|(def, _)| !def.is_hidden())
            .collect()
    };

    let prefix_str = prefix.join(" ");
    let entries = store::group_by_namespace(skills, prefix);

    if entries.is_empty() {
        eprintln!("no commands found. use 'creft add' to create one.");
        return Ok(());
    }

    print!(
        "{}",
        render_namespace_listing(&entries, prefix, &prefix_str)
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use serial_test::serial;

    use super::{collapse_blank_lines, render_skill_docs, strip_code_blocks};

    // Disable ANSI for all tests so assertions compare plain text.
    fn plain(s: &str) -> String {
        // yansi wraps bold with ESC sequences; strip them for comparison.
        // Pattern: ESC [ <params> m ... ESC [ 0 m
        let mut out = String::new();
        let bytes = s.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == 0x1b && bytes.get(i + 1) == Some(&b'[') {
                // skip until 'm'
                i += 2;
                while i < bytes.len() && bytes[i] != b'm' {
                    i += 1;
                }
                i += 1; // skip 'm'
            } else {
                out.push(bytes[i] as char);
                i += 1;
            }
        }
        out
    }

    #[test]
    fn executable_code_blocks_are_stripped() {
        let body = "Some prose.\n\n```bash\necho hello\n```\n\nMore prose.\n";
        let result = strip_code_blocks(body);
        assert!(
            !result.contains("echo hello"),
            "bash block content must be removed"
        );
        assert!(!result.contains("```"), "fence delimiters must be removed");
        assert!(result.contains("Some prose."));
        assert!(result.contains("More prose."));
    }

    #[test]
    fn docs_block_content_preserved_fence_stripped() {
        let body = "Before.\n\n```docs\nThis is documentation.\n```\n\nAfter.\n";
        let result = strip_code_blocks(body);
        assert!(
            result.contains("This is documentation."),
            "docs block content must be kept"
        );
        assert!(
            !result.contains("```docs"),
            "docs opening fence must be stripped"
        );
        assert!(!result.contains("```\n"), "closing fence must be stripped");
        assert!(result.contains("Before."));
        assert!(result.contains("After."));
    }

    #[test]
    fn prose_between_blocks_preserved() {
        let body = "Intro.\n\n```bash\nstep1\n```\n\nMiddle.\n\n```python\nstep2\n```\n\nEnd.\n";
        let result = strip_code_blocks(body);
        assert!(result.contains("Intro."));
        assert!(result.contains("Middle."));
        assert!(result.contains("End."));
        assert!(!result.contains("step1"));
        assert!(!result.contains("step2"));
    }

    #[test]
    fn four_backtick_fence_stripped_by_matching_count() {
        // A 4-backtick fence must be closed by 4 backticks, not 3.
        let body = "````python\ncode here\n````\n";
        let result = strip_code_blocks(body);
        assert!(
            !result.contains("code here"),
            "4-backtick block content must be removed"
        );
        assert!(
            !result.contains("````"),
            "4-backtick fence delimiters must be removed"
        );
    }

    #[test]
    fn three_backtick_inside_four_backtick_fence_not_closing() {
        // A ``` line inside a ```` fence is not a closing delimiter.
        let body = "````bash\necho a\n```\nnot close\n````\n\nAfter.\n";
        let result = strip_code_blocks(body);
        assert!(
            !result.contains("echo a"),
            "content inside outer fence must be dropped"
        );
        assert!(
            !result.contains("not close"),
            "inner ``` must not close the outer fence"
        );
        assert!(result.contains("After."));
    }

    #[test]
    #[serial]
    fn headers_in_prose_receive_bold_markers() {
        yansi::enable();
        let body = "## Prerequisites\n\nSome text.\n";
        let result = strip_code_blocks(body);
        // When yansi is enabled, the header line must contain ANSI escape sequences.
        assert!(
            result.contains('\x1b'),
            "bold ANSI escape sequences must be present when yansi is enabled"
        );
        assert!(
            result.contains("## Prerequisites"),
            "header text must be preserved"
        );
    }

    #[test]
    #[serial]
    fn frontmatter_replaced_with_name_description_header() {
        yansi::disable();
        let raw =
            "---\nname: deploy-app\ndescription: Deploys the application.\n---\n\nSome docs.\n";
        let result = plain(&render_skill_docs("deploy-app", raw));
        assert!(result.contains("deploy-app"), "name must appear in header");
        assert!(
            result.contains("Deploys the application."),
            "description must appear"
        );
        assert!(
            result.contains("Some docs."),
            "body prose must be preserved"
        );
        assert!(
            !result.contains("---"),
            "frontmatter delimiters must not appear"
        );
        yansi::enable();
    }

    #[test]
    #[serial]
    fn skill_with_only_frontmatter_and_code_produces_header_only() {
        yansi::disable();
        let raw = "---\nname: minimal\ndescription: Minimal skill.\n---\n\n```bash\necho hi\n```\n";
        let result = plain(&render_skill_docs("minimal", raw));
        assert!(result.contains("minimal"));
        assert!(result.contains("Minimal skill."));
        assert!(!result.contains("echo hi"));
        yansi::enable();
    }

    #[test]
    #[serial]
    fn no_ansi_when_yansi_disabled() {
        yansi::disable();
        let raw = "---\nname: skill\ndescription: A skill.\n---\n\n## Header\n\nProse.\n";
        let result = render_skill_docs("skill", raw);
        assert!(
            !result.contains('\x1b'),
            "no ANSI escape sequences when yansi is disabled"
        );
        yansi::enable();
    }

    #[test]
    fn three_or_more_blank_lines_collapsed_to_two() {
        let text = "a\n\n\n\nb\n";
        let result = collapse_blank_lines(text);
        // "a", then exactly 2 blank lines, then "b"
        assert_eq!(result, "a\n\n\nb\n");
    }

    #[test]
    fn two_blank_lines_not_collapsed() {
        let text = "a\n\n\nb\n";
        let result = collapse_blank_lines(text);
        assert_eq!(result, "a\n\n\nb\n");
    }

    #[test]
    #[serial]
    fn malformed_frontmatter_falls_back_to_skill_name_header() {
        yansi::disable();
        let raw = "not valid frontmatter\n\nSome prose.\n";
        let result = plain(&render_skill_docs("fallback-name", raw));
        assert!(
            result.contains("fallback-name"),
            "skill name must appear as fallback header"
        );
        yansi::enable();
    }

    // -- extract_docs_query ---------------------------------------------------

    use super::extract_docs_query;

    fn strs(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn extract_docs_query_returns_none_when_no_docs_flag() {
        let (query, idx) = extract_docs_query(&strs(&["deploy", "rollback"]));
        assert!(query.is_none());
        assert!(idx.is_none());
    }

    #[test]
    fn extract_docs_query_returns_none_for_bare_docs_flag() {
        let (query, idx) = extract_docs_query(&strs(&["deploy", "--docs"]));
        assert!(query.is_none());
        assert!(idx.is_none());
    }

    #[test]
    fn extract_docs_query_space_form_returns_value_and_index() {
        // `--docs rollback`: query is "rollback" at index 2.
        let (query, idx) = extract_docs_query(&strs(&["deploy", "--docs", "rollback"]));
        assert_eq!(query.as_deref(), Some("rollback"));
        assert_eq!(idx, Some(2));
    }

    #[test]
    fn extract_docs_query_equals_form_returns_value_without_index() {
        // `--docs=rollback`: query is embedded in the flag, no separate index consumed.
        let (query, idx) = extract_docs_query(&strs(&["deploy", "--docs=rollback"]));
        assert_eq!(query.as_deref(), Some("rollback"));
        assert!(idx.is_none());
    }

    #[test]
    fn extract_docs_query_returns_none_when_next_arg_is_flag() {
        // `--docs --force` — the next arg starts with '-', so it's not a query.
        let (query, idx) = extract_docs_query(&strs(&["deploy", "--docs", "--force"]));
        assert!(query.is_none());
        assert!(idx.is_none());
    }

    #[test]
    fn extract_docs_query_handles_multi_word_style_query() {
        // The shell typically presents this as a single arg.
        let (query, idx) = extract_docs_query(&strs(&["deploy", "--docs", "roll back"]));
        assert_eq!(query.as_deref(), Some("roll back"));
        assert_eq!(idx, Some(2));
    }

    // -- space-separated form: query value excluded from command resolution ----

    #[test]
    fn space_form_query_excluded_from_filtered_args() {
        // When args are ["deploy", "--docs", "rollback"], the filtered vec must
        // contain only ["deploy"] — not ["deploy", "rollback"] — so that command
        // resolution looks up "deploy", not "deploy rollback".
        let args = strs(&["deploy", "--docs", "rollback"]);
        let (docs_query, docs_query_index) = extract_docs_query(&args);
        assert_eq!(docs_query.as_deref(), Some("rollback"));

        let filtered: Vec<String> = args
            .iter()
            .enumerate()
            .filter(|(i, a)| {
                *a != "--help"
                    && *a != "-h"
                    && !a.starts_with("--docs")
                    && *a != "--dry-run"
                    && *a != "--verbose"
                    && *a != "-v"
                    && Some(*i) != docs_query_index
            })
            .map(|(_, a)| a.clone())
            .collect();

        assert_eq!(filtered, vec!["deploy".to_string()]);
    }

    #[test]
    fn equals_form_query_not_in_filtered_args() {
        // `--docs=rollback` is excluded because it starts with `--docs`.
        // No separate query arg to skip.
        let args = strs(&["deploy", "--docs=rollback"]);
        let (docs_query, docs_query_index) = extract_docs_query(&args);
        assert_eq!(docs_query.as_deref(), Some("rollback"));

        let filtered: Vec<String> = args
            .iter()
            .enumerate()
            .filter(|(i, a)| {
                *a != "--help"
                    && *a != "-h"
                    && !a.starts_with("--docs")
                    && *a != "--dry-run"
                    && *a != "--verbose"
                    && *a != "-v"
                    && Some(*i) != docs_query_index
            })
            .map(|(_, a)| a.clone())
            .collect();

        assert_eq!(filtered, vec!["deploy".to_string()]);
    }
}
