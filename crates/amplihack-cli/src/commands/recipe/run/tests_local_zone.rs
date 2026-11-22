//! `local_zone::resolve` (issue #2967): the zone a reset with no stated zone
//! is read in. The supervisor and the `claude` child share a host and an
//! environment, so the zone is known and never guessed.
//!
//! Contract: `resolve(tz_var, system)` is pure. `tz_var` is the value of `TZ`
//! (`None` when unset) and `system` the host's IANA zone name (`None` when it
//! cannot be read).
//!
//! - `TZ` unset: the system zone, looked up in `chrono-tz`.
//! - `TZ` naming a `chrono-tz` zone, after one leading `:` is stripped: that zone.
//! - `TZ` set to anything else, including empty or a path: `None`. No file
//!   is read from a `TZ` value, and the system zone is not used instead.
//!
//! `get()` is `resolve` over the real variable and the real system zone, and
//! is `pub(crate)` because `profile/attempt.rs` calls it.
//!
//! Wired from `recipe/run/mod.rs` with `#[cfg(test)] mod tests_local_zone;`.

use super::local_zone::resolve;
use chrono_tz::Tz;

const LA: Tz = chrono_tz::America::Los_Angeles;
const LONDON: Tz = chrono_tz::Europe::London;

#[test]
fn an_unset_tz_uses_the_system_zone() {
    assert_eq!(resolve(None, Some("Europe/London")), Some(LONDON));
}

#[test]
fn an_unset_tz_and_no_readable_system_zone_is_none() {
    assert_eq!(resolve(None, None), None);
}

#[test]
fn a_system_zone_chrono_tz_does_not_know_is_none() {
    assert_eq!(resolve(None, Some("Not/AZone")), None);
    assert_eq!(resolve(None, Some("")), None);
}

#[test]
fn tz_naming_a_zone_wins_over_the_system_zone() {
    assert_eq!(
        resolve(Some("America/Los_Angeles"), Some("Europe/London")),
        Some(LA)
    );
    assert_eq!(
        resolve(Some("UTC"), Some("Europe/London")),
        Some(chrono_tz::UTC)
    );
}

#[test]
fn one_leading_colon_is_stripped() {
    assert_eq!(resolve(Some(":America/Los_Angeles"), None), Some(LA));
    assert_eq!(resolve(Some("::America/Los_Angeles"), None), None);
}

#[test]
fn tz_set_to_anything_else_is_none_and_does_not_fall_back_to_the_system_zone() {
    for tz in [
        "",
        ":",
        "/etc/localtime",
        ":/etc/localtime",
        "../x",
        "../../etc/passwd",
        "Nowhere/Zone",
        "America/Los_Angeles/",
        " America/Los_Angeles",
        "America/Los_Angeles ",
        "America/Los_Angeles\n",
    ] {
        assert_eq!(resolve(Some(tz), Some("Europe/London")), None, "TZ={tz:?}");
        assert_eq!(resolve(Some(tz), None), None, "TZ={tz:?}");
    }
}
