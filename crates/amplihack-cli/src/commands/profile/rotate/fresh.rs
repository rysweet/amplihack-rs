//! Issue #1870: the freshness gate. A swap target chosen by its reading is
//! accepted only when that reading is fresh: taken no more than
//! [`MAX_READING_AGE_SECS`] ago, not earlier than the live login's reading,
//! and, for a login in the ledger's `swapped_away` or the ledger's
//! `previous` (the login the last swap left, issue #2211), strictly after the
//! swap away from it. A stale target is read again, by the user's probe method
//! (`AMPLIHACK_PROFILE_USAGE_PROBE`) with passive readings off, and the
//! decision is made again on the new figures. See "Rotate reading freshness and
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
use std::time::{Duration, Instant};

#[path = "live_fresh.rs"]
pub(super) mod live;

/// The oldest a target's reading may be. It equals the default passive
/// age (`DEFAULT_PASSIVE_MAX_AGE_SECS`) and the usage history's limit, so a
/// passive reading accepted by default is never too old on age alone.
pub const MAX_READING_AGE_SECS: i64 = 300;

/// The most re-reads one cycle makes.
pub const MAX_REREADS_PER_CYCLE: usize = 3;

/// Reads one candidate again, within the given deadline;
/// [`reread_as_configured`] outside tests. The deadline is what is left of the
/// gate's one time limit (see [`gate`]), never a fresh probe deadline per
/// read.
pub type RereadFn = fn(&Ctx, &ProfileName, Duration) -> Probed;

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
/// stale it is read again (with [`Reread::Allowed`] and a hook), or made
/// unknown, and the decision is made again. Each name is read at most once
/// and made unknown at most once, and at most [`MAX_REREADS_PER_CYCLE`]
/// reads are made. The loop is capped at `2n + 1` passes for `n` probes; a
/// decision that still names a stale target at the cap is an error
/// ([`ProfileError::GateUnsettled`]), so no swap is made. The failed live
/// probe count is unchanged. A ledger that cannot be read is an error, never
/// an empty damping map. A stop request seen before a re-read ends the loop
/// with no read and [`Decision::Stay`], so nothing is swapped.
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
    settle(
        ctx,
        m,
        mode,
        measure::timeout_quiet(),
        |live, probes, at| decide_measured(ctx, snap, live, probes, at),
    )
}

/// The other logins' probes, by name.
type Probes = Vec<(ProfileName, Probed)>;

/// [`gate`] with the re-reads' total time limit and the decision passed in
/// ([`measure::timeout_quiet`] and [`decide_measured`] outside tests), so a test
/// can check the cap and the time limit.
pub(super) fn settle(
    ctx: &Ctx,
    mut m: Measurement,
    mode: Reread,
    budget: Duration,
    mut decide: impl FnMut(Probed, Probes, i64) -> Result<Measurement, ProfileError>,
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
    // Issue #2211: the login the last swap left, and when. Its reading must
    // be taken after it stopped being used, whether or not it was full.
    let left = ledger
        .as_ref()
        .and_then(|l| Some((l.previous.clone()?, l.swapped_at?)));
    let damped = ledger.map(|l| l.swapped_away).unwrap_or_default();
    let live_at = read_at(&live, m.measured_at);
    let mut lines = std::mem::take(&mut m.fresh_lines);
    let mut done: Vec<ProfileName> = Vec::new();
    // Targets whose re-read logged new figures. If such a reading is still
    // stale on the next pass, the target is made unknown with its own line,
    // so the log does not read as if the re-read had been accepted.
    let mut accepted: Vec<ProfileName> = Vec::new();
    let mut reads = 0;
    let started = Instant::now();
    // Each name is handled at most twice: read (or made unknown), then made
    // unknown. One more pass finds the settled decision.
    let max_handled = 2 * probes.len();
    let mut handled = 0;
    while let Some(to) = gated_target(&m.decision).cloned() {
        let Some(i) = probes.iter().position(|(n, _)| n == &to) else {
            break;
        };
        let damped_at = damped.get(to.as_str()).copied();
        let left_at = left
            .as_ref()
            .filter(|(from, _)| from == to.as_str())
            .map(|(_, at)| *at);
        let t = (ctx.now)();
        let after = damped_at.max(left_at);
        if !stale(read_at(&probes[i].1, m.measured_at), live_at, after, t) {
            break;
        }
        if handled >= max_handled {
            return Err(ProfileError::GateUnsettled);
        }
        handled += 1;
        let new = if done.contains(&to) || mode == Reread::Never {
            // Under the lock, or a re-read that is still not strictly later
            // than the damping time (a clock step): unknown, with no read.
            if accepted.contains(&to) {
                lines.push(fresh_msg::failed(&to));
            }
            Probed::failed()
        } else if reads >= MAX_REREADS_PER_CYCLE {
            lines.push(fresh_msg::limit(&to));
            Probed::failed()
        } else if let Some(reread) = ctx.reread {
            if ctx.stopping() {
                // `rotate stop`: make no read, which may take a probe
                // timeout, and no swap. The cycle sees the stop next.
                m.decision = Decision::Stay;
                break;
            }
            let left = budget.saturating_sub(started.elapsed());
            if left.is_zero() {
                lines.push(fresh_msg::time_limit(&to));
                Probed::failed()
            } else {
                reads += 1;
                let p = reread(ctx, &to, left);
                let p = fresh_probe(ctx, &to, &probes[i].1, p, damped_at.is_some(), &mut lines);
                if p != Probed::failed() {
                    accepted.push(to.clone());
                }
                p
            }
        } else {
            // No hook: nothing may be read, so the target is unknown.
            Probed::failed()
        };
        if !done.contains(&to) {
            done.push(to);
        }
        probes[i].1 = new;
        let again = decide(live.clone(), probes.clone(), m.measured_at)?;
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
    if p.reading == Reading::Failed || p.unknown || p.still_running || p.carried {
        lines.push(fresh_msg::failed(to));
        return Probed::failed();
    }
    // Preserve the evidence time. Only an undated direct result receives
    // the completion time; the next gate pass validates dated results.
    p.observed_at.get_or_insert_with(ctx.now);
    p.carried = false;
    // Issue #1970: a re-read is made with passive readings off, so it is a
    // direct reading and never checked as a stale cached one.
    p.passive = false;
    lines.push(fresh_msg::stale(to, &Shown::of(old), &Shown::of(&p)));
    if damped && !swap_back::room(&p.reading, &ctx.settings) {
        lines.push(fresh_msg::still_damped(to));
    }
    p
}

/// The settings of a re-read: the user's probe method `mode`
/// (`AMPLIHACK_PROFILE_USAGE_PROBE`), with passive readings off and the
/// deadline `left`. So the re-read reads the target the way the cycle's own
/// measurement does, minus the stored reading that made it stale:
///
/// | `mode` | Re-read |
/// |---|---|
/// | unset | direct, then the `claude` fallback (`/usage` text when credits are on) |
/// | `direct` | direct only |
/// | `auto`, `text` | the `claude` fallback only, as named; never a direct read |
///
/// A login marked as overflow is read by `/usage` in every mode but
/// `direct`, as in the measurement.
pub(super) fn reread_env(mode: ProbeMode, left: Duration) -> SourceEnv {
    let mut env = SourceEnv::from_env(mode, left);
    env.passive_max_age = 0;
    env
}

/// The production re-read: one read of `name` by the user's probe method
/// with passive readings off ([`reread_env`]), resolved without a warning
/// because the cycle's measurement has already given it, bounded by `left`: what
/// remains of the gate's one time limit. At most one `claude` runs for it,
/// under the same deadline. It writes no rotate state and takes no rotate
/// lock; like any direct read it may refresh a near-expiry OAuth token
/// under Claude's `.oauth_refresh.lock`.
pub fn reread_as_configured(ctx: &Ctx, name: &ProfileName, left: Duration) -> Probed {
    let env = reread_env(measure::mode_quiet(), left);
    let reader = measure::reader(ctx, env, 1, now());
    measure::reading(name.as_str(), &reader.profile(name, false))
}

/// Issue #2211: the production re-read of the live login `name`. Like
/// [`reread_as_configured`], but it reads the login in `~/.claude` (under
/// `ctx.home`), as the cycle's measurement reads the live login, never the
/// profile directory's copy, which is stale or missing while the login is
/// live. A live read never refreshes the token (SR-3(d) in
/// `usage_cmd::RowReader::plan`; only the rotate prelude does, #1753) and
/// writes nothing.
pub fn reread_live_as_configured(ctx: &Ctx, name: &ProfileName, left: Duration) -> Probed {
    let env = reread_env(measure::mode_quiet(), left);
    let reader = measure::reader(ctx, env, 1, now());
    measure::reading(name.as_str(), &reader.profile(name, true))
}
