//! The rotate log lines for overflow profiles (issue #1606) and the
//! no-capacity ALERT (issue #1721). Re-exported
//! from `log::msg` with the other lines, and kept here to hold `log.rs`
//! within its line budget.

use super::decide::Reading;
use super::log::msg::usage;
use super::log::timestamp;
use crate::commands::profile::name::ProfileName;

/// After `profile switch` onto API-key profile `name` (issue #1700).
pub fn switch_platform_note(name: &ProfileName) -> String {
    format!(
        "note: {name} is an API-key profile; work in ~/.claude is billed per token \
         until you run: amplihack profile switch --none"
    )
}

/// What a per-token profile passed, for [`per_token_reason`].
#[derive(Clone, Copy)]
pub enum CheckNote {
    /// An API-key profile's login check, with its fixed detail.
    Key(&'static str),
    /// A credit profile's probe.
    Credit,
}

/// Why a move onto per-token profile `to` was allowed (issue #1804), written
/// before the swap line. `earliest` is the earliest plan reset, `None` when
/// no reading reports one. Everything here is fixed text, a tier or a time,
/// so nothing from a profile file can reach the log.
pub fn per_token_reason(
    to: &ProfileName,
    tier: u8,
    earliest: Option<i64>,
    check: CheckNote,
) -> String {
    let gate = earliest.map_or_else(
        || "reset unknown".to_string(),
        |at| format!("overflow_wait elapsed (earliest reset {})", timestamp(at)),
    );
    per_token_line(to, tier, &gate, check)
}

/// Why a move off the API key onto per-token profile `to` was allowed. It
/// passes no wait gate: credits cost less, so the move is made at once.
pub fn per_token_off_key(to: &ProfileName, tier: u8, check: CheckNote) -> String {
    per_token_line(to, tier, "leaving the API key, credits cost less", check)
}

fn per_token_line(to: &ProfileName, tier: u8, gate: &str, check: CheckNote) -> String {
    let check = match check {
        CheckNote::Key(detail) => format!("login check passed ({detail})"),
        CheckNote::Credit => "probe ok, no spend limit".to_string(),
    };
    format!(
        "per-token {to}: no plan login below 100%; {gate}; \
         marked overflow tier {tier} via profile overflow add; {check}"
    )
}

/// After an `overflow-in` swap onto API-key profile `to` (issue #1700).
pub fn onto_platform(to: &ProfileName) -> String {
    format!("moved ~/.claude onto API key profile {to} (billed per token)")
}

/// The login check refused a swap onto API-key profile `name`.
pub fn key_check_refused(name: &ProfileName, reason: &str) -> String {
    format!("key_check_refused {name}: {reason}")
}

/// The ALERT written with [`key_check_refused`], once per profile.
pub fn alert_key_check(name: &ProfileName, reason: &str) -> String {
    format!("ALERT: API key profile {name} failed its login check ({reason}); not moving onto it")
}

/// `--dry-run` in place of `would swap` when the login check fails.
pub fn would_refuse(name: &ProfileName, reason: &str) -> String {
    format!("would_refuse {name}: {reason}")
}

/// An API-key profile that `overflow=` does not mark (issue #1804).
pub fn platform_not_marked(name: &ProfileName) -> String {
    format!(
        "skip: API-key profile {name} is not marked; it is never used. \
         To allow per-token billing, run: amplihack profile overflow add {name}"
    )
}

/// A marked API-key profile that another marked one comes before by name.
pub fn platform_not_used(name: &ProfileName) -> String {
    format!("skip: API-key profile {name} is marked but not used; another comes first by name")
}

/// `<time>` for a known reset, else `unknown`.
fn reset(at: Option<i64>) -> String {
    at.map_or_else(|| "unknown".to_string(), timestamp)
}

/// After an `overflow-in` swap onto `to`.
pub fn onto_overflow(to: &ProfileName, next_reset: Option<i64>) -> String {
    format!(
        "overflow: on {to}; no plan login below 100%, next plan reset {}",
        reset(next_reset)
    )
}

/// A stay on overflow profile `live` that the previous cycle did not
/// record as on credits.
pub fn stay_on_overflow(live: &ProfileName, next_reset: Option<i64>) -> String {
    format!(
        "overflow: staying on {live}; no plan login below 100%, next plan reset {}",
        reset(next_reset)
    )
}

/// After a swap off overflow profile `from` onto plan profile `to`.
pub fn off_overflow(from: &ProfileName, to: &ProfileName) -> String {
    format!("overflow: off {from}; back on plan profile {to}")
}

/// A spend block was recorded for overflow profile `name`.
pub fn alert_spend_limit(name: &ProfileName, until: i64) -> String {
    format!(
        "ALERT: overflow profile {name} hit its spend limit; blocked until {}",
        timestamp(until)
    )
}

/// The spend-limit ALERT for an overflow profile that a `profile switch`
/// pin holds the live login on. The rotator will not move off it while
/// pinned, so the line says how to release it.
pub fn alert_spend_limit_pinned(name: &ProfileName, until: i64) -> String {
    format!(
        "ALERT: overflow profile {name} hit its spend limit; blocked until {}; \
         rotation is held on it by `profile switch`, so run \
         `amplihack profile switch --none` to move off it",
        timestamp(until)
    )
}

/// `profile switch` refuses an overflow profile with an active spend
/// block.
pub fn switch_spend_blocked(name: &ProfileName, until: i64) -> String {
    format!(
        "overflow profile {name} is blocked for its spend limit until {}; \
         clear the block with `amplihack profile overflow clear {name}` once \
         credits are added",
        timestamp(until)
    )
}

/// A spend limit on overflow profile `name` that could not be stored
/// as a block, and why. The cycle still leaves the profile alone.
pub fn spend_limit_not_blocked(name: &ProfileName, why: &str) -> String {
    format!("ALERT: overflow profile {name} hit its spend limit; not blocked ({why})")
}

#[cfg(test)]
pub fn alert_no_capacity(live: &ProfileName, reading: &Reading) -> String {
    alert_no_capacity_for(live.as_str(), reading, &[])
}

/// As [`alert_no_capacity`], with `-` for a login no profile owns. When any
/// candidate in `others` failed its probe or is exhausted, the line ends
/// with ` (candidates: F failed, E exhausted)`: two counts, no names.
pub fn alert_no_capacity_for(
    live: &str,
    reading: &Reading,
    others: &[(ProfileName, Reading)],
) -> String {
    let count = |r: Reading| others.iter().filter(|(_, o)| *o == r).count();
    let (failed, exhausted) = (count(Reading::Failed), count(Reading::Exhausted));
    let tail = match failed + exhausted {
        0 => String::new(),
        _ => format!(" (candidates: {failed} failed, {exhausted} exhausted)"),
    };
    format!(
        "ALERT: no profile has capacity live={live} {}{tail}",
        usage(reading, " ")
    )
}
