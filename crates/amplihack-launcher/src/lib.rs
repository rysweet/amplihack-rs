//! amplihack-launcher: Extended launcher support for all agent types.
//!
//! Provides Codex, Amplifier, and Copilot MCP launchers, plus session
//! forking and transcript capture — matching the Python amplihack launcher
//! subsystem.

pub mod agent_memory;
pub mod amplifier;
pub mod append_handler;
pub mod auto_mode;
pub mod auto_mode_coordinator;
pub mod auto_mode_exec;
pub mod auto_mode_state;
pub mod auto_mode_ui;
pub mod auto_stager;
pub mod claude_binary_manager;
pub mod codex;
pub mod completion_signals;
pub mod completion_verifier;
pub mod copilot_auto_install;
pub mod copilot_launcher;
pub mod copilot_mcp;
pub mod copilot_staging;
pub mod flag_matrix;
pub mod fork_manager;
pub mod json_logger;
pub mod launcher_core;
pub mod memory_config;
pub mod nesting_detector;
pub mod platform_check;
pub mod repo_checkout;
pub mod session_capture;
pub mod session_tracker;
pub mod settings_manager;
pub mod staging_cleanup;
pub mod staging_safety;
pub mod work_summary;

pub use amplifier::AmplifierInfo;
pub use auto_mode::{AutoModeConfig, SdkBackend, SessionResult, TurnResult};
pub use auto_mode_exec::AutoMode;
pub use codex::CodexInfo;
pub use copilot_launcher::PluginEntry;
pub use copilot_mcp::McpServerConfig;
pub use fork_manager::{ForkConfig, ForkDecision, ForkManager};
pub use launcher_core::{ClaudeLauncher, LauncherConfig, detect_repo_root};
pub use memory_config::{MemoryConfig, MemoryPreference};
pub use session_capture::{CapturedMessage, MessageCapture, MessageRole};

// Re-exports for ported supporting modules
pub use agent_memory::{AgentMemory, Experience, ExperienceStore, ExperienceType};
pub use append_handler::{AppendError, AppendResult, append_instructions};
pub use auto_mode_coordinator::{AutoModeCoordinator, AutoModeRunner};
pub use auto_mode_state::{AutoModeState, CostInfo, StateSnapshot};
pub use auto_mode_ui::AutoModeUi;
pub use completion_signals::{CompletionSignalDetector, CompletionSignals, SignalScore};
pub use completion_verifier::{CompletionVerifier, VerificationResult, VerificationStatus};
pub use nesting_detector::{NestingDetector, NestingResult};
pub use repo_checkout::{checkout_repository, parse_github_uri};
pub use session_tracker::{SessionEntry, SessionTracker};
pub use settings_manager::SettingsManager;
pub use work_summary::{
    GitHubState, GitState, TodoExtractor, TodoState, WorkSummary, WorkSummaryGenerator,
};

// Re-exports for newly ported modules
pub use auto_stager::{AutoStager, StagingResult};
pub use claude_binary_manager::{BinaryInfo, ClaudeBinaryManager};
pub use json_logger::JsonLogger;
pub use platform_check::{PlatformCheckResult, check_platform_compatibility, is_native_windows};
