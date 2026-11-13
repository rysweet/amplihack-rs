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
//! The live re-read runs after the measurement and is dated then, but it
//! never makes the cycle's own target readings stale: the target gate caps
//! the live reading time at the measurement's start (`fresh::settle`).
//!
//! The figures only change the decision, through [`Outlook`] on the cycle's
//! context, which `decide_measured` reads. They reach the decision rule's
//! threshold row only ([`with_outlook`]): the plan-room swap and the overflow
//! rules always run on the reading as measured. The ledger, the history and
//! the log lines keep the reading as measured. `Point` and [`Outlook`] hold
//! profile names, times and percentages. [`Series`] also retains verified
//! account identity for continuity, never credentials. Nothing here is persisted.
//! See "Rotate live reading freshness" in docs/reference/profile-command.md.

use super::super::cycle::Ctx;
use super::super::decide::{Probed, Reading};
use super::super::fresh_msg;
use super::super::measure;
use super::super::observe::{Measurement, Snap, decide_measured};
use super::read_at;
use crate::commands::profile::ProfileError;
use crate::commands::profile::name::ProfileName;

#[path = "live_series.rs"]
mod series;
pub use series::Series;
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
    ctx.live_series.bind(snap.trusted_live.as_ref());
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
    let s = &ctx.settings;
    let at = f64::from(s.session_at);
    // A rate needs points at least one interval apart (`Series::rate`).
    let span = interval;
    let basis = match live.reading {
        Reading::Measured { session, week } if !still_stale => {
            if let Some(read) = read_at(&live, m.measured_at) {
                let resets = session_resets(&live);
                ctx.live_series.note(&name, read, session, resets, t);
                ctx.live_series.note_week(
                    read,
                    week,
                    live.windows.week.and_then(|w| w.resets_at),
                    t,
                );
            }
            let session_projection = ctx.live_series.forecast(false, t, interval);
            let week_projection = ctx.live_series.forecast(true, t, interval);
            for (window, projection) in [
                ("session", &session_projection),
                ("weekly", &week_projection),
            ] {
                match projection {
                    Err(reason) => m
                        .fresh_lines
                        .push(format!("projection {window}: unavailable ({reason})")),
                    Ok(p) => m
                        .fresh_lines
                        .push(format!("projection {window}: next cycle {p:.1}%")),
                }
            }
            let session_projection = session_projection
                .ok()
                .filter(|p| *p >= at && session < at)
                .map(|p| (p, week, Basis::Projected));
            let week_projection = week_projection
                .ok()
                .filter(|p| *p >= f64::from(s.week_at) && week < f64::from(s.week_at))
                .map(|w| (session, w, Basis::Projected));
            session_projection.or(week_projection)
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
            let week = ctx.live_series.week();
            let rate = ctx
                .live_series
                .forecast(false, t, interval)
                .ok()
                .and_then(|_| ctx.live_series.rate(span));
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
    let mut again = decide_measured(ctx, snap, live, probes, m.measured_at)?;
    debug_assert_eq!(
        again.failed, m.failed,
        "the live step counts the live probe once"
    );
    again.fresh_lines = std::mem::take(&mut m.fresh_lines);
    Ok(again)
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
    let mut p = reread(ctx, name, measure::timeout_quiet());
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
