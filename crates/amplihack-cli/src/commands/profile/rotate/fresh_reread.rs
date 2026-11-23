//! The production re-reads of the freshness gate (issue #1870) and of the
//! live step (issue #2211): one read by the user's probe method with passive
//! readings off. Issue #2910: each returns the label of a read that said
//! nothing about its login (`measure::label_of`).

use super::super::cycle::Ctx;
use super::super::decide::last_good::{LAST_GOOD_MAX_AGE_SECS, LastGood};
use super::super::decide::target_confirm::Unconfirmed;
use super::super::decide::target_seen::Extra;
use super::super::decide::{Probed, Reading};
use super::super::fresh_msg;
use super::super::log::Shown;
use super::super::measure::{self, ProbeLabel};
use super::super::swap_back;
use crate::commands::profile::auto_switch::kit::now;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::probe_limits::ProbeMode;
use crate::commands::profile::usage_source::SourceEnv;
use std::time::Duration;

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
pub(in crate::commands::profile::rotate) fn reread_env(
    mode: ProbeMode,
    left: Duration,
) -> SourceEnv {
    let mut env = SourceEnv::from_env(mode, left);
    env.passive_max_age = 0;
    env
}

/// The settings of both production re-reads: [`reread_env`] with the probe
/// method `AMPLIHACK_PROFILE_USAGE_PROBE` names in the process environment
/// now. It is resolved without a warning (`measure::mode_quiet`), because
/// the cycle's measurement has already given one for a bad value, and no
/// mode is forced: a bad or empty value reads in the default order, as it
/// does in the measurement.
pub(in crate::commands::profile::rotate) fn configured_env(left: Duration) -> SourceEnv {
    reread_env(measure::mode_quiet(), left)
}

/// One read of `name` with [`configured_env`]: its live login in `~/.claude`
/// when `live`, else its profile directory.
fn reread_configured(
    ctx: &Ctx,
    name: &ProfileName,
    left: Duration,
    live: bool,
) -> (Probed, Option<ProbeLabel>) {
    let reader = measure::reader(ctx, configured_env(left), 1, now());
    let p = reader.profile(name, live);
    (measure::reading(name.as_str(), &p), measure::label_of(&p))
}

/// The production re-read: one read of `name` by the user's probe method
/// with passive readings off ([`configured_env`]), bounded by `left`: what
/// remains of the gate's one time limit. At most one `claude` runs for it,
/// under the same deadline. It writes no rotate state and takes no rotate
/// lock; like any direct read it may refresh a near-expiry OAuth token
/// under Claude's `.oauth_refresh.lock`.
pub fn reread_as_configured(
    ctx: &Ctx,
    name: &ProfileName,
    left: Duration,
) -> (Probed, Option<ProbeLabel>) {
    reread_configured(ctx, name, left, false)
}

/// Issue #2211: the production re-read of the live login `name`. Like
/// [`reread_as_configured`], but it reads the login in `~/.claude` (under
/// `ctx.home`), as the cycle's measurement reads the live login, never the
/// profile directory's copy, which is stale or missing while the login is
/// live. A live read never refreshes the token (SR-3(d) in
/// `usage_cmd::RowReader::plan`; only the rotate prelude does, #1753) and
/// writes nothing.
pub fn reread_live_as_configured(
    ctx: &Ctx,
    name: &ProfileName,
    left: Duration,
) -> (Probed, Option<ProbeLabel>) {
    reread_configured(ctx, name, left, true)
}

/// The probe that replaces `old` after a re-read returned `p`, and its log
/// lines. A failed read is unknown; a successful one is read now.
pub(super) fn fresh_probe(
    ctx: &Ctx,
    to: &ProfileName,
    old: &Probed,
    mut p: Probed,
    damped: bool,
    lines: &mut Vec<String>,
) -> Probed {
    // Issue #2910: a spend limit is what the login reported, not a failed read.
    let spent = p.spend_limit && !p.unknown && !p.still_running && !p.carried;
    if !spent && (p.reading == Reading::Failed || p.unknown || p.still_running || p.carried) {
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

/// Issue #2820: when the reading the decision used for a target was taken.
/// For a failed probe that is the last good reading standing in for it
/// (`sub`), and nothing without one; nothing for a read still running or
/// unknown; a carried result's own time, which it may not have; otherwise
/// the passive reading's time, or `measured_at` for a read made in this
/// cycle.
pub(in crate::commands::profile::rotate) fn taken_at(
    p: &Probed,
    measured_at: i64,
    sub: Option<&LastGood>,
) -> Option<i64> {
    if p.reading == Reading::Failed {
        return sub.map(|lg| lg.at);
    }
    if p.still_running || p.unknown {
        return None;
    }
    if p.carried {
        return p.observed_at;
    }
    Some(p.observed_at.unwrap_or(measured_at))
}

/// Issue #2820: whether a reading taken at `at` may stand for a move off a
/// live login with no room, `t` now: it has a time, is at most
/// [`LAST_GOOD_MAX_AGE_SECS`] old and not in the future, and is later than
/// `left_or_damped`, when the target was last left or damped, so the
/// rotator never swaps straight back onto a login on a reading from before
/// it left it.
pub(in crate::commands::profile::rotate) fn recent(
    at: Option<i64>,
    left_or_damped: Option<i64>,
    t: i64,
) -> bool {
    at.is_some_and(|at| {
        t.saturating_sub(LAST_GOOD_MAX_AGE_SECS) <= at
            && at <= t
            && left_or_damped.is_none_or(|d| at > d)
    })
}

/// Issue #2820: records `to` as accepted by the gate without a fresh
/// reading, for `why`, so a later gate of the cycle accepts it again while
/// the live login is still blocked; returns the cycle's `Seen::unconfirmed`.
pub(super) fn accept(
    extra: &mut Extra,
    to: ProfileName,
    why: Unconfirmed,
) -> (ProfileName, Unconfirmed) {
    extra.unconfirmed.insert(to.clone(), why);
    (to, why)
}
