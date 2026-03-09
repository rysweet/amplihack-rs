//! Tool decision types.

use serde::{Deserialize, Serialize};

/// Decision about whether to allow or deny a tool invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ToolDecision {
    /// Allow the tool to proceed.
    #[default]
    Allow,
    /// Deny the tool invocation.
    Deny,
    /// Ask the user for confirmation.
    Ask,
}
