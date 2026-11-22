//! Issues #2909 and #2910: the fixed lines about the rotator's event wake
//! and about a login it will not move onto. A file of its own because
//! `log.rs` is at its line budget, following the `fresh_msg`/`overflow_msg`
//! split. See "Rotate wake" and "Rotate target choice" in
//! docs/reference/profile-command.md.
//!
//! Every line is fixed text with, at most, a profile name, a whole
//! percentage, a threshold or a timestamp in it. A stored block reason
//! never reaches a line: [`blocked`] passes it through [`kind_label`],
//! which returns `&'static str`, so the compiler guarantees that a process
//! able to write the ledger cannot choose text in the rotate log.

use super::super::decide::{Decision, Measured, Reading};
use super::super::ledger::OverflowBlock;
use super::super::settings::Settings;
use super::super::spend_blocks::kind_label;
use super::timestamp;
use crate::commands::profile::name::ProfileName;
use std::collections::BTreeMap;

/// Why the watch loop stopped waiting. [`Reason::line`] is what it logs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// `ledger.json` was replaced by a writer other than this rotator: a
    /// step or hook recorded a limit.
    Ledger,
    /// The earliest recorded reset fell due, so a blocked login is back.
    Reset,
    /// `--interval` passed with neither of the above.
    Interval,
}

impl Reason {
    pub fn line(self) -> &'static str {
        match self {
            Self::Ledger => "wake: ledger changed",
            Self::Reset => "wake: reset due",
            Self::Interval => "wake: interval",
        }
    }
}

/// No watch could be made: not Linux, inotify refused, or the stop pipe
/// could not be set up. The rotator runs on with `--interval` as its only
/// wake; nothing ends. `wake: unavailable (<cause>), using --interval`.
pub fn wake_unavailable(e: &std::io::Error) -> String {
    format!("wake: unavailable ({}), using --interval", cause(e))
}

/// The wait failed after it had been working. As above: `--interval` only.
/// `wake: lost (<cause>), using --interval`.
pub fn wake_failed(e: &std::io::Error) -> String {
    format!("wake: lost ({}), using --interval", cause(e))
}

/// The watch was lost or its queue overflowed, and a new one replaced it.
pub const WAKE_REARMED: &str = "wake: watch rebuilt";

/// Fixed words for why the intake could not be made or kept, so the line
/// is evidence an operator can act on and never echoes an OS message. The
/// two inotify limits are named, because they are the likely causes on a
/// busy host and the generic words for them ("disk full", "I/O error")
/// would mislead.
pub fn cause(e: &std::io::Error) -> &'static str {
    match (e.kind(), e.raw_os_error()) {
        (std::io::ErrorKind::Unsupported, _) => "not supported here",
        (_, Some(libc::ENOSPC)) => "inotify watch limit reached",
        (_, Some(libc::EMFILE)) => "descriptor or inotify instance limit reached",
        (kind, _) => crate::commands::sessions::kind_word(kind),
    }
}

/// The chosen target could not be read again before the swap, so the swap
/// went ahead on the reading the cycle already had (issue #2910, rule 3).
/// A weaker guarantee, said out loud rather than hidden.
pub const CONFIRM_SKIPPED: &str = "confirm: skipped, no reading could be taken in this cycle";

/// The confirming read showed the chosen target has no room after all, so
/// the next-ranked candidate is tried.
pub fn no_room(name: &ProfileName) -> String {
    format!("reject: {name} fresh reading shows no room")
}

/// Why the rotator will not move onto a login this cycle (issue #2910).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reject {
    /// Its weekly window is at or above `--week-at`.
    Week(u64),
    /// Its session window is at or above `--session-at`.
    Session(u64),
    /// Its reading is exhausted: the probe found a window spent, or the
    /// reading taken for the swap showed no room.
    Spent,
    /// Its probe failed, timed out or read nothing usable.
    Unread,
}

impl Reject {
    /// Why `r` is not a target under `s`, or `None` when it is one. The
    /// week is reported first: that is the bound the rotator used to
    /// ignore, and the one the 21 November 2026 evidence named.
    pub fn of(s: &Settings, r: &Reading) -> Option<Self> {
        let pct = |v: f64| {
            if v.is_finite() && v > 0.0 {
                v as u64
            } else {
                0
            }
        };
        match *r {
            Reading::Measured { session, week } => {
                if week >= f64::from(s.week_at) {
                    Some(Self::Week(pct(week)))
                } else if session >= f64::from(s.session_at) {
                    Some(Self::Session(pct(session)))
                } else if session.is_nan() || week.is_nan() {
                    Some(Self::Unread)
                } else {
                    None
                }
            }
            Reading::Exhausted { .. } => Some(Self::Spent),
            Reading::Failed => Some(Self::Unread),
        }
    }

    fn text(self, s: &Settings) -> String {
        match self {
            Self::Week(w) => format!("week w={w}% at or above --week-at {}", s.week_at),
            Self::Session(v) => {
                format!("session s={v}% at or above --session-at {}", s.session_at)
            }
            Self::Spent => "no room left".to_string(),
            Self::Unread => "no usable reading".to_string(),
        }
    }
}

/// A candidate the rotator passed over, with its reason.
pub fn reject(s: &Settings, name: &ProfileName, why: Reject) -> String {
    format!("reject: {name} {}", why.text(s))
}

/// A candidate passed over for a recorded limit, with the kind as fixed
/// text and the reset it was recorded against.
pub fn blocked(name: &ProfileName, kind: &str, until: i64) -> String {
    format!(
        "reject: {name} {} recorded until {}",
        kind_label(kind),
        timestamp(until)
    )
}

/// One line per candidate the rotator may not move onto, with its reason.
/// A recorded block is reported first and by its kind, because that is the
/// evidence a limit event left; otherwise the threshold the reading failed.
///
/// Written only by a cycle that looked for a target: one that swapped, and
/// the ALERT, which found none. A cycle that stays was not choosing, and a
/// line per candidate on every quiet cycle would bury the lines that
/// matter. A swap needs these lines as much as the ALERT does: a login
/// passed over for a recorded block shows its probed figures on the
/// `cycle` line, so without its `reject:` line nothing says why the
/// rotator moved elsewhere. `live` and the swap's own target are left out:
/// the `cycle` and `swap` lines already say everything about them.
pub fn rejections(
    s: &Settings,
    d: &Decision,
    measured: &[Measured],
    live: Option<&ProfileName>,
    blocks: &BTreeMap<String, OverflowBlock>,
    now: i64,
) -> Vec<String> {
    let target = match d {
        Decision::Stay => return Vec::new(),
        Decision::Swap { to, .. } => Some(to),
        Decision::Alert => None,
    };
    measured
        .iter()
        .filter(|c| Some(&c.name) != live && Some(&c.name) != target)
        .filter_map(
            |c| match blocks.get(c.name.as_str()).filter(|b| now < b.until) {
                Some(b) => Some(blocked(&c.name, &b.reason, b.until)),
                None => Reject::of(s, &c.reading).map(|why| reject(s, &c.name, why)),
            },
        )
        .collect()
}

#[cfg(test)]
#[path = "tests_issue_2910_msg.rs"]
mod tests_issue_2910_msg;
