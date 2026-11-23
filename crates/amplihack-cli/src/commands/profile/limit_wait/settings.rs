//! The usage-limit wait's setting (issue #1618), from the layered user
//! config (`recipe.usage_limit_wait`): the environment variable, then the
//! project and user `config.toml`, then the default. With no explicit
//! setting the wait is on, everywhere.
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
//! There is no setting for how long a wait lasts (issue #2911). A wait for a
//! stated reset ends at that reset; a wait whose limit stated no usable
//! reset ends on a login with room or a login change, and on no timer
//! (`plan`). A length setting would be a timer that runs the step's agent
//! again on a login that may still be spent, so there is none:
//! `recipe.usage_limit_max_wait_secs` is not in the schema, and a config
//! file that carries it is reported as an unknown key by the loader.
//! [`RETIRED_MAX_WAIT_ENV`] is not read either, and a host that still
//! exports it is told so on stderr ([`retired_warning`]).

use crate::user_config::{Effective, Origin, Value};
use std::path::Path;

pub(crate) const ENABLED_KEY: &str = "recipe.usage_limit_wait";

/// The variable that set the removed outer bound. Read only to say that it
/// is no longer read.
pub(crate) const RETIRED_MAX_WAIT_ENV: &str = "AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WaitSettings {
    pub(crate) enabled: bool,
}

/// The settings for a step running in `cwd`, from this process's
/// environment and config files. A bad file value is reported by the
/// loader, a bad variable here.
pub(crate) fn resolve(cwd: &Path) -> WaitSettings {
    let effective = crate::user_config::load_for(cwd);
    // The reason names the variable, never its value.
    if let Origin::EnvInvalid { reason, .. } = &effective.entry(ENABLED_KEY).origin {
        eprintln!("warning: {reason}; ignoring it");
    }
    let settings = from_effective(&effective);
    let exported = std::env::var_os(RETIRED_MAX_WAIT_ENV).is_some_and(|v| !v.is_empty());
    if let Some(warning) = retired_warning(exported, &settings) {
        eprintln!("{warning}");
    }
    settings
}

/// The warning [`resolve`] prints when [`RETIRED_MAX_WAIT_ENV`] is still
/// exported and the wait is on, or `None`.
///
/// A host that set it to make waits short would otherwise not learn that a
/// wait with no reset now lasts until an event. The line names the variable,
/// never its value, and says what ends such a wait instead.
pub(crate) fn retired_warning(exported: bool, settings: &WaitSettings) -> Option<String> {
    (exported && settings.enabled).then(|| {
        format!(
            "warning: {RETIRED_MAX_WAIT_ENV} is no longer read. A usage-limit wait has no \
             timed bound: one whose limit stated a reset ends at that reset, and one whose \
             limit stated none ends when a login has room or the login changes \
             (`amplihack profile switch` makes that change). Set {ENABLED_KEY} = false \
             (AMPLIHACK_USAGE_LIMIT_WAIT=0) to stop steps waiting"
        )
    })
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
    }
}
