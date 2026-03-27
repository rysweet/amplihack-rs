//! Launcher-aware post_tool_use behavior.

use crate::pre_tool_use::launcher::{LauncherType, detect_launcher_for_dirs};
use amplihack_types::ProjectDirs;

pub(crate) fn copilot_post_tool_use_message(dirs: &ProjectDirs, tool_name: &str) -> Option<String> {
    if detect_launcher_for_dirs(dirs) != LauncherType::Copilot {
        return None;
    }

    let tool_name = if tool_name.trim().is_empty() {
        "unknown"
    } else {
        tool_name
    };

    Some(format!(
        "Post-tool hook in Copilot mode - tool: {tool_name} (logging only)"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::env_lock;
    use amplihack_cli::launcher_context::{LauncherKind, write_launcher_context};
    use std::collections::BTreeMap;

    fn set_launcher_env(copilot: Option<&str>, claude_code: Option<&str>) {
        match copilot {
            Some(value) => unsafe { std::env::set_var("GITHUB_COPILOT_AGENT", value) },
            None => unsafe { std::env::remove_var("GITHUB_COPILOT_AGENT") },
        }
        unsafe { std::env::remove_var("COPILOT_AGENT") };
        match claude_code {
            Some(value) => unsafe { std::env::set_var("CLAUDE_CODE_SESSION", value) },
            None => unsafe { std::env::remove_var("CLAUDE_CODE_SESSION") },
        }
        unsafe { std::env::remove_var("AMPLIFIER_SESSION") };
        unsafe { std::env::remove_var("CLAUDE_SESSION_ID") };
    }

    #[test]
    fn returns_logging_message_for_copilot_env() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        set_launcher_env(Some("1"), None);

        let dir = tempfile::tempdir().unwrap();
        let message = copilot_post_tool_use_message(&ProjectDirs::new(dir.path()), "Bash").unwrap();

        assert_eq!(
            message,
            "Post-tool hook in Copilot mode - tool: Bash (logging only)"
        );
    }

    #[test]
    fn returns_none_for_non_copilot_launcher() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        set_launcher_env(None, Some("session"));

        let dir = tempfile::tempdir().unwrap();
        assert!(copilot_post_tool_use_message(&ProjectDirs::new(dir.path()), "Read").is_none());
    }

    #[test]
    fn returns_logging_message_for_persisted_copilot_context() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        set_launcher_env(None, None);

        let dir = tempfile::tempdir().unwrap();
        write_launcher_context(
            dir.path(),
            LauncherKind::Copilot,
            "amplihack copilot",
            BTreeMap::new(),
        )
        .unwrap();

        let message = copilot_post_tool_use_message(&ProjectDirs::new(dir.path()), "Read").unwrap();

        assert_eq!(
            message,
            "Post-tool hook in Copilot mode - tool: Read (logging only)"
        );
    }

    #[test]
    fn falls_back_to_unknown_for_empty_tool_name() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        set_launcher_env(Some("1"), None);

        let dir = tempfile::tempdir().unwrap();
        let message = copilot_post_tool_use_message(&ProjectDirs::new(dir.path()), "   ").unwrap();

        assert_eq!(
            message,
            "Post-tool hook in Copilot mode - tool: unknown (logging only)"
        );
    }
}
