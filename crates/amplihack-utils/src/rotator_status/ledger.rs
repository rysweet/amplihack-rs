//! The rotate ledger, `<root>/.auto-switch.d/rotate/ledger.json`, read
//! loosely: only `version`, `live`, `alert` and
//! `last_cycle.{at,outcome,overflow}` matter, unknown fields are ignored, and a wrongly typed field reads as
//! absent. The rotator's own reader is strict; the hook must keep working
//! across ledger additions (#1656, #1657), so it is not reused.

use serde_json::Value;
use std::path::{Path, PathBuf};

const VERSION: u64 = 1;
const MAX_LEDGER_BYTES: u64 = 64 * 1024;
/// A cycle time this far past `now` is accepted as clock skew; later ones
/// are ignored.
const MAX_SKEW_SECS: i64 = 300;

/// What the hook needs from the ledger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CycleStatus {
    /// When the last cycle was recorded (at its end), epoch seconds.
    pub at: Option<i64>,
    /// The last cycle's outcome: `swapped`, `stayed`, `alerted`, …
    pub outcome: Option<String>,
    /// The rotator's no-capacity alert is set.
    pub alert: bool,
    /// The live profile, when it passes the profile-name rule.
    pub live: Option<String>,
    /// The last cycle's credit-backed overflow tag (issue #1606): `in` (moved
    /// onto an overflow profile), `out` (moved off one) or `on` (stayed on
    /// one). `None` for no overflow activity or any other value.
    pub overflow: Option<String>,
}

impl CycleStatus {
    /// The rotator left a login with capacity in place: no alert, and the
    /// last cycle swapped to a login or stayed on one under the threshold.
    /// Any other outcome, known or not, is no capacity.
    pub fn has_capacity(&self) -> bool {
        !self.alert && matches!(self.outcome.as_deref(), Some("swapped" | "stayed"))
    }

    /// The live login runs on usage credits after the last cycle: it moved
    /// onto an overflow profile or stayed on one. Capacity is still decided
    /// by [`Self::has_capacity`]; this only says what the capacity costs.
    pub fn on_overflow(&self) -> bool {
        matches!(self.overflow.as_deref(), Some("in" | "on"))
    }

    /// [`has_capacity`](Self::has_capacity), from a cycle recorded more than
    /// [`FRESH_MARGIN_SECS`](super::FRESH_MARGIN_SECS) after `start`, so its
    /// usage probe began after a limit hit at `start`.
    pub fn fresh_capacity(&self, start: i64) -> bool {
        self.has_capacity()
            && self
                .at
                .is_some_and(|at| at > start.saturating_add(super::FRESH_MARGIN_SECS))
    }
}

/// `<root>/.auto-switch.d/rotate/ledger.json`.
pub fn ledger_path(root: &Path) -> PathBuf {
    root.join(".auto-switch.d")
        .join("rotate")
        .join("ledger.json")
}

/// Parses ledger bytes. `None` for anything over 64 KiB, anything that is
/// not a JSON object, or a `version` other than the integer 1.
///
/// `at` is kept only when `0 < at <= now + 300`; `live` only when it passes
/// [`super::valid_live_name`]. `alert` is set by any non-null value, so a
/// wrongly typed alert still means no capacity.
pub fn parse_cycle(raw: &[u8], now: i64) -> Option<CycleStatus> {
    if raw.len() as u64 > MAX_LEDGER_BYTES {
        return None;
    }
    let value: Value = serde_json::from_slice(raw).ok()?;
    let map = value.as_object()?;
    if map.get("version").and_then(Value::as_u64) != Some(VERSION) {
        return None;
    }
    let last = map.get("last_cycle").and_then(Value::as_object);
    let at = last
        .and_then(|c| c.get("at"))
        .and_then(Value::as_i64)
        .filter(|at| *at > 0 && *at <= now.saturating_add(MAX_SKEW_SECS));
    let outcome = last
        .and_then(|c| c.get("outcome"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let overflow = last
        .and_then(|c| c.get("overflow"))
        .and_then(Value::as_str)
        .filter(|t| matches!(*t, "in" | "out" | "on"))
        .map(str::to_string);
    let alert = map.get("alert").is_some_and(|a| !a.is_null());
    let live = map
        .get("live")
        .and_then(Value::as_str)
        .filter(|n| super::valid_live_name(n))
        .map(str::to_string);
    Some(CycleStatus {
        at,
        outcome,
        alert,
        live,
        overflow,
    })
}

/// Reads and parses the ledger under `root`. `None` when it is missing,
/// not a regular file, a symlink, owned by another user, writable by other
/// users, larger than 64 KiB or unparseable. Creates nothing.
#[cfg(unix)]
pub fn read_cycle(root: &Path, now: i64) -> Option<CycleStatus> {
    use crate::secure_files::{owned_by_euid, read_bounded_nofollow, writable_by_others};
    match read_bounded_nofollow(&ledger_path(root), MAX_LEDGER_BYTES) {
        Ok(Some((raw, meta))) if owned_by_euid(&meta) && !writable_by_others(&meta) => {
            parse_cycle(&raw, now)
        }
        _ => None,
    }
}

/// Off Unix there is no rotator: no capacity.
#[cfg(not(unix))]
pub fn read_cycle(_root: &Path, _now: i64) -> Option<CycleStatus> {
    None
}
