mod aliases;
mod catalog;
mod cli;
mod cmd;
mod completions;
mod doctor;
mod error;
mod frontmatter;
mod help;
mod markdown;
mod model;
// Namespace module is not yet wired into binary entry points. The public API is
// exercised by module tests and consumed by search and runtime primitives.
#[allow(dead_code)]
mod namespace;
mod registry;
mod registry_config;
mod runner;
mod search;
mod settings;
mod setup;
mod shell;
mod skill_test;
mod store;
mod store_kv;
mod style;
mod validate;
mod wrap;
mod yaml;

use error::CreftError;

fn main() {
    style::init_color();

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
    // Hidden internal commands are dispatched before alias rewrite. The `_creft`
    // prefix is reserved for built-in infrastructure and must never be rewritten —
    // a misconfigured alias file must not be able to redirect internal control flow.
    if args.len() >= 2 && args[0] == "_creft" && args[1] == "welcome" {
        let force = args.iter().any(|a| a == "--force");
        return cmd::welcome::cmd_welcome(ctx, force);
    }

    // Apply alias rewrite once, ahead of every direct-dispatch branch. Rewrite
    // happens before the `help` short-circuit so that `creft bl --help` (where
    // `bl → backlog`) reaches cli::parse with `["backlog", "--help"]`. The
    // prefix match starts at args[0], so `creft help bl` is unaffected: the arg
    // vector `["help", "bl"]` does not match an alias whose `from` is `["bl"]`.
    let args = aliases::rewrite_args(ctx, args);

    // `creft help <args...>`: user skills take priority over built-in subcommand
    // names. Resolve as skill first; fall back to namespace help; then show root.
    if args.first().map(String::as_str) == Some("help") {
        return handle_help(ctx, &args[1..]);
    }

    // Build a lexopt parser from the raw args (no binary name prefix).
    let mut parser = lexopt::Parser::from_args(args.iter().map(String::as_str));

    match cli::parse(&mut parser)? {
        Some(cli::Parsed::Command(cmd)) => execute(ctx, cmd),
        Some(cli::Parsed::Help(which)) => {
            print!("{}", help::render_short(which));
            Ok(())
        }
        Some(cli::Parsed::Docs(which)) => {
            print!("{}", help::render_docs(which));
            Ok(())
        }
        Some(cli::Parsed::DocsSearch(which, query)) => dispatch_docs_search(ctx, which, &query),
        Some(cli::Parsed::DocsSearchAll(query)) => dispatch_docs_search_all(ctx, &query),
        Some(cli::Parsed::RootHelp) => cmd::skill::cmd_list(ctx, None, false, false, vec![]),
        Some(cli::Parsed::Version) => {
            println!("{}", help::render_version());
            Ok(())
        }
        None => {
            // Not a built-in — try as a user skill.
            cmd::run::run_user_command(ctx, &args)
        }
    }
}

/// Execute a parsed built-in command.
fn execute(ctx: &model::AppContext, cmd: cli::Command) -> Result<(), CreftError> {
    match cmd {
        cli::Command::Add {
            name,
            description,
            args: arg_defs,
            tags,
            force,
            no_validate,
            global,
        } => cmd::skill::cmd_add(
            ctx,
            name,
            description,
            arg_defs,
            tags,
            force,
            no_validate,
            global,
        ),

        cli::Command::AddTest { force } => cmd::skill::cmd_add_test(ctx, force),

        cli::Command::List {
            tag,
            all,
            names,
            namespace,
        } => cmd::skill::cmd_list(ctx, tag, all, names, namespace),

        cli::Command::Show { name, blocks } => {
            let name = name.join(" ");
            cmd::skill::cmd_show(ctx, &name, blocks)
        }

        cli::Command::Remove { skill, global } => cmd::skill::cmd_rm(ctx, &skill, global),

        cli::Command::RemoveTest { skill, name } => cmd::skill::cmd_remove_test(ctx, &skill, &name),

        cli::Command::Plugin(plugin_cmd) => match plugin_cmd {
            cli::PluginCommand::Install { source } => cmd::plugin::cmd_plugin_install(ctx, &source),
            cli::PluginCommand::Update { name } => cmd::plugin::cmd_plugin_update(ctx, name),
            cli::PluginCommand::Uninstall { name } => cmd::plugin::cmd_plugin_uninstall(ctx, &name),
            cli::PluginCommand::Activate { target, global } => {
                cmd::plugin::cmd_plugin_activate(ctx, &target, global)
            }
            cli::PluginCommand::Deactivate { target, global } => {
                cmd::plugin::cmd_plugin_deactivate(ctx, &target, global)
            }
            cli::PluginCommand::List { name } => cmd::plugin::cmd_plugin_list(ctx, name.as_deref()),
            cli::PluginCommand::Search { query } => cmd::plugin::cmd_plugin_search(ctx, &query),
        },

        cli::Command::Settings(settings_cmd) => match settings_cmd {
            cli::SettingsCommand::Show => cmd::settings::cmd_settings_show(ctx),
            cli::SettingsCommand::Set { key, value } => {
                cmd::settings::cmd_settings_set(ctx, &key, &value)
            }
        },

        cli::Command::Skills(skills_cmd) => match skills_cmd {
            cli::SkillsCommand::Test {
                skill,
                scenario,
                keep,
                detail,
                where_,
            } => cmd::skills::cmd_skills_test(ctx, skill, scenario, keep, detail, where_),
        },

        cli::Command::Up { system, local } => cmd::setup::cmd_up(ctx, system, local),

        cli::Command::Init => cmd::init::cmd_init(ctx),

        cli::Command::Doctor { name } => cmd::doctor::cmd_doctor(ctx, name),

        cli::Command::Completions { shell } => {
            let script = completions::generate(&shell)?;
            print!("{}", script);
            Ok(())
        }
    }
}

/// Handle `creft <builtin> --docs "<query>"`.
///
/// Loads `_builtin.idx`, searches it for the query, filters to the specific
/// built-in's entry, extracts content snippets, and prints matching lines to
/// stdout. Prints a no-match message to stderr when nothing is found.
fn dispatch_docs_search(
    ctx: &model::AppContext,
    which: help::BuiltinHelp,
    query: &str,
) -> Result<(), CreftError> {
    let dir = ctx.index_dir_for(model::Scope::Global)?;
    let path = dir.join("_builtin.idx");

    let index = if path.exists() {
        let bytes = std::fs::read(&path)?;
        search::index::SearchIndex::from_bytes(&bytes)
    } else {
        None
    };

    let cli_name = which.cli_name();
    let no_index_msg = || {
        eprintln!(
            "no documentation matches for \"{}\" (run 'creft up' to build the search index)",
            query
        );
    };

    match index {
        Some(idx) => {
            let terms: Vec<&str> = query.split_whitespace().collect();
            let docs_text = search::store::strip_code_blocks_plain(&help::render_docs(which));

            let all_matches = idx.search(query);
            let results: Vec<search::snippet::SnippetResult> = all_matches
                .into_iter()
                .filter(|e| e.name == cli_name)
                .map(|e| {
                    let snippets = search::snippet::extract_snippets(&docs_text, &terms, 2);
                    search::snippet::SnippetResult {
                        name: e.name.clone(),
                        namespace: "_builtin".to_owned(),
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
                    .filter(|e| e.name == cli_name)
                    .filter_map(|e| {
                        let (score, matched_words) =
                            search::tokenize::score_query_with_matches(query, &docs_text);
                        if score < search::FUZZY_THRESHOLD {
                            return None;
                        }
                        let snippets = search::snippet::extract_snippets_fuzzy(
                            &docs_text,
                            &terms,
                            &matched_words,
                            2,
                        );
                        Some((
                            score,
                            search::snippet::SnippetResult {
                                name: e.name.clone(),
                                namespace: "_builtin".to_owned(),
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
                        eprintln!(
                            "no documentation matches for \"{}\" (run 'creft up' to rebuild the search index)",
                            query
                        );
                    }
                }
            }
        }
        None => no_index_msg(),
    }
    Ok(())
}

/// Handle `creft --docs "<query>"`.
///
/// Searches all `.idx` files (including `_builtin.idx`) across all scopes,
/// extracts content snippets for each match, and prints results grouped by
/// namespace. Prints a no-match message to stderr when nothing is found.
fn dispatch_docs_search_all(ctx: &model::AppContext, query: &str) -> Result<(), CreftError> {
    let results = search::search_all_indexes(ctx, query);
    let terms: Vec<&str> = query.split_whitespace().collect();

    // Determine whether the index has been built at all for the no-match message.
    let index_exists = ctx
        .index_dir_for(model::Scope::Global)
        .map(|d| d.join("_builtin.idx").exists())
        .unwrap_or(false);

    match search::snippet::render_snippet_results(&results, &terms, true) {
        Some(rendered) => print!("{}", rendered),
        None => {
            if index_exists {
                eprintln!("no documentation matches for \"{}\"", query);
            } else {
                eprintln!(
                    "no documentation matches for \"{}\" (run 'creft up' to build the search index)",
                    query
                );
            }
        }
    }
    Ok(())
}

/// Handle `creft help [<name>...]`.
///
/// User skills take priority over built-in subcommand names: resolve as a skill
/// first, fall back to namespace help, then fall back to the root listing.
fn handle_help(ctx: &model::AppContext, rest: &[String]) -> Result<(), CreftError> {
    if !rest.is_empty() {
        if store::resolve_command(ctx, rest).is_ok() {
            let mut rewritten = rest.to_vec();
            rewritten.push("--help".to_string());
            return cmd::run::run_user_command(ctx, &rewritten);
        }
        let prefix: Vec<&str> = rest.iter().map(String::as_str).collect();
        if store::namespace_exists(ctx, &prefix).unwrap_or(false) {
            return cmd::run::cmd_namespace_help(ctx, &prefix);
        }
    }
    cmd::skill::cmd_list(ctx, None, false, false, vec![])
}
