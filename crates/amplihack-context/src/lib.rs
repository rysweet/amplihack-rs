//! Adaptive context detection, mode resolution, path handling, and LSP configuration.

pub mod launcher_detector;
pub mod strategies;
pub mod mode_detector;
pub mod path_resolver;
pub mod lsp_detector;

pub use launcher_detector::{LauncherDetector, LauncherType, LauncherContext};
pub use strategies::{HookStrategy, ClaudeStrategy, CopilotStrategy};
pub use mode_detector::{ClaudeMode, ModeDetector};
pub use path_resolver::PathResolver;
pub use lsp_detector::LSPDetector;
