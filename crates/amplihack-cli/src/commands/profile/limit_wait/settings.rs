//! The usage-limit wait's settings (issue #1618), from the layered user
//! config (`recipe.usage_limit_wait`, `recipe.usage_limit_max_wait_secs`):
//! the environment variable, then the project and user `config.toml`, then
//! the default. With no explicit setting the wait is on, except in CI (a
//! non-empty `CI` other than `false` or `0`), where it is off: a CI job has
//! no rotator or second login to wait for, and its own timeout is the
//! better bound. Setting the key in a user or project file, or the
//! variable, turns it back on there.
//!
//! `recipe.usage_limit_max_wait_secs` is the length of *one* wait, not a
//! budget for the whole step (issue #2911). A step that meets limit after
//! limit waits each of them out; nothing counts the waits or ends the step
//! when a total is reached. A configured value below
//! [`plan::MIN_WAIT_SECS`] is raised to it, which is what holds the pacing
//! rule for every configured value rather than only for zero: the wait loop
//! polls every 30 seconds, so a shorter wait has no wake condition that
//! could end it and would simply return. A zero therefore no longer means
//! "do not wait"; `recipe.usage_limit_wait = false` is how to turn the wait
//! off.
//!
//! The floor is applied here, where the value is read, and not in the
//! planners, so that there is one place to look and `lower_only` keeps
//! working: a filter that dropped a zero instead would silently raise a
//! project's configured value to the 21 600-second default.
//!
//! Raising it is not silent. Before #2911 a `0` meant "fail at once", so a
//! host may still carry one that now means a 60-second wait. While the wait
//! is on, [`resolve`] says so on stderr each time it reads such a value
//! ([`floor_warning`]), and names the key that turns the wait off.
//!
//! [`plan::MIN_WAIT_SECS`]: super::plan::MIN_WAIT_SECS

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
    let settings = from_effective(&effective, ci.as_deref());
    if let Some(warning) = floor_warning(&effective, &settings) {
        eprintln!("{warning}");
    }
    settings
}

/// The warning [`resolve`] prints when the configured length is below
/// [`plan::MIN_WAIT_SECS`] and was raised to it, or `None`.
///
/// Only while the wait is on: with it off the length is never used, so
/// there is nothing to warn about. The line names the key and the variable,
/// never the configured value, as the invalid-variable warning above does.
///
/// [`plan::MIN_WAIT_SECS`]: super::plan::MIN_WAIT_SECS
pub(crate) fn floor_warning(effective: &Effective, settings: &WaitSettings) -> Option<String> {
    let floor = super::plan::MIN_WAIT_SECS;
    let configured = effective
        .entry(MAX_WAIT_KEY)
        .value
        .as_ref()
        .and_then(Value::as_int)?;
    (settings.enabled && configured < floor).then(|| {
        format!(
            "warning: {MAX_WAIT_KEY} (AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS) is below \
             {floor} seconds, so each usage-limit wait lasts {floor} seconds; 0 no \
             longer turns the wait off: set {ENABLED_KEY} = false \
             (AMPLIHACK_USAGE_LIMIT_WAIT=0) to stop steps waiting"
        )
    })
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
            .and_then(Value::as_int)
            .map(|v| v.max(super::plan::MIN_WAIT_SECS)),
    }
}

/// Whether `CI` names a CI run: set and, trimmed, neither empty, `0` nor
/// `false` in any case.
fn in_ci(ci: Option<&str>) -> bool {
    ci.map(str::trim)
        .is_some_and(|v| !v.is_empty() && v != "0" && !v.eq_ignore_ascii_case("false"))
}
