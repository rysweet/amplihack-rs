//! The work-request rule for marked bodies, and the reports that start with a
//! change verb but are not requests, and the throttle that applies to a marked
//! body read as a request (issue #1742, review rounds 3 to 6 of PR #1752).

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
        // `work on` is one verb, so the outcome word is the word after it.
        "work on failed tests on PR 12",
        "work on merged branches",
        "fix red tests on main",
        "update green builds",
        // The outcome words are a closed list. A report whose outcome is not
        // one of them is read as a request, and is then throttled like the
        // same text without the marker (see the `process()` tests below). The
        // docs tell senders to start a report with its subject
        // (`PR 12 rolled back`), which always gets the note.
        "fix #12 rolled back",
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
        // A report that starts with its subject is never a work request.
        "PR 12 merged",
        "PR 12 rolled back",
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

/// Review round 4 of PR #1752: a report that names its object before its
/// outcome is a report wherever the outcome falls. Round 3's rule looked only
/// at the second word, so `fix #12 merged` was routed.
#[test]
fn reports_that_name_their_object_first_are_not_work_requests() {
    let reports = [
        // The four reports from the round-4 review.
        "fix #12 merged",
        "build #1740 failed",
        "update #57 merged",
        "fix for #12 landed",
        // Trailing punctuation, phrases, adverbs and objects.
        "fix #12 merged.",
        "build #1740 failed!",
        "fix #12 merged?",
        "build #1740 failed on main",
        "fix for #12 landed in main",
        "build #1740 failed, retrying",
        "fix #12 landed yesterday",
        "update PR 57 merged cleanly",
        "build #1740 passed CI",
        "fix #12 passed review",
        "build 1740 finished",
        "work on PR #57 done",
        // Outcome words added in review round 4.
        "fix #12 reverted",
        "update #57 pushed",
        "fix #12 deployed",
        "fix #12 released in 0.9.4",
        "build #1740 broke on main",
        "build #1740 broken",
        "build #1740 green",
        "build #1740 red on main",
        "build green",
        "fix reverted",
        // A copula or auxiliary after the object.
        "build #1740 is green",
        "fix for #12 has landed",
        "update to PR 57 was reverted",
        "build of main was red",
        // Across lines.
        "build #1740\nfailed on main",
        "fix for #12.\nIt has landed.",
        // The table in the round-6 review that this list did not yet hold.
        "fix #12 has landed",
        "build on main failed",
        "update PR 57 merged, nothing to do",
    ];
    for body in reports {
        assert!(
            !is_explicit_work_request(body),
            "{body:?} names its object, then its outcome, and must get the note"
        );
    }
}

/// Errors fall towards the note (review round 4 of PR #1752). A request that
/// the report patterns take for a report gets the note, and the note's last
/// sentence still sends a code change to dev-orchestrator. These are the
/// known costs; they are pinned so a change to them is deliberate.
#[test]
fn requests_taken_for_reports_get_the_note_that_names_dev_orchestrator() {
    let requests = [
        "fix the tests that failed",
        "fix the failed tests",
        "update the list of merged branches",
        "fix the case where the list is empty",
        "add a getting started guide",
        "start #12 once #11 has merged",
        "fix the broken build",
        "fix the red test on main",
    ];
    for body in requests {
        assert!(
            !is_explicit_work_request(body),
            "{body:?} contains an outcome word or copula after the verb's object"
        );
    }

    let _iso = Isolated::new();
    for (turn, body) in requests.iter().enumerate() {
        let out = run_hook(
            Some("s-cost"),
            Some(turn as u64),
            &format!("[coord] {body}"),
        );
        assert_note(&out);
    }
    assert!(
        COORDINATION_NOTE.ends_with("If it asks for a code change, use dev-orchestrator as usual."),
        "a request taken for a report reaches dev-orchestrator only through this sentence"
    );
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

/// Review rounds 3, 4 and 6 of PR #1752, replayed without a turn count. A
/// coordinator that marks its messages must not get more routing prompts than
/// one that does not. A report gets the note at any turn. A body read as a
/// request, rightly or not, is throttled like the same text without the
/// marker, and gets the note while the throttle holds.
#[test]
fn marked_verb_led_reports_are_not_routed_past_the_throttle() {
    let iso = Isolated::new();
    let s = Some("s-reports");
    assert_router(&run_hook(s, None, "Please investigate the install flow"));
    assert_router(&run_hook(s, None, "Please look at the docs build"));
    assert_eq!(run_hook(s, None, "fix #12 merged"), empty());

    // Turns 3 and 4 are inside the window of the classification at turn 1.
    // `fix #12 rolled back` is a report that the wording rule reads as a
    // request; `build #1740` is a request.
    for body in ["fix #12 rolled back", "build #1740"] {
        assert_note(&run_hook(s, None, &format!("[coord] {body}")));
        let state = iso.read_state("s-reports").unwrap();
        assert_eq!(
            last_rule(&state),
            Some("coordination_throttled"),
            "{body:?}"
        );
        assert_eq!(classified_turn(&state), Some(1), "{body:?}");
    }

    let reports = [
        // Round 3: the outcome is the second word.
        "[coord] build failed on PR 12",
        "[coord] fix landed on PR 12",
        "[coord] build is green on #1740",
        "[coord] update on PR 57: merged",
        // Rounds 4 and 6: the object comes before the outcome.
        "[coord] fix #12 merged",
        "[coord] build #1740 failed",
        "[coord] update #57 merged",
        "[coord] build #1740 is green",
        "[coord] fix #12 has landed",
        "[coord] build on main failed",
        "[coord] update PR 57 merged, nothing to do",
        "[coord] fix for #12 landed",
        "[coord] build #1740 failed on main",
        "[coord] fix #12 reverted",
        "[coord] build #1740 green",
    ];
    for report in reports {
        assert_note(&run_hook(s, None, report));
        let state = iso.read_state("s-reports").unwrap();
        assert_eq!(last_rule(&state), Some("coordination_marker"), "{report:?}");
        assert_eq!(classified_turn(&state), Some(1), "{report:?}");
    }

    // Outside the window a marked request is routed, as the same text
    // without the marker would be, and starts a new window.
    assert_router(&run_hook(s, None, "[coord] fix failed tests on PR 12"));
    let state = iso.read_state("s-reports").unwrap();
    assert_eq!(last_rule(&state), Some("coordination_explicit_dev"));
    let request_turn = 5 + reports.len() as u64;
    assert_eq!(classified_turn(&state), Some(request_turn));

    assert_note(&run_hook(s, None, "[coord] fix #12 rolled back"));
    let state = iso.read_state("s-reports").unwrap();
    assert_eq!(last_rule(&state), Some("coordination_throttled"));
    assert_eq!(classified_turn(&state), Some(request_turn));
}

/// Review round 6 of PR #1752: whatever the work-request rule gets wrong, the
/// marker never adds a routing prompt. Each body is sent with and without the
/// marker to two sessions in the same state, and the marked one must get:
///
/// - the same empty output, for an explicit `/dev` command;
/// - the routing prompt exactly when the unmarked body does, and the note
///   otherwise, for a body read as a work request;
/// - the note, for any other body.
#[test]
fn the_marker_never_adds_a_routing_prompt() {
    let iso = Isolated::new();
    let bodies = [
        // Work requests.
        "build #1740",
        "fix the parser for #12",
        "fix failed tests on PR 12",
        // Reports that the wording rule reads as requests.
        "fix #12 rolled back",
        "build #1740 timed out",
        // Reports that the wording rule recognises.
        "fix #12 merged",
        "build failed on PR 12",
        // Follow-up and new-topic wording, which the topic decision reads.
        "fix and test the parser",
        "build #1740, new task",
        // Neither a request nor a report.
        "report your lane status",
        "/dev fix the parser",
    ];
    // (last classified turn, current turn): no classification yet, inside the
    // window, at its edge, and past it.
    let states = [
        (None, 0),
        (None, 1),
        (None, 5),
        (Some(10), 11),
        (Some(10), 13),
        (Some(10), 14),
        (Some(10), 30),
    ];
    let routed = |out: &Value| {
        out.get("additionalContext").and_then(Value::as_str) == Some(DEFAULT_ROUTING_PROMPT)
    };
    for (i, body) in bodies.iter().enumerate() {
        for (j, (seed, turn)) in states.iter().enumerate() {
            let marked_session = format!("s-mark-{i}-{j}");
            let plain_session = format!("s-plain-{i}-{j}");
            if let Some(classified) = seed {
                iso.seed_classified(&marked_session, *classified);
                iso.seed_classified(&plain_session, *classified);
            }
            let marked = run_hook(
                Some(&marked_session),
                Some(*turn),
                &format!("[coord] {body}"),
            );
            let plain = run_hook(Some(&plain_session), Some(*turn), body);
            let case = format!("{body:?} at turn {turn}, last classified {seed:?}");

            if body.starts_with("/dev") {
                assert_eq!(marked, empty(), "{case}");
                assert_eq!(plain, empty(), "{case}");
            } else if is_explicit_work_request(body) {
                assert_eq!(routed(&marked), routed(&plain), "{case}");
                if !routed(&marked) {
                    assert_note(&marked);
                }
                assert_eq!(
                    classified_turn(&iso.read_state(&marked_session).unwrap()),
                    classified_turn(&iso.read_state(&plain_session).unwrap()),
                    "{case}: the marker must not change the throttle window"
                );
            } else {
                assert_note(&marked);
            }
        }
    }
}
