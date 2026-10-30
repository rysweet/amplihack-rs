//! The usage-limit wait's settings (issues #1618, #1761), from the layered
//! user config (`recipe.usage_limit_wait`, `recipe.usage_limit_max_wait_secs`,
//! `recipe.usage_limit_wait_budget_secs`,
//! `recipe.usage_limit_max_waits_per_step`): the environment variable, then
//! the project and user `config.toml`, then the default. With no explicit
//! setting the wait is on, except in CI (a non-empty `CI` other than `false`
//! or `0`), where it is off: a CI job has no rotator or second login to wait
//! for, and its own timeout is the better bound. Setting the key in a user
//! or project file, or the variable, turns it back on there.

use crate::user_config::{Effective, Origin, Value};
use std::path::Path;

pub(crate) const ENABLED_KEY: &str = "recipe.usage_limit_wait";
pub(crate) const MAX_WAIT_KEY: &str = "recipe.usage_limit_max_wait_secs";
pub(crate) const BUDGET_KEY: &str = "recipe.usage_limit_wait_budget_secs";
pub(crate) const MAX_WAITS_KEY: &str = "recipe.usage_limit_max_waits_per_step";
/// Used only when the loader gives no value; equal to the schema defaults.
pub(crate) const DEFAULT_BUDGET_SECS: u64 = 21_600;
pub(crate) const DEFAULT_MAX_WAITS_PER_STEP: u32 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WaitSettings {
    pub(crate) enabled: bool,
    pub(crate) max_wait_secs: Option<u64>,
    /// Off only because this is CI, for the `skipped` reason.
    pub(crate) off_in_ci: bool,
    /// Seconds all waits of one recipe run may take together.
    pub(crate) budget_secs: u64,
    /// Waits one step may take while a rotator runs.
    pub(crate) max_waits_per_step: u32,
}

/// The settings for a step running in `cwd`, from this process's
/// environment and config files. A bad file value is reported by the
/// loader, a bad variable here.
pub(crate) fn resolve(cwd: &Path) -> WaitSettings {
    let ci = std::env::var("CI").ok();
    let effective = crate::user_config::load_for(cwd);
    for key in [ENABLED_KEY, MAX_WAIT_KEY, BUDGET_KEY, MAX_WAITS_KEY] {
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
    let int = |key: &str| effective.entry(key).value.as_ref().and_then(Value::as_int);
    WaitSettings {
        enabled: on && !off_in_ci,
        off_in_ci,
        max_wait_secs: int(MAX_WAIT_KEY),
        budget_secs: int(BUDGET_KEY).unwrap_or(DEFAULT_BUDGET_SECS),
        max_waits_per_step: int(MAX_WAITS_KEY)
            .and_then(|v| u32::try_from(v).ok())
            .unwrap_or(DEFAULT_MAX_WAITS_PER_STEP),
    }
}

/// Whether `CI` names a CI run: set and, trimmed, neither empty, `0` nor
/// `false` in any case.
fn in_ci(ci: Option<&str>) -> bool {
    ci.map(str::trim)
        .is_some_and(|v| !v.is_empty() && v != "0" && !v.eq_ignore_ascii_case("false"))
}
