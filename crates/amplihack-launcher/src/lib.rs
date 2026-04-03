//! amplihack-launcher: Extended launcher support for all agent types.
//!
//! Provides Codex, Amplifier, and Copilot MCP launchers, plus session
//! forking and transcript capture — matching the Python amplihack launcher
//! subsystem.

pub mod agent_memory;
pub mod amplifier;
pub mod append_handler;
pub mod auto_mode_coordinator;
pub mod auto_mode_state;
pub mod auto_mode_ui;
pub mod codex;
pub mod completion_signals;
pub mod completion_verifier;
pub mod copilot_mcp;
pub mod fork_manager;
pub mod nesting_detector;
pub mod repo_checkout;
pub mod session_capture;
pub mod session_tracker;
pub mod settings_manager;
pub mod work_summary;

pub use amplifier::AmplifierInfo;
pub use codex::CodexInfo;
pub use copilot_mcp::McpServerConfig;
pub use fork_manager::{ForkConfig, ForkDecision, ForkManager};
pub use session_capture::{CapturedMessage, MessageCapture, MessageRole};

// Re-exports for ported supporting modules
pub use agent_memory::{AgentMemory, Experience, ExperienceStore, ExperienceType};
pub use append_handler::{append_instructions, AppendError, AppendResult};
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
