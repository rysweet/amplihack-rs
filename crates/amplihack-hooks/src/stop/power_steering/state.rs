use super::PowerSteeringConfig;
use super::TranscriptMessage;
use super::analysis;
use amplihack_types::{ProjectDirs, sanitize_session_id};
use serde_json::Value;
use std::fs;
use std::path::PathBuf;

pub(super) fn load_config(dirs: &ProjectDirs) -> Option<PowerSteeringConfig> {
    let path = dirs.power_steering_config();
    if !path.exists() {
        return None;
    }

    match fs::read_to_string(&path) {
        Ok(content) => match serde_json::from_str::<Value>(&content) {
            Ok(value) => Some(PowerSteeringConfig {
                enabled: value
                    .get("enabled")
                    .and_then(Value::as_bool)
                    .unwrap_or(true),
            }),
            Err(error) => {
                tracing::warn!(
                    "Invalid power steering config at {}: {error}",
                    path.display()
                );
                Some(PowerSteeringConfig::default())
            }
        },
        Err(error) => {
            tracing::warn!(
                "Failed reading power steering config at {}: {error}",
                path.display()
            );
            None
        }
    }
}

pub(super) fn is_disabled(dirs: &ProjectDirs) -> bool {
    if std::env::var_os("AMPLIHACK_SKIP_POWER_STEERING").is_some() {
        return true;
    }

    if dirs.power_steering.join(".disabled").exists() {
        return true;
    }

    load_config(dirs)
        .map(|config| !config.enabled)
        .unwrap_or(false)
}

pub(super) fn already_completed(dirs: &ProjectDirs, session_id: &str) -> bool {
    completion_semaphore(dirs, session_id).exists()
}

pub(super) fn mark_complete(dirs: &ProjectDirs, session_id: &str) -> anyhow::Result<()> {
    let semaphore = completion_semaphore(dirs, session_id);
    if let Some(parent) = semaphore.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(semaphore, "")?;
    Ok(())
}

pub(super) fn completion_semaphore(dirs: &ProjectDirs, session_id: &str) -> PathBuf {
    dirs.power_steering
        .join(format!(".{}_completed", sanitize_session_id(session_id)))
}

pub(super) fn write_summary(
    dirs: &ProjectDirs,
    session_id: &str,
    messages: &[TranscriptMessage],
) -> anyhow::Result<()> {
    let session_dir = dirs.session_power_steering(session_id);
    fs::create_dir_all(&session_dir)?;

    let first_user = analysis::first_user_message(messages).unwrap_or("unknown task");
    let final_assistant =
        analysis::last_assistant_message(messages).unwrap_or("no assistant summary recorded");
    let summary = format!(
        "# Power Steering Summary\n\n\
         - Session ID: `{session_id}`\n\
         - Status: approved\n\
         - First user request: {}\n\
         - Final assistant summary: {}\n",
        first_user.trim(),
        final_assistant.trim()
    );

    fs::write(session_dir.join("summary.md"), summary)?;
    Ok(())
}
