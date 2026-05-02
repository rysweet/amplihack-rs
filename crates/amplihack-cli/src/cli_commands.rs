//! Top-level `Commands` enum for the amplihack CLI.

use clap::Subcommand;
use clap_complete::Shell;
use std::path::PathBuf;

use super::{
    BuilderCommands, MemoryCommands, ModeCommands, MultitaskCommands, PluginCommands,
    QueryCodeCommands, RecipeCommands, ReflectCommands,
};

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Install amplihack framework assets to ~/.amplihack/.claude and wire ~/.claude/settings.json
    Install {
        /// Install from a local directory instead of cloning from git
        #[arg(long)]
        local: Option<PathBuf>,
        /// Run the interactive configuration wizard to choose default tool,
        /// hook scope, and update-check preference
        #[arg(long)]
        interactive: bool,
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
        /// Disable post-session reflection analysis.
        #[arg(long = "no-reflection")]
        no_reflection: bool,
        /// Skip shared launcher staging/env updates for subprocess delegates.
        #[arg(long = "subprocess-safe")]
        subprocess_safe: bool,
        /// Clone a GitHub repository and launch Claude in that checkout.
        #[arg(long = "checkout-repo", value_name = "GITHUB_URI")]
        checkout_repo: Option<String>,
        /// Run amplihack in Docker container for isolated execution.
        #[arg(long = "docker")]
        docker: bool,
        /// Append instructions to a running auto mode session and exit.
        #[arg(long = "append")]
        append: Option<String>,
        /// Run in autonomous agentic mode with iterative loop execution.
        #[arg(long = "auto")]
        auto: bool,
        /// Max turns for auto mode.
        #[arg(long = "max-turns", default_value_t = 10, value_parser = clap::value_parser!(u32).range(1..))]
        max_turns: u32,
        /// Enable interactive UI mode for auto mode.
        #[arg(long = "ui")]
        ui: bool,
        /// Extra args passed to claude
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        claude_args: Vec<String>,
    },
    /// Launch Claude Code (alias)
    Claude {
        /// Disable post-session reflection analysis.
        #[arg(long = "no-reflection")]
        no_reflection: bool,
        /// Skip shared launcher staging/env updates for subprocess delegates.
        #[arg(long = "subprocess-safe")]
        subprocess_safe: bool,
        /// Clone a GitHub repository and launch Claude in that checkout.
        #[arg(long = "checkout-repo", value_name = "GITHUB_URI")]
        checkout_repo: Option<String>,
        /// Run amplihack in Docker container for isolated execution.
        #[arg(long = "docker")]
        docker: bool,
        /// Append instructions to a running auto mode session and exit.
        #[arg(long = "append")]
        append: Option<String>,
        /// Run in autonomous agentic mode with iterative loop execution.
        #[arg(long = "auto")]
        auto: bool,
        /// Max turns for auto mode.
        #[arg(long = "max-turns", default_value_t = 10, value_parser = clap::value_parser!(u32).range(1..))]
        max_turns: u32,
        /// Enable interactive UI mode for auto mode.
        #[arg(long = "ui")]
        ui: bool,
        /// Extra args passed to claude
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        claude_args: Vec<String>,
    },
    /// Launch GitHub Copilot CLI
    Copilot {
        /// Disable post-session reflection analysis.
        #[arg(long = "no-reflection")]
        no_reflection: bool,
        /// Skip shared launcher staging/env updates for subprocess delegates.
        #[arg(long = "subprocess-safe")]
        subprocess_safe: bool,
        /// Run amplihack in Docker container for isolated execution.
        #[arg(long = "docker")]
        docker: bool,
        /// Append instructions to a running auto mode session and exit.
        #[arg(long = "append")]
        append: Option<String>,
        /// Run in autonomous agentic mode with iterative loop execution.
        #[arg(long = "auto")]
        auto: bool,
        /// Max turns for auto mode.
        #[arg(long = "max-turns", default_value_t = 10, value_parser = clap::value_parser!(u32).range(1..))]
        max_turns: u32,
        /// Enable interactive UI mode for auto mode.
        #[arg(long = "ui")]
        ui: bool,
        /// Extra args passed to copilot
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Launch OpenAI Codex CLI
    Codex {
        /// Disable post-session reflection analysis.
        #[arg(long = "no-reflection")]
        no_reflection: bool,
        /// Skip shared launcher staging/env updates for subprocess delegates.
        #[arg(long = "subprocess-safe")]
        subprocess_safe: bool,
        /// Run amplihack in Docker container for isolated execution.
        #[arg(long = "docker")]
        docker: bool,
        /// Append instructions to a running auto mode session and exit.
        #[arg(long = "append")]
        append: Option<String>,
        /// Run in autonomous agentic mode with iterative loop execution.
        #[arg(long = "auto")]
        auto: bool,
        /// Max turns for auto mode.
        #[arg(long = "max-turns", default_value_t = 10, value_parser = clap::value_parser!(u32).range(1..))]
        max_turns: u32,
        /// Enable interactive UI mode for auto mode.
        #[arg(long = "ui")]
        ui: bool,
        /// Extra args passed to codex
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Launch Amplifier
    Amplifier {
        /// Disable post-session reflection analysis.
        #[arg(long = "no-reflection")]
        no_reflection: bool,
        /// Skip shared launcher staging/env updates for subprocess delegates.
        #[arg(long = "subprocess-safe")]
        subprocess_safe: bool,
        /// Run amplihack in Docker container for isolated execution.
        #[arg(long = "docker")]
        docker: bool,
        /// Append instructions to a running auto mode session and exit.
        #[arg(long = "append")]
        append: Option<String>,
        /// Run in autonomous agentic mode with iterative loop execution.
        #[arg(long = "auto")]
        auto: bool,
        /// Max turns for auto mode.
        #[arg(long = "max-turns", default_value_t = 10, value_parser = clap::value_parser!(u32).range(1..))]
        max_turns: u32,
        /// Enable interactive UI mode for auto mode.
        #[arg(long = "ui")]
        ui: bool,
        /// Extra args passed to amplifier
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
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
    /// Reflection workflow commands
    Reflect {
        #[command(subcommand)]
        command: ReflectCommands,
    },
    /// Transcript / codex builders
    Builder {
        #[command(subcommand)]
        command: BuilderCommands,
    },
    /// Import blarify code-graph JSON into the native code-graph store
    IndexCode {
        /// Path to a blarify JSON export
        input: PathBuf,
        /// Override the target code-graph database path
        #[arg(long = "db-path")]
        db_path: Option<PathBuf>,
        /// Legacy compatibility alias for `--db-path`
        #[arg(long = "kuzu-path", hide = true)]
        legacy_kuzu_path: Option<PathBuf>,
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
        #[arg(long = "db-path")]
        db_path: Option<PathBuf>,
        /// Legacy compatibility alias for `--db-path`
        #[arg(long = "kuzu-path", hide = true)]
        legacy_kuzu_path: Option<PathBuf>,
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
    /// Self-update the amplihack binary, then run `install` to refresh framework assets.
    ///
    /// Use --skip-install (alias --no-install) for a binary-only update (legacy behavior).
    Update {
        /// Skip the automatic `install` step after a successful update.
        #[arg(long = "skip-install", alias = "no-install")]
        skip_install: bool,
    },

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
        /// Append instructions to a running auto mode session and exit.
        #[arg(long = "append")]
        append: Option<String>,
        /// Disable post-session reflection analysis.
        #[arg(long = "no-reflection")]
        no_reflection: bool,
        /// Skip shared launcher staging/env updates for subprocess delegates.
        #[arg(long = "subprocess-safe")]
        subprocess_safe: bool,
        /// Run in autonomous agentic mode with iterative loop execution.
        #[arg(long = "auto")]
        auto: bool,
        /// Max turns for auto mode.
        #[arg(long = "max-turns", default_value_t = 10, value_parser = clap::value_parser!(u32).range(1..))]
        max_turns: u32,
        /// Enable interactive UI mode for auto mode.
        #[arg(long = "ui")]
        ui: bool,
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

    /// Resolve a named bundle asset (helper-path, session-tree-path,
    /// multitask-orchestrator) or a relative path under amplifier-bundle/.
    /// Prints the resolved absolute path on success, exits 1 if not found,
    /// exits 2 on invalid input.
    /// Replaces `python3 -m amplihack.runtime_assets` in recipe shell steps.
    #[command(name = "resolve-bundle-asset")]
    ResolveBundleAsset {
        /// Asset name (e.g. helper-path) or relative path starting with `amplifier-bundle/`
        asset: String,
    },

    /// Parallel workstream orchestrator (native Rust)
    Multitask {
        #[command(subcommand)]
        command: MultitaskCommands,
    },

    /// Pull request utilities (watch-and-merge).
    Pr {
        #[command(subcommand)]
        command: crate::commands::pr::PrCommands,
    },

    /// Smart-orchestrator helper utilities (extract-json, normalise-type).
    ///
    /// Replaces `python3 -m ... orch_helper` calls in
    /// `amplifier-bundle/recipes/smart-orchestrator.yaml` (issue #270).
    Orch {
        #[command(subcommand)]
        command: crate::commands::orch::OrchCommands,
    },

    /// Session-tree management (atomic recursion / fan-out tracking).
    ///
    /// Native Rust port of `amplifier-bundle/tools/session_tree.py`.
    /// Replaces `python3 $TREE_SCRIPT register|complete` invocations in
    /// `amplifier-bundle/recipes/smart-orchestrator.yaml` (issue #331).
    #[command(name = "session-tree")]
    SessionTree {
        #[command(subcommand)]
        command: crate::commands::session_tree::SessionTreeCommands,
    },
}
