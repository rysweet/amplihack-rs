//! The rotator's own record of the logins it measured with no room (issue
//! #1804), kept in the rotate ledger as `no_room`.
//!
//! Auto-switch holds the API key back while a plan login has no reading, but
//! it cannot read the login in `~/.claude`: the status line records only a
//! session's own `CLAUDE_CONFIG_DIR` profile, and `profile usage` probes
//! each profile's own directory. The rotator measures that login every
//! cycle. It writes here what it found, whatever the capacity state, so
//! auto-switch counts a login the rotator measured with no room as
//! measured, and not as unread.
//!
//! The record is the rotator's alone: it is in the rotate ledger, which no
//! status line or `profile usage` rewrites. It covers every login of the
//! cycle, not only the live one, so a login the rotator has just moved off
//! stays measured until the record expires. It holds until `until`, at most
//! [`MAX_SECS`] after the measurement: a login regains room at a reset the
//! rotator does not record, so a stopped rotator's record must not last.
//!
//! `until` is counted from when the record is written, not from `at`. The
//! record is written at the end of a cycle, and the next one arrives a whole
//! interval plus the next cycle's length later, because the daemon sleeps
//! the interval after a cycle ends. Probes can take minutes on a loaded host
//! (`probe_limits`), so a span counted from `at` alone could expire before
//! the record was written, and auto-switch would call measured logins
//! unread.

use crate::commands::profile::auto_switch::kit::now;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::rotate::cycle::Ctx;
use crate::commands::profile::rotate::decide::Reading;
use crate::commands::profile::rotate::ledger::RotateLedger;
use crate::commands::profile::rotate::observe::{Snap, live_name};
use crate::commands::profile::rotate::plan_room::has_room;
use serde::{Deserialize, Serialize};

/// The longest a record holds after its measurement.
pub const MAX_SECS: i64 = 3600;
/// The most profiles one record may name.
pub const MAX_PROFILES: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoRoom {
    /// When the cycle measured them.
    pub at: i64,
    /// When the record stops holding; see [`of_cycle`].
    pub until: i64,
    /// The profiles measured with no room, sorted, never the API-key
    /// profile.
    pub profiles: Vec<String>,
}

/// The record for a cycle that measured at `at` and writes it at `written`,
/// every `interval` seconds: `live` and each of `others` whose reading is a
/// measurement with no room or a reached limit. A failed probe is no
/// reading, so it is left out, and so is `platform`, whose reading is
/// synthetic (issue #1700).
///
/// It holds for two intervals plus this cycle's length after `written`, so
/// the next record arrives in time even if that cycle takes up to one
/// interval longer than this one. [`MAX_SECS`] after `at` is the limit
/// whatever the cycle took: a measurement that old says nothing.
pub fn of_cycle(
    live: Option<&ProfileName>,
    reading: &Reading,
    others: &[(ProfileName, Reading)],
    platform: Option<&ProfileName>,
    at: i64,
    written: i64,
    interval: u64,
) -> NoRoom {
    let mut profiles: Vec<String> = live
        .map(|n| (n, reading))
        .into_iter()
        .chain(others.iter().map(|(n, r)| (n, r)))
        .filter(|(n, r)| Some(*n) != platform && !matches!(r, Reading::Failed) && !has_room(r))
        .map(|(n, _)| n.to_string())
        .collect();
    profiles.sort();
    profiles.dedup();
    let span = i64::try_from(interval.saturating_mul(2)).unwrap_or(MAX_SECS);
    // A clock stepped back mid-cycle gives no length, not a negative one.
    let written = written.max(at);
    let until = written
        .saturating_add(span)
        .saturating_add(written - at)
        .min(at.saturating_add(MAX_SECS));
    NoRoom {
        at,
        until,
        profiles,
    }
}

/// Records in `l` what the cycle that identified `snap` measured at `at`,
/// written now at `ctx`'s interval; see [`of_cycle`].
pub fn record(
    l: &mut RotateLedger,
    ctx: &Ctx,
    snap: &Snap,
    live: &Reading,
    others: &[(ProfileName, Reading)],
    at: i64,
) {
    let (name, platform) = (live_name(&snap.live), snap.shown_platform());
    let interval = ctx.settings.interval;
    l.no_room = Some(of_cycle(name, live, others, platform, at, now(), interval));
}

/// Whether a loaded record is one the rotator could have written.
pub fn valid(r: Option<&NoRoom>) -> bool {
    r.is_none_or(|r| {
        r.until >= r.at
            && r.until - r.at <= MAX_SECS
            && r.profiles.len() <= MAX_PROFILES
            && r.profiles.windows(2).all(|w| w[0] < w[1])
            && r.profiles.iter().all(|n| ProfileName::parse(n).is_ok())
    })
}

/// The profiles `r` names while it holds at `now`; none for a record from
/// the future, which no clock the rotator shares could have written.
pub fn holding(r: Option<&NoRoom>, now: i64) -> &[String] {
    match r {
        Some(r) if r.at <= now && now <= r.until => &r.profiles,
        _ => &[],
    }
}
