use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use yansi::Paint;

use crate::cmd::skill::{LIST_DESC_MAX, format_skill_desc, truncate_desc};
use crate::error::CreftError;
use crate::model::AppContext;
use crate::settings::Settings;
use crate::{model, runner, shell, store};

pub fn run_user_command(ctx: &AppContext, args: &[String]) -> Result<(), CreftError> {
    let has_help = args.iter().any(|a| a == "--help" || a == "-h");
    let dry_run = args.iter().any(|a| a == "--dry-run");
    let verbose = args.iter().any(|a| a == "--verbose" || a == "-v");

    // Filter out meta-flags before resolving command name so they are not
    // mistakenly matched as part of the command name or passed as remaining args.
    let filtered: Vec<String> = args
        .iter()
        .filter(|a| {
            *a != "--help" && *a != "-h" && *a != "--dry-run" && *a != "--verbose" && *a != "-v"
        })
        .cloned()
        .collect();

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
                        println!();
                        println!("{}", "Subcommands:".bold());
                        let max_name = subcommands
                            .iter()
                            .map(|(def, _)| def.name.len())
                            .max()
                            .unwrap_or(0);
                        for (def, _source) in &subcommands {
                            let desc = truncate_desc(def.description.as_str(), LIST_DESC_MAX);
                            let pad = " ".repeat(max_name - def.name.len());
                            println!("  {}{}  {}", def.name.as_str().bold(), pad, desc);
                        }
                        println!();
                        println!("Run 'creft <subcommand> --help' for more information.");
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
    let run_ctx = runner::RunContext::new(Arc::clone(&cancel), cwd, extra_env, verbose, dry_run)
        .with_shell_preference(shell::detect(settings_shell_pref.as_deref()));

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
    runner::run(&cmd, &remaining, &run_ctx)
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

    let skill_count = skills.len();
    let plural = if skill_count == 1 { "skill" } else { "skills" };
    let prefix_str = prefix.join(" ");
    println!(
        "{} \u{2014} {} {}",
        prefix_str.as_str().bold(),
        skill_count,
        plural
    );
    println!();

    let entries = store::group_by_namespace(skills, prefix);

    let max_name = entries
        .iter()
        .map(|e| match e {
            model::NamespaceEntry::Skill(def, _) => def.name.len(),
            model::NamespaceEntry::Namespace { name, .. } => name.len(),
        })
        .max()
        .unwrap_or(0);

    for entry in &entries {
        match entry {
            model::NamespaceEntry::Skill(def, source) => {
                let desc = format_skill_desc(def, source, LIST_DESC_MAX);
                let pad = " ".repeat(max_name - def.name.len());
                println!("  {}{}  {}", def.name.as_str().bold(), pad, desc);
            }
            model::NamespaceEntry::Namespace {
                name,
                skill_count: count,
                package,
            } => {
                let p = if *count == 1 { "skill" } else { "skills" };
                let pkg_suffix = if package.is_some() { " [package]" } else { "" };
                let pad = " ".repeat(max_name - name.len());
                println!(
                    "  {}{}  {} {}{}",
                    name.as_str().bold(),
                    pad,
                    count,
                    p,
                    pkg_suffix,
                );
            }
        }
    }

    Ok(())
}
