//! Issues #1658 (G) and #2085 (T10): the documented limit-resume limits are
//! the code's. The limits table is checked against the constants, the
//! outcome names and swap record fields must be documented, and the polling
//! rows the event model removed must be gone. Other prose is reviewed, not
//! tested.
//!
//! Lives in amplihack-hooks rather than the CLI's `tests_docs_sync.rs`
//! because the CLI does not depend on this crate.

use amplihack_hooks::limit_resume::keys::RESUME_KEYS;
use amplihack_hooks::limit_resume::{MAX_RESUMES_PER_SESSION, Outcome};
use amplihack_utils::rotator_status::swap_record::MAX_SWAP_BYTES;
use amplihack_utils::rotator_status::{
    LIMIT_RESUME_TIMEOUT_MARGIN, LIMIT_RESUME_TIMEOUT_SECS, MAX_WAIT,
};

const REFERENCE: &str = include_str!("../../../docs/reference/profile-command.md");
const HOWTO: &str = include_str!("../../../docs/howto/claude-profile-rotation.md");
const HOOK_SPECS: &str = include_str!("../../../docs/reference/hook-specifications.md");

/// The constants table in "Resume limits": `| <name> | <value> |`.
fn limits_row(name: &str) -> Option<&'static str> {
    REFERENCE
        .lines()
        .find(|l| l.starts_with(&format!("| {name} |")))
        .map(|l| l.split('|').nth(2).unwrap().trim())
}

fn row(name: &str) -> &'static str {
    limits_row(name).unwrap_or_else(|| panic!("reference row `{name}`"))
}

#[test]
fn reference_limits_table_matches_the_constants() {
    assert_eq!(
        row("Wait for a swap record"),
        format!("{} s (7 days)", MAX_WAIT.as_secs())
    );
    assert_eq!(
        row("Hook timeout margin"),
        format!("{} s", LIMIT_RESUME_TIMEOUT_MARGIN.as_secs())
    );
    assert_eq!(
        row("Registered hook timeout"),
        format!("{LIMIT_RESUME_TIMEOUT_SECS} s")
    );
    assert_eq!(LIMIT_RESUME_TIMEOUT_SECS, 608_400);
    assert_eq!(row("Swap record size"), format!("{MAX_SWAP_BYTES} bytes"));
    // The ledger's skew rule (`MAX_SKEW_SECS`, crate-private in utils).
    assert_eq!(row("Clock skew allowed"), "300 s");
    assert_eq!(row("Keys per resume"), RESUME_KEYS.len().to_string());
    assert_eq!(
        row("Resumes per session"),
        format!("{MAX_RESUMES_PER_SESSION} per 24 hours")
    );
    assert_eq!(row("Resumes per limit event"), "1");
}

#[test]
fn the_polling_rows_are_gone() {
    for gone in [
        "Wait for capacity",
        "Ledger read interval",
        "Freshness margin without `measured_at`",
    ] {
        assert!(limits_row(gone).is_none(), "stale row `{gone}`");
    }
    for text in ["15 s for the first hour", "every 15 s", "every 5 minutes"] {
        for (name, doc) in [
            ("profile-command.md", REFERENCE),
            ("claude-profile-rotation.md", HOWTO),
            ("hook-specifications.md", HOOK_SPECS),
        ] {
            assert!(!doc.contains(text), "{name} still says `{text}`");
        }
    }
}

#[test]
fn every_outcome_the_hook_logs_is_documented() {
    for o in [
        Outcome::Resumed,
        Outcome::ResumedKeys,
        Outcome::ResumeKeysFailed,
        Outcome::WatchLost,
        Outcome::SwapStale,
        Outcome::SwapNoRoom,
        Outcome::NoCapacityTimeout,
        Outcome::MovedOn,
        Outcome::Duplicate,
        Outcome::Cap,
        Outcome::StopHookActive,
        Outcome::SessionGone,
        Outcome::RotatorDown,
        Outcome::GuardFailed,
        Outcome::Disabled,
    ] {
        let name = format!("`{}`", o.as_str());
        assert!(REFERENCE.contains(&name), "profile-command.md lacks {name}");
    }
}

#[test]
fn the_swap_record_schema_is_documented() {
    assert!(REFERENCE.contains("`swap.json`"));
    for field in [
        "seq",
        "live_login",
        "from_login",
        "swapped_at",
        "reading",
        "session",
        "week",
        "measured_at",
    ] {
        assert!(
            REFERENCE.contains(&format!("`{field}`")),
            "profile-command.md lacks `{field}`"
        );
    }
}
