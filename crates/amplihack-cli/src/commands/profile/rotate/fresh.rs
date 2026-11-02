//! Issue #1870: the freshness gate. A swap target chosen by its reading is
//! accepted only when that reading is fresh: taken no more than
//! [`MAX_READING_AGE_SECS`] ago, not earlier than the live login's reading,
//! and, for a login in the ledger's `swapped_away`, strictly after the swap
//! away from it. A stale target is read again directly and the decision is
//! made again on the new figures. See "Rotate reading freshness and
//! swap-back damping" in docs/reference/profile-command.md.
//!
//! The gate writes nothing. Its log lines go to
//! [`Measurement::fresh_lines`], which the cycle writes.

use super::cycle::Ctx;
use super::decide::{Decision, Probed, Reading, Reason};
use super::fresh_msg;
use super::log::Shown;
use super::measure;
use super::observe::{Measurement, Snap, decide_measured};
use super::swap_back;
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::now;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::probe_limits::ProbeMode;
use crate::commands::profile::usage_source::SourceEnv;

/// The oldest a target's reading may be. It equals the default passive
/// age (`DEFAULT_PASSIVE_MAX_AGE_SECS`) and the usage history's limit, so a
/// passive reading accepted by default is never too old on age alone.
pub const MAX_READING_AGE_SECS: i64 = 300;

/// The most re-reads one cycle makes.
pub const MAX_REREADS_PER_CYCLE: usize = 3;

/// Reads one candidate again, directly; [`reread_direct`] outside tests.
pub type RereadFn = fn(&Ctx, &ProfileName) -> Probed;

/// Whether the gate may read a stale target again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reread {
    /// Unlocked: read it again.
    Allowed,
    /// Under the lock (revalidation): never read; a stale target is unknown.
    Never,
}

/// When `p` was read: its `observed_at` for a passive reading, else
/// `measured_at` for a read made in this cycle. `None` for a result carried
/// from an earlier cycle and for a still-running, unknown or failed probe.
pub fn read_at(p: &Probed, measured_at: i64) -> Option<i64> {
    if p.carried || p.still_running || p.unknown || p.reading == Reading::Failed {
        return None;
    }
    Some(p.observed_at.unwrap_or(measured_at))
}

/// Whether a reading taken at `read_at` is stale `now`: no reading time,
/// more than [`MAX_READING_AGE_SECS`] old, strictly older than the live
/// reading (`None` when the live probe failed), or, for a damped login, not
/// strictly later than `damped_at`.
pub fn stale(
    read_at: Option<i64>,
    live_read_at: Option<i64>,
    damped_at: Option<i64>,
    now: i64,
) -> bool {
    let Some(at) = read_at else {
        return true;
    };
    now.saturating_sub(at) > MAX_READING_AGE_SECS
        || live_read_at.is_some_and(|live| at < live)
        || damped_at.is_some_and(|d| at <= d)
}

/// The target of a swap chosen by reading; `None` for `overflow-in`, whose
/// target is chosen by tier, and for a stay or an ALERT.
pub fn gated_target(d: &Decision) -> Option<&ProfileName> {
    match d {
        Decision::Swap { to, reason } => match reason {
            Reason::Threshold
            | Reason::Upgrade
            | Reason::PlanRoom
            | Reason::FailedProbe
            | Reason::OverflowOut => Some(to),
            Reason::OverflowIn => None,
        },
        Decision::Stay | Decision::Alert => None,
    }
}

/// Applies the gate to `m`, decided on `snap`. While the chosen target is
/// stale it is read again (with [`Reread::Allowed`] and a hook), or made
/// unknown, and the decision is made again. Each name is handled at most
/// once and at most [`MAX_REREADS_PER_CYCLE`] reads are made, so the loop
/// ends. The failed live probe count is unchanged. A ledger that cannot be
/// read is an error, never an empty damping map.
pub fn gate(
    ctx: &Ctx,
    snap: &Snap,
    mut m: Measurement,
    mode: Reread,
) -> Result<Measurement, ProfileError> {
    if gated_target(&m.decision).is_none() {
        return Ok(m);
    }
    // A stub that built the measurement by hand kept no probes, so there is
    // nothing to judge or decide again on.
    let Some((live, mut probes)) = m.probed.clone() else {
        return Ok(m);
    };
    let damped = ctx
        .rs
        .load_ledger()?
        .map(|l| l.swapped_away)
        .unwrap_or_default();
    let live_at = read_at(&live, m.measured_at);
    let mut lines = std::mem::take(&mut m.fresh_lines);
    let mut done: Vec<ProfileName> = Vec::new();
    let mut reads = 0;
    while let Some(to) = gated_target(&m.decision).cloned() {
        let Some(i) = probes.iter().position(|(n, _)| n == &to) else {
            break;
        };
        let damped_at = damped.get(to.as_str()).copied();
        let t = now();
        if !stale(read_at(&probes[i].1, m.measured_at), live_at, damped_at, t) {
            break;
        }
        let new = if done.contains(&to) || mode == Reread::Never {
            // Under the lock, or a re-read that is still not strictly later
            // than the damping time (a clock step): unknown, with no read.
            Probed::failed()
        } else if reads >= MAX_REREADS_PER_CYCLE {
            lines.push(fresh_msg::limit(&to));
            Probed::failed()
        } else if let Some(reread) = ctx.reread {
            reads += 1;
            let p = reread(ctx, &to);
            fresh_probe(ctx, &to, &probes[i].1, p, damped_at.is_some(), &mut lines)
        } else {
            // No hook: nothing may be read, so the target is unknown.
            Probed::failed()
        };
        done.push(to);
        probes[i].1 = new;
        let again = decide_measured(ctx, snap, live.clone(), probes.clone(), m.measured_at)?;
        debug_assert_eq!(
            again.failed, m.failed,
            "the gate counts the live probe once"
        );
        m = again;
    }
    m.fresh_lines = lines;
    Ok(m)
}

/// The probe that replaces `old` after a re-read returned `p`, and its log
/// lines. A failed read is unknown; a successful one is read now.
fn fresh_probe(
    ctx: &Ctx,
    to: &ProfileName,
    old: &Probed,
    mut p: Probed,
    damped: bool,
    lines: &mut Vec<String>,
) -> Probed {
    if p.reading == Reading::Failed || p.unknown || p.still_running {
        lines.push(fresh_msg::failed(to));
        return Probed::failed();
    }
    // The reading time of a re-read is when it finished.
    p.observed_at = Some(now());
    p.carried = false;
    lines.push(fresh_msg::stale(to, &Shown::of(old), &Shown::of(&p)));
    if damped && !swap_back::room(&p.reading, &ctx.settings) {
        lines.push(fresh_msg::still_damped(to));
    }
    p
}

/// The production re-read: one direct read of `name`, with no passive
/// reading and no `claude` fallback, bounded by the probe deadline. It
/// writes nothing and starts no child process.
pub fn reread_direct(ctx: &Ctx, name: &ProfileName) -> Probed {
    let mut env = SourceEnv::from_env(ProbeMode::Direct, measure::timeout());
    env.passive_max_age = 0;
    let reader = measure::reader(ctx, env, 1, now());
    measure::reading(name.as_str(), &reader.profile(name, false))
}
