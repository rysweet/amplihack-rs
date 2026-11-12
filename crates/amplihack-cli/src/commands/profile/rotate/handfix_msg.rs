//! The rotate log lines for hand-change recovery (issues #1693, #1845 and
//! #1886). Kept apart from `log::msg` so that `log.rs` stays within its line
//! budget. Every line is built from profile names and fixed text only.

use super::cred_identity::Why;
use super::ledger::MAX_GUIDANCE_LINE;
use super::log::cut;
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

pub(super) fn join(v: &[ProfileName]) -> String {
    v.iter()
        .map(ProfileName::as_str)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Issue #1886: recovery cannot tell whose login is in `~/.claude`. Issue
/// #1962: cut to the ledger's line limit, because the note is saved in its
/// `guidance`, and a longer line would make the ledger unreadable.
pub fn note_unsure(l: &ProfileName, why: &Why) -> String {
    let why = match why {
        Why::Home(cause) => format!("~/.claude identity read failed: {cause}"),
        Why::Many(v) => format!(
            "it holds the same account as more than one profile ({})",
            join(v)
        ),
        Why::Unread(v) if v.is_empty() => "the profile root could not be listed".to_string(),
        Why::Unread(v) => format!("profile {}", unread(v)),
        Why::NotRecorded(m) => format!(
            "it holds the same account as profile {m}, which is not the account recorded for {m}; run amplihack profile verify"
        ),
        Why::NotLive => format!(
            "it holds an account other than the one recorded for {l}; run amplihack profile verify"
        ),
        Why::OutOfTime => "the reads ran out of time".to_string(),
        Why::ClockBack => "the last swap time is later than now".to_string(),
    };
    cut(
        &format!("note: {UNSURE_HEAD} (live={l}): {why}; nothing moved"),
        MAX_GUIDANCE_LINE,
    )
}

/// `kilo not read (http 429), mike not read (timeout)`.
fn unread(v: &[(ProfileName, &'static str)]) -> String {
    v.iter()
        .map(|(name, cause)| format!("{name} not read ({cause})"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Issue #1962: the rotate ledger cannot be read, so nothing may move and no
/// record may be written. `cause` is the fresh identity reading's cause, or
/// `readable`. Issue #1947: `field` is the fixed reason the load gave when it
/// refused a field ([`super::ledger::refused_field`]), shown in brackets.
pub fn ledger_unreadable(field: Option<&str>, cause: &str) -> String {
    let field = field.map_or_else(String::new, |f| format!(" ({f})"));
    format!(
        "rotate ledger unreadable{field}; live identity: {cause}; nothing moved, no record written"
    )
}

/// The `live:` value of `profile rotate status` for an unreadable ledger,
/// naming the refused field as [`ledger_unreadable`] does (issue #1947).
pub fn status_live_unreadable(field: Option<&str>) -> String {
    field.map_or_else(
        || "- (ledger unreadable)".into(),
        |f| format!("- (ledger unreadable: {f})"),
    )
}

/// Issue #2218: the `live:` value of `profile rotate status` when the
/// ledger names no live profile and the excluded profiles cannot be worked
/// out, so whether an excluded login is live is unknown, not `-`.
pub fn status_live_exclude_unknown(cause: &str) -> String {
    format!(
        "unknown (excluded profiles unreadable: {})",
        crate::commands::profile::usage_cmd::sanitize_display(cause, false)
    )
}

/// Issue #1886: a confirmed suspicion that would leave `l` with no login
/// anywhere (the only-login rule).
pub fn only_login(m: &ProfileName, l: &ProfileName) -> String {
    format!(
        "ALERT: ~/.claude holds the same account as profile {m} on two readings (ledger names {l}), but {l} has no login of its own, so recording {m} would leave {l} with no login anywhere; nothing moved, no swap"
    )
}
