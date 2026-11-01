//! The `governor.*` keys of the capacity governor (issue #1811). All are
//! user-only, so a repository cannot turn the governor off or loosen it,
//! and none has a resolver variable: the governor reads
//! `AMPLIHACK_GOVERNOR` and `AMPLIHACK_GOVERNOR_WAIT` itself
//! (`commands::governor::config`), and those outrank these keys.

use super::schema::{KeyDef, Scope, ValueKind};

const fn int(name: &'static str, min: u64, max: u64, default: &'static str) -> KeyDef {
    KeyDef {
        name,
        scope: Scope::UserOnly,
        kind: ValueKind::Int { min, max },
        env: None,
        default: Some(default),
        lower_only: false,
    }
}

pub(crate) const GOVERNOR_ENABLED: KeyDef = KeyDef {
    name: "governor.enabled",
    scope: Scope::UserOnly,
    kind: ValueKind::Bool,
    env: None,
    default: Some("true"),
    lower_only: false,
};
pub(crate) const GOVERNOR_MAX_CONCURRENCY: KeyDef = int("governor.max_concurrency", 1, 64, "4");
pub(crate) const GOVERNOR_RESERVE_POINTS: KeyDef = int("governor.reserve_points", 0, 100, "10");
pub(crate) const GOVERNOR_STEP: KeyDef = int("governor.step", 1, 8, "1");
pub(crate) const GOVERNOR_BAND_PCT: KeyDef = int("governor.band_pct", 0, 50, "10");
pub(crate) const GOVERNOR_EVAL_INTERVAL_SECS: KeyDef =
    int("governor.eval_interval_secs", 1, 3600, "60");
pub(crate) const GOVERNOR_WAIT_SECS: KeyDef = int("governor.wait_secs", 0, 86_400, "600");
