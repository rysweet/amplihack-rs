//! A login's last good reading from the usage history (issue #2910, D8).
//!
//! When a probe says nothing about a login's room (rate limited, failed in
//! transport, timed out, still running), the rotator decides on what that
//! login last showed instead of treating it as full or unread. It is used
//! for candidates only, never for the live login, and never written back:
//! the history, the capacity file and the ledger keep the probe's own
//! reading. The age limit is a check on data, not a timer.
//!
//! Rules:
//!
//! - L1: a point counts when `now - LAST_GOOD_MAX_AGE_SECS <= at < now`.
//! - L2: the newest point per login wins; on an equal time the later line.
//! - L3: when the newest point is unusable there is no last good reading,
//!   even if an older point would have been usable (E17): the newest point
//!   is what the login last showed.
//! - L4: a point is unusable when its week has reset (`w_reset <= now`), or
//!   when its session is missing and the login did not show "no session
//!   limit".
//! - L5: a missing `w_reset` is accepted.
//! - L6: a missing session with "no session limit" reads as session 0.
//! - L7: the result is [`Reading::Measured`] in whole percentages.
//!
//! The history stores whole percentages, so a stored 95 may have been 95.4.
//! Candidate selection uses the batch time, which may understate the age of
//! a point copied in place of a stale passive reading (issue #1970). The
//! history records a point only for a probe that read the login, so a spend
//! limit or a refused token leaves the newest point available.

use super::Reading;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::usage_history::{self, Point};
use std::collections::BTreeMap;
use std::path::Path;

/// The oldest a last good reading may be: one session window.
pub const LAST_GOOD_MAX_AGE_SECS: i64 = 18_000;

/// A login's last good reading and when it was taken.
#[derive(Debug, Clone, PartialEq)]
pub struct LastGood {
    pub reading: Reading,
    pub at: i64,
}

/// L4, L6, L7: the reading of a usable point at `now`.
fn usable(p: &Point, now: i64) -> Option<Reading> {
    if p.w_reset.is_some_and(|r| r <= now) {
        return None;
    }
    let session = match p.s {
        Some(s) => f64::from(s),
        None if p.no_session_limit => 0.0,
        None => return None,
    };
    Some(Reading::Measured {
        session,
        week: f64::from(p.w),
    })
}

/// The last good readings of `names` at `now`, from one read of the usage
/// history. A history that is refused or unreadable gives no readings and
/// its one fixed warning on stderr; the caller reads at most once a cycle.
pub fn last_good(root: &Path, names: &[ProfileName], now: i64) -> BTreeMap<ProfileName, LastGood> {
    if names.is_empty() {
        return BTreeMap::new();
    }
    let (entries, warning) = usage_history::read_recent(root, now, LAST_GOOD_MAX_AGE_SECS);
    if let Some(w) = warning {
        eprintln!("{w}");
    }
    // `read_recent` sorts by time with a stable sort, so on an equal time a
    // later line comes later and replaces an earlier one (L2).
    let mut newest: BTreeMap<&str, (i64, &Point)> = BTreeMap::new();
    for e in &entries {
        for (name, p) in &e.profiles {
            if names.iter().any(|n| n.as_str() == name) {
                newest.insert(name.as_str(), (e.at, p));
            }
        }
    }
    names
        .iter()
        .filter_map(|n| {
            let (at, p) = newest.get(n.as_str())?;
            let reading = usable(p, now)?;
            Some((n.clone(), LastGood { reading, at: *at }))
        })
        .collect()
}

#[cfg(test)]
#[path = "tests_issue_2910_last_good.rs"]
mod tests_issue_2910_last_good;
