//! Top-level `Commands` enum for the amplihack CLI.

use clap::Subcommand;
use clap_complete::Shell;
use std::path::PathBuf;

use super::{
    BuilderCommands, HygieneCommands, MemoryCommands, ModeCommands, MultitaskCommands,
    PluginCommands, QueryCodeCommands, RecipeCommands, ReflectCommands, RemoteCommands,
    SignalCommands,
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
        /// Accepted for diagnostic scripts; install already prints phase-level detail
        #[arg(long)]
        verbose: bool,
        /// Force a fresh network download of amplifier-bundle assets.
        /// Used internally by `amplihack update` when spawning the new binary
        /// as a post-update install subprocess (issue #683).
        #[arg(long = "force-refresh", hide = true)]
        force_refresh: bool,
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
        #[arg(long = "max-turns", env = "AMPLIHACK_AUTO_MAX_TURNS", default_value_t = 10, value_parser = clap::value_parser!(u32).range(1..))]
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
        #[arg(long = "max-turns", env = "AMPLIHACK_AUTO_MAX_TURNS", default_value_t = 10, value_parser = clap::value_parser!(u32).range(1..))]
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
        /// Force post-session reflection analysis ON. Overrides the
        /// subprocess-safe default flip (issue #621). Mutually exclusive
        /// with `--no-reflection`.
        #[arg(long = "reflection", conflicts_with = "no_reflection")]
        reflection: bool,
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
        #[arg(long = "max-turns", env = "AMPLIHACK_AUTO_MAX_TURNS", default_value_t = 10, value_parser = clap::value_parser!(u32).range(1..))]
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
        #[arg(long = "max-turns", env = "AMPLIHACK_AUTO_MAX_TURNS", default_value_t = 10, value_parser = clap::value_parser!(u32).range(1..))]
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
        #[arg(long = "max-turns", env = "AMPLIHACK_AUTO_MAX_TURNS", default_value_t = 10, value_parser = clap::value_parser!(u32).range(1..))]
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
    /// Conservative local cleanup automation
    Hygiene {
        #[command(subcommand)]
        command: HygieneCommands,
    },
    /// Remote execution and detached session management
    Remote {
        #[command(subcommand)]
        command: RemoteCommands,
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
    /// Provider-neutral workflow helper utilities.
    Workflow {
        #[command(subcommand)]
        command: crate::commands::workflow::WorkflowCommands,
    },
    /// Mode management
    Mode {
        #[command(subcommand)]
        command: ModeCommands,
    },
    /// Enable continuous work mode (creates project lock file)
    Lock {
        /// Optional custom instruction to persist alongside the lock
        #[arg(short = 'm', long = "message")]
        message: Option<String>,
    },
    /// Disable continuous work mode (removes project lock file)
    Unlock,
    /// Show whether continuous work mode is active
    LockStatus,
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
        #[arg(long = "max-turns", env = "AMPLIHACK_AUTO_MAX_TURNS", default_value_t = 10, value_parser = clap::value_parser!(u32).range(1..))]
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
    #[command(
        after_help = "Diagnostics:\n  node       Diagnose Node.js >=24.0.0 for Copilot CLI\n  copilot    Diagnose Copilot CLI prerequisites\n\nRemediation options:\n  amplihack doctor node --ensure\n  amplihack doctor copilot --ensure-node"
    )]
    Doctor {
        /// Arguments forwarded to doctor diagnostics: node, copilot, --ensure, --ensure-node
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Resolve a named bundle asset (multitask-orchestrator) or a relative path
    /// under amplifier-bundle/.
    /// Prints the resolved absolute path on success, exits 1 if not found,
    /// exits 2 on invalid input.
    /// Replaces `python3 -m amplihack.runtime_assets` in recipe shell steps.
    #[command(name = "resolve-bundle-asset")]
    ResolveBundleAsset {
        /// Asset name (e.g. multitask-orchestrator) or relative path starting with `amplifier-bundle/`
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

    /// Validate C# files at configurable strictness levels (1-4).
    ///
    /// Level 1: syntax (balanced delimiters, patterns). Level 2: + dotnet build.
    /// Level 3: + analyzers. Level 4: + dotnet format --verify-no-changes.
    #[command(name = "cs-validate")]
    CsValidate {
        /// Path to a .cs file or directory to validate
        path: PathBuf,
        /// Validation level (1-4)
        #[arg(long, default_value_t = 0, value_parser = clap::value_parser!(u8).range(0..=4))]
        level: u8,
        /// Override config file path
        #[arg(long)]
        config: Option<PathBuf>,
        /// Output format
        #[arg(long, default_value = "text", value_parser = ["text", "json"])]
        format: String,
    },

    /// Evaluate MCP tool integration quality using scenarios and scoring.
    ///
    /// Produces a recommendation: INTEGRATE / CONSIDER / DONT_INTEGRATE
    /// based on quality and efficiency metrics.
    #[command(name = "mcp-eval")]
    McpEval {
        /// Adapter to evaluate (default: mock)
        #[arg(default_value = "mock")]
        adapter: String,
        /// Filter to a specific scenario
        #[arg(long)]
        scenario: Option<String>,
        /// Run in mock/dry-run mode without an MCP server
        #[arg(long)]
        mock: bool,
        /// Output path for the report
        #[arg(long)]
        output: Option<PathBuf>,
        /// Override config file path
        #[arg(long)]
        config: Option<PathBuf>,
    },

    /// Signal channel onboarding and fleet distribution (issue #921).
    ///
    /// `setup` onboards THIS host (detect/install signal-cli, link a device,
    /// start a local JSON-RPC daemon, write ~/.amplihack/signal-config.toml);
    /// `distribute` rolls the same onboarding across an azlin fleet. Always
    /// registered; the implementation requires a `--features signal` build.
    Signal {
        #[command(subcommand)]
        command: SignalCommands,
    },
}

#[cfg(test)]
mod issue_1081_max_turns_env_tests {
    //! TDD contracts for issue #1081: `max_turns` must honour the
    //! `AMPLIHACK_AUTO_MAX_TURNS` env var as an operator-configured policy,
    //! with the explicit `--max-turns` CLI flag taking precedence, the default
    //! remaining 10, and the `range(1..)` validator rejecting invalid values.
    //!
    //! These tests assert the *desired* behaviour and fail until the six
    //! `#[arg(...)]` declarations gain `env = "AMPLIHACK_AUTO_MAX_TURNS"` and
    //! clap's `env` feature is enabled workspace-wide.
    use super::*;
    use crate::test_support::env_lock;

    const ENV_KEY: &str = "AMPLIHACK_AUTO_MAX_TURNS";

    /// RAII guard that sets/removes `AMPLIHACK_AUTO_MAX_TURNS` and restores the
    /// prior value on drop. Tests must hold `env_lock()` while it is alive.
    struct MaxTurnsEnvGuard {
        previous: Option<std::ffi::OsString>,
    }

    impl MaxTurnsEnvGuard {
        fn set(value: &str) -> Self {
            let previous = std::env::var_os(ENV_KEY);
            // SAFETY: edition 2024 requires unsafe; tests serialise via env_lock().
            unsafe {
                std::env::set_var(ENV_KEY, value);
            }
            Self { previous }
        }

        fn clear() -> Self {
            let previous = std::env::var_os(ENV_KEY);
            // SAFETY: edition 2024 requires unsafe; tests serialise via env_lock().
            unsafe {
                std::env::remove_var(ENV_KEY);
            }
            Self { previous }
        }
    }

    impl Drop for MaxTurnsEnvGuard {
        fn drop(&mut self) {
            // SAFETY: edition 2024 requires unsafe; tests serialise via env_lock().
            unsafe {
                match self.previous.take() {
                    Some(value) => std::env::set_var(ENV_KEY, value),
                    None => std::env::remove_var(ENV_KEY),
                }
            }
        }
    }

    fn launch_max_turns(argv: &[&str]) -> u32 {
        match crate::Cli::try_parse_from(argv)
            .expect("cli should parse")
            .command
        {
            Commands::Launch { max_turns, .. } => max_turns,
            other => panic!("expected Launch command, got {other:?}"),
        }
    }

    fn rustyclawd_max_turns(argv: &[&str]) -> u32 {
        match crate::Cli::try_parse_from(argv)
            .expect("cli should parse")
            .command
        {
            Commands::RustyClawd { max_turns, .. } => max_turns,
            other => panic!("expected RustyClawd command, got {other:?}"),
        }
    }

    #[test]
    fn default_max_turns_is_ten_without_env_or_flag() {
        let _lock = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let _guard = MaxTurnsEnvGuard::clear();
        assert_eq!(launch_max_turns(&["amplihack", "launch", "--auto"]), 10);
    }

    #[test]
    fn env_var_sets_max_turns() {
        let _lock = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let _guard = MaxTurnsEnvGuard::set("3");
        assert_eq!(launch_max_turns(&["amplihack", "launch", "--auto"]), 3);
    }

    #[test]
    fn cli_flag_overrides_env_var() {
        let _lock = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let _guard = MaxTurnsEnvGuard::set("3");
        assert_eq!(
            launch_max_turns(&["amplihack", "launch", "--auto", "--max-turns", "7"]),
            7
        );
    }

    #[test]
    fn env_var_applies_to_rustyclawd_variant() {
        let _lock = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let _guard = MaxTurnsEnvGuard::set("3");
        assert_eq!(
            rustyclawd_max_turns(&["amplihack", "RustyClawd", "--auto"]),
            3
        );
    }

    #[test]
    fn env_var_zero_is_rejected_by_range_validator() {
        let _lock = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let _guard = MaxTurnsEnvGuard::set("0");
        assert!(
            crate::Cli::try_parse_from(["amplihack", "launch", "--auto"]).is_err(),
            "AMPLIHACK_AUTO_MAX_TURNS=0 must be rejected by range(1..)"
        );
    }

    #[test]
    fn env_var_non_numeric_is_rejected() {
        let _lock = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let _guard = MaxTurnsEnvGuard::set("abc");
        assert!(
            crate::Cli::try_parse_from(["amplihack", "launch", "--auto"]).is_err(),
            "AMPLIHACK_AUTO_MAX_TURNS=abc must be rejected"
        );
    }
}
