//! Copilot home staging to match Python launcher behavior.

pub(crate) mod fs_helpers;
mod hooks;
mod jsonc;
mod staging;

use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;

#[cfg(test)]
use hooks::{build_wrapper_script, error_wrapper_script, replace_or_append_section};
use hooks::{generate_copilot_instructions, stage_repo_hooks, write_user_level_hooks};
use staging::{register_plugin, stage_agents, stage_command_docs, stage_directory, stage_skills};

const INSTRUCTIONS_MARKER_START: &str = "<!-- AMPLIHACK_INSTRUCTIONS_START -->";
const INSTRUCTIONS_MARKER_END: &str = "<!-- AMPLIHACK_INSTRUCTIONS_END -->";

/// Default hook timeout in seconds. Hooks that exceed this are killed by Copilot.
const COPILOT_HOOK_TIMEOUT_SEC: u32 = 30;

struct HookWrapperSpec {
    /// File name of the bash wrapper script under `.github/hooks/`.
    hook_name: &'static str,
    /// Copilot CLI camelCase event name (matches schema in copilot app.js
    /// `fWr` set: sessionStart, sessionEnd, userPromptSubmitted, preToolUse,
    /// postToolUse, postToolUseFailure, errorOccurred, agentStop,
    /// subagentStop, subagentStart, preCompact, permissionRequest,
    /// notification).
    copilot_event: &'static str,
    subcommands: &'static [&'static str],
}

const COPILOT_HOOK_WRAPPERS: &[HookWrapperSpec] = &[
    HookWrapperSpec {
        hook_name: "session-start",
        copilot_event: "sessionStart",
        subcommands: &["session-start"],
    },
    HookWrapperSpec {
        hook_name: "user-prompt-submit",
        copilot_event: "userPromptSubmitted",
        subcommands: &["workflow-classification-reminder", "user-prompt-submit"],
    },
    HookWrapperSpec {
        hook_name: "pre-tool-use",
        copilot_event: "preToolUse",
        subcommands: &["pre-tool-use"],
    },
    HookWrapperSpec {
        hook_name: "post-tool-use",
        copilot_event: "postToolUse",
        subcommands: &["post-tool-use"],
    },
    HookWrapperSpec {
        hook_name: "pre-compact",
        copilot_event: "preCompact",
        subcommands: &["pre-compact"],
    },
    HookWrapperSpec {
        hook_name: "stop",
        copilot_event: "agentStop",
        subcommands: &["stop"],
    },
];

pub fn ensure_copilot_home_staged() -> Result<()> {
    let staged = staged_framework_dir()?;
    let copilot_home = copilot_home()?;
    fs::create_dir_all(&copilot_home)?;

    let agents_dir = staged.join("agents");
    if agents_dir.is_dir() {
        stage_agents(&agents_dir, &copilot_home)?;
    }

    let skills_dir = staged.join("skills");
    if skills_dir.is_dir() {
        stage_skills(&skills_dir, &copilot_home)?;
    }

    let commands_dir = staged.join("commands").join("amplihack");
    if commands_dir.is_dir() {
        stage_command_docs(&commands_dir, &copilot_home)?;
        register_plugin(&commands_dir, &copilot_home)?;
    }

    for dir_name in &["workflow", "context"] {
        let source = staged.join(dir_name);
        if source.is_dir() {
            stage_directory(&source, &copilot_home, dir_name)?;
        }
    }

    generate_copilot_instructions(&copilot_home)?;

    if let Ok(cwd) = std::env::current_dir() {
        match stage_repo_hooks(&cwd) {
            Ok(count) => {
                tracing::debug!("staged {count} copilot hook file(s) into {}", cwd.display());
            }
            Err(err) => {
                eprintln!(
                    "⚠️  Failed to stage Copilot hooks into {}: {err}",
                    cwd.display()
                );
            }
        }
    }

    // Also stage hooks at the user level so they fire regardless of the
    // current working directory at Copilot launch time.
    if let Err(err) = write_user_level_hooks(&copilot_home) {
        eprintln!("⚠️  Failed to write user-level Copilot hooks: {err}");
    }

    Ok(())
}

fn staged_framework_dir() -> Result<PathBuf> {
    let home = home_dir()?;
    Ok(home.join(".amplihack").join(".claude"))
}

fn copilot_home() -> Result<PathBuf> {
    let home = home_dir()?;
    Ok(home.join(".copilot"))
}

fn home_dir() -> Result<PathBuf> {
    std::env::var("HOME")
        .map(PathBuf::from)
        .context("HOME is not set")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_copilot_home_stages_assets_and_plugin() {
        let _home_guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let previous_home = crate::test_support::set_home(temp.path());
        let repo_root = temp.path().join("repo");
        fs::create_dir_all(&repo_root).unwrap();
        let previous_cwd = crate::test_support::set_cwd(&repo_root).unwrap();

        let staged = temp.path().join(".amplihack/.claude");
        fs::create_dir_all(staged.join("agents/amplihack/core")).unwrap();
        fs::create_dir_all(staged.join("skills/dev-orchestrator")).unwrap();
        fs::create_dir_all(staged.join("skills/quality-audit")).unwrap();
        fs::create_dir_all(staged.join("workflow")).unwrap();
        fs::create_dir_all(staged.join("context")).unwrap();
        fs::create_dir_all(staged.join("commands/amplihack")).unwrap();
        fs::write(staged.join("agents/amplihack/core/architect.md"), "agent").unwrap();
        fs::write(staged.join("skills/dev-orchestrator/SKILL.md"), "skill-a").unwrap();
        fs::write(staged.join("skills/quality-audit/SKILL.md"), "skill-b").unwrap();
        fs::write(staged.join("workflow/DEFAULT_WORKFLOW.md"), "workflow").unwrap();
        fs::write(staged.join("context/USER_PREFERENCES.md"), "prefs").unwrap();
        fs::write(staged.join("commands/amplihack/dev.md"), "command").unwrap();
        fs::write(
            staged.join("commands/amplihack/plugin.json"),
            "{\"name\":\"amplihack\"}",
        )
        .unwrap();

        ensure_copilot_home_staged().unwrap();

        assert!(
            temp.path()
                .join(".copilot/agents/amplihack/architect.md")
                .exists()
        );
        assert!(
            temp.path()
                .join(".copilot/skills/dev-orchestrator/SKILL.md")
                .exists()
        );
        assert!(
            temp.path()
                .join(".copilot/skills/quality-audit/SKILL.md")
                .exists()
        );
        assert!(
            temp.path()
                .join(".copilot/workflow/amplihack/DEFAULT_WORKFLOW.md")
                .exists()
        );
        assert!(
            temp.path()
                .join(".copilot/commands/amplihack/dev.md")
                .exists()
        );
        assert!(
            temp.path()
                .join(".copilot/installed-plugins/amplihack@local/commands/dev.md")
                .exists()
        );
        assert!(repo_root.join(".github/hooks/session-start").exists());
        assert!(repo_root.join(".github/hooks/pre-tool-use").exists());
        assert!(
            repo_root
                .join(".github/hooks/amplihack-hooks.json")
                .exists()
        );

        // Verify the staged manifest uses Copilot's camelCase event names
        // and the documented entry schema (so Copilot doesn't ignore them).
        let manifest_raw =
            fs::read_to_string(repo_root.join(".github/hooks/amplihack-hooks.json")).unwrap();
        let manifest: serde_json::Value = serde_json::from_str(&manifest_raw).unwrap();
        assert_eq!(manifest["version"], 1);
        let hooks_obj = manifest["hooks"].as_object().expect("hooks is object");
        for event in [
            "sessionStart",
            "userPromptSubmitted",
            "preToolUse",
            "postToolUse",
            "preCompact",
            "agentStop",
        ] {
            let arr = hooks_obj
                .get(event)
                .unwrap_or_else(|| panic!("missing hook event {event}"))
                .as_array()
                .expect("event is array");
            assert!(!arr.is_empty(), "event {event} has no entries");
            let entry = &arr[0];
            assert_eq!(entry["type"], "command", "{event} entry type");
            assert!(
                entry["bash"]
                    .as_str()
                    .unwrap_or("")
                    .contains(event_basename(event)),
                "{event} bash path mismatch: {entry}"
            );
            assert!(
                entry["timeoutSec"].as_u64().is_some(),
                "{event} missing timeoutSec"
            );
        }
        // None of our event names should be the legacy kebab-case form.
        for legacy in [
            "session-start",
            "user-prompt-submit",
            "post-tool-use",
            "stop",
        ] {
            assert!(
                !hooks_obj.contains_key(legacy),
                "legacy event name leaked: {legacy}"
            );
        }

        // User-level hooks should be wired into ~/.copilot/config.json so they
        // fire regardless of cwd.
        let copilot_config_raw =
            fs::read_to_string(temp.path().join(".copilot/config.json")).unwrap();
        let copilot_config: serde_json::Value = serde_json::from_str(&copilot_config_raw).unwrap();
        let user_hooks = copilot_config["hooks"]
            .as_object()
            .expect("user-level hooks present");
        assert!(user_hooks.contains_key("sessionStart"));
        assert!(user_hooks.contains_key("preToolUse"));
        assert!(
            temp.path()
                .join(".copilot/.github/hooks/session-start")
                .exists()
        );
        assert!(copilot_config_raw.contains("\"name\": \"amplihack\""));

        crate::test_support::restore_cwd(&previous_cwd).unwrap();
        crate::test_support::restore_home(previous_home);
    }

    fn event_basename(event: &str) -> &'static str {
        match event {
            "sessionStart" => "session-start",
            "userPromptSubmitted" => "user-prompt-submit",
            "preToolUse" => "pre-tool-use",
            "postToolUse" => "post-tool-use",
            "preCompact" => "pre-compact",
            "agentStop" => "stop",
            _ => "",
        }
    }

    #[test]
    fn replace_or_append_section_updates_existing_block() {
        let existing =
            format!("before\n{INSTRUCTIONS_MARKER_START}\nold\n{INSTRUCTIONS_MARKER_END}\nafter\n");
        let updated = replace_or_append_section(&existing, "NEW");
        assert!(updated.contains("before"));
        assert!(updated.contains("after"));
        assert!(updated.contains("NEW"));
        assert!(!updated.contains("old"));
    }

    #[test]
    fn build_wrapper_script_uses_binary_subcommand_for_single_hook() {
        let script = build_wrapper_script(&HookWrapperSpec {
            hook_name: "session-start",
            copilot_event: "sessionStart",
            subcommands: &["session-start"],
        });

        assert!(script.contains("amplihack-hooks"));
        assert!(script.contains("exec \"$HOOKS_BIN\" session-start \"$@\""));
        assert!(!script.contains("python3"));
    }

    #[test]
    fn build_wrapper_script_uses_multiple_binary_subcommands() {
        let script = build_wrapper_script(&HookWrapperSpec {
            hook_name: "user-prompt-submit",
            copilot_event: "userPromptSubmitted",
            subcommands: &["workflow-classification-reminder", "user-prompt-submit"],
        });

        assert!(script.contains("\"$HOOKS_BIN\" workflow-classification-reminder"));
        assert!(script.contains("\"$HOOKS_BIN\" user-prompt-submit"));
        assert!(!script.contains("python3"));
    }

    #[test]
    fn ensure_copilot_home_preserves_leading_jsonc_comments_in_config() {
        // Regression: GitHub Copilot CLI writes ~/.copilot/config.json as
        // JSONC with a two-line `//` header. amplihack must (a) parse it
        // without choking on the comments and (b) preserve the comment block
        // when it writes the file back after registering the plugin and
        // wiring user-level hooks.
        let _home_guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let previous_home = crate::test_support::set_home(temp.path());
        let repo_root = temp.path().join("repo");
        fs::create_dir_all(&repo_root).unwrap();
        let previous_cwd = crate::test_support::set_cwd(&repo_root).unwrap();

        // Seed a JSONC config.json mirroring what real Copilot CLI writes.
        let copilot_dir = temp.path().join(".copilot");
        fs::create_dir_all(&copilot_dir).unwrap();
        let seeded = "// User settings belong in settings.json.\n\
                      // This file is managed automatically.\n\
                      {}\n";
        fs::write(copilot_dir.join("config.json"), seeded).unwrap();

        // Minimal staged framework so ensure_copilot_home_staged() succeeds.
        let staged = temp.path().join(".amplihack/.claude");
        fs::create_dir_all(staged.join("commands/amplihack")).unwrap();
        fs::write(staged.join("commands/amplihack/dev.md"), "command").unwrap();
        fs::write(
            staged.join("commands/amplihack/plugin.json"),
            "{\"name\":\"amplihack\"}",
        )
        .unwrap();

        ensure_copilot_home_staged().unwrap();

        let after = fs::read_to_string(copilot_dir.join("config.json")).unwrap();
        assert!(
            after.starts_with("// User settings belong in settings.json.\n// This file is managed automatically.\n"),
            "leading JSONC comment block was not preserved; got:\n{after}"
        );

        // Strip comments before parsing to verify the JSON body is valid and
        // the plugin/hook entries were merged in.
        let body = jsonc::strip_jsonc_comments(&after);
        let config: serde_json::Value = serde_json::from_str(&body).expect("body parses as JSON");
        let plugins = config["plugins"].as_array().expect("plugins array");
        assert!(
            plugins
                .iter()
                .any(|p| p.get("name").and_then(|n| n.as_str()) == Some("amplihack")),
            "amplihack plugin entry missing: {config}"
        );
        assert!(
            config["hooks"].is_object(),
            "user-level hooks were not merged: {config}"
        );

        crate::test_support::restore_cwd(&previous_cwd).unwrap();
        crate::test_support::restore_home(previous_home);
    }

    #[test]
    fn error_wrapper_script_is_python_free() {
        let script = error_wrapper_script();
        assert!(!script.contains("python3"));
        assert!(script.contains("sed -n"));
        assert!(script.contains("errors.log"));
    }
}
