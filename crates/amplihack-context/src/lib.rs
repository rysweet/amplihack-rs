//! Adaptive context detection, mode resolution, path handling, and LSP configuration.

pub mod launcher_detector;
pub mod lsp_detector;
pub mod migration;
pub mod mode_detector;
pub mod path_resolver;
pub mod strategies;
#[cfg(test)]
pub(crate) mod test_support;

pub use launcher_detector::{LauncherContext, LauncherDetector, LauncherType};
pub use lsp_detector::LSPDetector;
pub use migration::{MigrationHelper, MigrationInfo};
pub use mode_detector::{ClaudeMode, ModeDetector};
pub use path_resolver::PathResolver;
pub use strategies::{ClaudeStrategy, HookStrategy};
