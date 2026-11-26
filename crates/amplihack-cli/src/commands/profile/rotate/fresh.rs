//! Issue #1870: the freshness gate. A swap target chosen by its reading is
//! accepted only when that reading is fresh: taken no more than
//! [`MAX_READING_AGE_SECS`] ago, not earlier than the live login's reading
//! (taken as no later than when the cycle started measuring, [`live_time`]),
//! not earlier than the last swap or the start of the previous cycle's
//! measurement (issue #3181), and, for a login in the ledger's `swapped_away`
//! or the ledger's `previous` (the login the last swap left, issue #2211),
//! strictly after the swap away from it. A stale target is read again, by
//! the user's probe method (`AMPLIHACK_PROFILE_USAGE_PROBE`) with passive
//! readings off, and the decision is made again on the new figures. See
//! "Rotate reading freshness and swap-back damping" in docs/reference/profile-command.md.
//!
//! The gate writes nothing; its lines go to [`Measurement::fresh_lines`].

use super::cycle::Ctx;
use super::decide::target::{NoMove, Unfit, fit};
use super::decide::target_confirm::{Facts, Limit, Step, Unconfirmed, Why, after_unknown, next};
use super::decide::target_msg::{blocked, reject};
use super::decide::target_seen::Extra;
use super::decide::{Decision, Measured, Probed, Reading, Reason};
use super::fresh_msg;
use super::measure::{self, ProbeLabel};
use super::observe::{Measurement, Snap, decide_with};
use crate::commands::profile::ProfileError;
use crate::commands::profile::name::ProfileName;
use std::collections::BTreeSet;
use std::time::{Duration, Instant};

#[path = "live_fresh.rs"]
pub(super) mod live;
#[path = "fresh_reread.rs"]
mod reread;
#[cfg(test)]
pub(super) use reread::{configured_env, recent, reread_env, taken_at};
pub use reread::{reread_as_configured, reread_live_as_configured};

/// The oldest a target's reading may be. It equals the default passive
/// age (`DEFAULT_PASSIVE_MAX_AGE_SECS`) and the usage history's limit, so a
/// passive reading accepted by default is never too old on age alone.
pub const MAX_READING_AGE_SECS: i64 = 300;

/// The most re-reads one cycle makes.
pub const MAX_REREADS_PER_CYCLE: usize = 3;

/// Reads one candidate again, within the given deadline;
/// [`reread_as_configured`] outside tests. The deadline is what is left of the
/// gate's one time limit (see [`gate`]), never a fresh probe deadline per
/// read. Issue #2910: with the label of a read that said nothing.
pub type RereadFn = fn(&Ctx, &ProfileName, Duration) -> (Probed, Option<ProbeLabel>);

/// Whether the gate may read a stale target again.
///
/// The mode and [`Ctx`]'s `reread` hook are both needed: the hook says
/// whether this context can read at all (`None` in `Ctx::new`, so tests
/// never reach a live read), and the mode says whether this call may
/// (revalidation runs under the lock on the same context). `Never` takes
/// precedence: no read is made even with a hook. `Allowed` with no hook
/// makes a stale target unknown, with no read and no log line.
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

/// The live reading time the gate compares targets with: the live probe's
/// reading time ([`read_at`]), but never later than `measured_at`, when the
/// cycle started measuring. `None` when the live probe has no reading time.
///
/// The cap is for the issue #2211 live re-read. It runs after the
/// measurement, so it is dated after `measured_at`, while a target read
/// directly in the same measurement is dated `measured_at`. Without the cap
/// every such target would be "older than the live reading" whenever the
/// measurement crossed a second boundary: one more usage read per target,
/// and a false `no-capacity` ALERT when that read failed. A target passive
/// reading taken before the measurement is still older than the live
/// reading, as it is next to a direct live read.
pub fn live_time(live: &Probed, measured_at: i64) -> Option<i64> {
    read_at(live, measured_at).map(|at| at.min(measured_at))
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
            | Reason::OverflowOut
            | Reason::Projected => Some(to),
            Reason::OverflowIn => None,
        },
        Decision::Stay | Decision::Alert => None,
    }
}

/// Applies the gate to `m`, decided on `snap`. While the chosen target is
/// stale, `target_confirm::next` says what to do (issue #2910, D6): read it
/// again (with [`Reread::Allowed`] and a hook) and decide again on the new
/// figures; otherwise reject it with a `reject:` line and decide again, which
/// reaches the next-ranked login. A read that fails, comes back unknown or is
/// older than one already held rejects the target too (issue #3105), unless
/// the live login has a standing block and the target's reading is recent
/// (issue #2820, `target_confirm`). When the decision looked for a target,
/// the gate then writes one `reject:` line for each plan login with a
/// standing block; that makes no read.
/// Each name is read at most once, at most [`MAX_REREADS_PER_CYCLE`] reads and
/// read-less rejections are made, and the loop is capped at `2n + 1` passes
/// for `n` probes; a decision that still names a stale target at the cap is
/// an error ([`ProfileError::GateUnsettled`]), so no swap is made. The failed
/// live probe count is unchanged. A ledger that cannot be read is an error,
/// never an empty damping map. A stop request seen before a re-read ends the
/// loop with no read and [`Decision::Stay`], so nothing is swapped.
///
/// All re-reads of one gate share one time limit: a single probe deadline
/// ([`measure::timeout_quiet`], 120 s by default). Each read gets only what is
/// left of it, and once it is spent the target is unknown with no read, so
/// a slow usage endpoint delays a swap by at most one probe deadline in
/// total, however many re-reads the cycle makes.
///
/// The gate resolves the probe variables with the quiet readers: every gate
/// follows the cycle's own measurement, which has already warned once about
/// a bad `AMPLIHACK_PROFILE_USAGE_PROBE` or `..._TIMEOUT_SECS`.
pub fn gate(
    ctx: &Ctx,
    snap: &Snap,
    m: Measurement,
    mode: Reread,
) -> Result<Measurement, ProfileError> {
    let mut m = settle(
        ctx,
        m,
        mode,
        measure::timeout_quiet(),
        |live, probes, extra, at| decide_with(ctx, snap, live, probes, at, extra),
    )?;
    m.fresh_lines.extend(blocked(&m.seen, &m.decision));
    Ok(m)
}

/// The other logins' probes, by name.
type Probes = Vec<(ProfileName, Probed)>;

/// [`gate`] with the re-reads' total time limit and the decision passed in
/// ([`measure::timeout_quiet`] and [`decide_with`] outside tests), so a test
/// can check the cap and the time limit.
pub(super) fn settle(
    ctx: &Ctx,
    m: Measurement,
    mode: Reread,
    budget: Duration,
    decide: impl FnMut(Probed, Probes, &Extra, i64) -> Result<Measurement, ProfileError>,
) -> Result<Measurement, ProfileError> {
    let started = Instant::now();
    settle_timed(ctx, m, mode, budget, || started.elapsed(), decide)
}

/// [`settle`] with the time spent so far passed in as `spent`, which
/// `settle` answers from the clock (issue #3008). The time limit is a rule
/// about that figure alone: each read gets `budget` less what is spent, and
/// none is started once nothing is left. A test supplies a figure its fake
/// read advances, so it states the rule exactly and waits for nothing.
pub(super) fn settle_timed(
    ctx: &Ctx,
    mut m: Measurement,
    mode: Reread,
    budget: Duration,
    mut spent: impl FnMut() -> Duration,
    mut decide: impl FnMut(Probed, Probes, &Extra, i64) -> Result<Measurement, ProfileError>,
) -> Result<Measurement, ProfileError> {
    if gated_target(&m.decision).is_none() {
        return Ok(m);
    }
    // A stub that built the measurement by hand kept no probes, so there is
    // nothing to judge or decide again on.
    let Some((live, mut probes)) = m.probed.clone() else {
        return Ok(m);
    };
    let ledger = ctx.rs.load_ledger()?;
    // Issue #2211: the login the last swap left is read strictly after that
    // swap. Issue #3181: no target reading is older than the last swap or the
    // start of the previous cycle's measurement, so `bound` is a second less.
    // This cycle's own readings are never older: a swap made while it measured
    // is for the revalidation under the lock.
    let (left, bound) = ledger.as_ref().map_or((None, None), |l| {
        let last = l
            .swapped_at
            .max(l.last_cycle.as_ref().and_then(|c| c.measured_at));
        let bound = last.map(|b| b.min(m.measured_at).saturating_sub(1));
        (l.previous.clone().zip(l.swapped_at), bound)
    });
    let damped = ledger.map(|l| l.swapped_away).unwrap_or_default();
    // Capped at `measured_at`: a live re-read made after the measurement
    // does not make this measurement's own target readings stale.
    let live_at = live_time(&live, m.measured_at);
    let week_max = ctx.settings.target_week_max;
    let mut lines = std::mem::take(&mut m.fresh_lines);
    let mut extra = m.seen.extra.clone();
    let mut done: Vec<ProfileName> = Vec::new();
    // Targets whose re-read logged new figures. If such a reading is still
    // stale on the next pass, the target is made unknown with its own line,
    // so the log does not read as if the re-read had been accepted.
    let mut accepted: Vec<ProfileName> = Vec::new();
    let mut not_tried = BTreeSet::new();
    let (mut attempts, mut rejected_unknown, mut unconfirmed) = (0, false, None);
    // Each name is handled at most twice: read (or made unknown), then made
    // unknown. One more pass finds the settled decision.
    let max_handled = 2 * probes.len();
    let mut handled = 0;
    while let Some(to) = gated_target(&m.decision).cloned() {
        let Some(i) = probes.iter().position(|(n, _)| n == &to) else {
            break;
        };
        let left_at = left.as_ref().filter(|(f, _)| f == to.as_str()).map(|l| l.1);
        let damped_at = damped.get(to.as_str()).copied();
        let after = damped_at.max(left_at).max(bound);
        let t = (ctx.now)();
        if !stale(read_at(&probes[i].1, m.measured_at), live_at, after, t) {
            break;
        }
        if handled >= max_handled {
            return Err(ProfileError::GateUnsettled);
        }
        handled += 1;
        // Issue #2820: a login an earlier gate of this cycle accepted unread
        // is accepted again while the live login is still blocked; else it
        // counts as failed (an unknown one) or unknown with no `reject:` line.
        let known = extra.unconfirmed.get(&to).copied();
        if let Some(why) = known
            && m.seen.live_blocked.is_some()
        {
            unconfirmed = Some((to, why));
            break;
        }
        let known_label = known.and_then(|u| u.label());
        let failed_now = known_label.is_some()
            || (probes[i].1.reading == Reading::Failed && !probes[i].1.carried);
        let mut read = None;
        if !failed_now && (done.contains(&to) || mode == Reread::Never || ctx.reread.is_none()) {
            // Under the lock, with no hook, or a re-read that is still not
            // strictly later than the damping time (a clock step): unknown,
            // with no read.
            if accepted.contains(&to) {
                lines.push(fresh_msg::failed(&to));
            }
            probes[i].1 = Probed::failed();
        } else {
            if !failed_now && ctx.stopping() {
                // `rotate stop`: make no read, which may take a probe
                // timeout, and no swap. The cycle sees the stop next.
                m.decision = Decision::Stay;
                break;
            }
            let remaining = budget.saturating_sub(spent());
            let sub = extra.substitute(&to, &probes[i].1);
            let at = reread::taken_at(&probes[i].1, m.measured_at, sub);
            let facts = Facts {
                failed_now,
                attempts,
                can_read: !remaining.is_zero(),
                live_blocked: m.seen.live_blocked.is_some(),
                label: known_label.or_else(|| extra.labels.get(&to).copied()),
                recent: reread::recent(at, after, t),
            };
            match next(&facts) {
                // Issue #2820: no read, no line of its own, probe unchanged.
                Step::AcceptUnconfirmed(label) => {
                    unconfirmed = Some(reread::accept(&mut extra, to, label.into()));
                    break;
                }
                Step::AcceptStale(limit) => {
                    let age = at.map_or(0, |at| u64::try_from(t - at).unwrap_or(0));
                    let why = Unconfirmed::NotTried { limit, age };
                    unconfirmed = Some(reread::accept(&mut extra, to, why));
                    break;
                }
                Step::Reject(why) => {
                    attempts += 1;
                    rejected_unknown |= matches!(why, Why::Unknown(_));
                    lines.push(reject(&to, &why, week_max));
                    extra.rejected.insert(to.clone());
                    // Unknown for the rest of the cycle, as the arms below make it.
                    probes[i].1 = Probed::failed();
                }
                Step::NotTried(Limit::Count) => {
                    lines.push(fresh_msg::limit(&to));
                    not_tried.insert(to.clone());
                    extra.rejected.insert(to.clone());
                    probes[i].1 = Probed::failed();
                }
                Step::NotTried(Limit::Time) => {
                    // The gate's time budget is spent: unknown for this cycle.
                    lines.push(fresh_msg::time_limit(&to));
                    extra.labels.insert(to.clone(), ProbeLabel::Timeout);
                    extra.rejected.insert(to.clone());
                    probes[i].1 = Probed::failed();
                }
                Step::ReadAgain => {
                    attempts += 1;
                    let (p, label) = match ctx.reread {
                        Some(reread) => reread(ctx, &to, remaining),
                        None => (Probed::failed(), None),
                    };
                    let mut p = reread::fresh_probe(
                        ctx,
                        &to,
                        &probes[i].1,
                        p,
                        damped_at.is_some(),
                        &mut lines,
                    );
                    // Issue #3105: a read that says nothing, or is older than
                    // one held, is never swapped on; one with no label failed
                    // for a reason about the login (S2). #2820: see `after_unknown`.
                    let older = p.observed_at.and_then(|at| extra.older_by(&to, at));
                    if p == Probed::failed() || older.is_some() {
                        let why = match (older, label) {
                            (Some(n), _) => Why::Older(n),
                            (None, Some(l)) => {
                                if let Step::AcceptUnconfirmed(l) = after_unknown(l, &facts) {
                                    unconfirmed = Some(reread::accept(&mut extra, to, l.into()));
                                    break;
                                }
                                extra.labels.insert(to.clone(), l);
                                Why::Unknown(l)
                            }
                            (None, None) => Why::Unfit(Unfit::NoReading),
                        };
                        rejected_unknown |= matches!(why, Why::Unknown(_) | Why::Older(_));
                        lines.push(reject(&to, &why, week_max));
                        extra.rejected.insert(to.clone());
                        p = Probed::failed();
                    } else {
                        accepted.push(to.clone());
                        read = Some(p.clone());
                    }
                    probes[i].1 = p;
                }
            }
        }
        if !done.contains(&to) {
            done.push(to.clone());
        }
        let again = decide(live.clone(), probes.clone(), &extra, m.measured_at)?;
        debug_assert_eq!(
            again.failed, m.failed,
            "the gate counts the live probe once"
        );
        // A fresh reading that moved the decision off its target rejected it.
        if let Some(p) = read
            && gated_target(&again.decision) != Some(&to)
            && let Err(unfit) = fit(&Measured::new(to.clone(), &p), week_max)
        {
            lines.push(reject(&to, &Why::Unfit(unfit), week_max));
        }
        m = again;
    }
    // D10: a cycle left with nothing because every qualifying login's
    // reading was unknown or superseded is a stay with that note, not the
    // no-target line, whatever room the live login has (issue #3105).
    if m.decision == Decision::Alert && rejected_unknown {
        m.seen.why = Some(NoMove::after_rejection(m.seen.why.take()));
    }
    m.seen.extra = extra;
    m.seen.not_tried = not_tried;
    m.seen.unconfirmed = unconfirmed;
    m.fresh_lines = lines;
    Ok(m)
}
