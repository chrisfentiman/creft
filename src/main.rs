mod catalog;
mod cli;
mod cmd;
mod doctor;
mod error;
mod frontmatter;
mod help;
mod markdown;
mod model;
mod registry;
mod registry_config;
mod runner;
mod settings;
mod setup;
mod shell;
mod store;
mod style;
mod validate;

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
                return cmd::run::run_user_command(ctx, &rewritten);
            }
            let prefix: Vec<&str> = args[1..].iter().map(|s| s.as_str()).collect();
            if store::namespace_exists(ctx, &prefix).unwrap_or(false) {
                return cmd::run::cmd_namespace_help(ctx, &prefix);
            }
        }
        return run_builtin(ctx, None);
    }

    if store::is_reserved(first)
        || first == "--help"
        || first == "-h"
        || first == "--version"
        || first == "-V"
    {
        return run_builtin(ctx, None);
    }

    cmd::run::run_user_command(ctx, &args)
}

fn run_builtin(ctx: &model::AppContext, args: Option<Vec<String>>) -> Result<(), CreftError> {
    let cli = match args {
        Some(a) => cli::Cli::parse_from(a),
        None => cli::Cli::parse(),
    };

    match cli.command {
        cli::BuiltinCommand::Cmd { action } => match action {
            None => cmd::skill::cmd_list(ctx, None, false, vec![]),
            Some(cli::CmdAction::Add {
                name,
                description,
                args: arg_defs,
                tags,
                force,
                no_validate,
                global,
            }) => cmd::skill::cmd_add(
                ctx,
                name,
                description,
                arg_defs,
                tags,
                force,
                no_validate,
                global,
            ),
            Some(cli::CmdAction::List {
                tag,
                all,
                namespace,
            }) => cmd::skill::cmd_list(ctx, tag, all, namespace),
            Some(cli::CmdAction::Show { name }) => {
                let name = name.join(" ");
                cmd::skill::cmd_show(ctx, &name)
            }
            Some(cli::CmdAction::Cat { name }) => {
                let name = name.join(" ");
                cmd::skill::cmd_cat(ctx, &name)
            }
            Some(cli::CmdAction::Rm { name, global }) => {
                let name = name.join(" ");
                cmd::skill::cmd_rm(ctx, &name, global)
            }
        },

        cli::BuiltinCommand::Plugins { action } => match action {
            None => cmd::plugin::cmd_plugin_list(ctx, None),
            Some(cli::PluginAction::Install { source, plugin }) => {
                cmd::plugin::cmd_plugin_install(ctx, &source, plugin.as_deref())
            }
            Some(cli::PluginAction::Update { name }) => cmd::plugin::cmd_plugin_update(ctx, name),
            Some(cli::PluginAction::Uninstall { name }) => {
                cmd::plugin::cmd_plugin_uninstall(ctx, &name)
            }
            Some(cli::PluginAction::Activate { target, global }) => {
                cmd::plugin::cmd_plugin_activate(ctx, &target, global)
            }
            Some(cli::PluginAction::Deactivate { target, global }) => {
                cmd::plugin::cmd_plugin_deactivate(ctx, &target, global)
            }
            Some(cli::PluginAction::List { name }) => {
                cmd::plugin::cmd_plugin_list(ctx, name.as_deref())
            }
            Some(cli::PluginAction::Search { query }) => {
                cmd::plugin::cmd_plugin_search(ctx, &query)
            }
        },

        cli::BuiltinCommand::Settings { action } => match action {
            None | Some(cli::SettingsAction::Show) => cmd::settings::cmd_settings_show(ctx),
            Some(cli::SettingsAction::Set { key, value }) => {
                cmd::settings::cmd_settings_set(ctx, &key, &value)
            }
        },

        cli::BuiltinCommand::Up { system, global } => cmd::setup::cmd_up(ctx, system, global),

        cli::BuiltinCommand::Init => cmd::init::cmd_init(ctx),

        cli::BuiltinCommand::Doctor { name } => cmd::doctor::cmd_doctor(ctx, name),
    }
}

#[cfg(test)]
mod tests {
    use super::cmd::skill::truncate_desc;
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
            "truncated string should be at most max_len chars; got {} chars",
            result.chars().count()
        );
    }

    #[test]
    fn test_truncate_desc_unicode() {
        // 30 two-byte chars — should not truncate at max_len=60.
        let s = "é".repeat(30);
        let result = truncate_desc(&s, 60);
        assert_eq!(result.as_ref(), s.as_str());
    }

    #[test]
    fn truncate_desc_over_max_ends_with_ellipsis() {
        let result = truncate_desc("hello world foo bar", 10);
        assert!(result.ends_with("..."));
        assert!(result.chars().count() <= 10);
    }
}
