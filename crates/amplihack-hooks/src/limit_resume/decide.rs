//! Pure decision logic for `limit-resume`: is this stop a usage limit, the
//! keys that identify the event and the session, and the gate order.
//! Whether a swap record resumes is
//! [`amplihack_utils::rotator_status::swap_record::judge_at`].

use sha2::{Digest, Sha256};

/// The longest session id accepted.
const MAX_SESSION_ID_BYTES: usize = 256;

/// The `StopFailure` `error` values of Claude Code 2.1.329 that are not a
/// usage, spend or credit limit. Compared exactly and case-sensitively with
/// the raw value.
pub const NOT_LIMIT_ERRORS: [&str; 10] = [
    "authentication_failed",
    "oauth_org_not_allowed",
    "account_on_hold",
    "verification_required",
    "overloaded",
    "invalid_request",
    "model_not_found",
    "server_error",
    "max_output_tokens",
    "cloud_credential_error",
];

/// Whether the stop is treated as a usage limit: every `error` value except
/// the known non-limit ones in [`NOT_LIMIT_ERRORS`] (issue #3172). The hook
/// is registered with matcher `rate_limit`, so Claude Code has already
/// chosen which stops reach it; `rate_limit`, `billing_error`, `unknown`, an
/// absent or non-string value and any value not seen before all count, so a
/// renamed limit value cannot end the call silently again. No message text
/// is consulted.
///
/// This is not what decides whether keys are typed. The pane must still be
/// proved to be the session's own terminal, it must be showing a screen the
/// keys answer, and the guard claim must be committed first.
pub fn is_limit(error: Option<&str>) -> bool {
    !error.is_some_and(|e| NOT_LIMIT_ERRORS.contains(&e))
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
/// `error_details`, `last_assistant_message` and `live`, the login the
/// session stopped on: the outgoing login (`from_login`) of a swap the hook
/// noted at its start when that swap falls after the stop and is not later
/// than the start, otherwise the live profile the ledger named when the
/// hook started (#2235). The same limit on the same login gives the same
/// key; a limit after the rotator swapped logins does not. With no ledger
/// at the start and no such swap `live` is `None`, so a later limit seen
/// with a ledger is a new event; the event marker and one wake per session
/// still bound it.
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
}

/// What the gate decided, in contract order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    Disabled,
    NotLimit,
    BadSessionId,
    StopHookActive,
    Duplicate,
    Proceed,
}

/// The first reason not to wait, in the order Disabled, NotLimit,
/// BadSessionId (a missing, empty or oversized session id; issue #3172),
/// StopHookActive, Duplicate; else
/// Proceed. No count appears here: since issue #2912 nothing refuses a
/// resume on how often a session has already been resumed.
pub fn gate(f: &Facts) -> Gate {
    if !f.enabled {
        Gate::Disabled
    } else if !f.limit {
        Gate::NotLimit
    } else if !f.session_ok {
        Gate::BadSessionId
    } else if f.stop_hook_active {
        Gate::StopHookActive
    } else if f.duplicate {
        Gate::Duplicate
    } else {
        Gate::Proceed
    }
}
