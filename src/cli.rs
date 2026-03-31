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
        /// Show all commands in flat list (no namespace grouping)
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

    /// Edit a skill in $EDITOR or from stdin
    #[command(long_about = EDIT_LONG_ABOUT)]
    Edit {
        /// Command name
        name: Vec<String>,
        /// Force editing in global ~/.creft/ even if a local version exists
        #[arg(short = 'g', long)]
        global: bool,
        /// Skip code block validation when piping new content
        #[arg(long)]
        no_validate: bool,
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

    /// Print a skill's code blocks
    #[command(long_about = CAT_LONG_ABOUT)]
    Cat {
        /// Command name
        name: Vec<String>,
    },

    /// Install a skill package from a git repo
    #[command(long_about = INSTALL_LONG_ABOUT)]
    Install {
        /// Git repository URL
        url: String,
        /// Install to global ~/.creft/ instead of local .creft/
        #[arg(short = 'g', long)]
        global: bool,
    },

    /// Update installed skill packages
    #[command(long_about = UPDATE_LONG_ABOUT)]
    Update {
        /// Package name (updates all if omitted)
        name: Option<String>,
    },

    /// Remove an installed skill package
    #[command(long_about = UNINSTALL_LONG_ABOUT)]
    Uninstall {
        /// Package name
        name: String,
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
