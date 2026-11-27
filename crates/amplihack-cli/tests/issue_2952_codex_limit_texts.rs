//! Issue #2952: the limit lines Codex printed on 27 November 2026, met by a
//! recipe agent step, make the step wait as a limit and then continue, and
//! a recipe run whose step failed on one is classed `usage_limit`, not
//! `indeterminate`.
//!
//! None of these lines states a reset, so each wait has no deadline: it ends
//! on an event, here a login change the test makes when the wait reports
//! progress. Nothing here depends on a wall-clock window.

#![cfg(unix)]

use crate::issue_1618_fixture::{Fixture, describe, events};

const CREDITS_ERROR: &str = "ERROR: Your workspace is out of credits. Add credits to continue.";
const CREDITS_REFILL: &str =
    "Your workspace is out of credits. Ask your workspace owner to refill in order to continue.";
const SERVICE_RATE: &str = "agent step failed: Codex exec failed with status exit status: 1: service rate limit; retry later or check account quota";
const USAGE: &str = "Usage limit reached. You've reached your usage limit. Increase your limits to continue using codex.";

/// The step meets `line` on stderr once, waits, and runs again after a
/// login change. Returns the wait's event lines.
fn waits_then_continues(line: &str) -> Vec<String> {
    let f = Fixture::new();
    let mut c = f.step();
    c.env("LIMIT_MESSAGE", line);
    let out = f.run_with_a_login_change_per_open_wait(&mut c);
    assert!(out.status.success(), "{line:?}\n{}", describe(&out));
    assert_eq!(f.count("0"), 2, "the step ran again: {}", describe(&out));
    let ev = events(&out);
    assert!(
        !ev.iter()
            .any(|l| l.starts_with("skipped") || l.starts_with("failed")),
        "{line:?}: {ev:?}"
    );
    assert_eq!(
        ev.iter().filter(|l| l.starts_with("start ")).count(),
        1,
        "{line:?}: {ev:?}"
    );
    assert!(
        ev.iter()
            .any(|l| l.starts_with("start reason=usage_limit ") && l.contains(" harness=codex ")),
        "{line:?}: {ev:?}"
    );
    assert!(
        ev.contains(&"end reason=login changed".to_string()),
        "{line:?}: {ev:?}"
    );
    assert_eq!(
        ev.last().map(String::as_str),
        Some("resume attempt=2"),
        "{line:?}: {ev:?}"
    );
    for l in &ev {
        let l = l.to_lowercase();
        assert!(
            !l.contains("workspace") && !l.contains("quota"),
            "the message is never echoed into the wait lines: {l}"
        );
    }
    ev
}

#[test]
fn the_codex_credits_line_after_a_log_level_waits_and_continues() {
    let ev = waits_then_continues(CREDITS_ERROR);
    assert_eq!(
        ev[0], "waiting_for_capacity reason=credits harness=codex until=none",
        "{ev:?}"
    );
}

#[test]
fn the_codex_refill_line_waits_and_continues() {
    let ev = waits_then_continues(CREDITS_REFILL);
    assert_eq!(
        ev[0], "waiting_for_capacity reason=credits harness=codex until=none",
        "{ev:?}"
    );
}

#[test]
fn the_codex_service_rate_limit_line_waits_and_continues() {
    let ev = waits_then_continues(SERVICE_RATE);
    assert_eq!(
        ev[0], "waiting_for_capacity reason=no_reset harness=codex until=none",
        "{ev:?}"
    );
}

#[test]
fn the_codex_usage_limit_line_waits_and_continues() {
    let ev = waits_then_continues(USAGE);
    assert_eq!(
        ev[0], "waiting_for_capacity reason=no_reset harness=codex until=none",
        "{ev:?}"
    );
}

/// A failed recipe step whose evidence ends with one of the lines: the run's
/// failure class is `usage_limit`, so the run is not ended as
/// `indeterminate` and limit-resume can take it up.
#[test]
fn a_recipe_run_classes_each_codex_limit_line_as_a_usage_limit() {
    use crate::issue_1437_final_marker::runner_payload;
    let output = "Reading the repository layout before the change.";
    let wrapper = "Step 'step-04-address-blockers' failed: agent step failed: \
                   amplihack codex failed (exit 1)\n--- stderr (tail) ---\n";
    for line in [CREDITS_ERROR, CREDITS_REFILL, USAGE] {
        let (payload, text) = runner_payload(&format!("{wrapper}{line}"), output, "");
        assert_eq!(payload["class"], "usage_limit", "{line:?}\n{text}");
    }
    // The runner's own wording, as the run of 27 November printed it.
    let (payload, text) =
        runner_payload(&format!("Step 'decide' failed: {SERVICE_RATE}"), output, "");
    assert_eq!(payload["class"], "usage_limit", "{text}");
}
