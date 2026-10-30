//! The SessionEnd marker for `amplihack agent` (issue #1760, R6.3, S9,
//! S20b): when a session ends with `/exit` or `/logout`, write
//! `~/.amplihack/agents/ended/<session_id>` so the supervisor records the
//! agent as ended instead of resuming it. Written only when a registry
//! exists, through private directories and an atomic rename. Every failure is
//! logged at debug level and ignored: the hook never fails because of it.

use amplihack_utils::secure_files::{
    check_private_dir, ensure_private_dir, owned_by_euid, writable_by_others, write_private_atomic,
};
use serde_json::{Value, json};
use std::fs;
use std::path::Path;

/// The SessionEnd reasons that mean the user ended the session. A lost
/// terminal sends `other`; `clear` and `resume` keep the session running.
pub(super) const CLEAN_REASONS: &[&str] = &["prompt_input_exit", "logout"];

/// Writes the marker for `session_id` when `extra["reason"]` is a clean
/// reason, the id passes the conversation-id rule and
/// `home/.amplihack/agents/registry.json` exists.
pub(super) fn record_clean_end(
    home: Option<&Path>,
    session_id: Option<&str>,
    extra: &Value,
    now: u64,
) {
    let Some(reason) = extra.get("reason").and_then(Value::as_str) else {
        return;
    };
    if !CLEAN_REASONS.contains(&reason) {
        tracing::debug!(reason, "SessionEnd reason does not end an agent");
        return;
    }
    let (Some(home), Some(id)) = (home, session_id.filter(|id| valid_id(id))) else {
        return;
    };
    if let Err(e) = write_marker(home, id, reason, now) {
        tracing::debug!("no agent SessionEnd marker written: {e}");
    }
}

/// `^[A-Za-z0-9][A-Za-z0-9-]{0,63}$`, the supervisor's conversation-id rule.
fn valid_id(id: &str) -> bool {
    let mut bytes = id.bytes();
    bytes.next().is_some_and(|b| b.is_ascii_alphanumeric())
        && id.len() <= 64
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

fn write_marker(home: &Path, id: &str, reason: &str, now: u64) -> Result<(), String> {
    let top = home.join(".amplihack");
    let meta = fs::symlink_metadata(&top).map_err(|e| format!("~/.amplihack: {e}"))?;
    if meta.file_type().is_symlink()
        || !meta.is_dir()
        || !owned_by_euid(&meta)
        || writable_by_others(&meta)
    {
        return Err("~/.amplihack is not a private directory".into());
    }
    let agents = top.join("agents");
    check_private_dir(&agents).map_err(|e| format!("~/.amplihack/agents: {e}"))?;
    let registry = fs::symlink_metadata(agents.join("registry.json"))
        .map_err(|e| format!("registry.json: {e}"))?;
    if !registry.file_type().is_file() || !owned_by_euid(&registry) {
        return Err("registry.json is not a regular file of this user".into());
    }
    let ended = agents.join("ended");
    ensure_private_dir(&ended).map_err(|e| format!("~/.amplihack/agents/ended: {e}"))?;
    let body = json!({ "session_id": id, "reason": reason, "at": now }).to_string();
    write_private_atomic(&ended, id, body.as_bytes()).map_err(|e| format!("marker: {e}"))
}

#[cfg(test)]
#[path = "agent_marker_tests.rs"]
mod tests;
