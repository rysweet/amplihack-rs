//! Issue #2909, "Wanted" item 3: fewer probes. Steps 4 and 5 of a cycle,
//! measure and then decide, and the readings a watch keeps so that a cycle
//! woken by an event reads less than one woken by `--interval`.
//!
//! A cycle that follows a limit record or a recorded reset (an event
//! cycle, [`wake_msg::Reason::event`]) reads the live login. Each other
//! login's figures come from the last reading this watch took of it
//! ([`Stored`]) while that reading can still stand for the login; a login
//! without one is read, as in every cycle. An interval cycle, the first cycle of a watch, the one-shot
//! `rotate` and `--dry-run` read every login, as before.
//!
//! Why a stored reading is safe to decide on. Within one window a login's
//! use only rises until that window resets. While every window a stored
//! reading reports is still open, the reading never shows less use than the
//! login has now: a login it shows full is still full, and a login it shows
//! with room may have less room now. The first is decided on as it stands.
//! The second is never swapped to on the stored figure. A stored reading has
//! no reading time in this cycle (`carried`), so the freshness gate
//! (`fresh::gate`, issue #1870) reads the chosen target again before the
//! swap and decides again on what that read finds. A target that has filled
//! up is replaced by the next, read the same way, within the gate's limits.
//!
//! A stored reading stands for a login only while it was read no more than
//! `--interval` seconds ago and every window it reports has a known reset
//! still to come. So an event cycle never decides on figures older than an
//! interval cycle would have used, a window with no known reset is read
//! again, and so is a login whose window has reset since, which is how a
//! login comes back. Each login is therefore read at most once per
//! `--interval` for its figures, once when a window it reported resets, and
//! once before it becomes the target. The live login is read every cycle.
//!
//! Logins whose identity is not yet bound (`Skip::NoAccount`) are read only
//! by a cycle that reads every login: their readings never enter a decision,
//! and the read is only for the token refresh it may make.

use super::super::decide::{Probed, Win};
use super::super::fresh::read_at;
use super::super::identify::{LiveId, Skip};
use super::super::log::wake_msg;
use super::super::measure::{self, Live};
use super::super::observe::{Measurement, Probes, Snap, decide_measured};
use super::Ctx;
use crate::commands::profile::ProfileError;
use crate::commands::profile::name::ProfileName;
use std::collections::HashMap;

#[cfg(test)]
#[path = "tests_issue_2909_reads.rs"]
mod tests;

/// The last reading this watch took of each login, and when it was taken.
/// Held in memory for the life of the watch process only: nothing is written,
/// so a rotator that restarts reads every login in its first cycle.
#[derive(Debug, Default)]
pub struct Stored(HashMap<ProfileName, (i64, Probed)>);

impl Stored {
    /// Keeps each reading in `probes` that has a reading time in the cycle
    /// that measured from `measured_at` ([`read_at`]), the live probe under
    /// `live`. A probe with none (failed, unknown, still running, or itself a
    /// stored reading) leaves what is kept for its login as it was.
    pub fn keep(&mut self, live: Option<&ProfileName>, probes: Option<&Probes>, measured_at: i64) {
        let Some((live_probe, others)) = probes else {
            return;
        };
        let named = live
            .map(|n| (n, live_probe))
            .into_iter()
            .chain(others.iter().map(|(n, p)| (n, p)));
        for (name, p) in named {
            if let Some(at) = read_at(p, measured_at) {
                self.0.insert(name.clone(), (at, p.clone()));
            }
        }
    }

    /// The reading kept for `name`, for an event cycle at `now`, when it can
    /// still stand for the login: taken no more than `max_age` seconds before
    /// `now` and not after it, reporting at least one window, and with every
    /// window it reports resetting after `now`. A reading with no window, or
    /// with a window whose reset is unknown, never stands.
    ///
    /// It comes back marked `carried`, so it has no reading time in this
    /// cycle and the freshness gate reads the login again before any swap to
    /// it. `observed_at` is when it was taken, so the usage history dates it
    /// by its real age and never as a reading of this cycle.
    pub fn usable(&self, name: &ProfileName, now: i64, max_age: i64) -> Option<Probed> {
        let (at, p) = self.0.get(name)?;
        let age = now.checked_sub(*at)?;
        let open = |w: &Option<Win>| {
            w.as_ref()
                .is_none_or(|w| w.resets_at.is_some_and(|r| now < r))
        };
        let reported = p.windows.session.is_some() || p.windows.week.is_some();
        if !(0..=max_age).contains(&age)
            || !reported
            || !open(&p.windows.session)
            || !open(&p.windows.week)
        {
            return None;
        }
        let mut p = p.clone();
        p.carried = true;
        p.observed_at = Some(*at);
        Some(p)
    }

    /// `candidates` split into the logins an event cycle at `now` reads and
    /// the [`usable`](Self::usable) readings of the rest, each in the order of
    /// `candidates`.
    pub fn split(
        &self,
        candidates: &[ProfileName],
        now: i64,
        max_age: i64,
    ) -> (Vec<ProfileName>, Vec<(ProfileName, Probed)>) {
        let (mut read, mut stored) = (Vec::new(), Vec::new());
        for c in candidates {
            match self.usable(c, now, max_age) {
                Some(p) => stored.push((c.clone(), p)),
                None => read.push(c.clone()),
            }
        }
        (read, stored)
    }
}

/// Steps 4 and 5 of a cycle: measure without the lock, then decide. An
/// event cycle (`Ctx::event`) reads as [`event_reads`] says; every other
/// cycle reads the live login and every candidate.
pub fn measure_and_decide(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let measured_at = (ctx.now)();
    let platform = snap.platform();
    let live = match &snap.live {
        // Issue #2218: before the API-key case, so an excluded API-key
        // profile is moved off as well.
        LiveId::Known(_) if snap.live_excluded => Live::Excluded,
        _ if snap.platform_live() => Live::Platform,
        LiveId::Known(n) => Live::Profile(n),
        _ => Live::Unowned,
    };
    // The API-key profile is never probed: it gets its synthetic reading,
    // and it is in the overflow order, so never in the plan pool.
    let to_probe: Vec<ProfileName> = snap
        .candidates
        .iter()
        .filter(|c| Some(*c) != platform)
        .cloned()
        .collect();
    // Keep the existing usage reader's refresh opportunity for otherwise
    // eligible OAuth logins. Their readings cannot enter this decision:
    // a later cycle must first bind identity to the refreshed credentials.
    let unbound: Vec<ProfileName> = snap
        .skips
        .iter()
        .filter(|(_, reason)| matches!(reason, Skip::NoAccount))
        .map(|(name, _)| name.clone())
        .collect();
    let (probe, mut probes, line) = if ctx.event {
        event_reads(ctx, live, &to_probe, measured_at)
    } else {
        let (probe, probes) = measure::measure_ordered(ctx, live, &to_probe, &unbound, measured_at);
        (probe, probes, None)
    };
    if let Some(p) = platform.filter(|p| snap.candidates.contains(p)) {
        probes.push((p.clone(), Probed::platform()));
    }
    let mut m = decide_measured(ctx, snap, probe, probes, measured_at)?;
    // Written after the cycle line, before the freshness gate's lines.
    m.fresh_lines.extend(line);
    Ok(m)
}

/// An event cycle's reads: the live login, and each of `candidates` that has
/// no [`Stored::usable`] reading, within `--interval`. Every candidate comes
/// back, in the order of `candidates`, with the
/// [`wake_msg::stored`] line when any of them was taken from a stored
/// reading rather than read.
fn event_reads(
    ctx: &Ctx,
    live: Live<'_>,
    candidates: &[ProfileName],
    measured_at: i64,
) -> (Probed, Vec<(ProfileName, Probed)>, Option<String>) {
    let max_age = i64::try_from(ctx.settings.interval).unwrap_or(i64::MAX);
    let (to_read, stored) = ctx.stored.split(candidates, measured_at, max_age);
    let own = usize::from(live.probed());
    let line =
        (!stored.is_empty()).then(|| wake_msg::stored(own + to_read.len(), own + candidates.len()));
    let (probe, read) = measure::measure_ordered(ctx, live, &to_read, &[], measured_at);
    let mut got: HashMap<ProfileName, Probed> = read.into_iter().chain(stored).collect();
    let others = candidates
        .iter()
        .filter_map(|c| got.remove(c).map(|p| (c.clone(), p)))
        .collect();
    (probe, others, line)
}
