//! The usage-limit wait's settings (issue #1618), from the layered user
//! config (`recipe.usage_limit_wait`, `recipe.usage_limit_max_wait_secs`):
//! the environment variable, then the project and user `config.toml`, then
//! the default. With no explicit setting the wait is on, except under
//! `CI=true`, where it is off: a CI job has no rotator or second login to
//! wait for, and its own timeout is the better bound.

use crate::user_config::{Effective, Origin, Value};
use std::path::Path;

pub(crate) const ENABLED_KEY: &str = "recipe.usage_limit_wait";
pub(crate) const MAX_WAIT_KEY: &str = "recipe.usage_limit_max_wait_secs";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WaitSettings {
    pub(crate) enabled: bool,
    pub(crate) max_wait_secs: Option<u64>,
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
    WaitSettings {
        enabled: on && (explicit || ci != Some("true")),
        max_wait_secs: effective
            .entry(MAX_WAIT_KEY)
            .value
            .as_ref()
            .and_then(Value::as_int),
    }
}
