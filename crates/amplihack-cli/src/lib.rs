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
/// Local session management dashboard (fleet_local).
///
/// Python-to-Rust port of the amploxy local session TUI.
/// Reads `~/.claude/runtime/locks/*` to discover and manage active Claude
/// sessions on the local machine.  Separate from Azure-VM fleet orchestration
/// in `commands/fleet.rs`.
pub mod fleet_local;
pub mod launcher;
pub mod nesting;
pub mod resolve_bundle_asset;
pub mod settings_manager;
pub mod signals;
#[cfg(test)]
pub mod test_support;
pub mod tool_update_check;
pub mod update;
pub mod util;

use clap::{Parser, Subcommand};
use clap_complete::Shell;
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
        /// Inject --dangerously-skip-permissions into the claude invocation.
        /// This bypasses Claude's interactive confirmation prompts.
        /// Use only in trusted automated environments.
        #[arg(long = "skip-permissions")]
        skip_permissions: bool,
        /// Skip the pre-launch npm update availability check.
        /// Useful in CI, offline environments, or scripted pipelines.
        #[arg(long = "skip-update-check")]
        skip_update_check: bool,
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
    /// Import blarify code-graph JSON into the native code-graph store
    IndexCode {
        /// Path to a blarify JSON export
        input: PathBuf,
        /// Override the target code-graph database path
        #[arg(long = "db-path", alias = "kuzu-path")]
        db_path: Option<PathBuf>,
    },
    /// Generate native SCIP artifacts for the current project
    IndexScip {
        /// Project path to index (defaults to current working directory)
        #[arg(long = "project-path")]
        project_path: Option<PathBuf>,
        /// Restrict indexing to specific languages
        #[arg(long = "language")]
        languages: Vec<String>,
    },
    /// Query the native code graph
    QueryCode {
        /// Override the target code-graph database path
        #[arg(long = "db-path", alias = "kuzu-path")]
        db_path: Option<PathBuf>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
        /// Maximum number of rows to return
        #[arg(long, default_value_t = 50)]
        limit: u32,
        #[command(subcommand)]
        command: QueryCodeCommands,
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

    /// Fleet orchestration (native Rust runtime)
    Fleet {
        /// Arguments forwarded to the fleet dispatcher
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Generate a new goal-seeking agent from a prompt file (native Rust)
    New {
        /// Path to prompt.md file containing the goal description
        #[arg(short = 'f', long = "file", required = true)]
        file: std::path::PathBuf,
        /// Output directory for the goal agent (default: ./goal_agents)
        #[arg(short = 'o', long = "output")]
        output: Option<std::path::PathBuf>,
        /// Custom name for the goal agent (auto-generated if not provided)
        #[arg(short = 'n', long = "name")]
        name: Option<String>,
        /// Custom skills directory (default: .claude/agents/amplihack)
        #[arg(long = "skills-dir")]
        skills_dir: Option<std::path::PathBuf>,
        /// Enable verbose output
        #[arg(short = 'v', long = "verbose")]
        verbose: bool,
        /// Enable memory/learning capabilities
        #[arg(long = "enable-memory")]
        enable_memory: bool,
        /// SDK to use for agent execution
        #[arg(long = "sdk", default_value = "copilot",
              value_parser = ["copilot", "claude", "microsoft", "mini"])]
        sdk: String,
        /// Enable multi-agent architecture
        #[arg(long = "multi-agent")]
        multi_agent: bool,
        /// Enable dynamic sub-agent spawning (auto-enables --multi-agent)
        #[arg(long = "enable-spawning")]
        enable_spawning: bool,
    },
    /// RustyClawd tool (native Rust launcher path)
    #[command(name = "RustyClawd")]
    RustyClawd {
        /// Arguments forwarded to the RustyClawd/Claude binary
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// UVX help information
    #[command(name = "uvx-help")]
    UvxHelp {
        /// Find the detected UVX installation path
        #[arg(long)]
        find_path: bool,
        /// Show UVX staging information
        #[arg(long)]
        info: bool,
    },

    /// Generate shell completion scripts (bash, zsh, fish, powershell)
    Completions {
        /// Shell to generate completions for
        shell: Shell,
    },

    /// Run system health checks
    Doctor,
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
        #[arg(long, default_value = "graph-db", value_parser = ["graph-db", "kuzu", "sqlite"])]
        backend: String,
    },
    /// Export memory to file
    Export {
        /// Name of the agent whose memory to export
        #[arg(long)]
        agent: String,
        /// Output file path (.json) or directory (raw-db; compatibility alias: kuzu)
        #[arg(short, long)]
        output: String,
        /// Export format
        #[arg(short = 'f', long = "format", default_value = "json", value_parser = ["json", "raw-db", "kuzu"])]
        format: String,
        /// Custom storage path for the agent's graph DB
        #[arg(long = "storage-path")]
        storage_path: Option<String>,
    },
    /// Import memory from file
    Import {
        /// Name of the target agent to import into
        #[arg(long)]
        agent: String,
        /// Input file path (.json) or directory (raw-db; compatibility alias: kuzu)
        #[arg(short, long)]
        input: String,
        /// Import format
        #[arg(short = 'f', long = "format", default_value = "json", value_parser = ["json", "raw-db", "kuzu"])]
        format: String,
        /// Merge into existing memory
        #[arg(long)]
        merge: bool,
        /// Custom storage path for the agent's graph DB
        #[arg(long = "storage-path")]
        storage_path: Option<String>,
    },
    /// Clean stale memory entries
    Clean {
        /// Session ID pattern to match
        #[arg(long, default_value = "test_*")]
        pattern: String,
        /// Memory backend to use
        #[arg(long, default_value = "graph-db", value_parser = ["graph-db", "kuzu", "sqlite"])]
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
pub enum QueryCodeCommands {
    /// Show code graph statistics
    Stats,
    /// Show related code context for a memory
    Context {
        /// Memory identifier
        memory_id: String,
    },
    /// List indexed files
    Files {
        /// Optional path substring filter
        #[arg(long)]
        pattern: Option<String>,
    },
    /// List indexed functions
    Functions {
        /// Optional file substring filter
        #[arg(long)]
        file: Option<String>,
    },
    /// List indexed classes
    Classes {
        /// Optional file substring filter
        #[arg(long)]
        file: Option<String>,
    },
    /// Search files, functions, and classes by name
    Search {
        /// Search term
        name: String,
    },
    /// Find functions calling a given function
    Callers {
        /// Function name substring
        name: String,
    },
    /// Find functions called by a given function
    Callees {
        /// Function name substring
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

/// Re-export memory backend utilities for integration tests.
///
/// These functions are part of the internal implementation but are exposed
/// here so that `tests/integration/kuzu_ffi_test.rs` can exercise the kuzu
/// C++ FFI boundary without embedding integration tests inside production code.
pub mod memory {
    pub use crate::commands::memory::backend::kuzu::{
        init_kuzu_backend_schema, kuzu_rows, list_kuzu_sessions_from_conn,
    };
    pub use crate::commands::memory::{
        CodeGraphSummary, IndexStatus, PromptContextMemory, SessionSummary,
        background_index_job_active, background_index_job_path, check_index_status,
        default_code_graph_db_path_for_project, record_background_index_pid,
        resolve_code_graph_db_path_for_project, retrieve_prompt_context_memories,
        store_session_learning, summarize_code_graph,
    };
}
