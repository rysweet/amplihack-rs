//! Issue #2013 (D11): the hourly alert limit. Swap refusals, `repair:`
//! lines, `stay: no login with room` lines and a candidate's `trusted login
//! identity unavailable` line are each written at most once an hour per
//! key, with a count of the repeats not written. The state is kept in the
//! rotate ledger's `alert_log`. Pure functions: the time is passed in, read
//! by the caller from the injected clock (`Ctx.clock`), so nothing waits.
//! See "Rotate alert limit" in docs/reference/profile-command.md.

use super::super::ledger::{MAX_ACCOUNT, unprintable};
use super::super::log::timestamp;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The shortest time between two written lines with one key, in seconds.
pub const HOUR: i64 = 3600;
/// The most keys kept; the one seen longest ago goes first.
pub const MAX_ENTRIES: usize = 64;
/// How far ahead of the loading time a stored time may be.
const MAX_AHEAD: i64 = 86_400;

/// One key's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlertSeen {
    /// When the line was last written.
    pub last_logged_at: i64,
    /// When the key last occurred.
    pub seen_at: i64,
    /// Repeats not written since `last_logged_at`.
    pub suppressed: u32,
}

/// Whether to write a line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Note {
    /// Write this line.
    Log(String),
    /// Do not write it; the repeat was counted.
    Suppress,
}

/// Notes one occurrence of `key` at `t`. The first occurrence logs `line`;
/// a repeat less than [`HOUR`] after the line was last written is counted;
/// the first repeat an hour or more later logs `line` with
/// ` (repeated <n> times since <last>)` and starts the count again. A
/// `last_logged_at` later than `t` (a clock step back) logs at once.
pub fn note(map: &mut BTreeMap<String, AlertSeen>, key: &str, t: i64, line: &str) -> Note {
    let out = match map.get_mut(key) {
        None => {
            map.insert(
                key.to_string(),
                AlertSeen {
                    last_logged_at: t,
                    seen_at: t,
                    suppressed: 0,
                },
            );
            Note::Log(line.to_string())
        }
        Some(seen) => {
            seen.seen_at = t;
            if seen.last_logged_at > t {
                seen.last_logged_at = t;
                seen.suppressed = 0;
                Note::Log(line.to_string())
            } else if t.saturating_sub(seen.last_logged_at) < HOUR {
                seen.suppressed = seen.suppressed.saturating_add(1);
                Note::Suppress
            } else {
                let logged = if seen.suppressed == 0 {
                    line.to_string()
                } else {
                    format!(
                        "{line} (repeated {} times since {})",
                        seen.suppressed,
                        timestamp(seen.last_logged_at)
                    )
                };
                seen.last_logged_at = t;
                seen.suppressed = 0;
                Note::Log(logged)
            }
        }
    };
    cap(map);
    out
}

/// Drops the keys not seen since `cycle_started_at`: their cause has gone,
/// so the next occurrence is written at once.
pub fn prune(map: &mut BTreeMap<String, AlertSeen>, cycle_started_at: i64) {
    map.retain(|_, seen| seen.seen_at >= cycle_started_at);
}

/// Keeps at most [`MAX_ENTRIES`] keys, dropping those seen longest ago.
fn cap(map: &mut BTreeMap<String, AlertSeen>) {
    while map.len() > MAX_ENTRIES {
        let Some(oldest) = map
            .iter()
            .min_by(|a, b| a.1.seen_at.cmp(&b.1.seen_at).then_with(|| a.0.cmp(b.0)))
            .map(|(k, _)| k.clone())
        else {
            return;
        };
        map.remove(&oldest);
    }
}

/// Whether a stored key may be kept: non-empty, at most 320 bytes, no
/// control or bidirectional-override characters.
fn valid_key(k: &str) -> bool {
    !k.is_empty() && k.len() <= MAX_ACCOUNT && !k.chars().any(unprintable)
}

/// Whether a stored time may be kept at `now`: not negative and at most a
/// day ahead.
fn valid_time(v: i64, now: i64) -> bool {
    v >= 0 && v <= now.saturating_add(MAX_AHEAD)
}

/// A loaded `alert_log`: entries that are not objects of the right shape,
/// bad keys and bad times are dropped, and only the [`MAX_ENTRIES`] seen
/// most recently are kept. A bad entry never makes the ledger unreadable:
/// at worst a line is written once more.
pub fn load(raw: BTreeMap<String, serde_json::Value>, now: i64) -> BTreeMap<String, AlertSeen> {
    let mut map: BTreeMap<String, AlertSeen> = raw
        .into_iter()
        .filter(|(k, _)| valid_key(k))
        .filter_map(|(k, v)| Some((k, serde_json::from_value::<AlertSeen>(v).ok()?)))
        .filter(|(_, s)| valid_time(s.last_logged_at, now) && valid_time(s.seen_at, now))
        .collect();
    cap(&mut map);
    map
}

/// The ledger's serde form of `alert_log`: read as raw values, so that one
/// bad entry is dropped by [`load`] rather than failing the whole ledger.
pub mod serde_raw {
    use super::AlertSeen;
    use serde::{Deserialize, Deserializer};
    use std::collections::BTreeMap;

    pub fn deserialize<'de, D: Deserializer<'de>>(
        d: D,
    ) -> Result<BTreeMap<String, AlertSeen>, D::Error> {
        // Anything but an object holds no usable entry.
        let raw: BTreeMap<String, serde_json::Value> = match serde_json::Value::deserialize(d)? {
            serde_json::Value::Object(m) => m.into_iter().collect(),
            _ => BTreeMap::new(),
        };
        let now = crate::commands::profile::auto_switch::kit::now();
        Ok(super::load(raw, now))
    }
}
