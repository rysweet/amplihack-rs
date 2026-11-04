//! The rotate log lines for hand-change recovery (issues #1693, #1845 and
//! #1886). Kept apart from `log::msg` so that `log.rs` stays within its line
//! budget. Every line is built from profile names and fixed text only.

use super::cred_identity::Why;
use crate::commands::profile::name::ProfileName;

/// The start of every cannot-tell note, after `note: `; `--dry-run` goes on
/// to measure after a `would note:` line that starts with it.
pub const UNSURE_HEAD: &str = "cannot tell whose login is in ~/.claude";

/// Hand-change recovery: `m`'s own copy was moved to `to`.
pub fn recovered_moved(m: &ProfileName, to: &std::path::Path) -> String {
    format!(
        "recovered: ~/.claude and {m} held the same account; kept ~/.claude, moved {m}'s copy to {}; recorded {m} as live",
        to.display()
    )
}

/// Issue #1886: the first reading, or one before the interval has passed,
/// that `~/.claude` holds the same account as `m`'s own login while the
/// ledger names `l`.
pub fn note_suspect(m: &ProfileName, l: &ProfileName) -> String {
    format!(
        "note: ~/.claude holds the same account as profile {m} (ledger names {l}); nothing moved, the next cycle reads again"
    )
}

fn join(v: &[ProfileName]) -> String {
    v.iter()
        .map(ProfileName::as_str)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Issue #1886: recovery cannot tell whose login is in `~/.claude`.
pub fn note_unsure(l: &ProfileName, why: &Why) -> String {
    let why = match why {
        Why::HomeFailed => "~/.claude could not be read".to_string(),
        Why::Many(v) => format!(
            "it holds the same account as more than one profile ({})",
            join(v)
        ),
        Why::Unread(v) if v.is_empty() => "the profile list could not be read".to_string(),
        Why::Unread(v) => format!("profile {} could not be read", join(v)),
        Why::NotRecorded(m) => format!(
            "it holds the same account as profile {m}, which is not the account recorded for {m}; run amplihack profile verify"
        ),
        Why::NotLive => format!(
            "it holds an account other than the one recorded for {l}; run amplihack profile verify"
        ),
        Why::OutOfTime => "the reads ran out of time".to_string(),
        Why::ClockBack => "the last swap time is later than now".to_string(),
    };
    format!("note: {UNSURE_HEAD} (live={l}): {why}; nothing moved")
}

/// Issue #1886: a confirmed suspicion that would leave `l` with no login
/// anywhere (the only-login rule).
pub fn only_login(m: &ProfileName, l: &ProfileName) -> String {
    format!(
        "ALERT: ~/.claude holds the same account as profile {m} on two readings (ledger names {l}), but {l} has no login of its own, so recording {m} would leave {l} with no login anywhere; nothing moved, no swap"
    )
}
