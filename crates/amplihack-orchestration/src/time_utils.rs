//! Lightweight time formatting helpers shared across modules.

use std::time::{SystemTime, UNIX_EPOCH};

/// Format the current wall-clock time as `HH:MM:SS` in UTC.
pub(crate) fn current_hms() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let h = (secs / 3600) % 24;
    let m = (secs / 60) % 60;
    let s = secs % 60;
    format!("{:02}:{:02}:{:02}", h, m, s)
}
