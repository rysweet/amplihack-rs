//! The per-session state file of the workflow classification reminder.
//!
//! One JSON file per session at
//! `<project>/.claude/runtime/classification_state/<session>.json`. Writes are
//! best-effort and no prompt text is ever written. See
//! `docs/reference/coordination-messages.md#state-file`.

use amplihack_types::{ProjectDirs, sanitize_session_id};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct ClassificationState {
    /// Turn at which the routing prompt was last emitted.
    ///
    /// Absent until the session's first classification, so that a turn which
    /// classified nothing (a coordination note, an explicit `/dev` command or
    /// a gated prompt) cannot look like a classification at turn 0 to the
    /// throttle. Files written before issue #1742 always hold an integer and
    /// load unchanged. A binary from before #1742 fails to parse a file
    /// without the field and treats it as missing, which routes the prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) last_classified_turn: Option<u64>,
    pub(super) session_id: String,
    /// Prompts seen in this session, counted by us.
    ///
    /// Only used when the runtime does not report one. The Copilot CLI's
    /// `userPromptSubmitted` payload is `{prompt}` and carries no turn number,
    /// so treating a missing count as turn 0 would make `turn_count <= 1`
    /// permanently true and route every human prompt forever (issue #1333).
    #[serde(default)]
    pub(super) observed_turns: u64,
    /// Which rule decided the most recent prompt, from the `rule` module.
    ///
    /// Absent in files written before issue #1742. See
    /// `docs/reference/coordination-messages.md` for the values.
    #[serde(default)]
    pub(super) last_rule: Option<String>,
}

pub(super) fn state_file(dirs: &ProjectDirs, session_id: &str) -> PathBuf {
    dirs.runtime
        .join("classification_state")
        .join(format!("{}.json", sanitize_session_id(session_id)))
}

/// Load this session's state, or the defaults when it is missing or unreadable.
pub(super) fn load_state(path: &Path, session_id: &str) -> ClassificationState {
    fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str::<ClassificationState>(&raw).ok())
        .unwrap_or_else(|| ClassificationState {
            last_classified_turn: None,
            session_id: session_id.to_string(),
            observed_turns: 0,
            last_rule: None,
        })
}

/// Read, increment and persist this session's own prompt counter.
///
/// Returns the turn number for the prompt being handled now: 0 for the first,
/// 1 for the second, and so on -- matching what a runtime that reports
/// `turnCount` would send, so downstream logic needs no special case.
///
/// A write failure returns the un-incremented count rather than failing the
/// hook. The cost is one extra reminder, not a broken prompt.
pub(super) fn next_observed_turn(dirs: &ProjectDirs, session_id: &str) -> u64 {
    let path = state_file(dirs, session_id);
    let mut state = load_state(&path, session_id);
    let current = state.observed_turns;
    state.observed_turns = current.saturating_add(1);
    if let Some(parent) = path.parent()
        && fs::create_dir_all(parent).is_ok()
        && let Ok(body) = serde_json::to_vec(&state)
    {
        let _ = fs::write(&path, body);
    }
    current
}

/// Record which rule decided this prompt, without failing the hook.
///
/// Keeps `observed_turns` and the other fields of the existing state, and sets
/// `last_classified_turn` only when `mark_classified` is true. Unreadable state
/// falls back to the defaults. A write failure is logged and ignored: the cost
/// is one extra reminder later, not a broken prompt. No prompt text is ever
/// written or logged.
pub(super) fn record_outcome(
    dirs: &ProjectDirs,
    session_id: &str,
    turn_count: u64,
    rule: &str,
    mark_classified: bool,
) {
    let path = state_file(dirs, session_id);
    let mut state = load_state(&path, session_id);
    state.last_rule = Some(rule.to_string());
    if mark_classified {
        state.last_classified_turn = Some(turn_count);
    }

    let write = || -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, serde_json::to_vec(&state)?)?;
        Ok(())
    };
    if let Err(err) = write() {
        tracing::debug!(
            "workflow classification: could not record outcome at {}: {err}",
            path.display()
        );
    }
}
