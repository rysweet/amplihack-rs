//! Issue #1658 (G): the documented limit-resume limits are the code's.
//! Only the limits table is checked (#1659): prose is reviewed, not tested.
//!
//! Lives in amplihack-hooks rather than the CLI's `tests_docs_sync.rs`
//! because the CLI does not depend on this crate.

use amplihack_hooks::limit_resume::MAX_RESUMES_PER_SESSION;
use amplihack_utils::rotator_status::{
    FRESH_MARGIN_SECS, LIMIT_RESUME_TIMEOUT_SECS, MAX_WAIT, POLL_INTERVAL, SLOW_POLL_INTERVAL,
};

const REFERENCE: &str = include_str!("../../../docs/reference/profile-command.md");

/// The constants table in "Resume limits": `| <name> | <value> |`.
fn limits_row(name: &str) -> &'static str {
    REFERENCE
        .lines()
        .find(|l| l.starts_with(&format!("| {name} |")))
        .unwrap_or_else(|| panic!("reference row `{name}`"))
        .split('|')
        .nth(2)
        .unwrap()
        .trim()
}

#[test]
fn reference_limits_table_matches_the_constants() {
    assert_eq!(
        limits_row("Wait for capacity"),
        format!("{} s (7 days)", MAX_WAIT.as_secs())
    );
    assert_eq!(
        limits_row("Ledger read interval"),
        format!(
            "{} s for the first hour, then {} s",
            POLL_INTERVAL.as_secs(),
            SLOW_POLL_INTERVAL.as_secs()
        )
    );
    assert_eq!(
        limits_row("Freshness margin without `measured_at`"),
        format!("{FRESH_MARGIN_SECS} s")
    );
    assert_eq!(
        limits_row("Registered hook timeout"),
        format!("{LIMIT_RESUME_TIMEOUT_SECS} s")
    );
    assert_eq!(
        limits_row("Resumes per session"),
        format!("{MAX_RESUMES_PER_SESSION} per 24 hours")
    );
    assert_eq!(limits_row("Resumes per limit event"), "1");
}
