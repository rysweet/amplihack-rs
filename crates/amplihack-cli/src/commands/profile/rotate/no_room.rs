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
    /// When the record stops holding: two cycles later, at most
    /// [`MAX_SECS`] after `at`.
    pub until: i64,
    /// The profiles measured with no room, sorted, never the API-key
    /// profile.
    pub profiles: Vec<String>,
}

/// The record for a cycle that measured at `at`, every `interval` seconds:
/// `live` and each of `others` whose reading is a measurement with no room
/// or a reached limit. A failed probe is no reading, so it is left out, and
/// so is `platform`, whose reading is synthetic (issue #1700).
pub fn of_cycle(
    live: Option<&ProfileName>,
    reading: &Reading,
    others: &[(ProfileName, Reading)],
    platform: Option<&ProfileName>,
    at: i64,
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
    let span = i64::try_from(interval.saturating_mul(2)).map_or(MAX_SECS, |s| s.min(MAX_SECS));
    NoRoom {
        at,
        until: at.saturating_add(span),
        profiles,
    }
}

/// Records in `l` what the cycle that identified `snap` measured at `at`,
/// at `ctx`'s interval; see [`of_cycle`].
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
    l.no_room = Some(of_cycle(name, live, others, platform, at, interval));
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
