//! The rotate ledger, `<root>/.auto-switch.d/rotate/ledger.json`, read
//! loosely: only `version`, `live`, `alert` and
//! `last_cycle.{at,measured_at,outcome}` matter, unknown fields are ignored, and a wrongly typed field reads as
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
    /// When that cycle started measuring usage, epoch seconds. Absent for
    /// a cycle that measured nothing, and from rotators older than #1659.
    pub measured_at: Option<i64>,
    /// The last cycle's outcome: `swapped`, `stayed`, `alerted`, …
    pub outcome: Option<String>,
    /// The rotator's no-capacity alert is set.
    pub alert: bool,
    /// The live profile, when it passes the profile-name rule.
    pub live: Option<String>,
}

impl CycleStatus {
    /// The rotator left a login with capacity in place: no alert, and the
    /// last cycle swapped to a login or stayed on one under the threshold.
    /// Any other outcome, known or not, is no capacity.
    pub fn has_capacity(&self) -> bool {
        !self.alert && matches!(self.outcome.as_deref(), Some("swapped" | "stayed"))
    }

    /// [`has_capacity`](Self::has_capacity), from a cycle whose usage probe
    /// began after a limit hit at `start`: `measured_at` later than
    /// `start`. A ledger from an older rotator has no `measured_at`; then
    /// the cycle must have been recorded more than
    /// [`FRESH_MARGIN_SECS`](super::FRESH_MARGIN_SECS) after `start`.
    pub fn fresh_capacity(&self, start: i64) -> bool {
        self.has_capacity()
            && match self.measured_at {
                Some(measured) => measured > start,
                None => self
                    .at
                    .is_some_and(|at| at > start.saturating_add(super::FRESH_MARGIN_SECS)),
            }
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
/// `at` and `measured_at` are kept only when `0 < t <= now + 300`; `live`
/// only when it passes
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
    let time = |key| {
        last.and_then(|c| c.get(key))
            .and_then(Value::as_i64)
            .filter(|t| *t > 0 && *t <= now.saturating_add(MAX_SKEW_SECS))
    };
    let outcome = last
        .and_then(|c| c.get("outcome"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let alert = map.get("alert").is_some_and(|a| !a.is_null());
    let live = map
        .get("live")
        .and_then(Value::as_str)
        .filter(|n| super::valid_live_name(n))
        .map(str::to_string);
    Some(CycleStatus {
        at: time("at"),
        measured_at: time("measured_at"),
        outcome,
        alert,
        live,
    })
}

/// The ledger file under `root`, as the hook sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ledger {
    Missing,
    /// Present but not a private regular file of the current user, larger
    /// than 64 KiB, not JSON, or at a version other than 1.
    Unreadable,
    Read(CycleStatus),
}

/// Reads and parses the ledger under `root`. Creates nothing.
#[cfg(unix)]
pub fn read_ledger(root: &Path, now: i64) -> Ledger {
    use crate::secure_files::{owned_by_euid, read_bounded_nofollow, writable_by_others};
    match read_bounded_nofollow(&ledger_path(root), MAX_LEDGER_BYTES) {
        Ok(None) => Ledger::Missing,
        Ok(Some((raw, meta))) if owned_by_euid(&meta) && !writable_by_others(&meta) => {
            parse_cycle(&raw, now).map_or(Ledger::Unreadable, Ledger::Read)
        }
        _ => Ledger::Unreadable,
    }
}

/// Off Unix there is no rotator.
#[cfg(not(unix))]
pub fn read_ledger(_root: &Path, _now: i64) -> Ledger {
    Ledger::Missing
}

/// The ledger's last cycle; `None` unless [`read_ledger`] read it.
pub fn read_cycle(root: &Path, now: i64) -> Option<CycleStatus> {
    match read_ledger(root, now) {
        Ledger::Read(status) => Some(status),
        Ledger::Missing | Ledger::Unreadable => None,
    }
}
