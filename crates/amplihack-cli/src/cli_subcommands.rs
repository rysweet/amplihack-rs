//! Subcommand enums for nested CLI commands.

use clap::Subcommand;

use super::{graph_db_backend_value_parser, raw_db_format_value_parser};

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
        #[arg(long, default_value = "graph-db", value_parser = graph_db_backend_value_parser())]
        backend: String,
    },
    /// Export memory to file
    Export {
        /// Name of the agent whose memory to export
        #[arg(long)]
        agent: String,
        /// Output file path (.json) or directory (raw-db)
        #[arg(short, long)]
        output: String,
        /// Export format
        #[arg(short = 'f', long = "format", default_value = "json", value_parser = raw_db_format_value_parser())]
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
        /// Input file path (.json) or directory (raw-db)
        #[arg(short, long)]
        input: String,
        /// Import format
        #[arg(short = 'f', long = "format", default_value = "json", value_parser = raw_db_format_value_parser())]
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
        #[arg(long, default_value = "graph-db", value_parser = graph_db_backend_value_parser())]
        backend: String,
        /// Actually delete sessions instead of dry-run
        #[arg(long = "no-dry-run")]
        no_dry_run: bool,
        /// Skip confirmation prompt
        #[arg(long)]
        confirm: bool,
    },
    /// Get a value from agent memory
    Get {
        /// Agent name
        #[arg(long)]
        agent: String,
        /// Session ID (auto-generated if omitted)
        #[arg(long)]
        session: Option<String>,
        /// Memory key
        key: String,
        /// SQLite db path
        #[arg(long = "db-path")]
        db_path: Option<std::path::PathBuf>,
        /// Output format
        #[arg(long, default_value = "text", value_parser = ["text", "json"])]
        format: String,
    },
    /// Store a value in agent memory
    Store {
        /// Agent name
        #[arg(long)]
        agent: String,
        /// Session ID (auto-generated if omitted)
        #[arg(long)]
        session: Option<String>,
        /// Memory key
        key: String,
        /// Memory value (string)
        value: String,
        /// SQLite db path
        #[arg(long = "db-path")]
        db_path: Option<std::path::PathBuf>,
        /// Storage type
        #[arg(long = "type", default_value = "markdown",
              value_parser = ["markdown", "json", "yaml", "text"])]
        memory_type: String,
        /// Output format
        #[arg(long, default_value = "text", value_parser = ["text", "json"])]
        format: String,
    },
    /// List keys in the current session
    List {
        /// Agent name
        #[arg(long)]
        agent: String,
        /// Session ID (auto-generated if omitted)
        #[arg(long)]
        session: Option<String>,
        /// SQLite db path
        #[arg(long = "db-path")]
        db_path: Option<std::path::PathBuf>,
        /// Output format
        #[arg(long, default_value = "text", value_parser = ["text", "json"])]
        format: String,
    },
    /// Delete a value from agent memory
    Delete {
        /// Agent name
        #[arg(long)]
        agent: String,
        /// Session ID (auto-generated if omitted)
        #[arg(long)]
        session: Option<String>,
        /// Memory key to delete
        key: String,
        /// SQLite db path
        #[arg(long = "db-path")]
        db_path: Option<std::path::PathBuf>,
        /// Output format
        #[arg(long, default_value = "text", value_parser = ["text", "json"])]
        format: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum ReflectCommands {
    /// Analyze recent assistant responses for patterns
    Analyze {
        /// Session ID
        #[arg(long)]
        session: String,
        /// Path to a JSON array of {role, content} message objects
        #[arg(long)]
        messages: std::path::PathBuf,
        /// Optional error content to feed the contextual analyzer
        #[arg(long)]
        error: Option<String>,
        /// Runtime directory for state/lock files
        #[arg(long = "runtime-dir")]
        runtime_dir: Option<std::path::PathBuf>,
        /// Output format
        #[arg(long, default_value = "text", value_parser = ["text", "json"])]
        format: String,
    },
    /// Show current reflection state for a session
    State {
        /// Session ID
        #[arg(long)]
        session: String,
        /// Runtime directory for state files
        #[arg(long = "runtime-dir")]
        runtime_dir: Option<std::path::PathBuf>,
        /// Output format
        #[arg(long, default_value = "text", value_parser = ["text", "json"])]
        format: String,
    },
    /// Clear reflection state for a session (resets to Idle)
    Clear {
        /// Session ID
        #[arg(long)]
        session: String,
        /// Runtime directory for state files
        #[arg(long = "runtime-dir")]
        runtime_dir: Option<std::path::PathBuf>,
        /// Output format
        #[arg(long, default_value = "text", value_parser = ["text", "json"])]
        format: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum BuilderCommands {
    /// Build a Claude session transcript from a messages.json file
    Claude {
        /// Session ID
        #[arg(long)]
        session: String,
        /// Path to messages.json
        #[arg(long)]
        messages: std::path::PathBuf,
        /// Working directory used for relative output paths
        #[arg(long = "working-dir")]
        working_dir: Option<std::path::PathBuf>,
        /// Optional output file (markdown). Otherwise prints to stdout.
        #[arg(long)]
        out: Option<std::path::PathBuf>,
        /// Output format for `--out` is always markdown; this flag controls
        /// the metadata response printed to stdout.
        #[arg(long, default_value = "text", value_parser = ["text", "json"])]
        format: String,
    },
    /// Build a Codex aggregate codex from a directory of session JSON files
    Codex {
        /// Directory containing session JSON files
        #[arg(long)]
        input_dir: std::path::PathBuf,
        /// Optional focus area; switches to focused-codex mode
        #[arg(long)]
        focus: Option<String>,
        /// Optional output file (markdown). Otherwise prints to stdout.
        #[arg(long)]
        out: Option<std::path::PathBuf>,
        /// Output format
        #[arg(long, default_value = "text", value_parser = ["text", "json"])]
        format: String,
    },
    /// Run the export-on-compact integration over a JSON payload file
    ExportOnCompact {
        /// Path to a JSON file describing the compact event
        #[arg(long)]
        input: std::path::PathBuf,
        /// Root directory under which exports are stored
        #[arg(long = "root-dir")]
        root_dir: std::path::PathBuf,
        /// Output format
        #[arg(long, default_value = "text", value_parser = ["text", "json"])]
        format: String,
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
        /// Override step timeout in seconds (0 = disable all step timeouts)
        #[arg(long = "step-timeout")]
        step_timeout: Option<u64>,
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

#[derive(Subcommand, Debug)]
pub enum MultitaskCommands {
    /// Run parallel workstreams from a JSON config file
    Run {
        /// Path to workstreams JSON config file
        config: String,
        /// Execution mode
        #[arg(long, default_value = "recipe", value_parser = ["recipe", "classic"])]
        mode: String,
        /// Recipe name for recipe mode
        #[arg(long, default_value = "default-workflow")]
        recipe: String,
        /// Override workstream runtime budget in seconds
        #[arg(long = "max-runtime")]
        max_runtime: Option<u64>,
        /// Timeout policy for active workstreams
        #[arg(long = "timeout-policy", value_parser = ["interrupt-preserve", "continue-preserve"])]
        timeout_policy: Option<String>,
        /// Show what would be executed without launching
        #[arg(long)]
        dry_run: bool,
    },
    /// Clean up workstreams with merged PRs
    Cleanup {
        /// Path to workstreams JSON config file
        config: String,
        /// Show what would be deleted without deleting
        #[arg(long)]
        dry_run: bool,
    },
    /// Show status of existing workstreams
    Status {
        /// Base directory for workstream artifacts
        #[arg(long = "base-dir")]
        base_dir: Option<String>,
    },
}
