//! The zone a usage-limit reset with no stated zone is read in (issue #2967).
//!
//! Claude Code prints some limit messages with a bare clock time, as in
//! `your 5-hour limit resets 11:50pm`. The supervisor that reads the message
//! and the `claude` child that printed it share a host and an environment,
//! so the zone the child formatted the time in is known here and is never
//! guessed.
//!
//! The rule is strict on purpose:
//!
//! - `TZ` unset: the system zone, looked up in `chrono-tz`'s compiled-in table.
//! - `TZ` naming a `chrono-tz` zone, after one leading `:` is stripped: that
//!   zone.
//! - `TZ` set to anything else, including empty, a POSIX rule or a path such
//!   as `:/etc/localtime`: no zone. The system zone is not used in its place,
//!   because the child was told to use `TZ` and what it made of the value is
//!   not known here.
//!
//! No file is opened on behalf of a `TZ` value. With no zone, a zone-less
//! reset is not read and the step takes the handling it had before #2967; the
//! operator's remedy is to set `TZ` to an IANA name.

use chrono_tz::Tz;
use std::str::FromStr;

/// This process's zone, by the rule above.
pub(crate) fn get() -> Option<Tz> {
    match std::env::var_os("TZ") {
        // A value that is not UTF-8 names no zone in the table.
        Some(value) => resolve(Some(value.to_str()?), None),
        None => resolve(None, iana_time_zone::get_timezone().ok().as_deref()),
    }
}

/// The zone for a `TZ` value (`None` when unset) and the host's IANA zone
/// name (`None` when it cannot be read). Pure.
pub(super) fn resolve(tz_var: Option<&str>, system: Option<&str>) -> Option<Tz> {
    let name = match tz_var {
        Some(value) => value.strip_prefix(':').unwrap_or(value),
        None => system?,
    };
    Tz::from_str(name).ok()
}
