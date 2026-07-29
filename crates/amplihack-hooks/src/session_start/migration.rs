//! Hook migration and compatibility notices.

use amplihack_memory::cli_memory::default_code_graph_db_path_for_project;
use amplihack_state::AtomicJsonFile;
use amplihack_types::ProjectDirs;
use serde_json::Value;
use std::path::PathBuf;

pub(super) fn migrate_global_hooks(dirs: &ProjectDirs) -> Option<String> {
    let global_settings = ProjectDirs::global_settings()?;

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

    // The global hooks are only redundant once project-local hooks exist. If the
    // repo-local `.claude/settings.json` does not (yet) contain amplihack hooks,
    // the global copy is the ONLY working copy — deleting it here would silently
    // uninstall the framework (issue #1088). In that case do nothing and stay
    // quiet: no deletion, no bogus "migrated" message.
    if !repo_local_contains_amplihack_hooks(dirs) {
        return None;
    }

    // Project-local hooks are present, so the global copy is now redundant and
    // safe to remove. This is a pure cleanup — never claim a "migration/move".
    match settings_file.update(|settings: &mut Value| remove_amplihack_hooks(settings)) {
        Ok(updated) if !contains_amplihack_hooks(&updated) => Some(
            "Removed redundant global amplihack hooks from ~/.claude/settings.json; \
             project-local hooks in .claude/settings.json remain active."
                .to_string(),
        ),
        Ok(_) => Some(
            "⚠️ Redundant global amplihack hooks detected in ~/.claude/settings.json. \
             Automatic cleanup did not remove them — please remove them manually."
                .to_string(),
        ),
        Err(e) => {
            tracing::warn!("Hook cleanup failed: {}", e);
            Some(
                "⚠️ Redundant global amplihack hooks detected in ~/.claude/settings.json. \
                 Cleanup failed — please remove them manually."
                    .to_string(),
            )
        }
    }
}

/// Return `true` if the repo-local `<project-root>/.claude/settings.json`
/// already contains amplihack hooks. This is a read-only probe used to decide
/// whether the global hooks are redundant; it never writes.
fn repo_local_contains_amplihack_hooks(dirs: &ProjectDirs) -> bool {
    let repo_local = dirs.claude.join("settings.json");
    match AtomicJsonFile::new(&repo_local).read() {
        Ok(Some(value)) => contains_amplihack_hooks(&value),
        Ok(None) => false,
        Err(e) => {
            tracing::warn!("Failed to read repo-local settings: {}", e);
            false
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
        // The settings file did not parse to a JSON object (e.g. a truncated
        // write left an array/string/null). Removing "our" hooks must never
        // zero out a user's entire config, so leave the value untouched.
        tracing::warn!("Global settings.json was not a JSON object; left unchanged");
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

    /// (d) Data-loss guard: a non-object settings value must be left UNCHANGED
    /// and must never be replaced with `{}`.
    #[test]
    fn remove_amplihack_hooks_leaves_non_object_values_untouched() {
        for input in [
            serde_json::json!([]),
            serde_json::json!("x"),
            serde_json::json!(42),
            Value::Null,
        ] {
            let mut value = input.clone();
            remove_amplihack_hooks(&mut value);
            assert_eq!(
                value, input,
                "non-object settings value must be left untouched"
            );
            assert_ne!(
                value,
                serde_json::json!({}),
                "non-object value must not be replaced with an empty object"
            );
        }
    }

    fn amplihack_hooks_json() -> serde_json::Value {
        serde_json::json!({
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
        })
    }

    fn write_json(path: &std::path::Path, value: &serde_json::Value) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, serde_json::to_string_pretty(value).unwrap()).unwrap();
    }

    /// (a) Self-uninstall guard (regression test for #1088): global has
    /// amplihack hooks but the repo-local `.claude/settings.json` does NOT, so
    /// the global copy is the only working copy and MUST be left untouched.
    #[test]
    fn migrate_global_hooks_does_not_delete_when_no_repo_local_hooks() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let home = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        let prev_home = std::env::var_os("HOME");
        unsafe { std::env::set_var("HOME", home.path()) };

        let global_path = home.path().join(".claude/settings.json");
        let global_value = amplihack_hooks_json();
        write_json(&global_path, &global_value);

        let dirs = ProjectDirs::new(repo.path());

        // Case 1: repo-local settings.json absent entirely.
        let result_absent = migrate_global_hooks(&dirs);

        // Case 2: repo-local settings.json present but WITHOUT amplihack hooks.
        write_json(
            &dirs.claude.join("settings.json"),
            &serde_json::json!({
                "hooks": {
                    "SessionStart": [
                        { "hooks": [{"type": "command", "command": "/usr/local/bin/third-party-hook"}] }
                    ]
                }
            }),
        );
        let result_no_amplihack = migrate_global_hooks(&dirs);

        match prev_home {
            Some(value) => unsafe { std::env::set_var("HOME", value) },
            None => unsafe { std::env::remove_var("HOME") },
        }

        assert!(
            result_absent.is_none(),
            "must not act when repo-local settings absent"
        );
        assert!(
            result_no_amplihack.is_none(),
            "must not act when repo-local lacks amplihack hooks"
        );

        // The global file must be completely UNCHANGED (still has amplihack hooks).
        let after: Value =
            serde_json::from_str(&fs::read_to_string(&global_path).unwrap()).unwrap();
        assert_eq!(after, global_value, "global settings must be untouched");
        assert!(contains_amplihack_hooks(&after));
    }

    /// (b)/(c) Redundant-cleanup path: global AND repo-local both have amplihack
    /// hooks -> the global copy is removed, third-party global entries are
    /// preserved, and the message does NOT falsely claim a move/migration.
    #[test]
    fn migrate_global_hooks_removes_redundant_global_when_repo_local_present() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let home = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        let prev_home = std::env::var_os("HOME");
        unsafe { std::env::set_var("HOME", home.path()) };

        let global_path = home.path().join(".claude/settings.json");
        write_json(&global_path, &amplihack_hooks_json());

        let dirs = ProjectDirs::new(repo.path());
        write_json(&dirs.claude.join("settings.json"), &amplihack_hooks_json());

        let message = migrate_global_hooks(&dirs).expect("cleanup message expected");

        match prev_home {
            Some(value) => unsafe { std::env::set_var("HOME", value) },
            None => unsafe { std::env::remove_var("HOME") },
        }

        assert!(
            !message.to_lowercase().contains("migrated"),
            "message must not claim a migration/move; got: {message}"
        );
        assert!(
            message.contains("Removed redundant global amplihack hooks"),
            "message should state redundant global hooks were removed; got: {message}"
        );

        let updated: Value =
            serde_json::from_str(&fs::read_to_string(&global_path).unwrap()).unwrap();
        assert!(
            !contains_amplihack_hooks(&updated),
            "global amplihack hooks should be removed"
        );
        assert_eq!(
            updated["hooks"]["SessionStart"][0]["hooks"][0]["command"].as_str(),
            Some("/usr/local/bin/third-party-hook"),
            "third-party global entries must be preserved"
        );
    }

    /// Absent-global guard (#1123): with no global `~/.claude/settings.json`,
    /// the collapsed atomic read must yield `None` (nothing to migrate).
    #[test]
    fn migrate_global_hooks_returns_none_when_global_absent() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let home = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        let prev_home = std::env::var_os("HOME");
        unsafe { std::env::set_var("HOME", home.path()) };

        // No global settings.json is written under HOME.
        let dirs = ProjectDirs::new(repo.path());
        let result = migrate_global_hooks(&dirs);

        match prev_home {
            Some(value) => unsafe { std::env::set_var("HOME", value) },
            None => unsafe { std::env::remove_var("HOME") },
        }

        assert!(
            result.is_none(),
            "must return None when global settings file is absent"
        );
    }
}
