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
//! [`MAX_SECS`] after the measurement, so a stopped rotator's record does
//! not last.
//!
//! A login regains room when its full window resets. The rotator reads that
//! reset with each probe, and [`until_reset`] ends the record no later than
//! the earliest reset known for any login it names, so no login stays
//! recorded with no room after a reset that was known when it was written.
//! The whole record ends then, not only that login's entry: a login the
//! record no longer names is unread, which keeps the API key held back, the
//! safe side. A login whose reset is unknown does not shorten the record.
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
use crate::commands::profile::rotate::decide::{Reading, Win, Windows};
use crate::commands::profile::rotate::ledger::RotateLedger;
use crate::commands::profile::rotate::observe::{Probes, Snap, live_name};
use crate::commands::profile::rotate::plan_room::{has_room, room};
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

/// When a login with `reading` and `windows` has room again: the reset of
/// its full window, or the later of the two when both are full. `None` when
/// that is unknown, including a failed probe and a login with room.
pub fn room_back(reading: &Reading, windows: &Windows) -> Option<i64> {
    let (session, week) = match reading {
        Reading::Failed => return None,
        Reading::Measured { session, week } => (Some(*session), Some(*week)),
        Reading::Exhausted => (
            windows.session.map(|w| w.used),
            windows.week.map(|w| w.used),
        ),
    };
    let full = |used: Option<f64>| used.is_some_and(|u| !room(u, 0.0));
    let reset = |w: Option<Win>| w?.resets_at;
    match (full(session), full(week)) {
        (true, false) => reset(windows.session),
        (false, true) => reset(windows.week),
        (true, true) => Some(reset(windows.session)?.max(reset(windows.week)?)),
        (false, false) => None,
    }
}

/// `r` ended no later than the earliest [`room_back`] among the logins it
/// names, each looked up in `probes` by name. A reset before `r.at` ends it
/// at `r.at`, so it stays [`valid`] and holds nothing after its measurement.
pub fn until_reset(mut r: NoRoom, probes: &[(&ProfileName, &Reading, &Windows)]) -> NoRoom {
    let back = probes
        .iter()
        .filter(|(n, _, _)| r.profiles.iter().any(|p| p == n.as_str()))
        .filter_map(|(_, reading, w)| room_back(reading, w))
        .min();
    if let Some(back) = back {
        r.until = r.until.min(back.max(r.at));
    }
    r
}

/// Records in `l` what the cycle that identified `snap` measured at `at`,
/// written now at `ctx`'s interval, ended at the first known reset in
/// `probed`; see [`of_cycle`] and [`until_reset`]. `probed` is `None` only
/// from a stub, and then no reset shortens the record.
pub fn record(
    l: &mut RotateLedger,
    ctx: &Ctx,
    snap: &Snap,
    (live, others): (&Reading, &[(ProfileName, Reading)]),
    probed: Option<&Probes>,
    at: i64,
) {
    let (name, platform) = (live_name(&snap.live), snap.shown_platform());
    let interval = ctx.settings.interval;
    let rec = of_cycle(name, live, others, platform, at, now(), interval);
    // The windows are the probes'; the readings are the ones recorded, which
    // damping may have changed from the probes'.
    let mut seen: Vec<(&ProfileName, &Reading, &Windows)> = Vec::new();
    if let Some((probe, probes)) = probed {
        seen.extend(name.map(|n| (n, live, &probe.windows)));
        for (n, p) in probes {
            let reading = others
                .iter()
                .find(|(o, _)| o == n)
                .map_or(&p.reading, |(_, r)| r);
            seen.push((n, reading, &p.windows));
        }
    }
    l.no_room = Some(until_reset(rec, &seen));
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
