//! Pure decision logic for `limit-resume`: is this stop a usage limit, the
//! keys that identify the event and the session, the gate order, and
//! whether the rotator has recorded a fresh cycle with capacity.

use amplihack_utils::rotator_status::{CycleStatus, FRESH_MARGIN_SECS};
use regex::Regex;
use sha2::{Digest, Sha256};
use std::sync::OnceLock;

/// Only this many leading bytes of `error_details` and
/// `last_assistant_message` are matched.
pub const MAX_MATCH_BYTES: usize = 64 * 1024;
/// The longest session id accepted.
const MAX_SESSION_ID_BYTES: usize = 256;

/// The limit texts the tmux nudger matched, plus the limit menu's first
/// option.
const LIMIT_PATTERN: &str = "You['\u{2019}]ve hit your (org['\u{2019}]s )?(monthly spend |usage |session |weekly )?limit\
     |hit your limit\
     |Stop and wait for limit to reset";

fn limit_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(LIMIT_PATTERN).expect("LIMIT_PATTERN is a valid regex"))
}

/// Whether the stop was a usage limit.
///
/// `error == "rate_limit"` is a limit. Any other non-empty `error` except
/// `unknown` is not, whatever the text says, so model output alone cannot
/// trigger a resume. Only when `error` is missing, empty or `unknown` do the
/// limit texts decide, matched in the first `MAX_MATCH_BYTES` of
/// `error_details` and `last_assistant_message`.
pub fn is_limit(error: Option<&str>, details: Option<&str>, message: Option<&str>) -> bool {
    match error {
        Some("rate_limit") => true,
        None | Some("") | Some("unknown") => [details, message]
            .into_iter()
            .flatten()
            .any(|text| limit_regex().is_match(prefix(text, MAX_MATCH_BYTES))),
        Some(_) => false,
    }
}

/// The longest prefix of `s` that is at most `max` bytes and ends on a
/// character boundary.
fn prefix(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
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
/// `error_details` and `last_assistant_message`. The same limit reported
/// again gives the same key.
pub fn event_key(
    session_id: &str,
    error: Option<&str>,
    details: Option<&str>,
    message: Option<&str>,
) -> String {
    hex_digest(
        "amplihack-limit-resume-event-v1",
        &[Some(session_id), error, details, message],
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
    status.is_some_and(|s| {
        s.has_capacity()
            && s.at
                .is_some_and(|at| at > start.saturating_add(FRESH_MARGIN_SECS))
    })
}
