//! Issue #2911: Codex "Your workspace is out of credits" reaching the
//! supervisor waits as a usage limit, with a visible reason, and the run is
//! not ended by it. Credits state no reset, so the wait has no deadline: it
//! ends on an event, here a login change the test makes.

#![cfg(unix)]

use crate::issue_1618_fixture::{Fixture, describe, events};

const CODEX: &str = "Your workspace is out of credits. Add credits to continue.";

#[test]
fn codex_out_of_credits_waits_as_a_usage_limit_and_continues() {
    let f = Fixture::new();
    let mut c = f.step();
    c.env("LIMIT_MESSAGE", CODEX);
    let out = f.run_with_a_login_change_per_open_wait(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 2, "{}", describe(&out));
    let ev = events(&out);
    assert_eq!(
        ev,
        [
            "waiting_for_capacity reason=credits harness=codex until=none",
            "start reason=usage_limit wait=capacity harness=codex reset=none until=none \
             bound=none",
            "end reason=login changed",
            "resume attempt=2",
        ],
        "{ev:?}"
    );
    assert!(
        !ev.iter()
            .any(|l| l.starts_with("skipped") || l.starts_with("failed")),
        "{ev:?}"
    );
}

/// The same message in a failed recipe step, read by `amplihack recipe run`:
/// the run's failure class is `usage_limit`, not `indeterminate`, so the run
/// is no longer reported as an authentication rejection (#2876). The
/// Anthropic API's own credit message still classes `environmental`.
#[test]
fn a_recipe_run_classes_codex_credit_exhaustion_as_a_usage_limit() {
    use crate::issue_1437_final_marker::runner_payload;
    let error = "Step 'step-04-address-blockers' failed: agent step failed: \
                 amplihack codex failed (exit 1)\n--- stderr (tail) ---\n";
    let output = "Reading the repository layout before the change.";

    let (codex, text) = runner_payload(&format!("{error}{CODEX}"), output, "");
    assert_eq!(codex["class"], "usage_limit", "{text}");
    assert_eq!(
        codex["signal"], "your workspace is out of credits",
        "{text}"
    );

    let anthropic = "Your credit balance is too low to access the Anthropic API.";
    let (api, text) = runner_payload(&format!("{error}{anthropic}"), output, "");
    assert_eq!(api["class"], "environmental", "{text}");
}

#[test]
fn the_codex_message_is_never_echoed_into_the_wait_lines() {
    let f = Fixture::new();
    let mut c = f.step();
    c.env("LIMIT_MESSAGE", CODEX);
    let out = f.run_with_a_login_change_per_open_wait(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    for l in events(&out) {
        assert!(!l.to_lowercase().contains("workspace"), "{l}");
    }
}
