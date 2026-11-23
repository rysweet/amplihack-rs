//! The usage-limit wait's settings (issue #1618), from the layered user
//! config (`recipe.usage_limit_wait`, `recipe.usage_limit_max_wait_secs`):
//! the environment variable, then the project and user `config.toml`, then
//! the default. With no explicit setting the wait is on, everywhere.
//!
//! `CI` does not turn it off (issue #2911). It used to, by default, on the
//! reading that a CI job has no rotator or second login to wait for. But a
//! run that meets a limit must not end because of it, and `CI=true` is
//! exported by tools that only want to suppress prompts, not to say that
//! nothing could lift a limit: amplihack's own delegation layer
//! (`amplihack-delegation`, `platform_cli`) sets it for every delegated
//! `claude -p`, so a recipe run started from one had the wait silently off
//! and ended at its first limit. A job that should not wait says so with the
//! key or the variable.
//!
//! `recipe.usage_limit_wait = false` (or `AMPLIHACK_USAGE_LIMIT_WAIT=0`) is
//! then the one setting that lets a limit end a step, and it does so on
//! purpose: the operator asked for the limit result to be passed up at once
//! (`skipped reason=disabled`) rather than waited out. That is configuration,
//! not a limit ending. The run still learns of the limit as a limit: the
//! recipe runner classes the failure `usage_limit` and records the run
//! resumable, so relaunching the same command resumes the step. Nothing in
//! amplihack writes the key or the variable; only an operator does.
//!
//! `recipe.usage_limit_max_wait_secs` is the outer bound of *one* wait whose
//! limit stated no reset still ahead, not a budget for the whole step (issue
//! #2911). A wait whose limit stated a reset ahead does not read it: it ends
//! at that reset, on a login with room or on a login change. A step that
//! meets limit after limit waits each of them out; nothing counts the waits
//! or ends the step when a total is reached. The bound is the one timer that
//! ends a wait, an interim one (`plan` says why it is kept and that #3098
//! decides its future). A configured value below
//! [`plan::MIN_WAIT_SECS`] is raised to it, so that no configured value can
//! make the bound a busy loop of agent launches. A zero therefore no longer
//! means "do not wait"; `recipe.usage_limit_wait = false` is how to turn the
//! wait off.
//!
//! The floor is applied here, where the value is read, and not in the
//! planners, so that there is one place to look and `lower_only` keeps
//! working: a filter that dropped a zero instead would silently raise a
//! project's configured value to the 21 600-second default.
//!
//! Raising it is not silent. Before #2911 a `0` meant "fail at once", so a
//! host may still carry one that now means a 60-second bound. While the wait
//! is on, [`resolve`] says so on stderr each time it reads such a value
//! ([`length_warning`]), and names the key that turns the wait off.
//!
//! The same line reports the cost of any short bound, not only one below
//! the floor. A step still limited when such a wait reaches the bound runs
//! its agent again, so the value *is* the interval at which a step whose
//! limit stated no reset re-asks the harness, and each ask is a fresh agent
//! launch; see [`SHORT_WAIT_SECS`].
//!
//! [`plan::MIN_WAIT_SECS`]: super::plan::MIN_WAIT_SECS

use super::plan::{CAPACITY_DEFAULT_MAX, MIN_WAIT_SECS};
use crate::user_config::{Effective, Origin, Value};
use std::path::Path;

pub(crate) const ENABLED_KEY: &str = "recipe.usage_limit_wait";
pub(crate) const MAX_WAIT_KEY: &str = "recipe.usage_limit_max_wait_secs";

/// A configured length below this is reported as the poll it is.
///
/// Six hundred seconds is the rotator's own default cycle
/// (`--interval`/`AMPLIHACK_ROTATE_INTERVAL`), which on a host with no
/// signal to wake a wait is the soonest anything can have been learnt: a
/// shorter length re-launches the step's agent before a reading, a swap or
/// a reset could have arrived. It bounds nothing - [`MIN_WAIT_SECS`] is the
/// only floor, and a deliberately fast poll stays allowed - but an operator
/// who asks for one is told what it costs, because every expiry starts a
/// fresh agent.
pub(crate) const SHORT_WAIT_SECS: u64 = 600;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WaitSettings {
    pub(crate) enabled: bool,
    pub(crate) max_wait_secs: Option<u64>,
}

/// The settings for a step running in `cwd`, from this process's
/// environment and config files. A bad file value is reported by the
/// loader, a bad variable here.
pub(crate) fn resolve(cwd: &Path) -> WaitSettings {
    let effective = crate::user_config::load_for(cwd);
    for key in [ENABLED_KEY, MAX_WAIT_KEY] {
        // The reason names the variable, never its value.
        if let Origin::EnvInvalid { reason, .. } = &effective.entry(key).origin {
            eprintln!("warning: {reason}; ignoring it");
        }
    }
    let settings = from_effective(&effective);
    if let Some(warning) = length_warning(&effective, &settings) {
        eprintln!("{warning}");
    }
    settings
}

/// The warning [`resolve`] prints for a short configured length, or `None`.
///
/// Two causes, one line. A length below [`MIN_WAIT_SECS`] was raised to it,
/// which a host carrying the pre-#2911 `0` needs to be told. A length below
/// [`SHORT_WAIT_SECS`] is a poll of the harness: the value is the outer bound
/// of a wait whose limit stated no reset still ahead, and a step still
/// limited when that wait ends launches its agent again, so the step re-asks
/// about that often for as long as the limit lasts. Neither is refused.
///
/// Only while the wait is on: with it off the length is never used, so
/// there is nothing to warn about. The line names the keys, the variable and
/// the two lengths it compares, never the configured value, as the
/// invalid-variable warning above does.
pub(crate) fn length_warning(effective: &Effective, settings: &WaitSettings) -> Option<String> {
    let configured = effective
        .entry(MAX_WAIT_KEY)
        .value
        .as_ref()
        .and_then(Value::as_int)?;
    if !settings.enabled || configured >= SHORT_WAIT_SECS {
        return None;
    }
    let cause = if configured < MIN_WAIT_SECS {
        format!(
            "is below {MIN_WAIT_SECS} seconds, so the outer bound of a usage-limit \
             wait is {MIN_WAIT_SECS} seconds; 0 no longer turns the wait off"
        )
    } else {
        format!("is below {SHORT_WAIT_SECS} seconds, the rotator's own cycle")
    };
    Some(format!(
        "warning: {MAX_WAIT_KEY} (AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS) {cause}. It is \
         the outer bound of a wait whose limit stated no reset still ahead, and a \
         step still limited when such a wait ends runs its agent again, so this is \
         how often such a step re-asks; the default of {CAPACITY_DEFAULT_MAX} seconds \
         asks once every six hours. A wait for a stated reset is not shortened by \
         it. Set {ENABLED_KEY} = false (AMPLIHACK_USAGE_LIMIT_WAIT=0) to stop steps \
         waiting"
    ))
}

/// [`resolve`] from a loaded config. Only an explicit `false` turns the
/// wait off; `CI` is not read (see the module comment).
pub(crate) fn from_effective(effective: &Effective) -> WaitSettings {
    WaitSettings {
        enabled: effective
            .entry(ENABLED_KEY)
            .value
            .as_ref()
            .and_then(Value::as_bool)
            .unwrap_or(true),
        max_wait_secs: effective
            .entry(MAX_WAIT_KEY)
            .value
            .as_ref()
            .and_then(Value::as_int)
            .map(|v| v.max(MIN_WAIT_SECS)),
    }
}
