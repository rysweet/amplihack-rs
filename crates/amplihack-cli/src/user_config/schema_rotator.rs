//! The `rotator.*` keys (issue #2007): the thresholds that decide whether
//! a Claude login or a Codex account is still usable. All are user-only, so
//! a repository cannot move the swap points or steer the Codex choice.
//!
//! - `rotator.session_at` and `rotator.week_at` are the rotator's
//!   `--session-at` and `--week-at`. The review reserve is not a separate
//!   key: it is `100 - week_at` percent of each weekly allowance.
//! - `rotator.interval` is the seconds between `rotate watch` cycles.
//! - `rotator.codex_floor` is the credit balance a Codex account needs once
//!   its rate limits are used up, for the next choice of `profile usage`.
//!
//! The rotator resolves these in `commands::profile::rotate::settings`: a
//! flag, then the `AMPLIHACK_ROTATE_*` variable named here, then this key,
//! then the built-in default. A test pins each default and range to the
//! rotator's own. `codex_floor` has no variable.

use super::schema::{KeyDef, Scope, ValueKind};

const fn int(
    name: &'static str,
    min: u64,
    max: u64,
    default: &'static str,
    env: Option<&'static str>,
) -> KeyDef {
    KeyDef {
        name,
        scope: Scope::UserOnly,
        kind: ValueKind::Int { min, max },
        env,
        default: Some(default),
        lower_only: false,
    }
}

pub(crate) const ROTATOR_SESSION_AT: KeyDef = int(
    "rotator.session_at",
    1,
    100,
    "85",
    Some("AMPLIHACK_ROTATE_SESSION_AT"),
);
pub(crate) const ROTATOR_WEEK_AT: KeyDef = int(
    "rotator.week_at",
    1,
    100,
    "90",
    Some("AMPLIHACK_ROTATE_WEEK_AT"),
);
pub(crate) const ROTATOR_INTERVAL: KeyDef = int(
    "rotator.interval",
    1,
    86_400,
    "600",
    Some("AMPLIHACK_ROTATE_INTERVAL"),
);
/// `codex_capacity::DEFAULT_FLOOR`, which a test pins it to.
pub(crate) const ROTATOR_CODEX_FLOOR: KeyDef =
    int("rotator.codex_floor", 0, 1_000_000, "3000", None);
