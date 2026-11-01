//! Why auto-switch moved work onto a per-token profile (issue #1804): the
//! rotator's `per-token` line, for a nested launch, a rerun or a status-line
//! swap. It goes to the rotate log, beside the rotator's own lines, as well
//! as to stderr, which for a recipe step ends with the step.

use super::candidate::{Pool, earliest_plan_reset, unread_plan};
use super::config::Config;
use super::overflow_inputs::OverflowInputs;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::overflow::PLATFORM_TIER;
use crate::commands::profile::rotate::{self, CheckNote};
use std::path::Path;

/// The reason line for a move from `current` onto `to`, or `None` when `to`
/// is a plan profile. A move off the API key passed no wait, as in step 3 of
/// `candidate::select_with_overflow`; any other passed step 5's.
pub fn reason(
    pool: &Pool<'_>,
    inputs: &OverflowInputs,
    cfg: &Config,
    current: &str,
    to: &ProfileName,
    check: CheckNote,
) -> Option<String> {
    if !inputs.order.contains(to) {
        return None;
    }
    let tier = if inputs.platform.as_ref() == Some(to) {
        PLATFORM_TIER
    } else {
        cfg.overflow
            .iter()
            .find(|e| e.name == *to)
            .map_or(amplihack_utils::rotator_status::DEFAULT_TIER, |e| e.tier)
    };
    let line = if inputs
        .platform
        .as_ref()
        .is_some_and(|k| k.as_str() == current)
    {
        rotate::per_token_off_key(to, tier, check)
    } else {
        let ov = inputs.pool();
        let earliest = ov
            .gate
            .and_then(|g| earliest_plan_reset(pool, &ov, &g, current));
        rotate::per_token_reason(to, tier, earliest, unread_plan(pool, &ov), check)
    };
    Some(format!("{line}; auto-switch from {current}"))
}

/// Writes `line`, if there is one, to stderr and to the rotate log.
pub fn record(root: &Path, line: Option<&str>) {
    if let Some(line) = line {
        eprintln!("amplihack: {line}");
        rotate::append_to_log(root, line);
    }
}
