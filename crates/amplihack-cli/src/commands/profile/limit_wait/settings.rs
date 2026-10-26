//! The usage-limit wait's settings (issue #1618), from the layered user
//! config (`recipe.usage_limit_wait`, `recipe.usage_limit_max_wait_secs`):
//! the environment variable, then the project and user `config.toml`, then
//! the default. With no explicit setting the wait is on, except in CI (a
//! non-empty `CI` other than `false` or `0`), where it is off: a CI job has
//! no rotator or second login to wait for, and its own timeout is the
//! better bound. Setting the key in a user or project file, or the
//! variable, turns it back on there.

use crate::user_config::{Effective, Origin, Value};
use std::path::Path;

pub(crate) const ENABLED_KEY: &str = "recipe.usage_limit_wait";
pub(crate) const MAX_WAIT_KEY: &str = "recipe.usage_limit_max_wait_secs";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WaitSettings {
    pub(crate) enabled: bool,
    pub(crate) max_wait_secs: Option<u64>,
    /// Off only because this is CI, for the `skipped` reason.
    pub(crate) off_in_ci: bool,
}

/// The settings for a step running in `cwd`, from this process's
/// environment and config files. A bad file value is reported by the
/// loader, a bad variable here.
pub(crate) fn resolve(cwd: &Path) -> WaitSettings {
    let ci = std::env::var("CI").ok();
    let effective = crate::user_config::load_for(cwd);
    for key in [ENABLED_KEY, MAX_WAIT_KEY] {
        // The reason names the variable, never its value.
        if let Origin::EnvInvalid { reason, .. } = &effective.entry(key).origin {
            eprintln!("warning: {reason}; ignoring it");
        }
    }
    from_effective(&effective, ci.as_deref())
}

/// [`resolve`] from a loaded config and the value of `CI`.
pub(crate) fn from_effective(effective: &Effective, ci: Option<&str>) -> WaitSettings {
    let enabled = effective.entry(ENABLED_KEY);
    let explicit = matches!(
        enabled.origin,
        Origin::Env(_) | Origin::Project(_) | Origin::User(_) | Origin::Legacy(_)
    );
    let on = enabled
        .value
        .as_ref()
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let off_in_ci = on && !explicit && in_ci(ci);
    WaitSettings {
        enabled: on && !off_in_ci,
        off_in_ci,
        max_wait_secs: effective
            .entry(MAX_WAIT_KEY)
            .value
            .as_ref()
            .and_then(Value::as_int),
    }
}

/// Whether `CI` names a CI run: set, non-empty, and not `false` or `0`.
fn in_ci(ci: Option<&str>) -> bool {
    ci.is_some_and(|v| !v.is_empty() && v != "false" && v != "0")
}
