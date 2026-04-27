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
    /// A specific built-in's `--docs` page filtered by a search query.
    DocsSearch(BuiltinHelp, String),
    /// Cross-source `--docs` search from the root level.
    DocsSearchAll(String),
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
    /// `creft add test [--force]`
    ///
    /// Reads a test scenario from stdin (frontmatter + YAML body) and appends
    /// it to the skill's fixture file. `force` allows replacing an existing
    /// scenario with the same name; without it, a name collision is an error.
    AddTest {
        /// Replace an existing scenario with the same `name` instead of erroring.
        /// When `force` is set but no scenario with the supplied `name` exists,
        /// `cmd_add_test` writes a stderr warning and proceeds to append.
        force: bool,
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
    Skills(SkillsCommand),
    Up {
        system: Option<String>,
        local: bool,
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
    Install { source: String },
    Update { name: Option<String> },
    Uninstall { name: String },
    Activate { target: String, global: bool },
    Deactivate { target: String, global: bool },
    List { name: Option<String> },
    Search { query: Vec<String> },
}

/// Subcommands for `creft settings`.
#[derive(Debug)]
pub(crate) enum SettingsCommand {
    Show,
    Set { key: String, value: String },
}

/// Subcommands for the `creft skills` namespace.
///
/// New subcommands (e.g. `Lint`, `Inspect`) are added as new variants without
/// affecting existing dispatch.
#[derive(Debug)]
pub(crate) enum SkillsCommand {
    Test {
        skill: Option<String>,
        scenario: Option<String>,
        keep: bool,
        detail: bool,
        where_: bool,
    },
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
        Some(Long("docs")) => {
            // `creft --docs <query>` or `creft --docs=<query>` → cross-source search.
            // `creft --docs` (bare) → root listing.
            if let Some(val) = parser.optional_value() {
                return Ok(Some(Parsed::DocsSearchAll(val.string()?)));
            }
            match parser.next()? {
                Some(Value(v)) => return Ok(Some(Parsed::DocsSearchAll(v.string()?))),
                Some(arg) => {
                    // A flag follows --docs: treat as bare --docs (root listing).
                    // The flag is consumed here; it is not re-inserted. For root-level
                    // --docs the only valid follow-on is a query value, so an unexpected
                    // flag is treated as "no query" and we return RootHelp.
                    drop(arg);
                    return Ok(Some(Parsed::RootHelp));
                }
                None => return Ok(Some(Parsed::RootHelp)),
            }
        }
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
        "skills" => parse_skills(parser),
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

/// Parse `--docs` with an optional query value.
///
/// Handles both `--docs=query` (value attached via `=`) and `--docs query`
/// (value as a separate argument). When a query follows, returns
/// [`Parsed::DocsSearch`]. When `--docs` is bare or followed by another flag,
/// returns [`Parsed::Docs`].
///
/// Must be called after `Long("docs")` has been matched but before the next
/// call to `parser.next()`. Calling `parser.optional_value()` first drains any
/// `=value` that lexopt stored internally — failing to do so would cause the
/// subsequent `parser.next()` to return an `UnexpectedValue` error.
fn docs_or_search(parser: &mut lexopt::Parser, which: BuiltinHelp) -> Result<Parsed, CliError> {
    use lexopt::prelude::*;

    // Check for `--docs=query` form first.
    if let Some(val) = parser.optional_value() {
        return Ok(Parsed::DocsSearch(which, val.string()?));
    }
    // Check for `--docs query` form: peek at the next token.
    match parser.next()? {
        Some(Value(v)) => Ok(Parsed::DocsSearch(which, v.string()?)),
        // A flag follows --docs: bare --docs, not a search.
        Some(_) => Ok(Parsed::Docs(which)),
        None => Ok(Parsed::Docs(which)),
    }
}

fn parse_add(parser: &mut lexopt::Parser) -> Result<Parsed, CliError> {
    use lexopt::prelude::*;

    let mut name = None;
    let mut description = None;
    let mut args = Vec::new();
    let mut tags = Vec::new();
    let mut force = false;
    let mut no_validate = false;
    let mut global = false;

    // Peek at the first token. If it is the `test` sub-command keyword, route
    // to parse_add_test and leave the rest of argv in the parser. Otherwise
    // treat it as a flag (or an error for any other positional).
    //
    // `parse_plugin` and `parse_settings` use the same consume-and-dispatch
    // shape: consume the first token, branch on its value, delegate to a child
    // parser with the remaining argv.
    match parser.next()? {
        None => {
            return Ok(Parsed::Command(Command::Add {
                name,
                description,
                args,
                tags,
                force,
                no_validate,
                global,
            }));
        }
        Some(Value(v)) => {
            let s = v.string()?;
            if s == "test" {
                return parse_add_test(parser);
            }
            return Err(CliError::Usage(format!("unexpected argument: {s}")));
        }
        Some(Long("name")) => name = Some(parser.value()?.string()?),
        Some(Long("description")) => description = Some(parser.value()?.string()?),
        Some(Long("arg")) => args.push(parser.value()?.string()?),
        Some(Long("tag")) => tags.push(parser.value()?.string()?),
        Some(Long("force")) => force = true,
        Some(Long("no-validate")) => no_validate = true,
        Some(Short('g') | Long("global")) => global = true,
        Some(Long("help") | Short('h')) => return Ok(Parsed::Help(BuiltinHelp::Add)),
        Some(Long("docs")) => return docs_or_search(parser, BuiltinHelp::Add),
        Some(arg) => return Err(CliError::Usage(arg.unexpected().to_string())),
    }

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
            Long("docs") => return docs_or_search(parser, BuiltinHelp::Add),
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

fn parse_add_test(parser: &mut lexopt::Parser) -> Result<Parsed, CliError> {
    use lexopt::prelude::*;

    let mut force = false;

    while let Some(arg) = parser.next()? {
        match arg {
            Long("force") => force = true,
            Long("help") | Short('h') => return Ok(Parsed::Help(BuiltinHelp::AddTest)),
            Long("docs") => return docs_or_search(parser, BuiltinHelp::AddTest),
            _ => return Err(CliError::Usage(arg.unexpected().to_string())),
        }
    }

    Ok(Parsed::Command(Command::AddTest { force }))
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
            Long("docs") => return docs_or_search(parser, BuiltinHelp::List),
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
            Long("docs") => return docs_or_search(parser, BuiltinHelp::Show),
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
            Long("docs") => return docs_or_search(parser, BuiltinHelp::Remove),
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
        Some(Long("docs")) => return docs_or_search(parser, BuiltinHelp::Plugin),
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

    while let Some(arg) = parser.next()? {
        match arg {
            Long("help") | Short('h') => return Ok(Parsed::Help(BuiltinHelp::PluginInstall)),
            Long("docs") => return docs_or_search(parser, BuiltinHelp::PluginInstall),
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
    })))
}

fn parse_plugin_update(parser: &mut lexopt::Parser) -> Result<Parsed, CliError> {
    use lexopt::prelude::*;

    let mut name = None;

    while let Some(arg) = parser.next()? {
        match arg {
            Long("help") | Short('h') => return Ok(Parsed::Help(BuiltinHelp::PluginUpdate)),
            Long("docs") => return docs_or_search(parser, BuiltinHelp::PluginUpdate),
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
            Long("docs") => return docs_or_search(parser, BuiltinHelp::PluginUninstall),
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
            Long("docs") => return docs_or_search(parser, BuiltinHelp::PluginActivate),
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
            Long("docs") => return docs_or_search(parser, BuiltinHelp::PluginDeactivate),
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
            Long("docs") => return docs_or_search(parser, BuiltinHelp::PluginList),
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
            Long("docs") => return docs_or_search(parser, BuiltinHelp::PluginSearch),
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
        Some(Long("docs")) => return docs_or_search(parser, BuiltinHelp::Settings),
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
            Long("docs") => return docs_or_search(parser, BuiltinHelp::SettingsShow),
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
            Long("docs") => return docs_or_search(parser, BuiltinHelp::SettingsSet),
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

fn parse_skills(parser: &mut lexopt::Parser) -> Result<Parsed, CliError> {
    use lexopt::prelude::*;

    let sub = match parser.next()? {
        // Bare `creft skills` → namespace help.
        None => return Ok(Parsed::Help(BuiltinHelp::Skills)),
        Some(Long("help") | Short('h')) => return Ok(Parsed::Help(BuiltinHelp::Skills)),
        Some(Long("docs")) => return docs_or_search(parser, BuiltinHelp::Skills),
        Some(Value(v)) => v.string()?,
        Some(arg) => return Err(CliError::Usage(arg.unexpected().to_string())),
    };

    match sub.as_str() {
        "test" => parse_skills_test(parser),
        other => Err(CliError::UnknownCommand(format!("skills {other}"))),
    }
}

fn parse_skills_test(parser: &mut lexopt::Parser) -> Result<Parsed, CliError> {
    use lexopt::prelude::*;

    let mut skill = None;
    let mut scenario = None;
    let mut keep = false;
    let mut detail = false;
    let mut where_ = false;

    while let Some(arg) = parser.next()? {
        match arg {
            Long("keep") => keep = true,
            Long("detail") => detail = true,
            Long("where") => where_ = true,
            Long("help") | Short('h') => return Ok(Parsed::Help(BuiltinHelp::SkillsTest)),
            Long("docs") => return docs_or_search(parser, BuiltinHelp::SkillsTest),
            Value(v) if skill.is_none() => skill = Some(v.string()?),
            Value(v) if scenario.is_none() => scenario = Some(v.string()?),
            Value(v) => {
                return Err(CliError::Usage(format!(
                    "unexpected argument: {}",
                    v.string()?
                )));
            }
            _ => return Err(CliError::Usage(arg.unexpected().to_string())),
        }
    }

    Ok(Parsed::Command(Command::Skills(SkillsCommand::Test {
        skill,
        scenario,
        keep,
        detail,
        where_,
    })))
}

fn parse_up(parser: &mut lexopt::Parser) -> Result<Parsed, CliError> {
    use lexopt::prelude::*;

    let mut system = None;
    let mut local = false;

    while let Some(arg) = parser.next()? {
        match arg {
            Short('l') | Long("local") => local = true,
            Long("help") | Short('h') => return Ok(Parsed::Help(BuiltinHelp::Up)),
            Long("docs") => return docs_or_search(parser, BuiltinHelp::Up),
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

    Ok(Parsed::Command(Command::Up { system, local }))
}

fn parse_init(parser: &mut lexopt::Parser) -> Result<Parsed, CliError> {
    use lexopt::prelude::*;

    if let Some(arg) = parser.next()? {
        match arg {
            Long("help") | Short('h') => return Ok(Parsed::Help(BuiltinHelp::Init)),
            Long("docs") => return docs_or_search(parser, BuiltinHelp::Init),
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
            Long("docs") => return docs_or_search(parser, BuiltinHelp::Doctor),
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
            Long("docs") => return docs_or_search(parser, BuiltinHelp::Completions),
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
    use rstest::rstest;

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

    #[rstest]
    #[case::completions("completions", crate::help::BuiltinHelp::Completions)]
    #[case::add("add", crate::help::BuiltinHelp::Add)]
    #[case::list("list", crate::help::BuiltinHelp::List)]
    #[case::doctor("doctor", crate::help::BuiltinHelp::Doctor)]
    #[case::plugin("plugin", crate::help::BuiltinHelp::Plugin)]
    #[case::settings("settings", crate::help::BuiltinHelp::Settings)]
    #[case::skills("skills", crate::help::BuiltinHelp::Skills)]
    #[case::up("up", crate::help::BuiltinHelp::Up)]
    #[case::init("init", crate::help::BuiltinHelp::Init)]
    fn docs_flag_returns_docs_variant(
        #[case] cmd: &str,
        #[case] expected: crate::help::BuiltinHelp,
    ) {
        let result = parse_args(&[cmd, "--docs"]).unwrap().unwrap();
        assert!(
            matches!(result, Parsed::Docs(ref v) if *v == expected),
            "{cmd} --docs must return Parsed::Docs({expected:?}); got: {result:?}",
        );
    }

    #[test]
    fn plugin_install_accepts_source_without_plugin_flag() {
        let result = parse_args(&["plugin", "install", "creft/ask"])
            .unwrap()
            .unwrap();
        assert!(
            matches!(
                result,
                Parsed::Command(Command::Plugin(PluginCommand::Install {
                    source: ref s,
                })) if s == "creft/ask"
            ),
            "plugin install <source> must parse correctly; got: {result:?}",
        );
    }

    #[test]
    fn plugin_install_rejects_plugin_flag() {
        let result = parse_args(&["plugin", "install", "creft/ask", "--plugin", "ask"]);
        assert!(
            matches!(result, Err(CliError::Usage(_))),
            "--plugin must be rejected as an unknown flag; got: {result:?}",
        );
    }

    #[test]
    fn plugin_install_short_flag_p_rejected() {
        let result = parse_args(&["plugin", "install", "creft/ask", "-p", "ask"]);
        assert!(
            matches!(result, Err(CliError::Usage(_))),
            "-p must be rejected as an unknown flag; got: {result:?}",
        );
    }

    #[test]
    fn up_bare_defaults_local_false() {
        let result = parse_args(&["up"]).unwrap().unwrap();
        let Parsed::Command(Command::Up { local, .. }) = result else {
            panic!("expected Command::Up");
        };
        assert!(
            !local,
            "bare `creft up` must default local=false (global install)"
        );
    }

    #[test]
    fn up_local_long_flag_sets_local_true() {
        let result = parse_args(&["up", "--local"]).unwrap().unwrap();
        let Parsed::Command(Command::Up { local, .. }) = result else {
            panic!("expected Command::Up");
        };
        assert!(local, "--local must set local=true");
    }

    #[test]
    fn up_local_short_flag_sets_local_true() {
        let result = parse_args(&["up", "-l"]).unwrap().unwrap();
        let Parsed::Command(Command::Up { local, .. }) = result else {
            panic!("expected Command::Up");
        };
        assert!(local, "-l must set local=true");
    }

    #[test]
    fn up_global_flag_rejected() {
        let result = parse_args(&["up", "--global"]);
        assert!(
            matches!(result, Err(CliError::Usage(_))),
            "--global must be rejected as an unknown flag; got: {result:?}",
        );
    }

    #[test]
    fn up_system_with_local_flag_parses_both() {
        let result = parse_args(&["up", "--local", "claude-code"])
            .unwrap()
            .unwrap();
        let Parsed::Command(Command::Up { system, local }) = result else {
            panic!("expected Command::Up");
        };
        assert_eq!(system.as_deref(), Some("claude-code"));
        assert!(local, "--local must set local=true even with a system arg");
    }

    // ── docs search (Stage 6) ─────────────────────────────────────────────────

    #[test]
    fn docs_with_query_returns_docs_search_variant() {
        let result = parse_args(&["add", "--docs", "template"]).unwrap().unwrap();
        assert!(
            matches!(result, Parsed::DocsSearch(crate::help::BuiltinHelp::Add, ref q) if q == "template"),
            "add --docs template must return DocsSearch(Add, \"template\"); got: {result:?}",
        );
    }

    #[test]
    fn docs_equals_form_returns_docs_search_variant() {
        let result = parse_args(&["add", "--docs=template"]).unwrap().unwrap();
        assert!(
            matches!(result, Parsed::DocsSearch(crate::help::BuiltinHelp::Add, ref q) if q == "template"),
            "add --docs=template must return DocsSearch(Add, \"template\"); got: {result:?}",
        );
    }

    #[test]
    fn docs_followed_by_flag_returns_docs_not_search() {
        let result = parse_args(&["add", "--docs", "--force"]).unwrap().unwrap();
        assert!(
            matches!(result, Parsed::Docs(crate::help::BuiltinHelp::Add)),
            "add --docs --force must return Docs(Add) (flag is not a query); got: {result:?}",
        );
    }

    #[test]
    fn root_docs_with_query_returns_docs_search_all() {
        let result = parse_args(&["--docs", "env"]).unwrap().unwrap();
        assert!(
            matches!(result, Parsed::DocsSearchAll(ref q) if q == "env"),
            "--docs env must return DocsSearchAll(\"env\"); got: {result:?}",
        );
    }

    #[test]
    fn root_docs_bare_returns_root_help() {
        let result = parse_args(&["--docs"]).unwrap().unwrap();
        assert!(
            matches!(result, Parsed::RootHelp),
            "bare --docs must return RootHelp; got: {result:?}",
        );
    }

    #[rstest]
    #[case::add("add", crate::help::BuiltinHelp::Add)]
    #[case::list("list", crate::help::BuiltinHelp::List)]
    #[case::show("show", crate::help::BuiltinHelp::Show)]
    #[case::remove("remove", crate::help::BuiltinHelp::Remove)]
    #[case::up("up", crate::help::BuiltinHelp::Up)]
    #[case::doctor("doctor", crate::help::BuiltinHelp::Doctor)]
    fn docs_search_variant_per_command(
        #[case] cmd: &str,
        #[case] expected: crate::help::BuiltinHelp,
    ) {
        let result = parse_args(&[cmd, "--docs", "query"]).unwrap().unwrap();
        assert!(
            matches!(result, Parsed::DocsSearch(ref v, ref q) if *v == expected && q == "query"),
            "{cmd} --docs query must return DocsSearch({expected:?}, \"query\"); got: {result:?}",
        );
    }

    // ── creft skills ──────────────────────────────────────────────────────────

    #[rstest]
    #[case::bare(&["skills"] as &[&str])]
    #[case::long_flag(&["skills", "--help"])]
    #[case::short_flag(&["skills", "-h"])]
    fn skills_invocation_returns_skills_help(#[case] args: &[&str]) {
        let result = parse_args(args).unwrap().unwrap();
        assert!(
            matches!(result, Parsed::Help(crate::help::BuiltinHelp::Skills)),
            "`creft {:?}` must return Parsed::Help(Skills); got: {result:?}",
            args,
        );
    }

    #[test]
    fn skills_bogus_subcommand_returns_unknown_command() {
        let result = parse_args(&["skills", "bogus"]);
        assert!(
            matches!(result, Err(CliError::UnknownCommand(ref s)) if s == "skills bogus"),
            "`creft skills bogus` must return UnknownCommand(\"skills bogus\"); got: {result:?}",
        );
    }

    #[test]
    fn skills_test_bare_parses_all_defaults() {
        let result = parse_args(&["skills", "test"]).unwrap().unwrap();
        let Parsed::Command(Command::Skills(SkillsCommand::Test {
            skill,
            scenario,
            keep,
            detail,
            where_,
        })) = result
        else {
            panic!("expected Command::Skills(Test); got: {result:?}");
        };
        assert!(skill.is_none(), "skill must default to None");
        assert!(scenario.is_none(), "scenario must default to None");
        assert!(!keep, "keep must default to false");
        assert!(!detail, "detail must default to false");
        assert!(!where_, "where_ must default to false");
    }

    #[test]
    fn skills_test_with_skill_positional() {
        let result = parse_args(&["skills", "test", "setup"]).unwrap().unwrap();
        let Parsed::Command(Command::Skills(SkillsCommand::Test {
            skill, scenario, ..
        })) = result
        else {
            panic!("expected Command::Skills(Test)");
        };
        assert_eq!(skill.as_deref(), Some("setup"));
        assert!(scenario.is_none());
    }

    #[test]
    fn skills_test_with_skill_and_scenario_positionals() {
        let result = parse_args(&["skills", "test", "setup", "fresh-install"])
            .unwrap()
            .unwrap();
        let Parsed::Command(Command::Skills(SkillsCommand::Test {
            skill, scenario, ..
        })) = result
        else {
            panic!("expected Command::Skills(Test)");
        };
        assert_eq!(skill.as_deref(), Some("setup"));
        assert_eq!(scenario.as_deref(), Some("fresh-install"));
    }

    #[test]
    fn skills_test_flags_parsed_correctly() {
        let result = parse_args(&["skills", "test", "--keep", "--detail", "--where"])
            .unwrap()
            .unwrap();
        let Parsed::Command(Command::Skills(SkillsCommand::Test {
            keep,
            detail,
            where_,
            ..
        })) = result
        else {
            panic!("expected Command::Skills(Test)");
        };
        assert!(keep, "--keep must set keep=true");
        assert!(detail, "--detail must set detail=true");
        assert!(where_, "--where must set where_=true");
    }

    #[test]
    fn skills_test_help_flag_returns_help() {
        let result = parse_args(&["skills", "test", "--help"]).unwrap().unwrap();
        assert!(
            matches!(result, Parsed::Help(crate::help::BuiltinHelp::SkillsTest)),
            "`creft skills test --help` must return Parsed::Help(SkillsTest); got: {result:?}",
        );
    }

    #[test]
    fn skills_test_docs_flag_returns_docs() {
        let result = parse_args(&["skills", "test", "--docs"]).unwrap().unwrap();
        assert!(
            matches!(result, Parsed::Docs(crate::help::BuiltinHelp::SkillsTest)),
            "`creft skills test --docs` must return Parsed::Docs(SkillsTest); got: {result:?}",
        );
    }

    #[test]
    fn skills_test_third_positional_returns_usage_error() {
        let result = parse_args(&["skills", "test", "skill", "scenario", "extra"]);
        assert!(
            matches!(result, Err(CliError::Usage(_))),
            "third positional must return Usage error; got: {result:?}",
        );
    }

    // ── `creft add test` parser tests ────────────────────────────────────────

    #[test]
    fn add_test_routes_to_add_test_variant() {
        let result = parse_args(&["add", "test"]).unwrap().unwrap();
        assert!(
            matches!(result, Parsed::Command(Command::AddTest { force: false })),
            "`creft add test` must return Command::AddTest {{ force: false }}; got: {result:?}",
        );
    }

    #[test]
    fn add_test_force_flag_sets_force_true() {
        let result = parse_args(&["add", "test", "--force"]).unwrap().unwrap();
        assert!(
            matches!(result, Parsed::Command(Command::AddTest { force: true })),
            "`creft add test --force` must return Command::AddTest {{ force: true }}; got: {result:?}",
        );
    }

    #[test]
    fn add_test_help_returns_help_variant() {
        let result = parse_args(&["add", "test", "--help"]).unwrap().unwrap();
        assert!(
            matches!(result, Parsed::Help(crate::help::BuiltinHelp::AddTest)),
            "`creft add test --help` must return Parsed::Help(AddTest); got: {result:?}",
        );
    }

    #[test]
    fn add_test_docs_returns_docs_variant() {
        let result = parse_args(&["add", "test", "--docs"]).unwrap().unwrap();
        assert!(
            matches!(result, Parsed::Docs(crate::help::BuiltinHelp::AddTest)),
            "`creft add test --docs` must return Parsed::Docs(AddTest); got: {result:?}",
        );
    }

    #[test]
    fn add_test_unknown_flag_returns_usage_error() {
        let result = parse_args(&["add", "test", "--bogus"]);
        assert!(
            matches!(result, Err(CliError::Usage(_))),
            "`creft add test --bogus` must return Usage error; got: {result:?}",
        );
    }

    #[test]
    fn add_with_no_test_keyword_keeps_existing_behavior() {
        let result = parse_args(&["add", "--name", "foo"]).unwrap().unwrap();
        let Parsed::Command(Command::Add { name, .. }) = result else {
            panic!("expected Command::Add; got: {result:?}");
        };
        assert_eq!(name.as_deref(), Some("foo"));
    }

    #[test]
    fn add_with_unknown_positional_returns_usage_error() {
        let result = parse_args(&["add", "bogus"]);
        assert!(
            matches!(result, Err(CliError::Usage(_))),
            "`creft add bogus` must return Usage error (unknown positional); got: {result:?}",
        );
    }

    #[test]
    fn add_flag_handler_is_one_source_of_truth() {
        // Both orderings of flags must produce the same Command::Add value,
        // proving the leading-dispatcher branch and the trailing loop both
        // go through the same apply_add_flag handler.
        let result_a = parse_args(&["add", "--name", "foo", "--tag", "x", "-g"])
            .unwrap()
            .unwrap();
        let result_b = parse_args(&["add", "-g", "--name", "foo", "--tag", "x"])
            .unwrap()
            .unwrap();

        let Parsed::Command(Command::Add {
            name: name_a,
            tags: tags_a,
            global: global_a,
            ..
        }) = result_a
        else {
            panic!("expected Command::Add from first invocation");
        };
        let Parsed::Command(Command::Add {
            name: name_b,
            tags: tags_b,
            global: global_b,
            ..
        }) = result_b
        else {
            panic!("expected Command::Add from second invocation");
        };

        assert_eq!(name_a, name_b);
        assert_eq!(tags_a, tags_b);
        assert_eq!(global_a, global_b);
    }
}
