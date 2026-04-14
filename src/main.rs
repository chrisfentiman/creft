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
mod registry;
mod registry_config;
mod runner;
mod settings;
mod setup;
mod shell;
mod store;
mod style;
mod validate;
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
            print!("{}", help::render(which));
            Ok(())
        }
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

        cli::Command::Remove { name, global } => {
            let name = name.join(" ");
            cmd::skill::cmd_rm(ctx, &name, global)
        }

        cli::Command::Plugin(plugin_cmd) => match plugin_cmd {
            cli::PluginCommand::Install { source, plugin } => {
                cmd::plugin::cmd_plugin_install(ctx, &source, plugin.as_deref())
            }
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

        cli::Command::Up { system, global } => cmd::setup::cmd_up(ctx, system, global),

        cli::Command::Init => cmd::init::cmd_init(ctx),

        cli::Command::Doctor { name } => cmd::doctor::cmd_doctor(ctx, name),

        cli::Command::Completions { shell } => {
            let script = completions::generate(&shell)?;
            print!("{}", script);
            Ok(())
        }
    }
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
