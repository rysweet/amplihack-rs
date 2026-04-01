//! Reflection: post-session analysis via native Claude CLI invocation.
//!
//! After a session ends (and lock/power-steering don't block), runs a
//! headless Claude reflection prompt to generate feedback on the work done.
//! Results are saved to timestamped files for history preservation.

mod conversation;
mod prompt;

use amplihack_types::{ProjectDirs, sanitize_session_id};
use anyhow::Result;
use serde_json::Value;
use std::fs;
use std::path::Path;

use conversation::load_transcript_conversation;
use prompt::{build_reflection_prompt, run_claude_reflection};

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
}
