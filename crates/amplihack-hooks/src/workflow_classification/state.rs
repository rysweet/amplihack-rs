//! The per-session state file of the workflow classification reminder.
//!
//! One JSON file per session at
//! `<state root>/runtime/sessions/<key>/classification_state.json`, under the
//! amplihack state root rather than the project (#1963). Writes are
//! best-effort and no prompt text is ever written. See
//! `docs/reference/coordination-messages.md#state-file`.
//!
//! Every function takes the session state directory as `Option<&Path>`; `None`
//! means no state root resolved, and nothing is read or written.

use crate::state_dir::create_private_dir_all;
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

/// `<session_dir>/classification_state.json`.
pub(super) fn state_file(session_dir: &Path) -> PathBuf {
    session_dir.join("classification_state.json")
}

/// Write `state` to `path`, creating its directory with mode `0700` on Unix.
fn write_state(path: &Path, state: &ClassificationState) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        create_private_dir_all(parent)?;
    }
    fs::write(path, serde_json::to_vec(state)?)?;
    Ok(())
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
/// hook. While writes keep failing, every prompt gets the same turn number, and
/// [`record_outcome`] fails as well, so the hook emits no routing prompt. A
/// single failed write costs at most one extra reminder.
///
/// With no session directory (no state root) the count stays at 0.
pub(super) fn next_observed_turn(session_dir: Option<&Path>, session_id: &str) -> u64 {
    let Some(session_dir) = session_dir else {
        tracing::debug!("workflow classification: no amplihack state root, turn counter not kept");
        return 0;
    };
    let path = state_file(session_dir);
    let mut state = load_state(&path, session_id);
    let current = state.observed_turns;
    state.observed_turns = current.saturating_add(1);
    if let Err(err) = write_state(&path, &state) {
        tracing::debug!(
            "workflow classification: could not record turn at {}: {err}",
            path.display()
        );
    }
    current
}

/// Record which rule decided this prompt, without failing the hook.
///
/// Keeps `observed_turns` and the other fields of the existing state, and sets
/// `last_classified_turn` only when `mark_classified` is true. Unreadable state
/// falls back to the defaults. No prompt text is ever written or logged.
///
/// Returns whether the state was written. A failure is logged at debug level.
/// The caller must not emit the routing prompt after a failed write: the
/// throttle cannot see a classification that was never written, and while
/// writes keep failing [`next_observed_turn`] cannot advance either, so every
/// later prompt would be a new topic and would be routed. With no session
/// directory (no state root) nothing is written and this returns `false`.
pub(super) fn record_outcome(
    session_dir: Option<&Path>,
    session_id: &str,
    turn_count: u64,
    rule: &str,
    mark_classified: bool,
) -> bool {
    let Some(session_dir) = session_dir else {
        tracing::debug!("workflow classification: no amplihack state root, outcome not recorded");
        return false;
    };
    let path = state_file(session_dir);
    let mut state = load_state(&path, session_id);
    state.last_rule = Some(rule.to_string());
    if mark_classified {
        state.last_classified_turn = Some(turn_count);
    }

    match write_state(&path, &state) {
        Ok(()) => true,
        Err(err) => {
            tracing::debug!(
                "workflow classification: could not record outcome at {}: {err}",
                path.display()
            );
            false
        }
    }
}
