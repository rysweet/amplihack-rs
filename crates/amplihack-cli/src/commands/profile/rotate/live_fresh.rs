//! Issue #2211: the live login's reading freshness and the projected swap.
//! Not to be confused with `live_refresh.rs` (#1753), which refreshes the
//! live login's credentials; this module only reads usage again and never
//! touches a credential.
//!
//! Once per cycle, after the measurement and before the target gate,
//! [`step`] (never for a live login in the auto-switch `exclude` list,
//! issue #2218, nor for the platform tier):
//!
//! - reads the live login again when its cached (passive) reading is frozen
//!   (the same session and week figures on [`FROZEN_CYCLES`] cycles after
//!   the first, with a session above 0), when its reading is older than one
//!   `--interval` (R1, R1b), or when it is below the last re-read after a
//!   re-read that disagreed with it (the cached source is behind); the
//!   re-read reads `~/.claude` with no token refresh (`Ctx::reread_live`).
//!   A direct reading is never frozen, and it restarts the frozen count:
//!   it was read from the usage source in this cycle, so identical direct
//!   figures are an idle login, not a frozen cache. A failed re-read leaves
//!   the frozen count as it was, so a frozen cached reading is read again on
//!   every identical cycle until a re-read succeeds;
//! - also reads it again when a cached (passive) figure contradicts the
//!   last accepted point: lower in the same session window, or from an
//!   earlier one (`series::contradicts`). Use never falls within one window, so
//!   only a later session reset time (`Point::resets`, compared by
//!   `series::window`) makes a lower figure a new window. A newer observation
//!   time does not: the status line rewrites old figures with new times;
//! - projects the session use one interval ahead from a short in-memory
//!   [`Series`] whose points span at least one `--interval`, and decides on
//!   the projection when it reaches `--session-at` (R2);
//! - decides a still-stale live reading on the larger of `--session-at` and
//!   the last known session use when the last known point was near the
//!   threshold (R2b: within `--margin`, or reaching it one interval ahead at
//!   the series rate; [`near`]), so the figure never falls. For a failed
//!   probe the same substitution changes only the timing: the failed-probe
//!   rules choose the target, and the substitution removes their damping
//!   when that target beats the substituted figure (`outlook::on_failed`).
//!   The last known point counts only while its session window can still
//!   be open ([`Series::last_known`]).
//!
//! Issue #2889: a cycle decides on the freshest reading it holds for the
//! live login, ranked by how the reading was obtained and not by the
//! observation time a cache carries. First, a reading taken from the usage
//! source in this cycle: a direct probe, or a re-read above that succeeded.
//! Second, the last reading the step accepted ([`Series::held`]), while both
//! its windows can still be open. Third, this cycle's own cached reading.
//! Rank two stands in for rank three only when the step has already judged
//! the cached reading stale and could read nothing in its place, and only
//! when the held reading is no lower on either window, so the view of the
//! login never falls to a figure the cycle has reason to doubt. There is no
//! [`near`] test on this path: a held reading is a measurement of this
//! login, not a projection, so it reaches the whole decision, the capacity
//! room and the `live_room` bit the usage-limit wait reads. The substituted
//! reading keeps its own reading time, so the usage history records it by
//! its age and the log says how old it is (`log::Shown::cached`).
//!
//! The live re-read runs after the measurement and is dated then, but it
//! never makes the cycle's own target readings stale: the target gate caps
//! the live reading time at the measurement's start (`fresh::settle`).
//!
//! The figures only change the decision, through [`Outlook`] on the cycle's
//! context, which `decide_with` reads. They reach the decision rule's
//! threshold row only ([`with_outlook`]): the overflow
//! rules always run on the reading as measured. The ledger, the history and
//! the log lines keep the reading as measured. `Point`, [`Series`] and
//! [`Outlook`] hold a profile name, times and percentages only: never a
//! token, an account id, an email or a path. Nothing here is persisted.
//! See "Rotate live reading freshness" in docs/reference/profile-command.md.

use super::super::cycle::Ctx;
use super::super::decide::{Probed, Reading};
use super::super::fresh_msg;
use super::super::measure;
use super::super::observe::{Measurement, Snap, decide_with};
use super::read_at;
use crate::commands::profile::ProfileError;
use crate::commands::profile::name::ProfileName;

#[path = "live_series.rs"]
mod series;
pub use series::{Held, Series, week_resets};
use series::{session_resets, stale_at};
// Used inside the series and the step; the tests check them on their own.
#[cfg(test)]
pub use series::{
    MAX_POINTS, Point, SAME_WINDOW_SECS, SESSION_WINDOW_SECS, Window, contradicts, window,
};

/// Unchanged cycles after which a cached (passive) live reading is frozen:
/// the third identical cached reading in a row.
pub const FROZEN_CYCLES: u32 = 2;

#[path = "live_outlook.rs"]
mod outlook;
pub use outlook::{Basis, Outlook, near, projected, with_outlook};
// Used inside `with_outlook`; the tests check it on its own.
#[cfg(test)]
pub use outlook::map_reason;

/// The live freshness step for `m`, decided on `snap`. Sets
/// `ctx.live_outlook`, updates `ctx.live_series`, makes at most one live
/// re-read (logged as [`fresh_msg::STALE_LIVE`]) and decides again when
/// anything changed. The failed live probe count is unchanged.
pub(in crate::commands::profile::rotate) fn step(
    ctx: &mut Ctx,
    snap: &Snap,
    mut m: Measurement,
) -> Result<Measurement, ProfileError> {
    ctx.live_outlook = None;
    // Issue #2218: an excluded live login is never read again, whatever its
    // probe reads (`Snap::measured_live`, the `config::excluded_from` rule
    // the snapshot applied). This does not rely on `Probed::excluded()`
    // being `Exhausted`.
    let name = match snap.measured_live() {
        Some(n) if !snap.platform_live() => n.clone(),
        _ => {
            ctx.live_series.clear();
            return Ok(m);
        }
    };
    // A stub that built the measurement by hand kept no probes.
    let Some((probe, probes)) = m.probed.clone() else {
        return Ok(m);
    };
    let t = (ctx.now)();
    let interval = i64::try_from(ctx.settings.interval).unwrap_or(i64::MAX);
    let mut live = probe.clone();
    let mut still_stale = false;
    if let Reading::Measured { session, week } = probe.reading {
        let unchanged = ctx.live_series.seen(&name, session, week, probe.passive);
        // Only a cached (passive) figure can be frozen. A direct reading was
        // read from the usage source in this cycle: identical direct figures
        // are an idle login, and a re-read by the same method seconds later
        // could only agree, or fail and discard the reading just taken.
        let frozen = probe.passive && session > 0.0 && unchanged >= FROZEN_CYCLES;
        let old = read_at(&probe, m.measured_at).is_none_or(|a| stale_at(a, t, interval));
        let behind = ctx
            .live_series
            .still_behind(&name, &probe, m.measured_at, t, interval);
        let regressed = read_at(&probe, m.measured_at)
            .zip(ctx.live_series.last_for(&name))
            .is_some_and(|(at, last)| at < last.at);
        // A cached figure lower than the last point in the same window (or
        // an unknown one), or from an earlier window, is stale, however new
        // its observation time.
        let contradicted = probe.passive
            && ctx
                .live_series
                .last_for(&name)
                .is_some_and(|last| series::contradicts(&last, session, session_resets(&probe)));
        if frozen || old || behind || regressed || contradicted {
            match reread_live(ctx, &name, t, interval, &mut m.fresh_lines) {
                Some(p) => {
                    let figures = match p.reading {
                        Reading::Measured { session, week } => Some((session, week)),
                        _ => None,
                    };
                    ctx.live_series.after_reread((session, week), figures);
                    live = p;
                }
                None => {
                    ctx.live_series.after_reread((session, week), None);
                    still_stale = true;
                }
            }
        }
    }
    // Issue #2889: nothing could be read from the usage source this cycle,
    // so the cycle falls back to the freshest reading it holds rather than
    // to the cached figures the step has just judged stale. The held reading
    // stands in only while both its windows can still be open and it is no
    // lower than the stale figures on either, so a substitution never grants
    // room the login lacks and never hides use it has made.
    let mut held = None;
    if still_stale
        && let Reading::Measured { session, week } = probe.reading
        && let Some(h) = ctx.live_series.held(&name, t)
        && h.session >= session
        && h.week >= week
    {
        live = held_live(&probe, &h);
        held = Some(h);
    }
    let s = &ctx.settings;
    let at = f64::from(s.session_at);
    // A rate needs points at least one interval apart (`Series::rate`).
    let span = interval;
    let basis = match live.reading {
        Reading::Measured { session, week } if !still_stale => {
            if let Some(read) = read_at(&live, m.measured_at) {
                let resets = session_resets(&live);
                // Issue #2889: only a reading the series accepted is held;
                // one it refused as stale must never stand in later.
                if ctx.live_series.note(&name, read, session, resets, t) {
                    let week_resets = week_resets(&live);
                    ctx.live_series.hold(
                        &name,
                        Held {
                            at: read,
                            session,
                            week,
                            resets,
                            week_resets,
                        },
                    );
                }
            }
            ctx.live_series
                .rate(span)
                .filter(|r| *r > 0.0 && session < at)
                .map(|r| projected(session, r, s.interval))
                .filter(|p| *p >= at)
                .map(|p| (p, week, Basis::Projected))
        }
        Reading::Exhausted { .. } => None,
        // A failed probe, or a reading still stale after the step: never
        // decided on less than the last known session use while its window
        // can still be open. For a failed probe the outlook only times the
        // failed-probe rules' own swap (`outlook::on_failed`).
        other => {
            let stale = match other {
                Reading::Measured { session, .. } => session,
                _ => at,
            };
            // Issue #2889: with a substitution the week is the held
            // reading's own, not the last measured one.
            let week = held.map_or_else(|| ctx.live_series.week(), |h| Some(h.week));
            let rate = ctx.live_series.rate(span);
            ctx.live_series
                .last_known(&name, t)
                .filter(|last| near(last.session, rate, s))
                .zip(week.filter(|w| w.is_finite()))
                .map(|(last, w)| (at.max(last.session).max(stale), w, Basis::Substituted))
        }
    };
    ctx.live_outlook = basis.map(|(session, week, basis)| Outlook {
        name,
        session,
        week,
        basis,
    });
    if ctx.live_outlook.is_none() && live == probe {
        return Ok(m);
    }
    // Issue #2910: the cycle's own `Extra`, so the step decides on the same
    // last good readings as the measurement did.
    let mut again = decide_with(ctx, snap, live, probes, m.measured_at, &m.seen.extra)?;
    debug_assert_eq!(
        again.failed, m.failed,
        "the live step counts the live probe once"
    );
    again.fresh_lines = std::mem::take(&mut m.fresh_lines);
    Ok(again)
}

/// Issue #2889: the cached probe `cached` with the held reading `h` in place
/// of its figures. The source, the login and the window shapes are the
/// cached probe's; the figures, both reset times and the reading time are
/// the held reading's. `passive` is left as it was and `observed_at` is the
/// held reading's own time, so the usage history and the capacity file
/// record it by its real age and never as a reading taken in this cycle.
/// `carried` is cleared, as it is after a re-read: the held reading was read
/// from the usage source and `fresh::read_at` dates it, where a probe still
/// marked carried would have no reading time and would count as unusable.
///
/// A window the cached probe did not read is not invented: a login with no
/// session window keeps none, so no session limit is claimed for it.
fn held_live(cached: &Probed, h: &Held) -> Probed {
    let mut p = cached.clone();
    p.reading = Reading::Measured {
        session: h.session,
        week: h.week,
    };
    if let Some(w) = p.windows.session.as_mut() {
        w.used = h.session;
        w.resets_at = h.resets;
    }
    if let Some(w) = p.windows.week.as_mut() {
        w.used = h.week;
        w.resets_at = h.week_resets;
    }
    p.observed_at = Some(h.at);
    p.carried = false;
    p
}

/// One direct re-read of the live login `name` in `~/.claude`, by
/// `ctx.reread_live` (never the profile directory's copy, and never a token
/// refresh), after the
/// [`fresh_msg::STALE_LIVE`] line, timed at `t`. `None` when no read may be
/// made (no hook, or `rotate stop`), or, after a [`fresh_msg::failed`] line,
/// when the read failed or is still stale at `t`.
fn reread_live(
    ctx: &Ctx,
    name: &ProfileName,
    t: i64,
    interval: i64,
    lines: &mut Vec<String>,
) -> Option<Probed> {
    let reread = ctx.reread_live?;
    if ctx.stopping() {
        return None;
    }
    lines.push(fresh_msg::STALE_LIVE.to_string());
    let (mut p, _) = reread(ctx, name, measure::timeout_quiet());
    let failed = p.reading == Reading::Failed || p.unknown || p.still_running || p.carried;
    // The re-read has just run, so a missing observation time means now.
    let at = p.observed_at.unwrap_or(t);
    let older = ctx
        .live_series
        .last_for(name)
        .is_some_and(|last| at < last.at);
    if failed || stale_at(at, t, interval) || older {
        lines.push(fresh_msg::failed(name));
        return None;
    }
    // A re-read is a direct read made in this cycle.
    p.observed_at = Some(at);
    p.carried = false;
    p.passive = false;
    Some(p)
}
