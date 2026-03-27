//! Reflection: post-session analysis via native Claude CLI invocation.
//!
//! After a session ends (and lock/power-steering don't block), runs a
//! headless Claude reflection prompt to generate feedback on the work done.
//! Results are saved to timestamped files for history preservation.

use amplihack_cli::env_builder::active_agent_binary;
use amplihack_types::{ProjectDirs, sanitize_session_id};
use anyhow::{Context, Result};
use serde_json::Value;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

const AMPLIHACK_REPO_URI: &str = "https://github.com/rysweet/MicrosoftHackathon2025-AgenticCoding";

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

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReflectionMessage {
    role: String,
    content: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RedirectRecord {
    redirect_number: Option<u64>,
    timestamp: Option<String>,
    failed_considerations: Vec<String>,
    continuation_prompt: String,
}

/// Check if reflection should run.
pub fn should_run(dirs: &ProjectDirs) -> bool {
    if std::env::var_os("AMPLIHACK_SKIP_REFLECTION").is_some_and(|value| !value.is_empty()) {
        return false;
    }

    let reflection_lock = dirs.runtime.join("reflection").join(".reflection_lock");
    if reflection_lock.exists() {
        return false;
    }

    if std::env::var("AMPLIHACK_ENABLE_REFLECTION")
        .ok()
        .is_some_and(|value| matches!(value.to_lowercase().as_str(), "1" | "true" | "yes"))
    {
        return true;
    }

    let config_path = dirs.tools_amplihack.join(".reflection_config");
    let Ok(config_text) = fs::read_to_string(&config_path) else {
        return false;
    };

    match serde_json::from_str::<Value>(&config_text) {
        Ok(config) => config
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        Err(err) => {
            tracing::warn!(
                path = %config_path.display(),
                "Failed to parse reflection config: {err}"
            );
            false
        }
    }
}

/// Save reflection artifacts to disk.
///
/// Writes FEEDBACK_SUMMARY.md, timestamped reflection file, and current_findings.md.
fn save_reflection_artifacts(dirs: &ProjectDirs, session_id: &str, template: &str) {
    let session_dir = dirs.session_logs(session_id);
    let safe_id = sanitize_session_id(session_id);

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let filename = format!("reflection_{safe_id}_{timestamp}.md");

    if let Err(e) = fs::write(session_dir.join("FEEDBACK_SUMMARY.md"), template) {
        tracing::warn!("Failed to write FEEDBACK_SUMMARY.md: {}", e);
    }

    let reflection_dir = reflection_runtime_dir(dirs);
    if let Err(e) = fs::create_dir_all(&reflection_dir) {
        tracing::warn!("Failed to create reflection dir: {}", e);
    }
    if let Err(e) = fs::write(reflection_dir.join(&filename), template) {
        tracing::warn!("Failed to write {}: {}", filename, e);
    }
    if let Err(e) = fs::write(reflection_dir.join("current_findings.md"), template) {
        tracing::warn!("Failed to write current_findings.md: {}", e);
    }
}

fn reflection_runtime_dir(dirs: &ProjectDirs) -> std::path::PathBuf {
    dirs.runtime.join("reflection")
}

fn reflection_semaphore_path(dirs: &ProjectDirs, session_id: &str) -> std::path::PathBuf {
    reflection_runtime_dir(dirs).join(format!(
        ".reflection_presented_{}",
        sanitize_session_id(session_id)
    ))
}

/// Run session reflection and return findings if session should be blocked.
///
/// Returns `Some(block_json)` if reflection produced findings that should
/// be presented to the user, `None` otherwise.
pub fn run_reflection(
    dirs: &ProjectDirs,
    session_id: &str,
    transcript_path: Option<&Path>,
) -> Result<Option<Value>> {
    let session_dir = dirs.session_logs(session_id);
    fs::create_dir_all(&session_dir)?;
    fs::create_dir_all(reflection_runtime_dir(dirs))?;

    let semaphore_path = reflection_semaphore_path(dirs, session_id);
    if semaphore_path.exists() {
        if let Err(error) = fs::remove_file(&semaphore_path) {
            tracing::warn!(
                path = %semaphore_path.display(),
                "Failed to remove reflection semaphore: {error}"
            );
        }
        return Ok(None);
    }

    let conversation = match transcript_path {
        Some(path) => match load_transcript_conversation(path) {
            Ok(messages) => messages,
            Err(error) => {
                tracing::warn!("Failed to parse reflection transcript: {}", error);
                Vec::new()
            }
        },
        None => Vec::new(),
    };

    let prompt = build_reflection_prompt(dirs, &session_dir, &conversation)?;
    let Some(template) = run_claude_reflection(&dirs.root, &prompt)? else {
        return Ok(None);
    };

    save_reflection_artifacts(dirs, session_id, &template);

    if let Err(e) = fs::write(&semaphore_path, "") {
        tracing::warn!("Failed to write reflection semaphore: {}", e);
    }

    Ok(Some(serde_json::json!({
        "decision": "block",
        "reason": format!(
            "📋 Session Reflection\n\n{}\n\nPlease review the findings above.",
            template
        )
    })))
}

fn build_reflection_prompt(
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

fn load_power_steering_redirects(session_dir: &Path) -> Option<Vec<RedirectRecord>> {
    let path = session_dir.join("redirects.jsonl");
    let raw = fs::read_to_string(path).ok()?;
    let redirects = raw
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                return None;
            }
            let entry: Value = serde_json::from_str(trimmed).ok()?;
            let failed_considerations = entry
                .get("failed_considerations")
                .and_then(Value::as_array)
                .map(|values| {
                    values
                        .iter()
                        .filter_map(|value| value.as_str().map(ToString::to_string))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let continuation_prompt = entry.get("continuation_prompt")?.as_str()?.to_string();
            Some(RedirectRecord {
                redirect_number: entry.get("redirect_number").and_then(Value::as_u64),
                timestamp: entry
                    .get("timestamp")
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
                failed_considerations,
                continuation_prompt,
            })
        })
        .collect::<Vec<_>>();
    (!redirects.is_empty()).then_some(redirects)
}

fn format_redirects_context(redirects: Option<Vec<RedirectRecord>>) -> String {
    let Some(redirects) = redirects else {
        return String::new();
    };

    let redirect_word = if redirects.len() == 1 {
        "redirect"
    } else {
        "redirects"
    };
    let mut parts = vec![
        String::new(),
        "## Power-Steering Redirect History".to_string(),
        String::new(),
        format!(
            "This session had {} power-steering {} where Claude was blocked from stopping due to incomplete work:",
            redirects.len(),
            redirect_word
        ),
        String::new(),
    ];

    for redirect in redirects {
        parts.push(format!(
            "### Redirect #{} ({})",
            redirect
                .redirect_number
                .map(|value| value.to_string())
                .unwrap_or_else(|| "?".to_string()),
            redirect.timestamp.unwrap_or_else(|| "unknown".to_string())
        ));
        parts.push(String::new());
        parts.push(format!(
            "**Failed Checks:** {}",
            redirect.failed_considerations.join(", ")
        ));
        parts.push(String::new());
        parts.push("**Continuation Prompt Given:**".to_string());
        parts.push("```".to_string());
        parts.push(redirect.continuation_prompt);
        parts.push("```".to_string());
        parts.push(String::new());
    }

    parts.push("**Analysis Note:** These redirects indicate areas where work was incomplete. In your feedback, consider whether the redirects were justified and whether Claude successfully addressed the blockers after being redirected.".to_string());
    parts.push(String::new());
    parts.join("\n")
}

fn format_conversation_summary(conversation: &[ReflectionMessage], max_length: usize) -> String {
    let mut summary_parts = Vec::new();
    let mut current_length = 0usize;

    for (index, message) in conversation.iter().enumerate() {
        let mut content = message.content.clone();
        if content.len() > 500 {
            content.truncate(497);
            content.push_str("...");
        }

        let snippet = format!(
            "\n**Message {} ({}):** {}\n",
            index + 1,
            message.role,
            content
        );

        if current_length + snippet.len() > max_length {
            summary_parts.push(format!(
                "\n[... {} more messages ...]",
                conversation.len().saturating_sub(index)
            ));
            break;
        }

        current_length += snippet.len();
        summary_parts.push(snippet);
    }

    summary_parts.join("")
}

fn load_transcript_conversation(path: &Path) -> Result<Vec<ReflectionMessage>> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read transcript {}", path.display()))?;
    let mut conversation = Vec::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let entry: Value = serde_json::from_str(trimmed)
            .with_context(|| format!("invalid transcript JSON in {}", path.display()))?;
        if let Some(message) = parse_reflection_message(&entry) {
            conversation.push(message);
        }
    }
    Ok(conversation)
}

fn parse_reflection_message(entry: &Value) -> Option<ReflectionMessage> {
    if let Some(role) = entry.get("role").and_then(Value::as_str) {
        return Some(ReflectionMessage {
            role: role.to_string(),
            content: extract_text_content(entry.get("content")?)?,
        });
    }

    let entry_type = entry.get("type").and_then(Value::as_str)?;
    if !matches!(entry_type, "user" | "assistant") {
        return None;
    }

    let message = entry.get("message")?;
    let role = message
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or(entry_type)
        .to_string();
    Some(ReflectionMessage {
        role,
        content: extract_text_content(message.get("content")?)?,
    })
}

fn extract_text_content(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => {
            let trimmed = text.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        }
        Value::Array(blocks) => {
            let text = blocks
                .iter()
                .filter_map(|block| {
                    if block.get("type").and_then(Value::as_str) == Some("text") {
                        block.get("text").and_then(Value::as_str)
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            let trimmed = text.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        }
        _ => None,
    }
}

fn run_claude_reflection(project_root: &Path, prompt: &str) -> Result<Option<String>> {
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
    fn not_enabled_by_default() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = ProjectDirs::new(dir.path());
        assert!(!should_run(&dirs));
    }

    #[test]
    fn enabled_config_allows_reflection() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let dirs = ProjectDirs::new(dir.path());
        fs::create_dir_all(&dirs.tools_amplihack).unwrap();
        fs::write(
            dirs.tools_amplihack.join(".reflection_config"),
            r#"{"enabled": true}"#,
        )
        .unwrap();
        let previous_enable = std::env::var_os("AMPLIHACK_ENABLE_REFLECTION");
        let previous_skip = std::env::var_os("AMPLIHACK_SKIP_REFLECTION");
        unsafe {
            std::env::remove_var("AMPLIHACK_ENABLE_REFLECTION");
            std::env::remove_var("AMPLIHACK_SKIP_REFLECTION");
        }

        let should_run_reflection = should_run(&dirs);

        match previous_enable {
            Some(value) => unsafe { std::env::set_var("AMPLIHACK_ENABLE_REFLECTION", value) },
            None => unsafe { std::env::remove_var("AMPLIHACK_ENABLE_REFLECTION") },
        }
        match previous_skip {
            Some(value) => unsafe { std::env::set_var("AMPLIHACK_SKIP_REFLECTION", value) },
            None => unsafe { std::env::remove_var("AMPLIHACK_SKIP_REFLECTION") },
        }

        assert!(should_run_reflection);
    }

    #[test]
    fn skip_flag_blocks_reflection_even_when_enabled() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let dirs = ProjectDirs::new(dir.path());
        fs::create_dir_all(&dirs.tools_amplihack).unwrap();
        fs::write(
            dirs.tools_amplihack.join(".reflection_config"),
            r#"{"enabled": true}"#,
        )
        .unwrap();
        let previous_enable = std::env::var_os("AMPLIHACK_ENABLE_REFLECTION");
        let previous_skip = std::env::var_os("AMPLIHACK_SKIP_REFLECTION");
        unsafe {
            std::env::set_var("AMPLIHACK_ENABLE_REFLECTION", "1");
            std::env::set_var("AMPLIHACK_SKIP_REFLECTION", "1");
        }

        let should_run_reflection = should_run(&dirs);

        match previous_enable {
            Some(value) => unsafe { std::env::set_var("AMPLIHACK_ENABLE_REFLECTION", value) },
            None => unsafe { std::env::remove_var("AMPLIHACK_ENABLE_REFLECTION") },
        }
        match previous_skip {
            Some(value) => unsafe { std::env::set_var("AMPLIHACK_SKIP_REFLECTION", value) },
            None => unsafe { std::env::remove_var("AMPLIHACK_SKIP_REFLECTION") },
        }

        assert!(!should_run_reflection);
    }

    #[test]
    fn reflection_lock_blocks_execution() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let dirs = ProjectDirs::new(dir.path());
        fs::create_dir_all(&dirs.tools_amplihack).unwrap();
        fs::create_dir_all(dirs.runtime.join("reflection")).unwrap();
        fs::write(
            dirs.tools_amplihack.join(".reflection_config"),
            r#"{"enabled": true}"#,
        )
        .unwrap();
        fs::write(dirs.runtime.join("reflection/.reflection_lock"), "").unwrap();
        let previous_enable = std::env::var_os("AMPLIHACK_ENABLE_REFLECTION");
        let previous_skip = std::env::var_os("AMPLIHACK_SKIP_REFLECTION");
        unsafe {
            std::env::remove_var("AMPLIHACK_ENABLE_REFLECTION");
            std::env::remove_var("AMPLIHACK_SKIP_REFLECTION");
        }

        let should_run_reflection = should_run(&dirs);

        match previous_enable {
            Some(value) => unsafe { std::env::set_var("AMPLIHACK_ENABLE_REFLECTION", value) },
            None => unsafe { std::env::remove_var("AMPLIHACK_ENABLE_REFLECTION") },
        }
        match previous_skip {
            Some(value) => unsafe { std::env::set_var("AMPLIHACK_SKIP_REFLECTION", value) },
            None => unsafe { std::env::remove_var("AMPLIHACK_SKIP_REFLECTION") },
        }

        assert!(!should_run_reflection);
    }

    #[test]
    fn semaphore_prevents_re_presentation() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = ProjectDirs::new(dir.path());
        let session_id = "test-session";
        let session_dir = dirs.session_logs(session_id);
        fs::create_dir_all(&session_dir).unwrap();
        fs::create_dir_all(reflection_runtime_dir(&dirs)).unwrap();
        fs::write(reflection_semaphore_path(&dirs, session_id), "").unwrap();

        let result = run_reflection(&dirs, session_id, None).unwrap();
        assert!(result.is_none());
        assert!(!reflection_semaphore_path(&dirs, session_id).exists());
    }

    #[test]
    fn load_transcript_conversation_parses_text_blocks() {
        let dir = tempfile::tempdir().unwrap();
        let transcript = dir.path().join("transcript.jsonl");
        fs::write(
            &transcript,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"Investigate auth"}]}}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Done"}]}}
"#,
        )
        .unwrap();

        let conversation = load_transcript_conversation(&transcript).unwrap();

        assert_eq!(conversation.len(), 2);
        assert_eq!(conversation[0].content, "Investigate auth");
        assert_eq!(conversation[1].role, "assistant");
    }

    #[test]
    fn run_reflection_invokes_cli_and_writes_artifacts() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let dirs = ProjectDirs::new(dir.path());
        fs::create_dir_all(dirs.tools_amplihack.join("hooks/templates")).unwrap();
        fs::create_dir_all(dirs.claude.join("templates")).unwrap();
        fs::write(
            dirs.tools_amplihack
                .join("hooks/templates/reflection_prompt.txt"),
            "Messages: {message_count}\n{conversation_summary}\n{template}\n",
        )
        .unwrap();
        fs::write(
            dirs.claude.join("templates/FEEDBACK_SUMMARY.md"),
            "## Task Summary\nplaceholder\n",
        )
        .unwrap();

        let transcript = dir.path().join("session.jsonl");
        fs::write(
            &transcript,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"Ship the fix"}]}}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Implemented and tested the change."}]}}
"#,
        )
        .unwrap();

        let fake_cli = dir.path().join("fake-claude.sh");
        fs::write(
            &fake_cli,
            "#!/usr/bin/env bash\ncat >/dev/null\nprintf '## Task Summary\\nReflected session\\n'\n",
        )
        .unwrap();
        let mut perms = fs::metadata(&fake_cli).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&fake_cli, perms).unwrap();

        let previous = std::env::var_os("AMPLIHACK_REFLECTION_BINARY");
        unsafe { std::env::set_var("AMPLIHACK_REFLECTION_BINARY", &fake_cli) };

        let result = run_reflection(&dirs, "test-session", Some(&transcript)).unwrap();

        match previous {
            Some(value) => unsafe { std::env::set_var("AMPLIHACK_REFLECTION_BINARY", value) },
            None => unsafe { std::env::remove_var("AMPLIHACK_REFLECTION_BINARY") },
        }

        let session_dir = dirs.session_logs("test-session");
        assert_eq!(result.as_ref().unwrap()["decision"], "block");
        assert!(
            result.as_ref().unwrap()["reason"]
                .as_str()
                .unwrap()
                .contains("Reflected session")
        );
        assert!(session_dir.join("FEEDBACK_SUMMARY.md").exists());
        assert!(dirs.runtime.join("reflection/current_findings.md").exists());
        assert!(reflection_semaphore_path(&dirs, "test-session").exists());
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
}
