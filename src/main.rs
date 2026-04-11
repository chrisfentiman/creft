mod cli;
mod doctor;
mod error;
mod frontmatter;
mod help;
mod markdown;
mod model;
mod registry;
mod registry_config;
mod runner;
mod setup;
mod store;
mod style;
mod validate;

use std::io::{IsTerminal, Read};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use clap::Parser;
use error::CreftError;

fn main() {
    let ctx = match model::AppContext::from_env() {
        Ok(ctx) => ctx,
        Err(e) => {
            eprintln!("error: {}", e);
            std::process::exit(e.exit_code());
        }
    };

    let args: Vec<String> = std::env::args().skip(1).collect();

    if let Err(e) = dispatch(&ctx, args) {
        if !e.is_quiet() {
            eprintln!("error: {}", e);
        }
        std::process::exit(e.exit_code());
    }
}

fn dispatch(ctx: &model::AppContext, args: Vec<String>) -> Result<(), CreftError> {
    if args.is_empty() {
        // parse_from prints short help and exits — same as `creft -h`, matching cargo convention.
        cli::Cli::parse_from(["creft", "-h"]);
        return Ok(());
    }

    let first = &args[0];

    // `creft help <args...>`: user skills take priority over built-in subcommand names.
    // Resolve as skill first; fall back to namespace help; then let clap handle builtins.
    if first == "help" {
        let rest: Vec<String> = args[1..].to_vec();
        if !rest.is_empty() {
            if store::resolve_command(ctx, &rest).is_ok() {
                let mut rewritten = rest;
                rewritten.push("--help".to_string());
                return run_user_command(ctx, &rewritten);
            }
            let prefix: Vec<&str> = args[1..].iter().map(|s| s.as_str()).collect();
            if store::namespace_exists(ctx, &prefix).unwrap_or(false) {
                return cmd_namespace_help(ctx, &prefix);
            }
        }
        return run_builtin(ctx, None);
    }

    // Deprecated root-level commands: forward to `creft plugin <cmd>` with a warning.
    if first == "install" || first == "update" || first == "uninstall" {
        eprintln!("warning: 'creft {first}' is deprecated, use 'creft plugin {first}' instead");
        let mut rewritten = vec!["creft".to_string(), "plugin".to_string()];
        rewritten.extend(args);
        return run_builtin(ctx, Some(rewritten));
    }

    if store::is_reserved(first)
        || first == "--help"
        || first == "-h"
        || first == "--version"
        || first == "-V"
    {
        return run_builtin(ctx, None);
    }

    run_user_command(ctx, &args)
}

fn run_builtin(ctx: &model::AppContext, args: Option<Vec<String>>) -> Result<(), CreftError> {
    let cli = match args {
        Some(a) => cli::Cli::parse_from(a),
        None => cli::Cli::parse(),
    };

    match cli.command {
        cli::BuiltinCommand::Add {
            name,
            description,
            args: arg_defs,
            tags,
            force,
            no_validate,
            global,
        } => cmd_add(
            ctx,
            name,
            description,
            arg_defs,
            tags,
            force,
            no_validate,
            global,
        ),

        cli::BuiltinCommand::List {
            tag,
            all,
            namespace,
        } => cmd_list(ctx, tag, all, namespace),

        cli::BuiltinCommand::Show { name } => {
            let name = name.join(" ");
            cmd_show(ctx, &name)
        }

        cli::BuiltinCommand::Edit {
            name,
            global,
            no_validate,
        } => {
            let name = name.join(" ");
            cmd_edit(ctx, &name, global, no_validate)
        }

        cli::BuiltinCommand::Rm { name, global } => {
            let name = name.join(" ");
            cmd_rm(ctx, &name, global)
        }

        cli::BuiltinCommand::Cat { name } => {
            let name = name.join(" ");
            cmd_cat(ctx, &name)
        }

        cli::BuiltinCommand::Plugin { action } => match action {
            cli::PluginAction::Install { source, plugin } => {
                cmd_plugin_install(ctx, &source, plugin.as_deref())
            }
            cli::PluginAction::Update { name } => cmd_plugin_update(ctx, name),
            cli::PluginAction::Uninstall { name } => cmd_plugin_uninstall(ctx, &name),
            cli::PluginAction::Activate { target, global } => {
                cmd_plugin_activate(ctx, &target, global)
            }
            cli::PluginAction::Deactivate { target, global } => {
                cmd_plugin_deactivate(ctx, &target, global)
            }
            cli::PluginAction::List { name } => cmd_plugin_list(ctx, name.as_deref()),
            cli::PluginAction::Search { query } => cmd_plugin_search(ctx, &query),
        },

        cli::BuiltinCommand::Up { system, global } => cmd_up(ctx, system, global),

        cli::BuiltinCommand::Init => cmd_init(ctx),

        cli::BuiltinCommand::Doctor { name } => cmd_doctor(ctx, name),
    }
}

fn run_user_command(ctx: &model::AppContext, args: &[String]) -> Result<(), CreftError> {
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
            let _ = cli::Cli::try_parse_from(["creft", "--help"]);
            return Ok(());
        }
        match store::resolve_command(ctx, &filtered) {
            Ok((name, _, source)) => {
                let cmd = store::load_from(ctx, &name, &source)?;
                let ansi = style::use_ansi();
                print!("{}", cmd.help_text(ansi));
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
                        println!("{}", style::bold("Subcommands:", ansi));
                        let max_name = subcommands
                            .iter()
                            .map(|(def, _)| def.name.len())
                            .max()
                            .unwrap_or(0);
                        for (def, _source) in &subcommands {
                            let desc = truncate_desc(def.description.as_str(), LIST_DESC_MAX);
                            let pad = " ".repeat(max_name - def.name.len());
                            println!("  {}{}  {}", style::bold(&def.name, ansi), pad, desc);
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

    let (name, remaining, source) = store::resolve_command(ctx, &filtered)?;
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

    let run_ctx = runner::RunContext::new(Arc::clone(&cancel), cwd, extra_env, verbose, dry_run);

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
                );
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
fn cmd_namespace_help(ctx: &model::AppContext, prefix: &[&str]) -> Result<(), CreftError> {
    let ansi = style::use_ansi();
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
        style::bold(&prefix_str, ansi),
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
                println!("  {}{}  {}", style::bold(&def.name, ansi), pad, desc);
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
                    style::bold(name, ansi),
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

#[allow(clippy::too_many_arguments)]
fn cmd_add(
    ctx: &model::AppContext,
    name_override: Option<String>,
    desc_override: Option<String>,
    arg_defs: Vec<String>,
    tags: Vec<String>,
    force: bool,
    no_validate: bool,
    global: bool,
) -> Result<(), CreftError> {
    let mut input = String::new();

    if std::io::stdin().is_terminal() {
        let name = name_override.ok_or_else(|| {
            CreftError::InvalidName("--name is required when not piping from stdin".into())
        })?;
        let desc = desc_override.unwrap_or_default();

        let mut args_yaml = String::new();
        for arg_str in &arg_defs {
            let parts: Vec<&str> = arg_str.splitn(2, ':').collect();
            let arg_name = parts[0].trim();
            let arg_desc = parts.get(1).map(|s| s.trim()).unwrap_or("");
            args_yaml.push_str(&format!(
                "  - name: {}\n    description: {}\n",
                arg_name, arg_desc
            ));
        }

        let mut tags_yaml = String::new();
        if !tags.is_empty() {
            tags_yaml = format!("tags: [{}]\n", tags.join(", "));
        }

        input = format!(
            "---\nname: {}\ndescription: {}\n{}{}---\n",
            name,
            desc,
            if args_yaml.is_empty() {
                String::new()
            } else {
                format!("args:\n{}", args_yaml)
            },
            tags_yaml,
        );
    } else {
        std::io::stdin()
            .read_to_string(&mut input)
            .map_err(CreftError::Io)?;

        if name_override.is_some()
            || desc_override.is_some()
            || !arg_defs.is_empty()
            || !tags.is_empty()
        {
            let (mut def, body) = frontmatter::parse(&input)?;
            if let Some(n) = name_override {
                def.name = n;
            }
            if let Some(d) = desc_override {
                def.description = d;
            }
            input = frontmatter::serialize(&def, &body)?;
        }
    }

    if !force && !no_validate {
        let (def, body) = frontmatter::parse(&input)?;
        let (_, blocks) = markdown::extract_blocks(&body);
        let result = validate::validate_skill(&def, &blocks, Some(ctx));

        for w in &result.warnings {
            eprintln!("warning: {}", w);
        }

        if result.has_errors() {
            for e in &result.errors {
                eprintln!("error: {}", e);
            }
            return Err(CreftError::ValidationErrors(result.errors));
        }
    }

    let scope = if global {
        model::Scope::Global
    } else {
        ctx.default_write_scope()
    };
    let name = store::save(ctx, &input, force, scope)?;
    eprintln!("added: {}", name);
    Ok(())
}

fn cmd_list(
    ctx: &model::AppContext,
    tag: Option<String>,
    show_all: bool,
    namespace: Vec<String>,
) -> Result<(), CreftError> {
    use std::collections::{HashMap, HashSet};

    let all = store::list_all_with_source(ctx)?;

    if all.is_empty() {
        eprintln!("no commands found. use 'creft add' to create one.");
        return Ok(());
    }

    let prefix: Vec<&str> = namespace.iter().map(|s| s.as_str()).collect();

    // Check the unfiltered list so a tag filter that empties a valid namespace doesn't
    // produce a misleading "no skills found under '...'" message.
    if !prefix.is_empty() {
        let namespace_exists = all.iter().any(|(def, _)| {
            let parts = def.name_parts();
            parts.len() > prefix.len()
                && parts[..prefix.len()]
                    .iter()
                    .zip(prefix.iter())
                    .all(|(a, b)| a == b)
        });
        if !namespace_exists {
            eprintln!("no skills found under '{}'", prefix.join(" "));
            return Ok(());
        }
    }

    let tag_filtered: Vec<_> = if let Some(ref t) = tag {
        all.into_iter()
            .filter(|(d, _)| d.tags.iter().any(|dt| dt == t))
            .collect()
    } else {
        all
    };

    if tag_filtered.is_empty() {
        eprintln!("no commands found. use 'creft add' to create one.");
        return Ok(());
    }

    // Suppress hidden commands unless the user explicitly named a hidden prefix or
    // passed --all (which opts into seeing everything).
    let explicit_hidden = prefix.iter().any(|p| p.starts_with('_'));
    let visible: Vec<_> = if explicit_hidden || show_all {
        tag_filtered
    } else {
        tag_filtered
            .into_iter()
            .filter(|(def, _)| !def.is_hidden())
            .collect()
    };

    if show_all {
        let flat: Vec<_> = if prefix.is_empty() {
            visible
        } else {
            visible
                .into_iter()
                .filter(|(def, _)| {
                    let parts = def.name_parts();
                    parts.len() > prefix.len()
                        && parts[..prefix.len()]
                            .iter()
                            .zip(prefix.iter())
                            .all(|(a, b)| a == b)
                })
                .collect()
        };

        if flat.is_empty() {
            eprintln!("no commands found. use 'creft add' to create one.");
            return Ok(());
        }

        let ansi = style::use_ansi();
        println!("{}", style::bold("Skills:", ansi));
        println!();

        let max_name = flat.iter().map(|(d, _)| d.name.len()).max().unwrap_or(0);

        for (def, source) in &flat {
            let desc = format_skill_desc(def, source, LIST_DESC_MAX);
            let pad = " ".repeat(max_name - def.name.len());
            println!("  {}{}  {}", style::bold(&def.name, ansi), pad, desc);
        }
        return Ok(());
    }

    let entries = store::group_by_namespace(visible, &prefix);

    if entries.is_empty() {
        eprintln!("no commands found. use 'creft add' to create one.");
        return Ok(());
    }

    let ansi = style::use_ansi();

    if prefix.is_empty() {
        println!("{}", style::bold(help::ROOT_ABOUT, ansi));
        println!();
        println!(
            "{}creft <skill> [ARGS] [OPTIONS]",
            style::bold("Usage: ", ansi)
        );
        println!();
        println!("{}", style::bold("Skills:", ansi));
    } else {
        let header = format!("Skills in '{}':", prefix.join(" "));
        println!("{}", style::bold(&header, ansi));
    }
    println!();

    // When a leaf skill and a namespace share the same relative name, suppress
    // the namespace entry and annotate the leaf with "[N subskills]" instead.
    let mut namespace_map: HashMap<String, (usize, Option<String>)> = HashMap::new();
    let mut leaf_names: HashSet<String> = HashSet::new();

    for entry in &entries {
        match entry {
            model::NamespaceEntry::Namespace {
                name,
                skill_count,
                package,
            } => {
                let relative = name
                    .split_whitespace()
                    .next_back()
                    .unwrap_or(name.as_str())
                    .to_string();
                namespace_map.insert(relative, (*skill_count, package.clone()));
            }
            model::NamespaceEntry::Skill(def, _) => {
                let parts = def.name_parts();
                if let Some(relative) = parts.get(prefix.len()) {
                    leaf_names.insert((*relative).to_string());
                }
            }
        }
    }

    // Exclude suppressed namespace entries from column-width computation.
    let max_name = entries
        .iter()
        .filter_map(|e| match e {
            model::NamespaceEntry::Skill(def, _) => Some(def.name.len()),
            model::NamespaceEntry::Namespace { name, .. } => {
                let relative = name.split_whitespace().next_back().unwrap_or(name.as_str());
                if leaf_names.contains(relative) {
                    None
                } else {
                    Some(name.len())
                }
            }
        })
        .max()
        .unwrap_or(0);

    for entry in &entries {
        match entry {
            model::NamespaceEntry::Skill(def, source) => {
                let desc = format_skill_desc(def, source, LIST_DESC_MAX);
                let parts = def.name_parts();
                let relative = parts
                    .get(prefix.len())
                    .map(|s| s.to_string())
                    .unwrap_or_default();
                let suffix = if let Some((count, _)) = namespace_map.get(&relative) {
                    let label = if *count == 1 { "subskill" } else { "subskills" };
                    format!("  [{count} {label}]")
                } else {
                    String::new()
                };
                let pad = " ".repeat(max_name - def.name.len());
                println!("  {}{}  {desc}{suffix}", style::bold(&def.name, ansi), pad);
            }
            model::NamespaceEntry::Namespace {
                name,
                skill_count,
                package,
            } => {
                let relative = name.split_whitespace().next_back().unwrap_or(name.as_str());
                if leaf_names.contains(relative) {
                    continue;
                }
                let plural = if *skill_count == 1 { "skill" } else { "skills" };
                let pkg_suffix = if package.is_some() { "  [package]" } else { "" };
                let pad = " ".repeat(max_name - name.len());
                println!(
                    "  {}{}  {skill_count} {plural}{pkg_suffix}",
                    style::bold(name, ansi),
                    pad,
                );
            }
        }
    }

    if prefix.is_empty() {
        println!();
        println!("See 'creft <skill> --help' for details.");
    }

    Ok(())
}

/// Maximum description characters shown in list output.
/// Matches the visual width used by cargo's command listing.
const LIST_DESC_MAX: usize = 60;

/// Truncate a string to `max_len` characters, appending "..." if truncated.
/// Operates on character count, not byte count (handles Unicode).
fn truncate_desc(s: &str, max_len: usize) -> std::borrow::Cow<'_, str> {
    if s.chars().count() <= max_len {
        std::borrow::Cow::Borrowed(s)
    } else {
        let truncated: String = s.chars().take(max_len.saturating_sub(3)).collect();
        std::borrow::Cow::Owned(format!("{}...", truncated.trim_end()))
    }
}

/// Format the description column for a flat skill entry.
///
/// Scope annotations (`(local)` / `(global)`) are omitted — they are noise
/// for discovery. Package provenance is preserved as `(pkg: <name>)` because
/// it is actionable (users can uninstall or inspect package skills separately).
/// The description is truncated to `max_desc_len` characters before appending
/// any package annotation so the annotation is never truncated.
fn format_skill_desc(
    def: &model::CommandDef,
    source: &model::SkillSource,
    max_desc_len: usize,
) -> String {
    let desc = truncate_desc(&def.description, max_desc_len);
    match source {
        model::SkillSource::Owned(_) => desc.into_owned(),
        model::SkillSource::Package(pkg, _) => format!("{desc}  (pkg: {pkg})"),
    }
}

fn cmd_show(ctx: &model::AppContext, name: &str) -> Result<(), CreftError> {
    let args: Vec<String> = name.split_whitespace().map(String::from).collect();
    let (resolved_name, _, source) = store::resolve_command(ctx, &args)?;
    let content = store::read_raw_from(ctx, &resolved_name, &source)?;
    println!("{}", content);
    Ok(())
}

fn cmd_edit(
    ctx: &model::AppContext,
    name: &str,
    global: bool,
    no_validate: bool,
) -> Result<(), CreftError> {
    let args: Vec<String> = name.split_whitespace().map(String::from).collect();
    let (resolved_name, _, source) = store::resolve_command(ctx, &args)?;

    if let model::SkillSource::Package(_, _) = &source {
        return Err(CreftError::Setup(
            "cannot edit installed package skills -- they are read-only".into(),
        ));
    }

    let scope = if global {
        model::Scope::Global
    } else {
        match &source {
            model::SkillSource::Owned(s) => *s,
            model::SkillSource::Package(_, s) => *s,
        }
    };
    let path = store::name_to_path_in(ctx, &resolved_name, scope)?;
    if !path.exists() {
        return Err(CreftError::CommandNotFound(resolved_name));
    }

    if std::io::stdin().is_terminal() {
        // Split on whitespace so multi-word editors like "code --wait" work correctly.
        let editor_var = std::env::var("EDITOR").unwrap_or_else(|_| "vi".into());
        let mut parts = editor_var.split_whitespace();
        let binary = parts.next().unwrap_or("vi");
        let extra_args: Vec<&str> = parts.collect();

        std::process::Command::new(binary)
            .args(&extra_args)
            .arg(&path)
            .status()
            .map_err(CreftError::Io)?;
    } else {
        let mut input = String::new();
        std::io::stdin()
            .read_to_string(&mut input)
            .map_err(CreftError::Io)?;

        // Parse first to validate frontmatter before touching the file.
        let (def, body) = frontmatter::parse(&input)?;

        if !no_validate {
            let (_, blocks) = markdown::extract_blocks(&body);
            let result = validate::validate_skill(&def, &blocks, Some(ctx));

            for w in &result.warnings {
                eprintln!("warning: {}", w);
            }

            if result.has_errors() {
                for e in &result.errors {
                    eprintln!("error: {}", e);
                }
                return Err(CreftError::ValidationErrors(result.errors));
            }
        }

        // Write raw stdin content — do NOT re-serialize, to preserve agent formatting.
        std::fs::write(&path, &input).map_err(CreftError::Io)?;
        eprintln!("edited: {}", resolved_name);
    }

    Ok(())
}

fn cmd_rm(ctx: &model::AppContext, name: &str, global: bool) -> Result<(), CreftError> {
    let args: Vec<String> = name.split_whitespace().map(String::from).collect();
    let (_, _, source) = store::resolve_command(ctx, &args)?;

    if let model::SkillSource::Package(_, _) = &source {
        return Err(CreftError::Setup(
            "cannot remove individual skills from an installed package -- use 'creft uninstall <package>' instead".into(),
        ));
    }

    let scope = if global {
        model::Scope::Global
    } else {
        match &source {
            model::SkillSource::Owned(s) => *s,
            model::SkillSource::Package(_, s) => *s,
        }
    };
    store::remove_in(ctx, name, scope)?;
    eprintln!("removed: {}", name);
    Ok(())
}

fn cmd_cat(ctx: &model::AppContext, name: &str) -> Result<(), CreftError> {
    let args: Vec<String> = name.split_whitespace().map(String::from).collect();
    let (resolved_name, _, source) = store::resolve_command(ctx, &args)?;
    let cmd = store::load_from(ctx, &resolved_name, &source)?;
    for block in &cmd.blocks {
        println!("{}", block.code);
    }
    Ok(())
}

fn cmd_plugin_install(
    ctx: &model::AppContext,
    source: &str,
    plugin: Option<&str>,
) -> Result<(), CreftError> {
    let pkg = registry::plugin_install(ctx, source, plugin)?;
    eprintln!(
        "installed: {} ({})",
        pkg.manifest.name, pkg.manifest.version
    );
    Ok(())
}

fn cmd_plugin_update(ctx: &model::AppContext, name: Option<String>) -> Result<(), CreftError> {
    match name {
        Some(n) => {
            let pkg = registry::plugin_update(ctx, &n)?;
            eprintln!("updated: {} ({})", pkg.manifest.name, pkg.manifest.version);
        }
        None => {
            let results = registry::plugin_update_all(ctx)?;
            if results.is_empty() {
                eprintln!("no plugins installed");
                return Ok(());
            }
            for result in results {
                match result {
                    Ok(pkg) => {
                        eprintln!("updated: {} ({})", pkg.manifest.name, pkg.manifest.version)
                    }
                    Err(e) => eprintln!("error: {}", e),
                }
            }
        }
    }
    Ok(())
}

fn cmd_plugin_uninstall(ctx: &model::AppContext, name: &str) -> Result<(), CreftError> {
    registry::plugin_uninstall(ctx, name)?;
    eprintln!("uninstalled: {}", name);
    Ok(())
}

fn cmd_plugin_activate(
    ctx: &model::AppContext,
    _target: &str,
    _global: bool,
) -> Result<(), CreftError> {
    // Stage 2: activation model.
    let _ = ctx;
    Err(CreftError::Setup(
        "creft plugin activate is not yet implemented".into(),
    ))
}

fn cmd_plugin_deactivate(
    ctx: &model::AppContext,
    _target: &str,
    _global: bool,
) -> Result<(), CreftError> {
    // Stage 2: activation model.
    let _ = ctx;
    Err(CreftError::Setup(
        "creft plugin deactivate is not yet implemented".into(),
    ))
}

fn cmd_plugin_list(ctx: &model::AppContext, name: Option<&str>) -> Result<(), CreftError> {
    let plugins_dir = ctx.plugins_dir()?;
    if !plugins_dir.exists() {
        eprintln!("no plugins installed");
        return Ok(());
    }

    match name {
        Some(plugin_name) => {
            let plugin_dir = plugins_dir.join(plugin_name);
            if !plugin_dir.exists() {
                return Err(CreftError::PackageNotFound(plugin_name.to_string()));
            }
            let skills = registry::list_plugin_skills_in(ctx, plugin_name)?;
            if skills.is_empty() {
                eprintln!("no commands found in plugin '{}'", plugin_name);
            } else {
                for skill in &skills {
                    println!("{}", skill.name);
                }
            }
        }
        None => {
            let entries = std::fs::read_dir(&plugins_dir)?;
            let mut found = false;
            for entry in entries {
                let entry = entry?;
                if entry.file_type()?.is_dir()
                    && let Some(name) = entry.file_name().to_str()
                {
                    println!("{}", name);
                    found = true;
                }
            }
            if !found {
                eprintln!("no plugins installed");
            }
        }
    }
    Ok(())
}

fn cmd_plugin_search(ctx: &model::AppContext, _query: &[String]) -> Result<(), CreftError> {
    // Stage 5: search and discovery.
    let _ = ctx;
    Err(CreftError::Setup(
        "creft plugin search is not yet implemented".into(),
    ))
}

fn cmd_up(ctx: &model::AppContext, system: Option<String>, global: bool) -> Result<(), CreftError> {
    let cwd = ctx.cwd.clone();

    if let Some(name) = system {
        let sys = setup::System::from_name(&name).ok_or_else(|| {
            CreftError::InvalidName(format!(
                "unknown system '{}'. supported: claude-code, cursor, windsurf, aider, copilot, codex, gemini",
                name
            ))
        })?;
        eprintln!(
            "installing creft instructions for {}...",
            sys.display_name()
        );
        setup::install(ctx, sys, &cwd, global)?;
    } else if global {
        // Aider global requires a manual config step, so it's excluded here.
        let global_systems = [
            setup::System::ClaudeCode,
            setup::System::Codex,
            setup::System::Gemini,
        ];
        eprintln!("installing creft instructions globally...");
        for sys in &global_systems {
            eprintln!();
            eprintln!("{}:", sys.display_name());
            match setup::install(ctx, *sys, &cwd, true) {
                Ok(_) => {}
                Err(e) => eprintln!("  error: {}", e),
            }
        }
    } else {
        let detected = setup::detect_systems(&cwd);
        if detected.is_empty() {
            eprintln!("no coding AI systems detected in current directory.");
            eprintln!("specify one explicitly: creft up <system>");
            eprintln!();
            eprintln!("supported systems:");
            for sys in setup::System::all() {
                eprintln!("  {:14} {}", sys.name(), sys.display_name());
            }
            return Ok(());
        }

        eprintln!(
            "detected {} system(s), installing creft instructions...",
            detected.len()
        );
        for sys in &detected {
            eprintln!();
            eprintln!("{}:", sys.display_name());
            match setup::install(ctx, *sys, &cwd, false) {
                Ok(_) => {}
                Err(e) => eprintln!("  error: {}", e),
            }
        }
    }

    eprintln!();
    eprintln!("done. the LLM now knows about creft.");
    Ok(())
}

fn cmd_doctor(ctx: &model::AppContext, name: Vec<String>) -> Result<(), CreftError> {
    if name.is_empty() {
        let results = doctor::run_global_check(ctx);
        doctor::render_global(&results);
        if doctor::has_failures(&results) {
            std::process::exit(1);
        }
        Ok(())
    } else {
        let (resolved_name, _, source) = store::resolve_command(ctx, &name)?;
        let report = doctor::run_skill_check(ctx, &resolved_name, &source)?;
        doctor::render_skill(&report);
        if doctor::report_has_failures(&report) {
            std::process::exit(1);
        }
        Ok(())
    }
}

fn cmd_init(ctx: &model::AppContext) -> Result<(), CreftError> {
    let cwd = ctx.cwd.clone();

    if store::has_local_root(&cwd).is_some() {
        eprintln!("already initialized: {}", cwd.join(".creft").display());
        return Ok(());
    }

    if let Some(parent_root) = store::find_parent_local_root(&cwd) {
        eprintln!(
            "note: parent directory already has local skills at {}",
            parent_root.display()
        );
        eprintln!("creating nested .creft/ in current directory anyway");
    }

    let target = cwd.join(".creft").join("commands");
    std::fs::create_dir_all(&target).map_err(CreftError::Io)?;

    eprintln!("created: {}", target.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::truncate_desc;
    #[allow(unused_imports)]
    use pretty_assertions::{assert_eq, assert_ne};

    #[test]
    fn test_truncate_desc_empty_string() {
        let result = truncate_desc("", 60);
        assert_eq!(result, "");
    }

    #[test]
    fn test_truncate_desc_at_max_not_truncated() {
        let s = "a".repeat(60);
        let result = truncate_desc(&s, 60);
        assert_eq!(
            result.as_ref(),
            s.as_str(),
            "should not truncate at exactly max_len"
        );
        // Must be borrowed, not owned — no allocation when no truncation needed.
        assert!(matches!(result, std::borrow::Cow::Borrowed(_)));
    }

    #[test]
    fn test_truncate_desc_under_max_not_truncated() {
        let s = "Short description";
        let result = truncate_desc(s, 60);
        assert_eq!(result.as_ref(), s);
    }

    #[test]
    fn test_truncate_desc_over_max_truncated() {
        let s = "a".repeat(100);
        let result = truncate_desc(&s, 60);
        assert!(
            result.ends_with("..."),
            "truncated string should end with '...'; got: {result:?}"
        );
        assert!(
            result.chars().count() <= 60,
            "truncated string should be at most 60 chars; got {} chars",
            result.chars().count()
        );
    }

    #[test]
    fn test_truncate_desc_unicode_safe() {
        // Each '中' is 3 bytes but 1 char. With 20 such chars = 20 char-count.
        // Truncating at 10 chars: take(7) = 7 chars + "..." = 10 chars total.
        let s = "中".repeat(20);
        let result = truncate_desc(&s, 10);
        assert!(
            result.ends_with("..."),
            "unicode truncation should end with '...'"
        );
        assert_eq!(
            result.chars().count(),
            10,
            "truncated unicode string should be exactly 10 chars"
        );
    }

    #[test]
    fn test_truncate_desc_trims_trailing_space_before_dots() {
        // If truncation point falls after a space, trim_end removes it before "...".
        let s = "hello world foo bar baz"; // 23 chars
        // max_len = 12: take(9) = "hello wor", trim_end = "hello wor", + "..." = "hello wor..."
        let result = truncate_desc(s, 12);
        assert!(result.ends_with("..."));
        assert!(
            !result.contains("  ..."),
            "should not have double space before ..."
        );
    }
}
