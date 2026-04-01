//! Hook migration and compatibility notices.

use amplihack_cli::memory::default_code_graph_db_path_for_project;
use amplihack_state::AtomicJsonFile;
use amplihack_types::ProjectDirs;
use serde_json::Value;
use std::path::PathBuf;

pub(super) fn migrate_global_hooks() -> Option<String> {
    let global_settings = ProjectDirs::global_settings()?;
    if !global_settings.exists() {
        return None;
    }

    let settings_file = AtomicJsonFile::new(&global_settings);
    let settings: Value = match settings_file.read() {
        Ok(Some(value)) => value,
        Ok(None) => return None,
        Err(e) => {
            tracing::warn!("Failed to read global settings: {}", e);
            return Some(
                "⚠️ Global amplihack hooks may exist in ~/.claude/settings.json. \
                 Failed to read the file for migration."
                    .to_string(),
            );
        }
    };

    if !contains_amplihack_hooks(&settings) {
        return None;
    }

    match settings_file.update(|settings: &mut Value| remove_amplihack_hooks(settings)) {
        Ok(updated) if !contains_amplihack_hooks(&updated) => Some(
            "✅ Migrated amplihack hooks from global ~/.claude/settings.json to project-local hooks."
                .to_string(),
        ),
        Ok(_) => Some(
            "⚠️ Global amplihack hooks detected in ~/.claude/settings.json. \
             These should be migrated to project-local hooks."
                .to_string(),
        ),
        Err(e) => {
            tracing::warn!("Hook migration failed: {}", e);
            Some(
                "⚠️ Global amplihack hooks detected in ~/.claude/settings.json. \
                 Migration failed — please remove them manually."
                    .to_string(),
            )
        }
    }
}

fn contains_amplihack_hooks(settings: &Value) -> bool {
    settings
        .get("hooks")
        .and_then(Value::as_object)
        .map(|hooks_map| {
            hooks_map.values().any(|wrappers| {
                wrappers
                    .as_array()
                    .is_some_and(|wrappers| wrappers.iter().any(wrapper_references_amplihack))
            })
        })
        .unwrap_or(false)
}

fn wrapper_references_amplihack(wrapper: &Value) -> bool {
    wrapper
        .get("hooks")
        .and_then(Value::as_array)
        .is_some_and(|hooks| {
            hooks.iter().any(|hook| {
                hook.get("command")
                    .and_then(Value::as_str)
                    .map(|cmd| cmd.contains("amplihack-hooks") || cmd.contains("tools/amplihack/"))
                    .unwrap_or(false)
            })
        })
}

fn remove_amplihack_hooks(settings: &mut Value) {
    let Some(root) = settings.as_object_mut() else {
        *settings = serde_json::json!({});
        return;
    };
    let Some(hooks) = root.get_mut("hooks").and_then(Value::as_object_mut) else {
        return;
    };

    for wrappers in hooks.values_mut() {
        if let Some(wrappers) = wrappers.as_array_mut() {
            wrappers.retain(|wrapper| !wrapper_references_amplihack(wrapper));
        }
    }

    hooks.retain(|_, wrappers| {
        wrappers
            .as_array()
            .map(|arr| !arr.is_empty())
            .unwrap_or(true)
    });
}

pub(super) fn code_graph_compatibility_notice(
    dirs: &ProjectDirs,
) -> anyhow::Result<Option<String>> {
    let graph_override = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
    if graph_override
        .as_ref()
        .is_some_and(|value| !value.is_empty())
    {
        return Ok(None);
    }

    let legacy_override = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
    if legacy_override
        .as_ref()
        .is_some_and(|value| !value.is_empty())
    {
        return Ok(Some(super::format_code_graph_status(
            "Using legacy `AMPLIHACK_KUZU_DB_PATH` compatibility alias for the code graph. Prefer `AMPLIHACK_GRAPH_DB_PATH`.".to_string(),
        )));
    }

    let neutral = default_code_graph_db_path_for_project(&dirs.root)?;
    let legacy = dirs.root.join(".amplihack").join("kuzu_db");
    if legacy.exists() && !neutral.exists() {
        return Ok(Some(super::format_code_graph_status(format!(
            "Using legacy code-graph store `{}` because `{}` is absent. Migrate to the neutral `graph_db` path to leave compatibility mode.",
            legacy.display(),
            neutral.display()
        ))));
    }

    Ok(None)
}

pub(super) fn memory_graph_compatibility_notice() -> Option<String> {
    if std::env::var("AMPLIHACK_MEMORY_BACKEND").ok().as_deref() == Some("sqlite") {
        return None;
    }

    let graph_override = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
    if graph_override
        .as_ref()
        .is_some_and(|value| !value.is_empty())
    {
        return None;
    }

    let legacy_override = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
    if legacy_override
        .as_ref()
        .is_some_and(|value| !value.is_empty())
    {
        return Some(super::format_memory_status(
            "Using legacy `AMPLIHACK_KUZU_DB_PATH` compatibility alias for the memory graph. Prefer `AMPLIHACK_GRAPH_DB_PATH`.".to_string(),
        ));
    }

    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    let neutral = home.join(".amplihack").join("memory_graph.db");
    let legacy = home.join(".amplihack").join("memory_kuzu.db");
    if legacy.exists() && !neutral.exists() {
        return Some(super::format_memory_status(format!(
            "Using legacy memory graph store `{}` because `{}` is absent. Migrate to `memory_graph.db` to leave compatibility mode.",
            legacy.display(),
            neutral.display()
        )));
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::env_lock;
    use std::fs;

    #[test]
    fn remove_amplihack_hooks_preserves_third_party_entries() {
        let mut settings = serde_json::json!({
            "hooks": {
                "SessionStart": [
                    {
                        "hooks": [
                            {"type": "command", "command": "/home/user/.local/bin/amplihack-hooks session-start"}
                        ]
                    },
                    {
                        "hooks": [
                            {"type": "command", "command": "/usr/local/bin/third-party-hook"}
                        ]
                    }
                ],
                "UserPromptSubmit": [
                    {
                        "hooks": [
                            {"type": "command", "command": "/home/user/.amplihack/.claude/tools/amplihack/hooks/user_prompt_submit.py"}
                        ]
                    }
                ]
            }
        });

        remove_amplihack_hooks(&mut settings);

        assert!(!contains_amplihack_hooks(&settings));
        let session_wrappers = settings["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(session_wrappers.len(), 1);
        assert_eq!(
            session_wrappers[0]["hooks"][0]["command"].as_str(),
            Some("/usr/local/bin/third-party-hook")
        );
        assert!(settings["hooks"].get("UserPromptSubmit").is_none());
    }

    #[test]
    fn migrate_global_hooks_updates_settings_atomically() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let prev_home = std::env::var_os("HOME");
        unsafe { std::env::set_var("HOME", dir.path()) };

        let settings_path = dir.path().join(".claude/settings.json");
        fs::create_dir_all(settings_path.parent().unwrap()).unwrap();
        fs::write(
            &settings_path,
            serde_json::to_string_pretty(&serde_json::json!({
                "hooks": {
                    "SessionStart": [
                        {
                            "hooks": [
                                {"type": "command", "command": "/home/user/.local/bin/amplihack-hooks session-start"}
                            ]
                        },
                        {
                            "hooks": [
                                {"type": "command", "command": "/usr/local/bin/third-party-hook"}
                            ]
                        }
                    ]
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let message = migrate_global_hooks().expect("migration message expected");

        match prev_home {
            Some(value) => unsafe { std::env::set_var("HOME", value) },
            None => unsafe { std::env::remove_var("HOME") },
        }

        assert!(message.contains("Migrated amplihack hooks"));
        let updated: Value =
            serde_json::from_str(&fs::read_to_string(&settings_path).unwrap()).unwrap();
        assert!(!contains_amplihack_hooks(&updated));
        assert_eq!(
            updated["hooks"]["SessionStart"][0]["hooks"][0]["command"].as_str(),
            Some("/usr/local/bin/third-party-hook")
        );
    }
}
