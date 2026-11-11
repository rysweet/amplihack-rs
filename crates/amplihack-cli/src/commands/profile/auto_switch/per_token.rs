//! Why auto-switch moved work onto a per-token profile (issue #1804): the
//! rotator's `per-token` line, for a nested launch, a rerun or a status-line
//! swap. It goes to the rotate log, beside the rotator's own lines, as well
//! as to stderr, which for a recipe step ends with the step.

use super::candidate::{
    OverflowPool, Pool, earliest_plan_reset, select_with_overflow, unread_plan,
};
use super::config::Config;
use super::overflow_inputs::OverflowInputs;
use super::state::State;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::overflow::PLATFORM_TIER;
use crate::commands::profile::rotate::{self, CheckNote};
use std::path::Path;

/// The reason line for a move from `current` onto `to`, or `None` when `to`
/// is a plan profile. A move off the API key passed no wait, as in step 3 of
/// `candidate::select_with_overflow`; any other passed step 5's.
pub fn reason(
    pool: &Pool<'_>,
    inputs: &OverflowInputs,
    cfg: &Config,
    current: &str,
    to: &ProfileName,
    check: CheckNote,
) -> Option<String> {
    if !inputs.order.contains(to) {
        return None;
    }
    let tier = if inputs.platform.as_ref() == Some(to) {
        PLATFORM_TIER
    } else {
        cfg.overflow
            .iter()
            .find(|e| e.name == *to)
            .map_or(amplihack_utils::rotator_status::DEFAULT_TIER, |e| e.tier)
    };
    let line = if inputs
        .platform
        .as_ref()
        .is_some_and(|k| k.as_str() == current)
    {
        rotate::per_token_off_key(to, tier, check)
    } else {
        let ov = inputs.pool();
        let earliest = ov
            .gate
            .and_then(|g| earliest_plan_reset(pool, &ov, &g, current));
        rotate::per_token_reason(to, tier, earliest, unread_plan(pool, &ov).len(), check)
    };
    Some(format!("{line}; auto-switch from {current}"))
}

/// Why the API key was held back for a session, from [`key_held`].
#[derive(Debug, Default, PartialEq, Eq)]
pub struct KeyHeld {
    /// The plan logins with no reading. Empty when there is no marked key.
    pub unread: Vec<ProfileName>,
    /// `ALERT: API key not used session=<current>: …` when those logins were
    /// the only thing that kept this selection off the API key. `None`
    /// otherwise: asked again as if they were not there, the selection would
    /// still not pick the key.
    pub line: Option<String>,
}

/// The rotator's `ALERT: API key not used` line, in auto-switch's form:
/// `session=` names the account of the session that ran out, not the login
/// in `~/.claude`, which is what `live=` means in the rotate log.
pub fn key_held(
    pool: &Pool<'_>,
    inputs: &OverflowInputs,
    current: &str,
    eligible: impl Fn(&ProfileName) -> bool,
) -> KeyHeld {
    let ov = inputs.pool();
    let Some(key) = ov.platform else {
        return KeyHeld::default();
    };
    let unread: Vec<ProfileName> = unread_plan(pool, &ov).into_iter().cloned().collect();
    if unread.is_empty() {
        return KeyHeld::default();
    }
    let ignored: Vec<ProfileName> = ov.no_login.iter().chain(&unread).cloned().collect();
    let unheld = OverflowPool {
        no_login: &ignored,
        ..inputs.pool()
    };
    let line = (select_with_overflow(pool, &unheld, current, eligible) == Some(key)).then(|| {
        format!(
            "{}; auto-switch from {current}",
            rotate::alert_key_held_session(current, &unread)
        )
    });
    KeyHeld { unread, line }
}

/// Writes the held-key ALERT to stderr every time, and to the rotate log
/// once per set of unread logins, as the rotator's `alert_once` does: every
/// exhausted launch and rerun asks, and a fleet of recipe steps would
/// otherwise fill the log with one incident. The last set written is kept in
/// the auto-switch state directory; it is forgotten once no plan login is
/// unread, so the next held set is written again. A marker that cannot be
/// read or written only means the line may be written again.
pub fn record_key_held(state: &State, held: &KeyHeld) {
    if held.unread.is_empty() {
        if let Err(e) = state.clear_key_held() {
            tracing::debug!(op = "clear key-held marker", error = %e);
        }
        return;
    }
    let Some(line) = held.line.as_deref() else {
        return;
    };
    eprintln!("amplihack: {line}");
    let set: Vec<&str> = held.unread.iter().map(ProfileName::as_str).collect();
    let set = set.join("\n");
    if state.load_key_held().as_deref() == Some(set.as_str()) {
        return;
    }
    rotate::append_to_log(&state.root, line);
    if let Err(e) = state.save_key_held(&set) {
        tracing::debug!(op = "save key-held marker", error = %e);
    }
}

/// Writes `line`, if there is one, to stderr and to the rotate log.
pub fn record(root: &Path, line: Option<&str>) {
    if let Some(line) = line {
        eprintln!("amplihack: {line}");
        rotate::append_to_log(root, line);
    }
}
