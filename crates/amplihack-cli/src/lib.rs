//! CLI command parsing, launcher, and process management for amplihack.
//!
//! This crate provides the full CLI experience: argument parsing via clap derive,
//! binary discovery, environment construction, process management with signal
//! handling, and nesting detection.

pub mod binary_finder;
pub mod bootstrap;
pub mod command_error;
pub mod commands;
pub mod copilot_setup;
pub mod env_builder;
pub mod launcher;
pub mod nesting;
pub mod settings_manager;
pub mod signals;
#[cfg(test)]
pub mod test_support;
pub mod update;

use clap::{Parser, Subcommand};
use std::path::PathBuf;

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
    /// Install amplihack framework assets to ~/.amplihack/.claude and wire ~/.claude/settings.json
    Install {
        /// Install from a local directory instead of cloning from git
        #[arg(long)]
        local: Option<PathBuf>,
    },
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
    /// Download and install the latest released binary
    Update,

    // ------------------------------------------------------------------
    // Python-only subcommands — delegated to `python3 -m amplihack.cli`
    // ------------------------------------------------------------------
    /// Fleet orchestration (delegated to Python)
    Fleet {
        /// Arguments forwarded to the Python fleet command
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Goal agent generator (delegated to Python)
    New {
        /// Arguments forwarded to the Python new command
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// RustyClawd tool (delegated to Python)
    #[command(name = "RustyClawd")]
    RustyClawd {
        /// Arguments forwarded to the Python RustyClawd command
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// UVX help information (delegated to Python)
    #[command(name = "uvx-help")]
    UvxHelp {
        /// Arguments forwarded to the Python uvx-help command
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum PluginCommands {
    /// Install a plugin
    Install {
        /// Git URL or local directory path
        source: String,
        /// Overwrite existing plugin
        #[arg(long)]
        force: bool,
    },
    /// Uninstall a plugin
    Uninstall {
        /// Plugin name
        plugin_name: String,
    },
    /// Link installed plugin to Claude Code settings
    Link {
        /// Plugin name to link
        #[arg(default_value = "amplihack")]
        plugin_name: String,
    },
    /// Verify installed plugins
    Verify {
        /// Plugin name to verify
        #[arg(default_value = "amplihack")]
        plugin_name: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum MemoryCommands {
    /// Show memory tree
    Tree {
        /// Filter by session ID
        #[arg(long = "session")]
        session: Option<String>,
        /// Filter by memory type
        #[arg(long = "type", value_parser = ["conversation", "decision", "pattern", "context", "learning", "artifact"])]
        memory_type: Option<String>,
        /// Maximum tree depth
        #[arg(long)]
        depth: Option<u32>,
        /// Memory backend to use
        #[arg(long, default_value = "kuzu", value_parser = ["kuzu", "sqlite"])]
        backend: String,
    },
    /// Export memory to file
    Export {
        /// Name of the agent whose memory to export
        #[arg(long)]
        agent: String,
        /// Output file path (.json) or directory (kuzu)
        #[arg(short, long)]
        output: String,
        /// Export format
        #[arg(short = 'f', long = "format", default_value = "json", value_parser = ["json", "kuzu"])]
        format: String,
        /// Custom storage path for the agent's Kuzu DB
        #[arg(long = "storage-path")]
        storage_path: Option<String>,
    },
    /// Import memory from file
    Import {
        /// Name of the target agent to import into
        #[arg(long)]
        agent: String,
        /// Input file path (.json) or directory (kuzu)
        #[arg(short, long)]
        input: String,
        /// Import format
        #[arg(short = 'f', long = "format", default_value = "json", value_parser = ["json", "kuzu"])]
        format: String,
        /// Merge into existing memory
        #[arg(long)]
        merge: bool,
        /// Custom storage path for the agent's Kuzu DB
        #[arg(long = "storage-path")]
        storage_path: Option<String>,
    },
    /// Clean stale memory entries
    Clean {
        /// Session ID pattern to match
        #[arg(long, default_value = "test_*")]
        pattern: String,
        /// Memory backend to use
        #[arg(long, default_value = "kuzu", value_parser = ["kuzu", "sqlite"])]
        backend: String,
        /// Actually delete sessions instead of dry-run
        #[arg(long = "no-dry-run")]
        no_dry_run: bool,
        /// Skip confirmation prompt
        #[arg(long)]
        confirm: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum RecipeCommands {
    /// Run a recipe
    Run {
        /// Recipe path
        recipe_path: String,
        /// Set context variable (key=value)
        #[arg(short = 'c', long = "context")]
        context: Vec<String>,
        /// Show what would be executed
        #[arg(long)]
        dry_run: bool,
        /// Show detailed output
        #[arg(short, long)]
        verbose: bool,
        /// Output format
        #[arg(short, long, default_value = "table", value_parser = ["table", "json", "yaml"])]
        format: String,
        /// Working directory for execution
        #[arg(short = 'w', long = "working-dir")]
        working_dir: Option<String>,
    },
    /// List available recipes
    List {
        /// Directory to search for recipes
        recipe_dir: Option<String>,
        /// Output format
        #[arg(short, long, default_value = "table", value_parser = ["table", "json", "yaml"])]
        format: String,
        /// Filter by tags
        #[arg(short, long)]
        tags: Vec<String>,
        /// Show full details
        #[arg(short, long)]
        verbose: bool,
    },
    /// Validate a recipe file
    Validate {
        /// Recipe file path
        file: String,
        /// Show details
        #[arg(short, long)]
        verbose: bool,
        /// Output format
        #[arg(short, long, default_value = "table", value_parser = ["table", "json", "yaml"])]
        format: String,
    },
    /// Show recipe details
    Show {
        /// Recipe name
        name: String,
        /// Output format
        #[arg(short, long, default_value = "table", value_parser = ["table", "json", "yaml"])]
        format: String,
        /// Hide step details
        #[arg(long)]
        no_steps: bool,
        /// Hide context variables
        #[arg(long)]
        no_context: bool,
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
