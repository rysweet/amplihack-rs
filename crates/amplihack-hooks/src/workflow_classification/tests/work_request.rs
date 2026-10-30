//! The work-request rule for marked bodies, and the reports that start with a
//! change verb but are not requests (issue #1742, review round 3 of PR #1752).

use super::support::*;
use super::*;

/// A marked body that starts with a change verb is a request for work and is
/// routed. Issue #1742: "an explicit development request inside a
/// coordination message still routes ... because it names the work". A
/// coordinator words such requests naturally, so the rule does not ask for an
/// issue or PR number straight after the verb.
#[test]
fn explicit_work_request_rule() {
    let matches = [
        "build #1740",
        "BUILD #1740",
        "fix issue 12",
        "fix issue #12",
        "fix issue#12",
        "work on PR #57",
        "work   on pr 57",
        "implement pr 3 now",
        "develop #9",
        "start issue 4",
        "Start #4 when ready",
        // Requests that name the work without a number next to the verb.
        "fix the parser for #12",
        "Fix\tthe parser",
        "refactor #57",
        "update PR 57",
        "implement retry in installer.rs",
        "add a retry to the installer",
        "create docs/reference/retry.md",
        "write tests for the parser",
        "build 1740",
        "start the build for issue 5",
        "start on #12",
        // An outcome word followed by a noun is an adjective in a request.
        "fix failed tests on PR 12",
        "fix broken links in the docs",
        "update merged branches",
        "fix passed-in args",
        "add completed tasks to the report",
        "update online docs",
        // A copula joined to more word characters is an identifier.
        "fix is_valid in parser.rs",
        "add has_many to the model",
        "fix has-many relation",
        // A report the report rule misses is routed: errors fall towards
        // routing. See VERB_LED_REPORT_PATTERN.
        "fix for #12 landed",
    ];
    for body in matches {
        assert!(
            is_explicit_work_request(body),
            "{body:?} starts with a change verb and must be routed"
        );
    }

    let non_matches = [
        "",
        "build",
        "fix   ",
        "building #1740",
        "fixes #3",
        "rebuild #1740",
        "fix-up landed on main",
        "update: CI passed on PR 12",
        "work #5",
        "please build #1740",
        "report your lane status",
        "CI passed on PR 12",
        "handing PR 57 over to you",
        "PR 134 is merged, retry the install",
        "what's the status of #12",
    ];
    for body in non_matches {
        assert!(
            !is_explicit_work_request(body),
            "{body:?} does not start with a change verb and a following word"
        );
    }
}

/// Review round 3 of PR #1752: a report that starts with a change verb used
/// as a noun is not a work request. The routing prompt applies its first
/// matching rule, and rule 4 (IMPLEMENT -> DEV) comes before the rule-8
/// exception for result reports, so routing these would start
/// dev-orchestrator for a status message.
#[test]
fn verb_led_reports_are_not_work_requests() {
    let reports = [
        // The four reports from the round-3 review.
        "build failed on PR 12",
        "build is green on #1740",
        "update on PR 57: merged",
        "fix landed on PR 12",
        // Copulas.
        "fix was merged",
        "fix has landed on main",
        "BUILD IS GREEN",
        "build is",
        "update is out",
        "builds are green",
        // Outcomes at the end, before punctuation, a preposition or a conjunction.
        "build passed",
        "build passed.",
        "build failed: see the log",
        "build failed, retrying",
        "build failed again",
        "fix landed in main",
        "fix merged to main",
        "fix merged into main",
        "build succeeded for #1740",
        "fix shipped with 0.9.3",
        "build finished at 10:02",
        "build started after the merge",
        "fix worked and the lane is idle",
        "build completed but slowly",
        "write done",
        "fix done on PR 12",
        "Update On PR 57",
    ];
    for body in reports {
        assert!(
            !is_explicit_work_request(body),
            "{body:?} is a report and must get the note"
        );
    }
}

/// The verb list must cover every verb of the routing prompt's IMPLEMENT
/// signal, so a marked body that the routing prompt would call DEV is routed
/// to it and not given the note.
#[test]
fn work_request_verbs_cover_the_implement_signal() {
    let start = DEFAULT_ROUTING_PROMPT
        .find("IMPLEMENT<br/>")
        .expect("the routing prompt has an IMPLEMENT signal")
        + "IMPLEMENT<br/>".len();
    let end = start
        + DEFAULT_ROUTING_PROMPT[start..]
            .find("\"]")
            .expect("the IMPLEMENT node is closed");
    let verbs: Vec<&str> = DEFAULT_ROUTING_PROMPT[start..end]
        .split(|c: char| c == ',' || c.is_whitespace() || c == '<' || c == '>' || c == '/')
        .filter(|word| !word.is_empty() && *word != "br")
        .collect();
    assert_eq!(
        verbs,
        [
            "build", "fix", "add", "create", "refactor", "update", "write"
        ],
        "the IMPLEMENT signal changed; update EXPLICIT_WORK_REQUEST_PATTERN with it"
    );
    for verb in verbs {
        let body = format!("{verb} the retry logic");
        assert!(
            is_explicit_work_request(&body),
            "IMPLEMENT verb {verb:?} is missing from the work-request rule"
        );
    }
}

// ---- process() ----

/// Review round 3 of PR #1752, item 1, replayed without a turn count. A
/// coordinator that marks its reports must not get more routing prompts than
/// one that does not.
#[test]
fn marked_verb_led_reports_are_not_routed_past_the_throttle() {
    let iso = Isolated::new();
    let s = Some("s-reports");
    assert_router(&run_hook(s, None, "Please investigate the install flow"));
    assert_router(&run_hook(s, None, "Please look at the docs build"));
    assert_eq!(run_hook(s, None, "build failed on PR 12"), empty());

    for report in [
        "[coord] build failed on PR 12",
        "[coord] fix landed on PR 12",
        "[coord] build is green on #1740",
        "[coord] update on PR 57: merged",
    ] {
        assert_note(&run_hook(s, None, report));
        let state = iso.read_state("s-reports").unwrap();
        assert_eq!(last_rule(&state), Some("coordination_marker"), "{report:?}");
        assert_eq!(classified_turn(&state), Some(1), "{report:?}");
    }

    // A marked request in the same session still bypasses the throttle.
    assert_router(&run_hook(s, None, "[coord] fix failed tests on PR 12"));
    let state = iso.read_state("s-reports").unwrap();
    assert_eq!(last_rule(&state), Some("coordination_explicit_dev"));
    assert_eq!(classified_turn(&state), Some(7));
}
