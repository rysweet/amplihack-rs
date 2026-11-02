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
