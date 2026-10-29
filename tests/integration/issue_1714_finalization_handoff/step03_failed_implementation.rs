//! autodrive-build step-03 on #1714 case 2: a `FAILED_IMPLEMENTATION` result
//! that finalization handed off (it does so only for a PR its open-PR check
//! confirmed) goes through the same PR checks as `BLOCKED_CI`.

use super::fixture::*;
use super::step03::{assert_stopped, build_result, state_reads};

// #1714 case 2: FAILED_IMPLEMENTATION handed off by the finaliser (it does so
// only for a PR the open-PR check confirmed) continues like BLOCKED_CI, and is
// still refused when the PR is not OPEN or is not the PR step-03 resolved.
#[test]
fn failed_implementation_with_the_handoff_and_an_open_pr_continues() {
    let fx = Step03::new();
    let outcome = fx.run(
        "134",
        Some(&handoff_result("FAILED_IMPLEMENTATION", "134")),
        &[],
    );
    assert!(outcome.success(), "{}", outcome.describe());
    let result = build_result(&outcome);
    assert_eq!(result["pr"], "134", "{}", outcome.describe());
    assert_eq!(result["terminal_state"], "FAILED_IMPLEMENTATION");
    assert_eq!(result["handed_off"], "true");
    assert!(state_reads(&fx, "134") >= 1, "{:?}", fx.gh_calls());

    let fx = Step03::new();
    fx.pr_states("134", &["CLOSED"]);
    let outcome = fx.run(
        "134",
        Some(&handoff_result("FAILED_IMPLEMENTATION", "134")),
        &[],
    );
    assert_stopped(&outcome, "PR #134 is CLOSED, not OPEN", "closed PR");

    let fx = Step03::new();
    let outcome = fx.run(
        "134",
        Some(&handoff_result("FAILED_IMPLEMENTATION", "135")),
        &[],
    );
    assert_stopped(
        &outcome,
        "workflow_result names PR #135 but phase 1 resolved PR #134",
        "PR mismatch",
    );

    let fx = Step03::new();
    let outcome = fx.run(
        "",
        Some(&handoff_result("FAILED_IMPLEMENTATION", "134")),
        &[],
    );
    assert_stopped(
        &outcome,
        "FAILED_IMPLEMENTATION was reported with no published open PR",
        "no resolved PR",
    );

    let mut standalone = handoff_result("FAILED_IMPLEMENTATION", "134");
    standalone.as_object_mut().unwrap().remove("handoff");
    let fx = Step03::new();
    let outcome = fx.run("134", Some(&standalone), &[]);
    assert_stopped(
        &outcome,
        "ended FAILED_IMPLEMENTATION without a merge-ready hand-off",
        "no handoff field",
    );
}
