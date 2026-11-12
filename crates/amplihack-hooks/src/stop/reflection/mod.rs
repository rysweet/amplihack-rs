//! Reflection: post-session analysis via native Claude CLI invocation.
//!
//! After a session ends (and lock/power-steering don't block), runs a
//! headless Claude reflection prompt to generate feedback on the work done.
//! Results are saved to timestamped files for history preservation.

mod conversation;
mod prompt;

use super::state_paths::StopStatePaths;
use crate::state_dir::create_private_dir_all;
use amplihack_types::{ProjectDirs, session_state_dir};
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
fn save_reflection_artifacts(state: &StopStatePaths, template: &str) {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let history_path = state.reflection_history(timestamp);

    if let Err(e) = fs::write(state.logs.join("FEEDBACK_SUMMARY.md"), template) {
        tracing::warn!("Failed to write FEEDBACK_SUMMARY.md: {}", e);
    }

    if let Err(e) = fs::write(&history_path, template) {
        tracing::warn!("Failed to write {}: {}", history_path.display(), e);
    }
    if let Err(e) = fs::write(state.reflection.join("current_findings.md"), template) {
        tracing::warn!("Failed to write current_findings.md: {}", e);
    }
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
    let Some(session_dir) = session_state_dir(Some(session_id)) else {
        tracing::warn!("Reflection state unavailable: no amplihack state root, approving");
        return Ok(None);
    };
    run_reflection_with_state(dirs, &StopStatePaths::new(session_dir), transcript_path)
}

pub(super) fn run_reflection_with_state(
    dirs: &ProjectDirs,
    state: &StopStatePaths,
    transcript_path: Option<&Path>,
) -> Result<Option<Value>> {
    create_private_dir_all(&state.logs)?;
    create_private_dir_all(&state.reflection)?;

    let semaphore_path = state.reflection_semaphore();
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

    // Redirect history remains a project-owned input, separate from feedback.
    let redirects_dir = dirs.session_logs(&state.session_key);
    let prompt = build_reflection_prompt(dirs, &redirects_dir, &conversation)?;
    let Some(template) = run_claude_reflection(&dirs.root, &prompt)? else {
        return Ok(None);
    };

    save_reflection_artifacts(state, &template);

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

    #[test]
    fn not_enabled_by_default() {
        let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
        let _enable = crate::test_support::EnvVarGuard::unset("AMPLIHACK_ENABLE_REFLECTION");
        let _skip = crate::test_support::EnvVarGuard::unset("AMPLIHACK_SKIP_REFLECTION");
        let dir = crate::test_support::isolated_tempdir();
        let dirs = ProjectDirs::new(dir.path());
        assert!(!should_run(&dirs));
    }

    #[test]
    fn enabled_config_allows_reflection() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = crate::test_support::isolated_tempdir();
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
        let dir = crate::test_support::isolated_tempdir();
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
        let dir = crate::test_support::isolated_tempdir();
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
        let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
        let home = crate::test_support::isolated_tempdir();
        let _state = crate::test_support::isolate_state_home(home.path());
        let _binary = crate::test_support::EnvVarGuard::set(
            "AMPLIHACK_REFLECTION_BINARY",
            home.path().join("absent-agent"),
        );
        let dir = crate::test_support::isolated_tempdir();
        let dirs = ProjectDirs::new(dir.path());
        let session_id = "test-session";
        let session_dir = amplihack_types::session_state_dir(Some(session_id)).unwrap();
        fs::create_dir_all(&session_dir).unwrap();
        fs::create_dir_all(session_dir.join("reflection")).unwrap();
        let semaphore = session_dir.join("reflection/.reflection_presented_test-session");
        fs::write(&semaphore, "").unwrap();

        let result = run_reflection(&dirs, session_id, None).unwrap();
        assert!(result.is_none());
        assert!(!semaphore.exists());
    }

    #[test]
    fn feedback_survives_history_and_current_findings_storage_failures() {
        let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
        let _environment = crate::stop::issue_2053_tests::Environment::new();
        let state = StopStatePaths::new(session_state_dir(Some("output-error")).unwrap());
        fs::create_dir_all(&state.logs).unwrap();
        // Obstruct both reflection outputs without depending on permissions,
        // effective uid or the timestamp chosen for the history filename.
        fs::write(&state.reflection, "not a directory").unwrap();
        save_reflection_artifacts(&state, "isolated findings");
        assert_eq!(
            fs::read_to_string(state.logs.join("FEEDBACK_SUMMARY.md")).unwrap(),
            "isolated findings"
        );
        assert_eq!(
            fs::read_to_string(&state.reflection).unwrap(),
            "not a directory"
        );
    }

    #[test]
    fn run_reflection_invokes_cli_and_writes_artifacts() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let fake_cli = crate::stop::issue_2053_tests::reflection_fixture();
        let _environment = crate::stop::issue_2053_tests::Environment::new();
        let dir = crate::test_support::isolated_tempdir();
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

        let _binary =
            crate::test_support::EnvVarGuard::set("AMPLIHACK_REFLECTION_BINARY", fake_cli);
        let _project = crate::test_support::EnvVarGuard::set("STOP_FIXTURE_PROJECT", &dirs.root);
        let _findings = crate::test_support::EnvVarGuard::set(
            "STOP_FIXTURE_UNIT_FINDINGS",
            "## Task Summary\nReflected session\n",
        );

        let result = run_reflection(&dirs, "test-session", Some(&transcript)).unwrap();

        let session_dir = amplihack_types::session_state_dir(Some("test-session")).unwrap();
        assert_eq!(result.as_ref().unwrap()["decision"], "block");
        assert!(
            result.as_ref().unwrap()["reason"]
                .as_str()
                .unwrap()
                .contains("Reflected session")
        );
        assert!(session_dir.join("logs/FEEDBACK_SUMMARY.md").exists());
        assert!(session_dir.join("reflection/current_findings.md").exists());
        assert!(
            session_dir
                .join("reflection/.reflection_presented_test-session")
                .exists()
        );
    }
}
