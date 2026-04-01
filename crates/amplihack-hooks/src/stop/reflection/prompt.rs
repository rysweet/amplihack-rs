//! Prompt construction, context assembly, and CLI execution for reflection.

use super::conversation::{
    format_conversation_summary, format_redirects_context, load_power_steering_redirects,
    ReflectionMessage,
};
use amplihack_cli::env_builder::active_agent_binary;
use amplihack_types::ProjectDirs;
use anyhow::{Context, Result};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

const AMPLIHACK_REPO_URI: &str =
    "https://github.com/rysweet/MicrosoftHackathon2025-AgenticCoding";

const DEFAULT_FEEDBACK_TEMPLATE: &str = "## Task Summary
[What was accomplished]

## Feedback Summary
**User Interactions:** [Observations]
**Workflow Adherence:** [Did workflow get followed?]
**Subagent Usage:** [Which agents used?]
**Learning Opportunities:** [What to improve]
";

const DEFAULT_REFLECTION_PROMPT_TEMPLATE: &str = r#"# Reflection Prompt Template
#
# This prompt is used by claude_reflection.py to analyze Claude Code sessions.
#
# Available variables for substitution:
#   {{user_preferences_context}}  - User preferences and mandatory behavior
#   {{repository_context}}        - Current repository detection results
#   {{amplihack_repo_uri}}        - Amplihack framework repository URL
#   {{message_count}}             - Number of messages in session
#   {{conversation_summary}}      - Formatted conversation excerpt
#   {{redirects_context}}         - Power-steering redirect history (if any)
#   {{template}}                  - FEEDBACK_SUMMARY template to fill out
#
# Use {{VARIABLE_NAME}} syntax for substitution.
#

You are analyzing a completed Claude Code session to provide feedback and identify learning opportunities.

{user_preferences_context}
{repository_context}

## Critical: Distinguish Problem Sources

When analyzing this session, you MUST clearly distinguish between TWO categories of issues:

### 1. Amplihack Framework Issues
Problems with the coding tools, agents, workflow, or process itself:
- Agent behavior, effectiveness, or orchestration
- Workflow step execution or adherence
- Tool functionality (hooks, commands, utilities, reflection system)
- Framework architecture or design decisions
- UltraThink coordination and delegation
- Command execution (/amplihack:* commands)
- Session management and logging

**These issues should be filed against**: {amplihack_repo_uri}

### 2. Project Code Issues
Problems with the actual application code being developed:
- Application logic bugs or errors
- Feature implementation quality
- Test failures in project-specific tests
- Project-specific design decisions
- User-facing functionality
- Business logic correctness

**These issues should be filed against**: The current project repository (see Repository Context above)

**IMPORTANT**: In your feedback, clearly label each issue as either "[AMPLIHACK]" or "[PROJECT]" so it's obvious which repository should handle it.

## Session Conversation

The session had {message_count} messages. Here are key excerpts:

{conversation_summary}

{redirects_context}

## Your Task

Please analyze this session and fill out the following feedback template:

{template}

## Guidelines

1. **Be specific and actionable** - Reference actual events from the session
2. **Identify patterns** - What worked well? What could improve?
3. **Track workflow adherence** - Did Claude follow the DEFAULT_WORKFLOW.md steps?
4. **Note subagent usage** - Which specialized agents were used (architect, builder, reviewer, etc.)?
5. **Categorize improvements** - Clearly mark each issue as [AMPLIHACK] or [PROJECT]
6. **Suggest improvements** - What would make future similar sessions better?

Please provide the filled-out template now.
"#;

pub(crate) fn build_reflection_prompt(
    dirs: &ProjectDirs,
    session_dir: &Path,
    conversation: &[ReflectionMessage],
) -> Result<String> {
    let mut prompt = load_prompt_template(dirs);
    for (key, value) in [
        (
            "{user_preferences_context}",
            load_user_preferences_context(dirs).unwrap_or_default(),
        ),
        ("{repository_context}", get_repository_context(&dirs.root)),
        ("{amplihack_repo_uri}", AMPLIHACK_REPO_URI.to_string()),
        ("{message_count}", conversation.len().to_string()),
        (
            "{conversation_summary}",
            format_conversation_summary(conversation, 5_000),
        ),
        (
            "{redirects_context}",
            format_redirects_context(load_power_steering_redirects(session_dir)),
        ),
        ("{template}", load_feedback_template(dirs)),
    ] {
        prompt = prompt.replace(key, &value);
    }
    Ok(prompt)
}

fn load_prompt_template(dirs: &ProjectDirs) -> String {
    let Some(path) = dirs
        .resolve_framework_file(".claude/tools/amplihack/hooks/templates/reflection_prompt.txt")
    else {
        return DEFAULT_REFLECTION_PROMPT_TEMPLATE.to_string();
    };
    fs::read_to_string(&path).unwrap_or_else(|_| DEFAULT_REFLECTION_PROMPT_TEMPLATE.to_string())
}

fn load_feedback_template(dirs: &ProjectDirs) -> String {
    let Some(path) = dirs.resolve_framework_file(".claude/templates/FEEDBACK_SUMMARY.md") else {
        return DEFAULT_FEEDBACK_TEMPLATE.to_string();
    };
    fs::read_to_string(&path).unwrap_or_else(|_| DEFAULT_FEEDBACK_TEMPLATE.to_string())
}

fn load_user_preferences_context(dirs: &ProjectDirs) -> Option<String> {
    let mut candidates = Vec::new();
    if let Some(path) = dirs.resolve_preferences_file() {
        candidates.push(path);
    }
    candidates.push(dirs.root.join("USER_PREFERENCES.md"));
    for path in candidates {
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        return Some(format!(
            "## User Preferences (MANDATORY - MUST FOLLOW)\n\nThe following preferences are REQUIRED and CANNOT be ignored:\n\n{}\n\n**IMPORTANT**: When analyzing this session, consider whether Claude followed these user preferences. Do NOT criticize behavior that aligns with configured preferences.",
            content
        ));
    }
    None
}

fn get_repository_context(project_root: &Path) -> String {
    let result = Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(project_root)
        .output();

    match result {
        Ok(output) if output.status.success() => {
            let current_repo = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let current_normalized = normalize_url(&current_repo);
            let amplihack_normalized = normalize_url(AMPLIHACK_REPO_URI);

            if current_normalized == amplihack_normalized {
                format!(
                    "\n## Repository Context\n\n**Current Repository**: {}\n**Context**: Working on Amplihack itself\n\n**IMPORTANT**: Since we're working on the Amplihack framework itself, ALL issues identified in this session are Amplihack framework issues and should be filed against the Amplihack repository.\n",
                    current_repo
                )
            } else {
                format!(
                    "\n## Repository Context\n\n**Current Repository**: {}\n**Amplihack Repository**: {}\n**Context**: Working on a user project (not Amplihack itself)\n",
                    current_repo, AMPLIHACK_REPO_URI
                )
            }
        }
        _ => format!(
            "\n## Repository Context\n\n**Amplihack Repository**: {}\n**Context**: Repository detection unavailable\n",
            AMPLIHACK_REPO_URI
        ),
    }
}

fn normalize_url(url: &str) -> String {
    let normalized = url.trim_end_matches('/').replace(".git", "");
    normalized
        .replace("git@github.com:", "https://github.com/")
        .to_lowercase()
}

pub(crate) fn run_claude_reflection(
    project_root: &Path,
    prompt: &str,
) -> Result<Option<String>> {
    let binary = std::env::var("AMPLIHACK_REFLECTION_BINARY")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(active_agent_binary);

    let mut child = Command::new(&binary)
        .args([
            "-p",
            "--permission-mode",
            "bypassPermissions",
            "--tools",
            "",
            "--no-session-persistence",
            "--setting-sources",
            "user",
        ])
        .env_remove("CLAUDECODE")
        .current_dir(project_root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to launch reflection binary '{}'", binary))?;

    if let Some(stdin) = child.stdin.as_mut() {
        stdin
            .write_all(prompt.as_bytes())
            .context("failed to write reflection prompt to stdin")?;
    }

    let output = child
        .wait_with_output()
        .context("failed waiting for reflection command")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        anyhow::bail!(
            "reflection command failed with status {}: {}",
            output
                .status
                .code()
                .map(|code| code.to_string())
                .unwrap_or_else(|| "signal".to_string()),
            stderr
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok((!stdout.is_empty()).then_some(stdout))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::env_lock;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn reflection_templates_use_amplihack_root_override() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let project = tempfile::tempdir().unwrap();
        let framework = tempfile::tempdir().unwrap();
        let dirs = ProjectDirs::new(project.path());
        fs::create_dir_all(
            framework
                .path()
                .join(".claude/tools/amplihack/hooks/templates"),
        )
        .unwrap();
        fs::create_dir_all(framework.path().join(".claude/templates")).unwrap();
        fs::write(
            framework
                .path()
                .join(".claude/tools/amplihack/hooks/templates/reflection_prompt.txt"),
            "Prompt from framework root",
        )
        .unwrap();
        fs::write(
            framework
                .path()
                .join(".claude/templates/FEEDBACK_SUMMARY.md"),
            "Feedback from framework root",
        )
        .unwrap();
        let previous = std::env::var_os("AMPLIHACK_ROOT");
        unsafe { std::env::set_var("AMPLIHACK_ROOT", framework.path()) };

        let prompt = load_prompt_template(&dirs);
        let feedback = load_feedback_template(&dirs);

        match previous {
            Some(value) => unsafe { std::env::set_var("AMPLIHACK_ROOT", value) },
            None => unsafe { std::env::remove_var("AMPLIHACK_ROOT") },
        }

        assert_eq!(prompt, "Prompt from framework root");
        assert_eq!(feedback, "Feedback from framework root");
    }

    #[test]
    fn run_claude_reflection_uses_active_agent_binary_when_override_unset() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let fake_cli = dir.path().join("copilot");
        fs::write(
            &fake_cli,
            "#!/usr/bin/env bash\ncat >/dev/null\nprintf '## Task Summary\\nReflected via agent binary\\n'\n",
        )
        .unwrap();
        let mut perms = fs::metadata(&fake_cli).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&fake_cli, perms).unwrap();

        let previous_path = std::env::var_os("PATH");
        let previous_agent_binary = std::env::var_os("AMPLIHACK_AGENT_BINARY");
        let previous_reflection_binary = std::env::var_os("AMPLIHACK_REFLECTION_BINARY");
        let temp_path = match previous_path.as_ref() {
            Some(path) => format!("{}:{}", dir.path().display(), path.to_string_lossy()),
            None => dir.path().display().to_string(),
        };
        unsafe {
            std::env::set_var("PATH", temp_path);
            std::env::set_var("AMPLIHACK_AGENT_BINARY", "copilot");
            std::env::remove_var("AMPLIHACK_REFLECTION_BINARY");
        }

        let result = run_claude_reflection(dir.path(), "reflect now").unwrap();

        match previous_path {
            Some(value) => unsafe { std::env::set_var("PATH", value) },
            None => unsafe { std::env::remove_var("PATH") },
        }
        match previous_agent_binary {
            Some(value) => unsafe { std::env::set_var("AMPLIHACK_AGENT_BINARY", value) },
            None => unsafe { std::env::remove_var("AMPLIHACK_AGENT_BINARY") },
        }
        match previous_reflection_binary {
            Some(value) => unsafe { std::env::set_var("AMPLIHACK_REFLECTION_BINARY", value) },
            None => unsafe { std::env::remove_var("AMPLIHACK_REFLECTION_BINARY") },
        }

        assert_eq!(
            result.as_deref(),
            Some("## Task Summary\nReflected via agent binary")
        );
    }
}
