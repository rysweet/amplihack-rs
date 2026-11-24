//! Issues #1658 (G) and #2085 (T10): the documented limit-resume limits are
//! the code's. The limits table is checked against the constants, the
//! outcome names and swap record fields must be documented, and the polling
//! rows the event model removed must be gone. Other prose is reviewed, not
//! tested.
//!
//! Lives in amplihack-hooks rather than the CLI's `tests_docs_sync.rs`
//! because the CLI does not depend on this crate.

use amplihack_hooks::limit_resume::Outcome;
use amplihack_hooks::limit_resume::keys::{RESUME_KEYS, UNRECOGNISED_REASONS, UNVERIFIED_REASONS};
use amplihack_utils::rotator_status::swap_record::MAX_SWAP_BYTES;
use amplihack_utils::rotator_status::{
    LIMIT_RESUME_TIMEOUT_MARGIN, LIMIT_RESUME_TIMEOUT_SECS, MAX_WAIT,
};

const REFERENCE: &str = include_str!("../../../docs/reference/profile-command.md");
const HOWTO: &str = include_str!("../../../docs/howto/claude-profile-rotation.md");
const HOOK_SPECS: &str = include_str!("../../../docs/reference/hook-specifications.md");
/// The skill agents load to explain `resume.log` to a user. It has its own
/// outcome table, which drifted from the hook once already (#2912).
const SKILL: &str =
    include_str!("../../../amplifier-bundle/skills/claude-profile-rotation/SKILL.md");

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
    assert!(
        limits_row("Resumes per session").is_none(),
        "the per-session resume cap is gone (#2912)"
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

/// Every outcome the hook logs, for the documentation checks below.
/// `Outcome::NotLimit` is not logged, so it is not here.
const OUTCOMES: [Outcome; 17] = [
    Outcome::NotResumed,
    Outcome::ResumedKeys,
    Outcome::ResumeKeysFailed,
    Outcome::PaneUnverified,
    Outcome::ScreenUnrecognised,
    Outcome::WatchLost,
    Outcome::SwapStale,
    Outcome::SwapNoRoom,
    Outcome::NoCapacityTimeout,
    Outcome::MovedOn,
    Outcome::Duplicate,
    Outcome::SessionRecordReset,
    Outcome::StopHookActive,
    Outcome::SessionGone,
    Outcome::RotatorDown,
    Outcome::GuardFailed,
    Outcome::Disabled,
];

#[test]
fn every_outcome_the_hook_logs_is_documented() {
    for o in OUTCOMES {
        let name = format!("`{}`", o.as_str());
        assert!(REFERENCE.contains(&name), "profile-command.md lacks {name}");
    }
}

/// The outcome names in the skill's `| Outcome | What to tell the user |`
/// table: the first cell of every row, split on ` / ` where one row covers
/// two outcomes.
fn skill_outcome_table() -> Vec<&'static str> {
    let rows = SKILL
        .lines()
        .skip_while(|l| !l.starts_with("| Outcome | What to tell the user |"))
        .skip(2)
        .take_while(|l| l.starts_with('|'));
    let mut names = Vec::new();
    for row in rows {
        let first = row.split('|').nth(1).expect("a table row has a first cell");
        for name in first.split(" / ") {
            let name = name
                .trim()
                .strip_prefix('`')
                .and_then(|n| n.strip_suffix('`'));
            names.push(name.unwrap_or_else(|| panic!("SKILL.md outcome row `{row}`")));
        }
    }
    names
}

/// The skill's outcome table names exactly the outcomes the hook logs: none
/// missing, and none the hook can no longer emit, such as the `cap` the
/// #2912 change removed. Agents load the skill to explain `resume.log`, so
/// a stale row tells them to expect a line that never comes.
#[test]
fn the_skill_outcome_table_is_the_hooks() {
    let table = skill_outcome_table();
    assert!(!table.is_empty(), "SKILL.md has no outcome table");
    for o in OUTCOMES {
        assert!(
            table.contains(&o.as_str()),
            "SKILL.md's outcome table lacks `{}`",
            o.as_str()
        );
    }
    for name in &table {
        assert!(
            OUTCOMES.iter().any(|o| o.as_str() == *name),
            "SKILL.md's outcome table names `{name}`, which the hook never logs"
        );
    }
    for stale in ["3 per session", "wake attempts in 24 hours"] {
        assert!(!SKILL.contains(stale), "SKILL.md still says `{stale}`");
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

/// Issue #2695: the `detail` vocabulary is a closed set, so the reference
/// must name every word the log can carry. #2905 records that the earlier
/// test pinned the outcome names but no `detail` value, which let the two
/// drift apart unnoticed.
#[test]
fn every_detail_the_hook_logs_is_documented() {
    for reason in UNVERIFIED_REASONS.iter().chain(UNRECOGNISED_REASONS.iter()) {
        let name = format!("`{reason}`");
        assert!(REFERENCE.contains(&name), "profile-command.md lacks {name}");
    }
}
