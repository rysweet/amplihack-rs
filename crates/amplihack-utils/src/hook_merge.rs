//! Hook merge utility for settings.json management.
//!
//! Ports Python `amplihack/utils/hook_merge_utility.py`:
//! - HookConfig for declaring required hooks
//! - MergeResult for tracking merge outcomes
//! - HookMergeUtility for reading/merging/writing settings.json

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Errors from hook-merge operations.
#[derive(Debug, Error)]
pub enum HookMergeError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

/// A hook configuration entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookConfig {
    pub hook_type: String,
    pub event: String,
    pub command: String,
    #[serde(default)]
    pub description: String,
}

impl HookConfig {
    /// Convert to a settings.json hook entry.
    pub fn to_entry(&self) -> serde_json::Value {
        serde_json::json!({
            "event": self.event,
            "command": self.command,
        })
    }
}

/// Result of a merge operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergeResult {
    pub hooks_added: usize,
    pub hooks_updated: usize,
    pub hooks_unchanged: usize,
    pub backup_path: Option<String>,
    pub success: bool,
    #[serde(default)]
    pub error: Option<String>,
}

/// Hook merge utility for settings.json management.
pub struct HookMergeUtility {
    settings_path: PathBuf,
}

impl HookMergeUtility {
    pub fn new(settings_path: impl AsRef<Path>) -> Self {
        Self {
            settings_path: settings_path.as_ref().to_path_buf(),
        }
    }

    /// Merge required hooks into settings.json.
    pub fn merge_hooks(
        &self,
        hooks: &[HookConfig],
    ) -> Result<MergeResult, HookMergeError> {
        // Load or create settings
        let mut settings = self.load_settings()?;

        // Backup before modifying
        let backup_path = self.backup_settings(&settings)?;

        // Merge hooks
        let mut added = 0;
        let mut updated = 0;
        let mut unchanged = 0;

        let hooks_array = settings
            .entry("hooks")
            .or_insert_with(|| serde_json::json!([]))
            .as_array_mut()
            .ok_or_else(|| {
                HookMergeError::Json(serde_json::Error::io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "hooks is not an array",
                )))
            })?
            .clone();

        let mut new_hooks = hooks_array;

        for hook in hooks {
            let entry = hook.to_entry();
            let existing_idx = Self::find_existing_hook(&new_hooks, &hook.command);

            if let Some(idx) = existing_idx {
                if new_hooks[idx] != entry {
                    new_hooks[idx] = entry;
                    updated += 1;
                } else {
                    unchanged += 1;
                }
            } else {
                new_hooks.push(entry);
                added += 1;
            }
        }

        settings["hooks"] = serde_json::Value::Array(new_hooks);

        // Save
        self.save_settings(&settings)?;

        Ok(MergeResult {
            hooks_added: added,
            hooks_updated: updated,
            hooks_unchanged: unchanged,
            backup_path: Some(backup_path),
            success: true,
            error: None,
        })
    }

    fn load_settings(&self) -> Result<serde_json::Map<String, serde_json::Value>, HookMergeError> {
        if self.settings_path.exists() {
            let content = std::fs::read_to_string(&self.settings_path)?;
            let val: serde_json::Value = serde_json::from_str(&content)?;
            Ok(val.as_object().cloned().unwrap_or_default())
        } else {
            Ok(Self::default_settings())
        }
    }

    fn default_settings() -> serde_json::Map<String, serde_json::Value> {
        let mut m = serde_json::Map::new();
        m.insert(
            "hooks".to_string(),
            serde_json::Value::Array(Vec::new()),
        );
        m
    }

    fn backup_settings(
        &self,
        settings: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<String, HookMergeError> {
        if !self.settings_path.exists() {
            return Ok(String::new());
        }
        let backup_name = format!(
            "{}.backup",
            self.settings_path.display()
        );
        let content = serde_json::to_string_pretty(
            &serde_json::Value::Object(settings.clone()),
        )?;
        std::fs::write(&backup_name, content)?;
        Ok(backup_name)
    }

    fn save_settings(
        &self,
        settings: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<(), HookMergeError> {
        if let Some(parent) = self.settings_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(
            &serde_json::Value::Object(settings.clone()),
        )?;
        std::fs::write(&self.settings_path, content)?;
        Ok(())
    }

    fn find_existing_hook(
        hooks: &[serde_json::Value],
        command: &str,
    ) -> Option<usize> {
        hooks.iter().position(|h| {
            h.get("command")
                .and_then(|c| c.as_str())
                .map(|c| c == command)
                .unwrap_or(false)
        })
    }
}

/// Get the required XPIA hooks for amplihack.
pub fn get_required_xpia_hooks() -> Vec<HookConfig> {
    vec![
        HookConfig {
            hook_type: "PostToolUse".to_string(),
            event: "PostToolUse".to_string(),
            command: "amplihack-xpia-hook".to_string(),
            description: "XPIA detection hook".to_string(),
        },
        HookConfig {
            hook_type: "PreToolUse".to_string(),
            event: "PreToolUse".to_string(),
            command: "amplihack-pre-tool-hook".to_string(),
            description: "Pre-tool validation hook".to_string(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn hook_config_to_entry() {
        let hook = HookConfig {
            hook_type: "PostToolUse".to_string(),
            event: "PostToolUse".to_string(),
            command: "test-cmd".to_string(),
            description: "desc".to_string(),
        };
        let entry = hook.to_entry();
        assert_eq!(entry["command"], "test-cmd");
        assert_eq!(entry["event"], "PostToolUse");
    }

    #[test]
    fn merge_into_empty_settings() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("settings.json");
        let util = HookMergeUtility::new(&path);

        let hooks = vec![HookConfig {
            hook_type: "Post".to_string(),
            event: "PostToolUse".to_string(),
            command: "my-hook".to_string(),
            description: "test".to_string(),
        }];

        let result = util.merge_hooks(&hooks).unwrap();
        assert!(result.success);
        assert_eq!(result.hooks_added, 1);
        assert_eq!(result.hooks_unchanged, 0);
    }

    #[test]
    fn merge_idempotent() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("settings.json");

        let hooks = vec![HookConfig {
            hook_type: "Post".to_string(),
            event: "PostToolUse".to_string(),
            command: "my-hook".to_string(),
            description: "test".to_string(),
        }];

        // First merge
        let util = HookMergeUtility::new(&path);
        let r1 = util.merge_hooks(&hooks).unwrap();
        assert_eq!(r1.hooks_added, 1);

        // Second merge — should be unchanged
        let util2 = HookMergeUtility::new(&path);
        let r2 = util2.merge_hooks(&hooks).unwrap();
        assert_eq!(r2.hooks_unchanged, 1);
        assert_eq!(r2.hooks_added, 0);
    }

    #[test]
    fn required_xpia_hooks() {
        let hooks = get_required_xpia_hooks();
        assert_eq!(hooks.len(), 2);
        assert!(hooks.iter().any(|h| h.event == "PostToolUse"));
        assert!(hooks.iter().any(|h| h.event == "PreToolUse"));
    }

    #[test]
    fn merge_result_serde() {
        let r = MergeResult {
            hooks_added: 1,
            hooks_updated: 0,
            hooks_unchanged: 2,
            backup_path: Some("/tmp/backup".to_string()),
            success: true,
            error: None,
        };
        let json = serde_json::to_string(&r).unwrap();
        let back: MergeResult = serde_json::from_str(&json).unwrap();
        assert_eq!(back.hooks_added, 1);
    }
}
