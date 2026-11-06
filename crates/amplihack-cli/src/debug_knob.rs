//! Debug-build time limits that tests may raise (issue #1973).
//!
//! A product time limit that a test cannot change makes the test depend on
//! how fast the host is. In debug builds an `AMPLIHACK_TEST_*_SECS`
//! variable may raise such a limit; release builds never read it, so the
//! limit there is always the constant. Callers keep the knob names and
//! these functions under `#[cfg(debug_assertions)]`, so a release binary
//! holds none of the names. Each knob name starts
//! `AMPLIHACK_TEST_`, outside the `AMPLIHACK_ROTATE*` prefix the rotate
//! daemon removes from its environment.

use std::time::Duration;

/// Whole seconds from 1 to 3600 written in ASCII digits, or `default` for
/// anything else. The digits are checked before parsing, so no sign, space
/// or other form is accepted; the raw value is never logged.
pub(crate) fn secs_or(raw: Option<&str>, default: Duration) -> Duration {
    raw.filter(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|s| (1..=3600).contains(s))
        .map_or(default, Duration::from_secs)
}

/// The value of the variable `name` read by [`secs_or`], or `default`.
pub(crate) fn secs(name: &str, default: Duration) -> Duration {
    secs_or(std::env::var(name).ok().as_deref(), default)
}
