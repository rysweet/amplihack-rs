//! CLI command parsing, launcher, and process management for amplihack.
//!
//! This crate provides the full CLI experience: argument parsing via clap derive,
//! binary discovery, environment construction, process management with signal
//! handling, and nesting detection.

pub mod binary_finder;
pub mod commands;
pub mod env_builder;
pub mod launcher;
pub mod nesting;
pub mod settings_manager;
pub mod signals;

use clap::{Parser, Subcommand};

/// amplihack CLI — Rust core runtime.
#[derive(Parser, Debug)]
#[command(
    name = "amplihack",
    version,
    about = "amplihack CLI — Rust core runtime"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Install amplihack agents and tools to ~/.claude
    Install,
    /// Remove amplihack agents and tools
    Uninstall,
    /// Launch Claude Code
    Launch {
        /// Resume the previous session
        #[arg(long)]
        resume: bool,
        /// Continue the previous session
        #[arg(long)]
        continue_session: bool,
        /// Extra args passed to claude
        #[arg(trailing_var_arg = true)]
        claude_args: Vec<String>,
    },
    /// Launch Claude Code (alias)
    Claude {
        /// Extra args passed to claude
        #[arg(trailing_var_arg = true)]
        claude_args: Vec<String>,
    },
    /// Launch GitHub Copilot CLI
    Copilot {
        /// Extra args passed to copilot
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },
    /// Launch OpenAI Codex CLI
    Codex {
        /// Extra args passed to codex
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },
    /// Launch Amplifier
    Amplifier {
        /// Extra args passed to amplifier
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },
    /// Plugin management
    Plugin {
        #[command(subcommand)]
        command: PluginCommands,
    },
    /// Memory system commands
    Memory {
        #[command(subcommand)]
        command: MemoryCommands,
    },
    /// Recipe management
    Recipe {
        #[command(subcommand)]
        command: RecipeCommands,
    },
    /// Mode management
    Mode {
        #[command(subcommand)]
        command: ModeCommands,
    },
    /// Show version information
    Version,
}

#[derive(Subcommand, Debug)]
pub enum PluginCommands {
    /// Install a plugin
    Install {
        /// Plugin name or path
        name: String,
    },
    /// Uninstall a plugin
    Uninstall {
        /// Plugin name
        name: String,
    },
    /// Link a local plugin for development
    Link {
        /// Path to the plugin directory
        path: String,
    },
    /// Verify installed plugins
    Verify,
}

#[derive(Subcommand, Debug)]
pub enum MemoryCommands {
    /// Show memory tree
    Tree,
    /// Export memory to file
    Export {
        /// Output file path
        #[arg(short, long)]
        output: Option<String>,
    },
    /// Import memory from file
    Import {
        /// Input file path
        file: String,
    },
    /// Clean stale memory entries
    Clean,
}

#[derive(Subcommand, Debug)]
pub enum RecipeCommands {
    /// Run a recipe
    Run {
        /// Recipe name
        name: String,
        /// Extra args passed to the recipe
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },
    /// List available recipes
    List,
    /// Validate a recipe file
    Validate {
        /// Recipe file path
        file: String,
    },
    /// Show recipe details
    Show {
        /// Recipe name
        name: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum ModeCommands {
    /// Detect current mode
    Detect,
    /// Switch to plugin mode
    ToPlugin,
    /// Switch to local mode
    ToLocal,
}
