//! The rotate log lines for hand-change recovery (issues #1693 and #1845).
//! Kept apart from `log::msg` so that `log.rs` stays within its line budget.

use crate::commands::profile::name::ProfileName;

/// Hand-change recovery, case A: `m` had no login of its own.
pub fn recovered_record(m: &ProfileName, l: &ProfileName) -> String {
    format!("recovered: ~/.claude holds {m}'s login (ledger named {l}); recorded {m} as live")
}

/// Hand-change recovery when `l`'s own record names the new login.
pub fn recovered_relogin(l: &ProfileName) -> String {
    format!("recovered: ~/.claude holds the account recorded for {l}; kept {l} as live")
}

/// Hand-change recovery, case B: `m`'s own copy was moved to `to`.
pub fn recovered_moved(m: &ProfileName, to: &std::path::Path) -> String {
    format!(
        "recovered: ~/.claude and {m} held the same account; kept ~/.claude, moved {m}'s copy to {}; recorded {m} as live",
        to.display()
    )
}

/// The fixed prefix of each line `--recovery log` writes in place of a
/// recovery (#1890). Lines name profiles and paths only.
pub const LOG_ONLY: &str = "recovery (log only): ";

/// `--recovery log`, case A: `m` would be recorded in place of `l`.
pub fn log_record(m: &ProfileName, l: &ProfileName) -> String {
    format!("{LOG_ONLY}would record {m} as live (ledger named {l})")
}

/// `--recovery log`, #1845: `l` would stay live with its own recorded login.
pub fn log_relogin(l: &ProfileName) -> String {
    format!("{LOG_ONLY}would record {l} as live again with its own login (ledger account differs)")
}

/// `--recovery log`, case B: `m`'s copy would be moved to `to`; `None` when
/// no backup name is free, so recovery would refuse.
pub fn log_moved(m: &ProfileName, to: Option<&std::path::Path>) -> String {
    match to {
        Some(to) => format!(
            "{LOG_ONLY}would move {m}'s login to {} and record {m} as live",
            to.display()
        ),
        None => format!(
            "{LOG_ONLY}would move {m}'s login aside and record {m} as live, but no backup name is free"
        ),
    }
}
