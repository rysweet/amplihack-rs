//! Settings types for amplihack configuration.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Hook configuration entry in settings.json.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookConfig {
    /// The hook type (e.g., "PreToolUse", "Stop").
    #[serde(rename = "type")]
    pub hook_type: String,

    /// Command to execute for this hook.
    pub command: String,

    /// Timeout in seconds.
    #[serde(default = "default_timeout")]
    pub timeout: u64,
}

fn default_timeout() -> u64 {
    10
}

/// Top-level settings structure.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Settings {
    /// Hook configurations keyed by hook name.
    #[serde(default)]
    pub hooks: HashMap<String, Vec<HookConfig>>,

    /// Project root path.
    #[serde(default)]
    pub project_root: Option<PathBuf>,

    /// Additional settings (forward-compatible).
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_settings_with_hooks() {
        let json = r#"{
            "hooks": {
                "PreToolUse": [
                    {"type": "PreToolUse", "command": "/usr/bin/amplihack-hooks pre-tool-use", "timeout": 10}
                ]
            }
        }"#;
        let settings: Settings = serde_json::from_str(json).unwrap();
        assert_eq!(settings.hooks.len(), 1);
        assert_eq!(settings.hooks["PreToolUse"][0].timeout, 10);
    }

    #[test]
    fn deserialize_empty_settings() {
        let json = "{}";
        let settings: Settings = serde_json::from_str(json).unwrap();
        assert!(settings.hooks.is_empty());
        assert!(settings.project_root.is_none());
    }

    #[test]
    fn forward_compatible_extra_fields() {
        let json = r#"{"hooks": {}, "future_setting": true}"#;
        let settings: Settings = serde_json::from_str(json).unwrap();
        assert!(settings.extra.contains_key("future_setting"));
    }
}
