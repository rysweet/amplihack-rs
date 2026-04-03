//! Copilot home staging to match Python launcher behavior.

pub(crate) mod fs_helpers;
mod hooks;
mod staging;

use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;

use hooks::{generate_copilot_instructions, stage_repo_hooks};
#[cfg(test)]
use hooks::{build_wrapper_script, error_wrapper_script, replace_or_append_section};
use staging::{register_plugin, stage_agents, stage_command_docs, stage_directory, stage_skills};

const INSTRUCTIONS_MARKER_START: &str = "<!-- AMPLIHACK_INSTRUCTIONS_START -->";
const INSTRUCTIONS_MARKER_END: &str = "<!-- AMPLIHACK_INSTRUCTIONS_END -->";
const COPILOT_HOOKS_MANIFEST: &str = r#"{
  "hooks": {
    "session-start": [".github/hooks/session-start"],
    "user-prompt-submit": [".github/hooks/user-prompt-submit"],
    "post-tool-use": [".github/hooks/post-tool-use"],
    "pre-compact": [".github/hooks/pre-compact"],
    "stop": [".github/hooks/stop"]
  }
}
"#;

struct HookWrapperSpec {
    hook_name: &'static str,
    subcommands: &'static [&'static str],
}

const COPILOT_HOOK_WRAPPERS: &[HookWrapperSpec] = &[
    HookWrapperSpec {
        hook_name: "session-start",
        subcommands: &["session-start"],
    },
    HookWrapperSpec {
        hook_name: "user-prompt-submit",
        subcommands: &["workflow-classification-reminder", "user-prompt-submit"],
    },
    HookWrapperSpec {
        hook_name: "post-tool-use",
        subcommands: &["post-tool-use"],
    },
    HookWrapperSpec {
        hook_name: "pre-compact",
        subcommands: &["pre-compact"],
    },
    HookWrapperSpec {
        hook_name: "stop",
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
        let _ = stage_repo_hooks(&cwd);
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
        assert!(
            repo_root
                .join(".github/hooks/amplihack-hooks.json")
                .exists()
        );

        let config = fs::read_to_string(temp.path().join(".copilot/config.json")).unwrap();
        assert!(config.contains("\"name\": \"amplihack\""));

        crate::test_support::restore_cwd(&previous_cwd).unwrap();
        crate::test_support::restore_home(previous_home);
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
            subcommands: &["workflow-classification-reminder", "user-prompt-submit"],
        });

        assert!(script.contains("\"$HOOKS_BIN\" workflow-classification-reminder"));
        assert!(script.contains("\"$HOOKS_BIN\" user-prompt-submit"));
        assert!(!script.contains("python3"));
    }

    #[test]
    fn error_wrapper_script_is_python_free() {
        let script = error_wrapper_script();
        assert!(!script.contains("python3"));
        assert!(script.contains("sed -n"));
        assert!(script.contains("errors.log"));
    }
}
