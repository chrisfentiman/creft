use clap::{Parser, Subcommand};

use crate::help::*;

/// Top-level CLI parser for creft.
#[derive(Parser, Debug)]
#[command(
    name = "creft",
    about = ROOT_ABOUT,
    long_about = ROOT_LONG_ABOUT,
    version
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: BuiltinCommand,
}

/// Built-in subcommands dispatched before user-defined skills.
#[derive(Subcommand, Debug)]
pub enum BuiltinCommand {
    /// Manage local and global skills
    #[command(long_about = CMD_LONG_ABOUT, visible_alias = "command")]
    Cmd {
        #[command(subcommand)]
        action: Option<CmdAction>,
    },

    /// Manage plugins
    #[command(long_about = PLUGINS_LONG_ABOUT)]
    Plugins {
        #[command(subcommand)]
        action: Option<PluginAction>,
    },

    /// Manage settings
    #[command(long_about = SETTINGS_LONG_ABOUT)]
    Settings {
        #[command(subcommand)]
        action: Option<SettingsAction>,
    },

    /// Set up creft for a coding AI system
    #[command(long_about = UP_LONG_ABOUT)]
    Up {
        /// Target system (claude-code, cursor, windsurf, aider, copilot, codex, gemini).
        /// Auto-detects if omitted.
        system: Option<String>,
        /// Install globally instead of project-level
        #[arg(short = 'g', long)]
        global: bool,
    },

    /// Initialize local skill storage
    #[command(long_about = INIT_LONG_ABOUT)]
    Init,

    /// Check environment and skill health
    #[command(long_about = DOCTOR_LONG_ABOUT)]
    Doctor {
        /// Skill name to check (omit for global health check)
        #[arg(trailing_var_arg = true)]
        name: Vec<String>,
    },
}

/// Subcommands for `creft cmd`.
#[derive(Subcommand, Debug)]
pub enum CmdAction {
    /// Save a new skill from stdin
    #[command(long_about = ADD_LONG_ABOUT)]
    Add {
        /// Command name (overrides frontmatter)
        #[arg(long)]
        name: Option<String>,
        /// Description (overrides frontmatter)
        #[arg(long)]
        description: Option<String>,
        /// Add an arg in NAME:DESC format (repeatable)
        #[arg(long = "arg", value_name = "NAME:DESC")]
        args: Vec<String>,
        /// Add a tag (repeatable)
        #[arg(long = "tag")]
        tags: Vec<String>,
        /// Overwrite if command already exists
        #[arg(long)]
        force: bool,
        /// Skip code block validation (syntax and template checks)
        #[arg(long)]
        no_validate: bool,
        /// Save to global ~/.creft/ instead of local .creft/
        #[arg(short = 'g', long)]
        global: bool,
    },

    /// List available skills
    #[command(long_about = LIST_LONG_ABOUT)]
    List {
        /// Filter by tag
        #[arg(long)]
        tag: Option<String>,
        /// Show all commands in flat list, including hidden `_`-prefixed commands
        #[arg(long)]
        all: bool,
        /// Namespace path to drill into
        #[arg(trailing_var_arg = true)]
        namespace: Vec<String>,
    },

    /// Show a skill's full definition
    #[command(long_about = SHOW_LONG_ABOUT)]
    Show {
        /// Command name
        name: Vec<String>,
    },

    /// Print a skill's code blocks
    #[command(long_about = CAT_LONG_ABOUT)]
    Cat {
        /// Command name
        name: Vec<String>,
    },

    /// Delete a skill
    #[command(long_about = RM_LONG_ABOUT)]
    Rm {
        /// Command name
        name: Vec<String>,
        /// Force deletion from global ~/.creft/ even if a local version exists
        #[arg(short = 'g', long)]
        global: bool,
    },
}

/// Subcommands for `creft plugins`.
#[derive(Subcommand, Debug)]
pub enum PluginAction {
    /// Install a plugin from a git repo
    #[command(long_about = PLUGIN_INSTALL_LONG_ABOUT)]
    Install {
        /// Plugin source: git URL or path to a local repo
        source: String,
        /// Install a specific plugin from a multi-plugin repo
        #[arg(short = 'p', long)]
        plugin: Option<String>,
    },

    /// Update installed plugins
    #[command(long_about = PLUGIN_UPDATE_LONG_ABOUT)]
    Update {
        /// Plugin name (updates all if omitted)
        name: Option<String>,
    },

    /// Remove an installed plugin
    #[command(long_about = PLUGIN_UNINSTALL_LONG_ABOUT)]
    Uninstall {
        /// Plugin name
        name: String,
    },

    /// Activate a command from an installed plugin
    #[command(long_about = PLUGIN_ACTIVATE_LONG_ABOUT)]
    Activate {
        /// Command to activate: plugin/cmd or plugin (activates all)
        target: String,
        /// Activate globally instead of in the nearest .creft/
        #[arg(short = 'g', long)]
        global: bool,
    },

    /// Deactivate a command
    #[command(long_about = PLUGIN_DEACTIVATE_LONG_ABOUT)]
    Deactivate {
        /// Command to deactivate: plugin/cmd or plugin (deactivates all)
        target: String,
        /// Deactivate only in the global scope
        #[arg(short = 'g', long)]
        global: bool,
    },

    /// List installed plugins, or commands in a specific plugin
    #[command(long_about = PLUGIN_LIST_LONG_ABOUT)]
    List {
        /// Show commands in this plugin instead of listing all plugins
        name: Option<String>,
    },

    /// Search for commands across installed plugins
    #[command(long_about = PLUGIN_SEARCH_LONG_ABOUT)]
    Search {
        /// Search query (matches name, description, tags)
        query: Vec<String>,
    },
}

/// Subcommands for `creft settings`.
#[derive(Subcommand, Debug)]
pub enum SettingsAction {
    /// Show current settings
    Show,

    /// Set a configuration value
    Set {
        /// Setting key
        key: String,
        /// Setting value
        value: String,
    },
}
