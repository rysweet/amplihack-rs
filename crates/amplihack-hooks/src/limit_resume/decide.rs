//! Pure decision logic for `limit-resume`: is this stop a usage limit, the
//! keys that identify the event and the session, the gate order, and
//! whether the rotator has recorded a fresh cycle with capacity.

use amplihack_utils::rotator_status::{
    CycleStatus, POLL_INTERVAL, SLOW_POLL_AFTER, SLOW_POLL_INTERVAL,
};
use sha2::{Digest, Sha256};

/// The longest session id accepted.
const MAX_SESSION_ID_BYTES: usize = 256;

/// Whether the stop was a usage limit: `error == "rate_limit"`, exactly.
/// With matcher `rate_limit` Claude Code runs the hook only for that error,
/// so nothing else, and no message text, is treated as a limit.
pub fn is_limit(error: Option<&str>) -> bool {
    error == Some("rate_limit")
}

fn hex_digest(domain: &str, fields: &[Option<&str>]) -> String {
    let mut h = Sha256::new();
    h.update(domain.as_bytes());
    h.update([0]);
    // Tag and length before each field, so no two field lists hash alike.
    for f in fields {
        match f {
            None => h.update([0]),
            Some(s) => {
                h.update([1]);
                h.update((s.len() as u64).to_be_bytes());
                h.update(s.as_bytes());
            }
        }
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// The limit event: SHA-256 (lowercase hex) of the session id, `error`,
/// `error_details`, `last_assistant_message` and the live profile the
/// ledger named when the hook started. The same limit on the same login
/// gives the same key; a limit after the rotator swapped logins does not.
/// With no ledger at the start `live` is `None`, so a later limit seen with
/// a ledger is a new event; the session cap and one wake per session still
/// bound it.
pub fn event_key(
    session_id: &str,
    error: Option<&str>,
    details: Option<&str>,
    message: Option<&str>,
    live: Option<&str>,
) -> String {
    hex_digest(
        "amplihack-limit-resume-event-v2",
        &[Some(session_id), error, details, message, live],
    )
}

/// The session: SHA-256 (lowercase hex) of the session id, so the id itself
/// never reaches the disk or a path.
pub fn session_key(session_id: &str) -> String {
    hex_digest("amplihack-limit-resume-session-v1", &[Some(session_id)])
}

/// A session id is usable when present, non-empty and at most 256 bytes.
pub fn session_id_ok(session_id: Option<&str>) -> bool {
    session_id.is_some_and(|s| !s.is_empty() && s.len() <= MAX_SESSION_ID_BYTES)
}

/// Everything the gate looks at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Facts {
    /// Auto-switch is on for a resolvable root.
    pub enabled: bool,
    /// [`is_limit`] said yes.
    pub limit: bool,
    /// [`session_id_ok`] said yes.
    pub session_ok: bool,
    /// The payload had `stop_hook_active: true`.
    pub stop_hook_active: bool,
    /// This event already has a marker.
    pub duplicate: bool,
    /// The session reached `MAX_RESUMES_PER_SESSION` in the last 24 hours.
    pub capped: bool,
}

/// What the gate decided, in contract order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    Disabled,
    NotLimit,
    StopHookActive,
    Duplicate,
    Cap,
    Proceed,
}

/// The first reason not to wait, in the order Disabled, NotLimit (also a
/// missing or oversized session id), StopHookActive, Duplicate, Cap; else
/// Proceed.
pub fn gate(f: &Facts) -> Gate {
    if !f.enabled {
        Gate::Disabled
    } else if !f.limit || !f.session_ok {
        Gate::NotLimit
    } else if f.stop_hook_active {
        Gate::StopHookActive
    } else if f.duplicate {
        Gate::Duplicate
    } else if f.capped {
        Gate::Cap
    } else {
        Gate::Proceed
    }
}

/// The rotator recorded a cycle with capacity more than
/// `FRESH_MARGIN_SECS` after the hook started at `start`.
pub fn fresh_capacity(status: Option<&CycleStatus>, start: i64) -> bool {
    status.is_some_and(|s| s.fresh_capacity(start))
}

/// How long to sleep between ledger reads, `elapsed` seconds into a wait:
/// `POLL_INTERVAL` for the first `SLOW_POLL_AFTER`, then `SLOW_POLL_INTERVAL`.
pub fn poll_interval(elapsed: i64) -> std::time::Duration {
    if elapsed < SLOW_POLL_AFTER.as_secs() as i64 {
        POLL_INTERVAL
    } else {
        SLOW_POLL_INTERVAL
    }
}
