//! The production re-reads of the freshness gate (issue #1870) and of the
//! live step (issue #2211): one read by the user's probe method with passive
//! readings off. Issue #2910: each returns the label of a read that said
//! nothing about its login (`measure::label_of`).

use super::super::cycle::Ctx;
use super::super::decide::Probed;
use super::super::measure::{self, ProbeLabel};
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

/// The production re-read: one read of `name` by the user's probe method
/// with passive readings off ([`reread_env`]), resolved without a warning
/// because the cycle's measurement has already given it, bounded by `left`: what
/// remains of the gate's one time limit. At most one `claude` runs for it,
/// under the same deadline. It writes no rotate state and takes no rotate
/// lock; like any direct read it may refresh a near-expiry OAuth token
/// under Claude's `.oauth_refresh.lock`.
pub fn reread_as_configured(
    ctx: &Ctx,
    name: &ProfileName,
    left: Duration,
) -> (Probed, Option<ProbeLabel>) {
    let env = reread_env(measure::mode_quiet(), left);
    let reader = measure::reader(ctx, env, 1, now());
    let p = reader.profile(name, false);
    (measure::reading(name.as_str(), &p), measure::label_of(&p))
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
    let env = reread_env(measure::mode_quiet(), left);
    let reader = measure::reader(ctx, env, 1, now());
    let p = reader.profile(name, true);
    (measure::reading(name.as_str(), &p), measure::label_of(&p))
}
