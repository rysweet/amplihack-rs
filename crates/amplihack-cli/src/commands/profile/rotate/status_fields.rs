//! The `key: value` fields of `rotate status` that are built from the
//! ledger: `overflow:`, `api key:` and `blocked logins:`. Pure formatting,
//! split from `control.rs`, which starts and stops the rotator process, to
//! keep each file within the rotate line budget.

use super::super::ledger::{RotateLedger, active_blocks};
use super::super::log::timestamp;
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::now;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::store::ProfileStore;
use crate::commands::profile::usage_cmd::sanitize_display;
use std::collections::BTreeMap;
use std::path::Path;

/// The `overflow:` value: the marked profiles `names` in list order, each
/// with its active spend block, or `-`. An unreadable ledger (`Err`) adds
/// `(blocks unknown)` rather than showing no blocks.
pub(crate) fn overflow_line(
    names: &[ProfileName],
    ledger: Result<Option<&RotateLedger>, &ProfileError>,
    now: i64,
) -> String {
    if names.is_empty() {
        return "-".into();
    }
    let (blocks, unknown) = match ledger {
        Ok(l) => (l.map(|l| active_blocks(l, now)).unwrap_or_default(), false),
        Err(_) => (BTreeMap::new(), true),
    };
    let line = names
        .iter()
        .map(|n| match blocks.get(n.as_str()) {
            Some(b) => format!("{n} (blocked until {})", timestamp(b.until)),
            None => n.to_string(),
        })
        .collect::<Vec<_>>()
        .join(", ");
    if unknown {
        format!("{line} (blocks unknown)")
    } else {
        line
    }
}

/// The `api key:` line (issue #1700): the API-key profile the tier uses,
/// `(marked)` or `(not marked)` (issue #1804), with the active API-key
/// block, or `-`.
pub(crate) fn api_key_line(
    store: &ProfileStore,
    home: Option<&Path>,
    ledger: Option<&RotateLedger>,
) -> String {
    use crate::commands::profile::overflow::{PLATFORM_TIER, tiers_detail};
    // Issue #1804: the marked one, else the first by name, labelled.
    let marked = tiers_detail(store, home)
        .0
        .into_iter()
        .find(|e| e.tier == PLATFORM_TIER)
        .map(|e| e.name);
    let platforms = crate::commands::profile::kind::Ctx::load(store, home).platform_profiles(store);
    let (first, label) = match (marked, platforms.into_iter().next()) {
        (Some(m), _) => (m, "marked"),
        (None, Some(p)) => (p, "not marked"),
        (None, None) => return "-".into(),
    };
    match ledger.and_then(|l| super::super::platform_block::active(l, now())) {
        Some(b) => format!(
            "{first} ({label}) (blocked until {}, {})",
            timestamp(b.until),
            sanitize_display(&b.reason, false)
        ),
        None => format!("{first} ({label})"),
    }
}

/// The `blocked logins:` value (issue #2890): every login with an active
/// spend block that `overflow_shown` does not already list, in name order,
/// each as `<name> (blocked until <time>)`, joined by `, `; `-` when there
/// are none.
///
/// A step records a block for the login it ran under, which is usually a
/// plan login and so appears in no overflow list. [`overflow_line`]
/// describes the marked overflow profiles and is left exactly as it was, so
/// this field is where such a block is shown.
///
/// An unreadable ledger prints `-` and no `(blocks unknown)` suffix: that
/// suffix belongs to `overflow:`, which reports the same failure from the
/// same read on the same output, and repeating it would say the read failed
/// twice.
pub(crate) fn blocked_logins_line(
    overflow_shown: &[ProfileName],
    ledger: Result<Option<&RotateLedger>, &ProfileError>,
    now: i64,
) -> String {
    let Ok(Some(l)) = ledger else {
        return "-".into();
    };
    let shown = |n: &str| overflow_shown.iter().any(|o| o.as_str() == n);
    // `active_blocks` is a `BTreeMap`, so this is already in name order.
    let line = active_blocks(l, now)
        .into_iter()
        .filter(|(n, _)| !shown(n))
        .filter_map(|(n, b)| {
            ProfileName::parse(&n)
                .ok()
                .map(|n| format!("{n} (blocked until {})", timestamp(b.until)))
        })
        .collect::<Vec<_>>()
        .join(", ");
    if line.is_empty() { "-".into() } else { line }
}
