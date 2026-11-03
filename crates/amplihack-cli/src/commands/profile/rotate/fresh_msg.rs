//! Issue #1870: the `reread:` and `damp:` log lines of the freshness gate
//! and swap-back damping. Fixed text, a validated profile name and whole
//! percentages only; never an error string, Claude's output or an account.

use super::log::Shown;
use super::log::msg::usage;
use crate::commands::profile::name::ProfileName;

/// `reread: <name> reading stale s=<a>% w=<b>%; fresh s=<c>% w=<d>%`.
pub fn stale(name: &ProfileName, old: &Shown, new: &Shown) -> String {
    format!(
        "reread: {name} reading stale {}; fresh {}",
        usage(old, " "),
        usage(new, " ")
    )
}

/// `reread: <name> failed; treated as unknown`.
pub fn failed(name: &ProfileName) -> String {
    format!("reread: {name} failed; treated as unknown")
}

/// `reread: <name> limit reached; treated as unknown`.
pub fn limit(name: &ProfileName) -> String {
    format!("reread: {name} limit reached; treated as unknown")
}

/// `reread: <name> time limit reached; treated as unknown`: the gate's one
/// time limit for all its re-reads is spent.
pub fn time_limit(name: &ProfileName) -> String {
    format!("reread: {name} time limit reached; treated as unknown")
}

/// `damp: <name> swapped away exhausted; no later reading shows room`.
pub fn still_damped(name: &ProfileName) -> String {
    format!("damp: {name} swapped away exhausted; no later reading shows room")
}

/// `damp: <name> later reading shows room; swap-back allowed`.
pub fn lifted(name: &ProfileName) -> String {
    format!("damp: {name} later reading shows room; swap-back allowed")
}
