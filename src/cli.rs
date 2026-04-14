//! CLI argument types and lexopt-based parser.
//!
//! The public surface is [`parse`], which consumes tokens from a [`lexopt::Parser`]
//! and returns either a built-in [`Parsed`] result or `None` (meaning the caller
//! should treat the invocation as a user skill).

use crate::help::BuiltinHelp;

/// Result of parsing built-in CLI arguments.
#[derive(Debug)]
pub(crate) enum Parsed {
    /// A built-in command with its arguments.
    Command(Command),
    /// A specific built-in's `--help` page (short form, 10-15 lines).
    Help(BuiltinHelp),
    /// A specific built-in's `--docs` page (full reference).
    Docs(BuiltinHelp),
    /// Global `--help`: the two-section listing (requires the skill registry).
    RootHelp,
    /// `--version`: print the version string and exit.
    Version,
}

/// A parsed built-in command with its arguments.
#[derive(Debug)]
pub(crate) enum Command {
    Add {
        name: Option<String>,
        description: Option<String>,
        args: Vec<String>,
        tags: Vec<String>,
        force: bool,
        no_validate: bool,
        global: bool,
    },
    List {
        tag: Option<String>,
        all: bool,
        names: bool,
        namespace: Vec<String>,
    },
    Show {
        name: Vec<String>,
        blocks: bool,
    },
    Remove {
        name: Vec<String>,
        global: bool,
    },
    Plugin(PluginCommand),
    Settings(SettingsCommand),
    Up {
        system: Option<String>,
        global: bool,
    },
    Init,
    Doctor {
        name: Vec<String>,
    },
    Completions {
        shell: String,
    },
}

/// Subcommands for `creft plugin`.
#[derive(Debug)]
pub(crate) enum PluginCommand {
    Install {
        source: String,
        plugin: Option<String>,
    },
    Update {
        name: Option<String>,
    },
    Uninstall {
        name: String,
    },
    Activate {
        target: String,
        global: bool,
    },
    Deactivate {
        target: String,
        global: bool,
    },
    List {
        name: Option<String>,
    },
    Search {
        query: Vec<String>,
    },
}

/// Subcommands for `creft settings`.
#[derive(Debug)]
pub(crate) enum SettingsCommand {
    Show,
    Set { key: String, value: String },
}

/// Errors produced during argument parsing.
#[derive(Debug, thiserror::Error)]
pub(crate) enum CliError {
    /// A general usage error (unknown flag, wrong value type, etc.).
    #[error("{0}")]
    Usage(String),

    /// The first positional argument did not match any built-in command name.
    #[error("unknown command: {0}")]
    UnknownCommand(String),

    /// A required argument was not supplied.
    #[error("missing required argument: {0}")]
    MissingArg(String),
}

impl From<lexopt::Error> for CliError {
    fn from(e: lexopt::Error) -> Self {
        CliError::Usage(e.to_string())
    }
}

/// Parse CLI arguments from a [`lexopt::Parser`] into a [`Parsed`] result.
///
/// Returns `None` when the first positional argument is not a recognized
/// built-in command name. The caller should treat this as a user skill
/// invocation and pass the original raw args to the skill runner.
///
/// `--help` / `-h` and `--version` / `-V` at the top level return
/// [`Parsed::RootHelp`] and [`Parsed::Version`] respectively.
pub(crate) fn parse(parser: &mut lexopt::Parser) -> Result<Option<Parsed>, CliError> {
    use lexopt::prelude::*;

    let first = match parser.next()? {
        None => return Ok(Some(Parsed::RootHelp)),
        Some(Long("help") | Short('h')) => return Ok(Some(Parsed::RootHelp)),
        Some(Long("version") | Short('V')) => return Ok(Some(Parsed::Version)),
        Some(Value(v)) => v.string()?,
        Some(arg) => return Err(CliError::Usage(arg.unexpected().to_string())),
    };

    match first.as_str() {
        "add" => parse_add(parser),
        "list" => parse_list(parser),
        "show" => parse_show(parser, false),
        "remove" => parse_remove(parser),
        "plugin" => parse_plugin(parser),
        "settings" => parse_settings(parser),
        "up" => parse_up(parser),
        "init" => parse_init(parser),
        "doctor" => parse_doctor(parser),
        "completions" => parse_completions(parser),

        // Not a built-in: caller should try as a user skill.
        _ => return Ok(None),
    }
    .map(Some)
}

// ── Per-command parsers ───────────────────────────────────────────────────────

fn parse_add(parser: &mut lexopt::Parser) -> Result<Parsed, CliError> {
    use lexopt::prelude::*;

    let mut name = None;
    let mut description = None;
    let mut args = Vec::new();
    let mut tags = Vec::new();
    let mut force = false;
    let mut no_validate = false;
    let mut global = false;

    while let Some(arg) = parser.next()? {
        match arg {
            Long("name") => name = Some(parser.value()?.string()?),
            Long("description") => description = Some(parser.value()?.string()?),
            Long("arg") => args.push(parser.value()?.string()?),
            Long("tag") => tags.push(parser.value()?.string()?),
            Long("force") => force = true,
            Long("no-validate") => no_validate = true,
            Short('g') | Long("global") => global = true,
            Long("help") | Short('h') => return Ok(Parsed::Help(BuiltinHelp::Add)),
            Long("docs") => return Ok(Parsed::Docs(BuiltinHelp::Add)),
            _ => return Err(CliError::Usage(arg.unexpected().to_string())),
        }
    }

    Ok(Parsed::Command(Command::Add {
        name,
        description,
        args,
        tags,
        force,
        no_validate,
        global,
    }))
}

fn parse_list(parser: &mut lexopt::Parser) -> Result<Parsed, CliError> {
    use lexopt::prelude::*;

    let mut tag = None;
    let mut all = false;
    let mut names = false;
    let mut namespace = Vec::new();

    while let Some(arg) = parser.next()? {
        match arg {
            Long("tag") => tag = Some(parser.value()?.string()?),
            Long("all") => all = true,
            Long("names") | Short('n') => names = true,
            Long("help") | Short('h') => return Ok(Parsed::Help(BuiltinHelp::List)),
            Long("docs") => return Ok(Parsed::Docs(BuiltinHelp::List)),
            Value(v) => namespace.push(v.string()?),
            _ => return Err(CliError::Usage(arg.unexpected().to_string())),
        }
    }

    Ok(Parsed::Command(Command::List {
        tag,
        all,
        names,
        namespace,
    }))
}

/// Parse `show`. When `initial_blocks` is true, only code blocks are printed.
fn parse_show(parser: &mut lexopt::Parser, initial_blocks: bool) -> Result<Parsed, CliError> {
    use lexopt::prelude::*;

    let mut name = Vec::new();
    let mut blocks = initial_blocks;

    while let Some(arg) = parser.next()? {
        match arg {
            Long("blocks") => blocks = true,
            Long("help") | Short('h') => return Ok(Parsed::Help(BuiltinHelp::Show)),
            Long("docs") => return Ok(Parsed::Docs(BuiltinHelp::Show)),
            Value(v) => name.push(v.string()?),
            _ => return Err(CliError::Usage(arg.unexpected().to_string())),
        }
    }

    Ok(Parsed::Command(Command::Show { name, blocks }))
}

fn parse_remove(parser: &mut lexopt::Parser) -> Result<Parsed, CliError> {
    use lexopt::prelude::*;

    let mut name = Vec::new();
    let mut global = false;

    while let Some(arg) = parser.next()? {
        match arg {
            Short('g') | Long("global") => global = true,
            Long("help") | Short('h') => return Ok(Parsed::Help(BuiltinHelp::Remove)),
            Long("docs") => return Ok(Parsed::Docs(BuiltinHelp::Remove)),
            Value(v) => name.push(v.string()?),
            _ => return Err(CliError::Usage(arg.unexpected().to_string())),
        }
    }

    Ok(Parsed::Command(Command::Remove { name, global }))
}

fn parse_plugin(parser: &mut lexopt::Parser) -> Result<Parsed, CliError> {
    use lexopt::prelude::*;

    let sub = match parser.next()? {
        // Bare `creft plugin` lists installed plugins.
        None => {
            return Ok(Parsed::Command(Command::Plugin(PluginCommand::List {
                name: None,
            })));
        }
        Some(Long("help") | Short('h')) => return Ok(Parsed::Help(BuiltinHelp::Plugin)),
        Some(Long("docs")) => return Ok(Parsed::Docs(BuiltinHelp::Plugin)),
        Some(Value(v)) => v.string()?,
        Some(arg) => return Err(CliError::Usage(arg.unexpected().to_string())),
    };

    match sub.as_str() {
        "install" => parse_plugin_install(parser),
        "update" => parse_plugin_update(parser),
        "uninstall" => parse_plugin_uninstall(parser),
        "activate" => parse_plugin_activate(parser),
        "deactivate" => parse_plugin_deactivate(parser),
        "list" => parse_plugin_list(parser),
        "search" => parse_plugin_search(parser),
        other => Err(CliError::UnknownCommand(format!("plugin {other}"))),
    }
}

fn parse_plugin_install(parser: &mut lexopt::Parser) -> Result<Parsed, CliError> {
    use lexopt::prelude::*;

    let mut source = None;
    let mut plugin = None;

    while let Some(arg) = parser.next()? {
        match arg {
            Short('p') | Long("plugin") => plugin = Some(parser.value()?.string()?),
            Long("help") | Short('h') => return Ok(Parsed::Help(BuiltinHelp::PluginInstall)),
            Long("docs") => return Ok(Parsed::Docs(BuiltinHelp::PluginInstall)),
            Value(v) if source.is_none() => source = Some(v.string()?),
            Value(v) => {
                return Err(CliError::Usage(format!(
                    "unexpected argument: {}",
                    v.string()?
                )));
            }
            _ => return Err(CliError::Usage(arg.unexpected().to_string())),
        }
    }

    let source = source.ok_or_else(|| {
        CliError::MissingArg("<source>\n\nUsage: creft plugin install <source>".to_string())
    })?;
    Ok(Parsed::Command(Command::Plugin(PluginCommand::Install {
        source,
        plugin,
    })))
}

fn parse_plugin_update(parser: &mut lexopt::Parser) -> Result<Parsed, CliError> {
    use lexopt::prelude::*;

    let mut name = None;

    while let Some(arg) = parser.next()? {
        match arg {
            Long("help") | Short('h') => return Ok(Parsed::Help(BuiltinHelp::PluginUpdate)),
            Long("docs") => return Ok(Parsed::Docs(BuiltinHelp::PluginUpdate)),
            Value(v) if name.is_none() => name = Some(v.string()?),
            Value(v) => {
                return Err(CliError::Usage(format!(
                    "unexpected argument: {}",
                    v.string()?
                )));
            }
            _ => return Err(CliError::Usage(arg.unexpected().to_string())),
        }
    }

    Ok(Parsed::Command(Command::Plugin(PluginCommand::Update {
        name,
    })))
}

fn parse_plugin_uninstall(parser: &mut lexopt::Parser) -> Result<Parsed, CliError> {
    use lexopt::prelude::*;

    let mut name = None;

    while let Some(arg) = parser.next()? {
        match arg {
            Long("help") | Short('h') => return Ok(Parsed::Help(BuiltinHelp::PluginUninstall)),
            Long("docs") => return Ok(Parsed::Docs(BuiltinHelp::PluginUninstall)),
            Value(v) if name.is_none() => name = Some(v.string()?),
            Value(v) => {
                return Err(CliError::Usage(format!(
                    "unexpected argument: {}",
                    v.string()?
                )));
            }
            _ => return Err(CliError::Usage(arg.unexpected().to_string())),
        }
    }

    let name = name.ok_or_else(|| {
        CliError::MissingArg("<name>\n\nUsage: creft plugin uninstall <name>".to_string())
    })?;
    Ok(Parsed::Command(Command::Plugin(PluginCommand::Uninstall {
        name,
    })))
}

fn parse_plugin_activate(parser: &mut lexopt::Parser) -> Result<Parsed, CliError> {
    use lexopt::prelude::*;

    let mut target = None;
    let mut global = false;

    while let Some(arg) = parser.next()? {
        match arg {
            Short('g') | Long("global") => global = true,
            Long("help") | Short('h') => return Ok(Parsed::Help(BuiltinHelp::PluginActivate)),
            Long("docs") => return Ok(Parsed::Docs(BuiltinHelp::PluginActivate)),
            Value(v) if target.is_none() => target = Some(v.string()?),
            Value(v) => {
                return Err(CliError::Usage(format!(
                    "unexpected argument: {}",
                    v.string()?
                )));
            }
            _ => return Err(CliError::Usage(arg.unexpected().to_string())),
        }
    }

    let target = target.ok_or_else(|| {
        CliError::MissingArg("<target>\n\nUsage: creft plugin activate <target>".to_string())
    })?;
    Ok(Parsed::Command(Command::Plugin(PluginCommand::Activate {
        target,
        global,
    })))
}

fn parse_plugin_deactivate(parser: &mut lexopt::Parser) -> Result<Parsed, CliError> {
    use lexopt::prelude::*;

    let mut target = None;
    let mut global = false;

    while let Some(arg) = parser.next()? {
        match arg {
            Short('g') | Long("global") => global = true,
            Long("help") | Short('h') => return Ok(Parsed::Help(BuiltinHelp::PluginDeactivate)),
            Long("docs") => return Ok(Parsed::Docs(BuiltinHelp::PluginDeactivate)),
            Value(v) if target.is_none() => target = Some(v.string()?),
            Value(v) => {
                return Err(CliError::Usage(format!(
                    "unexpected argument: {}",
                    v.string()?
                )));
            }
            _ => return Err(CliError::Usage(arg.unexpected().to_string())),
        }
    }

    let target = target.ok_or_else(|| {
        CliError::MissingArg("<target>\n\nUsage: creft plugin deactivate <target>".to_string())
    })?;
    Ok(Parsed::Command(Command::Plugin(
        PluginCommand::Deactivate { target, global },
    )))
}

fn parse_plugin_list(parser: &mut lexopt::Parser) -> Result<Parsed, CliError> {
    use lexopt::prelude::*;

    let mut name = None;

    while let Some(arg) = parser.next()? {
        match arg {
            Long("help") | Short('h') => return Ok(Parsed::Help(BuiltinHelp::PluginList)),
            Long("docs") => return Ok(Parsed::Docs(BuiltinHelp::PluginList)),
            Value(v) if name.is_none() => name = Some(v.string()?),
            Value(v) => {
                return Err(CliError::Usage(format!(
                    "unexpected argument: {}",
                    v.string()?
                )));
            }
            _ => return Err(CliError::Usage(arg.unexpected().to_string())),
        }
    }

    Ok(Parsed::Command(Command::Plugin(PluginCommand::List {
        name,
    })))
}

fn parse_plugin_search(parser: &mut lexopt::Parser) -> Result<Parsed, CliError> {
    use lexopt::prelude::*;

    let mut query = Vec::new();

    while let Some(arg) = parser.next()? {
        match arg {
            Long("help") | Short('h') => return Ok(Parsed::Help(BuiltinHelp::PluginSearch)),
            Long("docs") => return Ok(Parsed::Docs(BuiltinHelp::PluginSearch)),
            Value(v) => query.push(v.string()?),
            _ => return Err(CliError::Usage(arg.unexpected().to_string())),
        }
    }

    Ok(Parsed::Command(Command::Plugin(PluginCommand::Search {
        query,
    })))
}

fn parse_settings(parser: &mut lexopt::Parser) -> Result<Parsed, CliError> {
    use lexopt::prelude::*;

    let sub = match parser.next()? {
        // Bare `creft settings` shows current settings.
        None => return Ok(Parsed::Command(Command::Settings(SettingsCommand::Show))),
        Some(Long("help") | Short('h')) => return Ok(Parsed::Help(BuiltinHelp::Settings)),
        Some(Long("docs")) => return Ok(Parsed::Docs(BuiltinHelp::Settings)),
        Some(Value(v)) => v.string()?,
        Some(arg) => return Err(CliError::Usage(arg.unexpected().to_string())),
    };

    match sub.as_str() {
        "show" => parse_settings_show(parser),
        "set" => parse_settings_set(parser),
        other => Err(CliError::UnknownCommand(format!("settings {other}"))),
    }
}

fn parse_settings_show(parser: &mut lexopt::Parser) -> Result<Parsed, CliError> {
    use lexopt::prelude::*;

    if let Some(arg) = parser.next()? {
        match arg {
            Long("help") | Short('h') => return Ok(Parsed::Help(BuiltinHelp::SettingsShow)),
            Long("docs") => return Ok(Parsed::Docs(BuiltinHelp::SettingsShow)),
            _ => return Err(CliError::Usage(arg.unexpected().to_string())),
        }
    }

    Ok(Parsed::Command(Command::Settings(SettingsCommand::Show)))
}

fn parse_settings_set(parser: &mut lexopt::Parser) -> Result<Parsed, CliError> {
    use lexopt::prelude::*;

    let mut key = None;
    let mut value = None;

    while let Some(arg) = parser.next()? {
        match arg {
            Long("help") | Short('h') => return Ok(Parsed::Help(BuiltinHelp::SettingsSet)),
            Long("docs") => return Ok(Parsed::Docs(BuiltinHelp::SettingsSet)),
            Value(v) if key.is_none() => key = Some(v.string()?),
            Value(v) if value.is_none() => value = Some(v.string()?),
            Value(v) => {
                return Err(CliError::Usage(format!(
                    "unexpected argument: {}",
                    v.string()?
                )));
            }
            _ => return Err(CliError::Usage(arg.unexpected().to_string())),
        }
    }

    let key = key.ok_or_else(|| {
        CliError::MissingArg("<key>\n\nUsage: creft settings set <key> <value>".to_string())
    })?;
    let value = value.ok_or_else(|| {
        CliError::MissingArg("<value>\n\nUsage: creft settings set <key> <value>".to_string())
    })?;
    Ok(Parsed::Command(Command::Settings(SettingsCommand::Set {
        key,
        value,
    })))
}

fn parse_up(parser: &mut lexopt::Parser) -> Result<Parsed, CliError> {
    use lexopt::prelude::*;

    let mut system = None;
    let mut global = false;

    while let Some(arg) = parser.next()? {
        match arg {
            Short('g') | Long("global") => global = true,
            Long("help") | Short('h') => return Ok(Parsed::Help(BuiltinHelp::Up)),
            Long("docs") => return Ok(Parsed::Docs(BuiltinHelp::Up)),
            Value(v) if system.is_none() => system = Some(v.string()?),
            Value(v) => {
                return Err(CliError::Usage(format!(
                    "unexpected argument: {}",
                    v.string()?
                )));
            }
            _ => return Err(CliError::Usage(arg.unexpected().to_string())),
        }
    }

    Ok(Parsed::Command(Command::Up { system, global }))
}

fn parse_init(parser: &mut lexopt::Parser) -> Result<Parsed, CliError> {
    use lexopt::prelude::*;

    if let Some(arg) = parser.next()? {
        match arg {
            Long("help") | Short('h') => return Ok(Parsed::Help(BuiltinHelp::Init)),
            Long("docs") => return Ok(Parsed::Docs(BuiltinHelp::Init)),
            _ => return Err(CliError::Usage(arg.unexpected().to_string())),
        }
    }

    Ok(Parsed::Command(Command::Init))
}

fn parse_doctor(parser: &mut lexopt::Parser) -> Result<Parsed, CliError> {
    use lexopt::prelude::*;

    let mut name = Vec::new();

    while let Some(arg) = parser.next()? {
        match arg {
            Long("help") | Short('h') => return Ok(Parsed::Help(BuiltinHelp::Doctor)),
            Long("docs") => return Ok(Parsed::Docs(BuiltinHelp::Doctor)),
            Value(v) => name.push(v.string()?),
            _ => return Err(CliError::Usage(arg.unexpected().to_string())),
        }
    }

    Ok(Parsed::Command(Command::Doctor { name }))
}

fn parse_completions(parser: &mut lexopt::Parser) -> Result<Parsed, CliError> {
    use lexopt::prelude::*;

    let mut shell = None;

    while let Some(arg) = parser.next()? {
        match arg {
            Long("help") | Short('h') => return Ok(Parsed::Help(BuiltinHelp::Completions)),
            Long("docs") => return Ok(Parsed::Docs(BuiltinHelp::Completions)),
            Value(v) if shell.is_none() => shell = Some(v.string()?),
            Value(v) => {
                return Err(CliError::Usage(format!(
                    "unexpected argument: {}",
                    v.string()?
                )));
            }
            _ => return Err(CliError::Usage(arg.unexpected().to_string())),
        }
    }

    let shell = shell.ok_or_else(|| {
        CliError::MissingArg("<shell>\n\nUsage: creft completions <shell>".to_string())
    })?;
    Ok(Parsed::Command(Command::Completions { shell }))
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    fn parse_args(args: &[&str]) -> Result<Option<Parsed>, CliError> {
        let mut parser = lexopt::Parser::from_args(args.iter().copied());
        parse(&mut parser)
    }

    #[test]
    fn list_names_long_flag_sets_names_true() {
        let result = parse_args(&["list", "--names"]).unwrap().unwrap();
        let Parsed::Command(Command::List { names, .. }) = result else {
            panic!("expected Command::List");
        };
        assert!(names, "--names flag must set names=true");
    }

    #[test]
    fn list_names_short_flag_sets_names_true() {
        let result = parse_args(&["list", "-n"]).unwrap().unwrap();
        let Parsed::Command(Command::List { names, .. }) = result else {
            panic!("expected Command::List");
        };
        assert!(names, "-n flag must set names=true");
    }

    #[test]
    fn list_without_names_flag_defaults_false() {
        let result = parse_args(&["list"]).unwrap().unwrap();
        let Parsed::Command(Command::List { names, .. }) = result else {
            panic!("expected Command::List");
        };
        assert!(!names, "names must default to false when flag is absent");
    }

    #[test]
    fn completions_missing_shell_returns_missing_arg_error() {
        let result = parse_args(&["completions"]);
        assert!(
            matches!(result, Err(CliError::MissingArg(_))),
            "completions with no shell must return MissingArg; got: {result:?}",
        );
    }

    #[test]
    fn completions_with_shell_parses_correctly() {
        let result = parse_args(&["completions", "bash"]).unwrap().unwrap();
        let Parsed::Command(Command::Completions { shell }) = result else {
            panic!("expected Command::Completions");
        };
        assert_eq!(shell, "bash");
    }

    #[test]
    fn completions_help_returns_help_variant() {
        let result = parse_args(&["completions", "--help"]).unwrap().unwrap();
        assert!(
            matches!(result, Parsed::Help(crate::help::BuiltinHelp::Completions)),
            "completions --help must return Parsed::Help(Completions); got: {result:?}",
        );
    }

    #[test]
    fn completions_docs_returns_docs_variant() {
        let result = parse_args(&["completions", "--docs"]).unwrap().unwrap();
        assert!(
            matches!(result, Parsed::Docs(crate::help::BuiltinHelp::Completions)),
            "completions --docs must return Parsed::Docs(Completions); got: {result:?}",
        );
    }

    #[test]
    fn add_docs_returns_docs_variant() {
        let result = parse_args(&["add", "--docs"]).unwrap().unwrap();
        assert!(
            matches!(result, Parsed::Docs(crate::help::BuiltinHelp::Add)),
            "add --docs must return Parsed::Docs(Add); got: {result:?}",
        );
    }

    #[test]
    fn list_docs_returns_docs_variant() {
        let result = parse_args(&["list", "--docs"]).unwrap().unwrap();
        assert!(
            matches!(result, Parsed::Docs(crate::help::BuiltinHelp::List)),
            "list --docs must return Parsed::Docs(List); got: {result:?}",
        );
    }

    #[test]
    fn doctor_docs_returns_docs_variant() {
        let result = parse_args(&["doctor", "--docs"]).unwrap().unwrap();
        assert!(
            matches!(result, Parsed::Docs(crate::help::BuiltinHelp::Doctor)),
            "doctor --docs must return Parsed::Docs(Doctor); got: {result:?}",
        );
    }

    #[test]
    fn plugin_docs_returns_docs_variant() {
        let result = parse_args(&["plugin", "--docs"]).unwrap().unwrap();
        assert!(
            matches!(result, Parsed::Docs(crate::help::BuiltinHelp::Plugin)),
            "plugin --docs must return Parsed::Docs(Plugin); got: {result:?}",
        );
    }

    #[test]
    fn settings_docs_returns_docs_variant() {
        let result = parse_args(&["settings", "--docs"]).unwrap().unwrap();
        assert!(
            matches!(result, Parsed::Docs(crate::help::BuiltinHelp::Settings)),
            "settings --docs must return Parsed::Docs(Settings); got: {result:?}",
        );
    }

    #[test]
    fn up_docs_returns_docs_variant() {
        let result = parse_args(&["up", "--docs"]).unwrap().unwrap();
        assert!(
            matches!(result, Parsed::Docs(crate::help::BuiltinHelp::Up)),
            "up --docs must return Parsed::Docs(Up); got: {result:?}",
        );
    }

    #[test]
    fn init_docs_returns_docs_variant() {
        let result = parse_args(&["init", "--docs"]).unwrap().unwrap();
        assert!(
            matches!(result, Parsed::Docs(crate::help::BuiltinHelp::Init)),
            "init --docs must return Parsed::Docs(Init); got: {result:?}",
        );
    }
}
