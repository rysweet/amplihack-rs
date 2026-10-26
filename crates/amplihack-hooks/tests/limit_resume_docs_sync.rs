//! Issue #1658 (G): the documented limit-resume contract is the code's.
//!
//! Lives in amplihack-hooks rather than the CLI's `tests_docs_sync.rs`
//! because the CLI does not depend on this crate, and the message and
//! outcomes are defined here.

use amplihack_hooks::limit_resume::{MAX_RESUMES_PER_SESSION, Outcome, RESUME_MSG};
use amplihack_utils::rotator_status::{
    FRESH_MARGIN_SECS, LIMIT_RESUME_TIMEOUT_SECS, MAX_WAIT, POLL_INTERVAL, SLOW_POLL_INTERVAL,
};

const REFERENCE: &str = include_str!("../../../docs/reference/profile-command.md");
const HOOK_SPECS: &str = include_str!("../../../docs/reference/hook-specifications.md");
const HOWTO: &str = include_str!("../../../docs/howto/claude-profile-rotation.md");
const MANAGEMENT: &str = include_str!("../../../docs/PROFILE_MANAGEMENT.md");
const SKILL: &str =
    include_str!("../../../amplifier-bundle/skills/claude-profile-rotation/SKILL.md");

const LOGGED_OUTCOMES: [Outcome; 9] = [
    Outcome::Resumed,
    Outcome::NoCapacityTimeout,
    Outcome::MovedOn,
    Outcome::Disabled,
    Outcome::Duplicate,
    Outcome::Cap,
    Outcome::StopHookActive,
    Outcome::SessionGone,
    Outcome::GuardFailed,
];

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
fn reference_quotes_the_resume_message_byte_for_byte() {
    let block = format!("```\n{RESUME_MSG}\n```");
    assert!(
        REFERENCE.contains(&block),
        "reference resume message differs"
    );
}

#[test]
fn reference_lists_every_logged_outcome() {
    for o in LOGGED_OUTCOMES {
        assert!(
            REFERENCE.contains(&format!("| `{}` |", o.as_str())),
            "reference outcome table is missing `{}`",
            o.as_str()
        );
    }
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
        limits_row("Freshness margin after hook start"),
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

#[test]
fn hook_reference_registers_stop_failure_with_the_real_timeout() {
    let row = HOOK_SPECS
        .lines()
        .find(|l| l.contains("`StopFailure`") && l.contains("limit-resume"))
        .expect("hook table row for StopFailure");
    assert!(
        row.contains(&format!("{LIMIT_RESUME_TIMEOUT_SECS} s")),
        "{row}"
    );
    assert!(
        row.contains("rate_limit") && row.contains("asyncRewake"),
        "{row}"
    );
    assert!(HOOK_SPECS.contains(&format!("\"timeout\": {LIMIT_RESUME_TIMEOUT_SECS},")));
    assert!(HOOK_SPECS.contains("\"asyncRewake\": true"));
}

#[test]
fn docs_link_the_howto_section() {
    let anchor = "#resume-sessions-stopped-by-a-usage-limit";
    assert!(HOWTO.contains("## Resume sessions stopped by a usage limit"));
    assert!(MANAGEMENT.contains(anchor), "PROFILE_MANAGEMENT.md link");
    assert!(REFERENCE.contains(anchor), "profile-command.md link");
}

#[test]
fn docs_say_the_tmux_nudger_is_no_longer_needed() {
    for (name, doc) in [
        ("howto", HOWTO),
        ("management", MANAGEMENT),
        ("skill", SKILL),
    ] {
        assert!(
            doc.contains("nudger"),
            "{name} must mention the tmux nudger"
        );
    }
    assert!(SKILL.contains("limit-resume"));
}

#[test]
fn docs_state_the_minimum_claude_code_version() {
    for (name, doc) in [
        ("howto", HOWTO),
        ("hook specs", HOOK_SPECS),
        ("skill", SKILL),
    ] {
        assert!(
            doc.contains("2.1.306"),
            "{name} must state Claude Code 2.1.306"
        );
    }
}
