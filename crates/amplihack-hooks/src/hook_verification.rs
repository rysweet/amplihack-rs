//! Hook file and registration verification.
//!
//! Checks that the expected amplihack hook files exist under
//! `~/.amplihack/.claude/tools/{amplihack,xpia}/hooks/` and that
//! native hook commands are registered in `~/.claude/settings.json`.

use std::fs;
use std::path::{Path, PathBuf};
use tracing::{debug, warn};

/// Hook files that must exist for a complete installation.
const REQUIRED_HOOKS: &[(&str, &str)] = &[
    ("amplihack", "PreToolUse.js"),
    ("amplihack", "PostToolUse.js"),
    ("amplihack", "Stop.js"),
    ("amplihack", "SessionStart.js"),
    ("amplihack", "SessionStop.js"),
    ("amplihack", "UserPromptSubmit.js"),
    ("amplihack", "PreCompact.js"),
    ("xpia", "PreToolUse.js"),
];

/// Native hook subcommands that must be registered in settings.json.
const REQUIRED_NATIVE_HOOKS: &[&str] = &[
    "session-start",
    "stop",
    "pre-tool-use",
    "post-tool-use",
    "workflow-classification-reminder",
    "user-prompt-submit",
    "pre-compact",
];

/// Resolve the amplihack home directory.
fn amplihack_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| Path::new(&h).join(".amplihack"))
}

/// Verify that all required hook files exist.
///
/// Returns `true` if every expected hook file is present under
/// `~/.amplihack/.claude/tools/{namespace}/hooks/`.
pub fn verify_hooks() -> bool {
    let home = match amplihack_home() {
        Some(h) => h,
        None => {
            warn!("HOME not set — cannot verify hooks");
            return false;
        }
    };

    let hooks_base = home.join(".claude").join("tools");
    let mut all_present = true;

    for (namespace, filename) in REQUIRED_HOOKS {
        let path = hooks_base.join(namespace).join("hooks").join(filename);
        if path.exists() {
            debug!("hook present: {}", path.display());
        } else {
            warn!("missing hook: {}", path.display());
            all_present = false;
        }
    }

    all_present
}

/// Verify hooks under a custom root (for testing).
pub fn verify_hooks_at(root: &Path) -> bool {
    let hooks_base = root.join(".claude").join("tools");
    let mut all_present = true;

    for (namespace, filename) in REQUIRED_HOOKS {
        let path = hooks_base.join(namespace).join("hooks").join(filename);
        if path.exists() {
            debug!("hook present: {}", path.display());
        } else {
            all_present = false;
        }
    }

    all_present
}

/// Verify that native `amplihack-hooks` commands are registered in
/// `~/.claude/settings.json`.
///
/// Returns a list of missing subcommand names. An empty list means all
/// required hooks are wired.
pub fn verify_native_hook_registrations() -> Vec<String> {
    let settings_path = match std::env::var_os("HOME") {
        Some(h) => Path::new(&h).join(".claude").join("settings.json"),
        None => {
            warn!("HOME not set — cannot verify hook registrations");
            return REQUIRED_NATIVE_HOOKS
                .iter()
                .map(|s| s.to_string())
                .collect();
        }
    };

    verify_native_hook_registrations_at(&settings_path)
}

/// Verify native hook registrations against a specific settings file (for testing).
pub fn verify_native_hook_registrations_at(settings_path: &Path) -> Vec<String> {
    let raw = match fs::read_to_string(settings_path) {
        Ok(r) => r,
        Err(_) => {
            return REQUIRED_NATIVE_HOOKS
                .iter()
                .map(|s| s.to_string())
                .collect();
        }
    };

    let json: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(_) => {
            return REQUIRED_NATIVE_HOOKS
                .iter()
                .map(|s| s.to_string())
                .collect();
        }
    };

    let registered = collect_registered_subcmds(&json);
    REQUIRED_NATIVE_HOOKS
        .iter()
        .filter(|subcmd| !registered.iter().any(|r| r == *subcmd))
        .map(|s| s.to_string())
        .collect()
}

/// Extract all `amplihack-hooks <subcmd>` subcommand names from settings JSON.
fn collect_registered_subcmds(settings: &serde_json::Value) -> Vec<String> {
    let mut subcmds = Vec::new();
    let Some(hooks_map) = settings.get("hooks").and_then(|h| h.as_object()) else {
        return subcmds;
    };
    for wrappers_val in hooks_map.values() {
        let Some(wrappers) = wrappers_val.as_array() else {
            continue;
        };
        for wrapper in wrappers {
            let Some(hook_entries) = wrapper.get("hooks").and_then(|h| h.as_array()) else {
                continue;
            };
            for entry in hook_entries {
                if let Some(cmd) = entry.get("command").and_then(|c| c.as_str())
                    && cmd.contains("amplihack-hooks")
                    && let Some(subcmd) = cmd.split_whitespace().last()
                {
                    subcmds.push(subcmd.to_string());
                }
            }
        }
    }
    subcmds
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_all_hooks(root: &Path) {
        for (namespace, filename) in REQUIRED_HOOKS {
            let dir = root
                .join(".claude")
                .join("tools")
                .join(namespace)
                .join("hooks");
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join(filename), "// hook").unwrap();
        }
    }

    #[test]
    fn all_hooks_present() {
        let tmp = tempfile::tempdir().unwrap();
        create_all_hooks(tmp.path());
        assert!(verify_hooks_at(tmp.path()));
    }

    #[test]
    fn missing_hook_returns_false() {
        let tmp = tempfile::tempdir().unwrap();
        create_all_hooks(tmp.path());
        // Remove one hook.
        let path = tmp
            .path()
            .join(".claude")
            .join("tools")
            .join("amplihack")
            .join("hooks")
            .join("Stop.js");
        fs::remove_file(path).unwrap();
        assert!(!verify_hooks_at(tmp.path()));
    }

    #[test]
    fn empty_dir_returns_false() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(!verify_hooks_at(tmp.path()));
    }

    #[test]
    fn required_hooks_not_empty() {
        assert!(!REQUIRED_HOOKS.is_empty());
    }

    #[test]
    fn all_hooks_have_js_extension() {
        for (_, filename) in REQUIRED_HOOKS {
            assert!(filename.ends_with(".js"), "{filename} should end with .js");
        }
    }

    #[test]
    fn native_registration_missing_file_returns_all() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nonexistent.json");
        let missing = verify_native_hook_registrations_at(&path);
        assert_eq!(missing.len(), REQUIRED_NATIVE_HOOKS.len());
    }

    #[test]
    fn native_registration_empty_settings_returns_all() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");
        fs::write(&path, "{}").unwrap();
        let missing = verify_native_hook_registrations_at(&path);
        assert_eq!(missing.len(), REQUIRED_NATIVE_HOOKS.len());
    }

    #[test]
    fn native_registration_with_hooks_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");
        let settings = serde_json::json!({
            "hooks": {
                "SessionStart": [{"hooks": [{"type": "command", "command": "amplihack-hooks session-start"}]}],
                "Stop": [{"hooks": [{"type": "command", "command": "amplihack-hooks stop"}]}],
                "PreToolUse": [{"matcher": "*", "hooks": [{"type": "command", "command": "amplihack-hooks pre-tool-use"}]}],
                "PostToolUse": [{"matcher": "*", "hooks": [{"type": "command", "command": "amplihack-hooks post-tool-use"}]}],
                "UserPromptSubmit": [
                    {"hooks": [{"type": "command", "command": "amplihack-hooks workflow-classification-reminder"}]},
                    {"hooks": [{"type": "command", "command": "amplihack-hooks user-prompt-submit"}]}
                ],
                "PreCompact": [{"hooks": [{"type": "command", "command": "amplihack-hooks pre-compact"}]}]
            }
        });
        fs::write(&path, serde_json::to_string_pretty(&settings).unwrap()).unwrap();
        let missing = verify_native_hook_registrations_at(&path);
        assert!(
            missing.is_empty(),
            "expected no missing hooks, got: {missing:?}"
        );
    }

    #[test]
    fn native_registration_partial_returns_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");
        let settings = serde_json::json!({
            "hooks": {
                "SessionStart": [{"hooks": [{"type": "command", "command": "amplihack-hooks session-start"}]}],
                "Stop": [{"hooks": [{"type": "command", "command": "amplihack-hooks stop"}]}]
            }
        });
        fs::write(&path, serde_json::to_string_pretty(&settings).unwrap()).unwrap();
        let missing = verify_native_hook_registrations_at(&path);
        assert!(missing.contains(&"pre-tool-use".to_string()));
        assert!(missing.contains(&"post-tool-use".to_string()));
        assert!(!missing.contains(&"session-start".to_string()));
        assert!(!missing.contains(&"stop".to_string()));
    }
}
