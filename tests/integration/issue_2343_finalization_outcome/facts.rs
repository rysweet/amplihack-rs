//! `finalization_outcome_facts` collects; it does not judge.
//!
//! These cases drive the reader directly. They assert the facts it reports
//! and the sentences it writes for a fact it could not read. None of them
//! asserts a verdict, a relation label or a terminal state, because the reader
//! produces none: a case here that started asserting one would be asserting
//! that the classifier came back.
//!
//! Nothing here reads the reader'"'"'s source. That it names no terminal state is
//! asserted by running it over the outcomes a classifier would have split on
//! and looking at what it printed, which is the only place a decision could
//! go.

use super::fixture_ext::*;
use crate::issue_1641_finalization_open_pr::builders::{check_run_named, green_rollup, open_pr};
use crate::issue_1641_finalization_open_pr::fixture::*;
use serde_json::{Value, json};

/// Every field the contract names, so a dropped one is caught here rather than
/// by a consumer reading `null`.
const FIELDS: &[&str] = &[
    "branch",
    "local_head",
    "remote",
    "github_origin",
    "remote_head",
    "remote_head_descends_local_head",
    "open_pr_count",
    "pr_number",
    "pr_url",
    "pr_state",
    "pr_draft",
    "pr_head",
    "pr_cross_repository",
    "pr_head_descends_local_head",
    "checks",
    "check_states",
    "unread",
];

fn assert_shape(value: &Value) {
    assert_eq!(value["schema_version"], json!(3), "schema version: {value}");
    for field in FIELDS {
        assert!(
            value[*field].is_string(),
            "field `{field}` must be a String: {value}"
        );
    }
    for forbidden in ["verdict", "gap", "branch_pushed", "pr_head_relation"] {
        assert!(
            value[forbidden].is_null(),
            "the reader reports facts; `{forbidden}` is a judgement: {value}"
        );
    }
}

#[test]
fn a_published_branch_with_an_open_green_pull_request_reads_every_fact() {
    let fx = Fixture::new();
    fx.with_remote();
    fx.push();
    let head = fx.head();
    fx.gh_pr_at(&head, green_rollup());

    let (value, outcome) = fx.facts(BRANCH, None);

    assert_shape(&value);
    assert!(
        outcome.success(),
        "the reader always exits 0\n{}",
        outcome.describe()
    );
    assert_eq!(value["branch"], BRANCH);
    assert_eq!(value["local_head"], head);
    assert_eq!(value["remote"], REPO);
    assert_eq!(value["github_origin"], "true");
    assert_eq!(value["remote_head"], head);
    assert_eq!(value["remote_head_descends_local_head"], "true");
    assert_eq!(value["open_pr_count"], "1");
    assert_eq!(value["pr_number"], "7");
    assert_eq!(value["pr_url"], PR_URL);
    assert_eq!(value["pr_state"], "OPEN");
    assert_eq!(value["pr_draft"], "false");
    assert_eq!(value["pr_head"], head);
    assert_eq!(value["pr_cross_repository"], "false");
    assert_eq!(value["pr_head_descends_local_head"], "true");
    assert_eq!(value["checks"], "green");
    assert_eq!(value["unread"], "", "nothing was unreadable: {value}");
    fx.assert_gh_reads_only();
}

// A branch the remote does not carry is a READ fact, not an unread one:
// `git ls-remote` answered and listed it nowhere.
#[test]
fn a_branch_the_remote_does_not_carry_reads_as_an_empty_remote_head() {
    let fx = Fixture::new();
    fx.with_remote();
    fx.gh_no_pr();

    let (value, _) = fx.facts(BRANCH, None);

    assert_eq!(value["remote_head"], "");
    assert_eq!(value["remote_head_descends_local_head"], "");
    assert_eq!(value["open_pr_count"], "0");
    assert_eq!(value["pr_number"], "");
    assert_eq!(
        value["unread"], "",
        "both facts were read; neither is unread: {value}"
    );
}

#[test]
fn check_classes_are_reported_as_the_shared_rule_computes_them() {
    for (rollup, expected) in [
        (green_rollup(), "green"),
        (pending_rollup(), "pending"),
        (failing_rollup(), "failing"),
        (json!([]), "none"),
    ] {
        let fx = Fixture::new();
        fx.with_remote();
        fx.push();
        fx.gh_pr_at(&fx.head(), rollup.clone());

        assert_eq!(
            fx.fact(BRANCH, None, "checks"),
            expected,
            "rollup {rollup} must read as {expected}"
        );
    }
}

// The check class is a fact about the pull request that was read, not about
// whether that pull request is this run's. The heads are reported beside it
// and the comparison is the reader's business, not the collector's.
#[test]
fn a_pull_request_at_an_unrelated_head_still_reports_its_own_facts() {
    let fx = Fixture::new();
    fx.with_remote();
    fx.push();
    let local = fx.head();
    let other = fx.unrelated_commit();
    fx.gh_pr_at(&other, green_rollup());

    let (value, _) = fx.facts(BRANCH, None);

    assert_eq!(value["local_head"], local);
    assert_eq!(value["pr_head"], other);
    assert_eq!(value["pr_head_descends_local_head"], "false");
    assert_eq!(value["pr_state"], "OPEN");
    assert_eq!(value["checks"], "green");
}

// #1714: a pull request head that fast-forwards the local head. The ancestry
// answer is the fact; nothing here decides what it means.
#[test]
fn a_pull_request_head_that_fast_forwards_the_local_head_reads_as_a_descendant() {
    let fx = Fixture::new();
    fx.with_remote();
    fx.push();
    let local = fx.head();
    let ahead = fx.commit_more("ahead.txt");
    fx.push();
    fx.git(&["reset", "-q", "--hard", &local]);
    fx.gh_pr_at(&ahead, green_rollup());

    let (value, _) = fx.facts(BRANCH, None);

    assert_eq!(value["local_head"], local);
    assert_eq!(value["pr_head"], ahead);
    assert_eq!(value["pr_head_descends_local_head"], "true");
}

#[test]
fn a_draft_pull_request_reports_its_draft_flag() {
    let fx = Fixture::new();
    fx.with_remote();
    fx.push();
    let mut body = open_pr(green_rollup());
    body["headRefOid"] = json!(fx.head());
    body["isDraft"] = json!(true);
    fx.gh_view(&body);
    fx.gh_list(&json!([{"number": 7, "url": PR_URL, "isCrossRepository": false}]));

    assert_eq!(fx.fact(BRANCH, None, "pr_draft"), "true");
}

// An unreadable fact is named with the reason, and never coerced into a
// negative one.
#[test]
fn a_gh_that_will_not_answer_leaves_the_pull_request_unread_and_says_so() {
    let fx = Fixture::new();
    fx.with_remote();
    fx.push();
    fx.gh_unreachable();

    let (value, outcome) = fx.facts(BRANCH, None);

    assert_shape(&value);
    assert!(
        outcome.success(),
        "the reader always exits 0\n{}",
        outcome.describe()
    );
    assert_eq!(value["pr_number"], "");
    assert_eq!(value["pr_state"], "");
    assert_eq!(value["checks"], "");
    assert_eq!(value["open_pr_count"], "");
    let unread = value["unread"].as_str().expect("unread is a String");
    assert!(
        unread.contains("gh pr list") && unread.contains(BRANCH),
        "the unread fact must name the tool and the branch: {unread}"
    );
    // The branch WAS read from the remote; only the pull request was not.
    assert_eq!(value["remote_head"], fx.head());
    crate::issue_1641_finalization_open_pr::builders::assert_no_leak(&outcome);
}

// A pull request reference the run recorded that is neither a number nor the
// canonical URL for this origin is not widened to discovery, which would
// adopt some other pull request.
#[test]
fn a_foreign_pull_request_reference_reads_nothing_and_never_falls_back_to_a_search() {
    let fx = Fixture::new();
    fx.with_remote();
    fx.push();
    fx.gh_pr_at(&fx.head(), green_rollup());

    let (value, _) = fx.facts(BRANCH, Some("https://github.com/other/repo/pull/7"));

    assert_eq!(value["pr_number"], "");
    assert_eq!(value["open_pr_count"], "", "no list was asked for");
    let unread = value["unread"].as_str().expect("unread is a String");
    assert!(
        unread.contains("canonical URL"),
        "the reason must say why the reference was unusable: {unread}"
    );
    fx.assert_gh_not_called("a reference for another repository is never asked about");
}

/// How one case arranges the fixture into the outcome it is named for.
type Arrange = fn(&Fixture);

// The reader's whole contract, driven rather than read: over the four
// outcomes a classifier used to split on, it exits 0, prints one object of
// facts, and names no terminal state anywhere in it. A reader that started
// deciding would have to put the decision somewhere, and the only place it
// could go is the object asserted here.
#[test]
fn no_outcome_makes_the_reader_name_a_terminal_state_or_end_anything() {
    let published_states = [
        "EXISTING_OPEN_PR",
        "IMPLEMENTED_VERIFIED",
        "ALLOW_NO_OP",
        "NO_DIFF_SUCCESS",
        "BLOCKED_CI",
        "FAILED_IMPLEMENTATION",
        "FAILED_REPORTING",
        "HOLLOW_SUCCESS",
        "FAILED_MEANINGFUL_DIFF",
        "FAILED_CLOSED_UNMERGED",
        "FAILED_PR_METADATA_UNAVAILABLE",
    ];
    // What the run produced, in the four shapes a classifier would have split
    // on. Named so a failure says which outcome made the reader speak.
    let outcomes: [(&str, Arrange); 4] = [
        ("open green pull request at the pushed head", |fx| {
            fx.push();
            fx.gh_pr_at(&fx.head(), green_rollup());
        }),
        ("a failing check", |fx| {
            fx.push();
            fx.gh_pr_at(&fx.head(), failing_rollup());
        }),
        ("nothing pushed and no pull request", Fixture::gh_no_pr),
        ("a pull request at an unrelated commit", |fx| {
            fx.push();
            let other = fx.unrelated_commit();
            fx.gh_pr_at(&other, green_rollup());
        }),
    ];

    for (what, arrange) in outcomes {
        let fx = Fixture::new();
        fx.with_remote();
        arrange(&fx);

        let (value, outcome) = fx.facts(BRANCH, None);

        assert!(
            outcome.success(),
            "{what}: the reader ends nothing\n{}",
            outcome.describe()
        );
        assert_shape(&value);
        let text = value.to_string();
        for state in published_states {
            assert!(
                !text.contains(state),
                "{what}: the reader named the terminal state `{state}`: {text}"
            );
        }
        assert!(
            value["terminal_state"].is_null() && value["terminal_success"].is_null(),
            "{what}: the reader reports facts, not a terminal state: {text}"
        );
    }
}

// #2819 R6: the rollup class is a reading, so the states it was computed from
// are reported beside it. A reader handed only `failing` cannot see which
// check failed.
#[test]
fn the_check_states_are_reported_beside_the_class_that_rolls_them_up() {
    let fx = Fixture::new();
    fx.with_remote();
    fx.push();
    let mut body = open_pr(json!([
        check_run_named("unit", "COMPLETED", json!("FAILURE")),
        check_run_named("lint", "IN_PROGRESS", Value::Null),
    ]));
    body["headRefOid"] = json!(fx.head());
    fx.gh_view(&body);
    fx.gh_list(&json!([{"number": 7, "url": PR_URL, "isCrossRepository": false}]));

    let (value, _) = fx.facts(BRANCH, None);

    assert_eq!(value["checks"], "failing", "the class is still reported");
    let states = value["check_states"].as_str().expect("a String");
    for named in ["unit=COMPLETED/FAILURE", "lint=IN_PROGRESS"] {
        assert!(
            states.contains(named),
            "the states gh reported must reach the reader: {states}"
        );
    }
}

#[test]
fn a_pull_request_with_no_checks_reports_no_check_states() {
    let fx = Fixture::new();
    fx.with_remote();
    fx.push();
    fx.gh_pr_at(&fx.head(), json!([]));

    let (value, _) = fx.facts(BRANCH, None);

    assert_eq!(value["checks"], "none");
    assert_eq!(value["check_states"], "");
}

// #2819 R3: no branch name is refused for being a default branch. `main`,
// `master` and `HEAD` were blanked by name here, so a run recorded on one of
// them read as having produced nothing -- a decision taken in the collector,
// without the assessment, and one that never covered a repository whose
// default branch is `trunk`.
#[test]
fn a_run_branch_named_like_a_default_branch_is_read_like_any_other() {
    for name in ["master", "main"] {
        let fx = Fixture::new();
        fx.with_remote();
        fx.git(&["checkout", "-q", "-B", name]);
        fx.git(&["push", "-q", "origin", name]);
        let head = fx.head();
        fx.gh_pr_at(&head, green_rollup());

        let (value, _) = fx.facts(name, None);

        assert_eq!(value["branch"], name, "the branch is reported: {value}");
        assert_eq!(value["remote_head"], head, "its remote is read: {value}");
        assert_eq!(value["pr_number"], "7", "its pull request is read: {value}");
        assert_eq!(
            value["unread"], "",
            "a branch is not unread for its name: {value}"
        );
    }
}

// A branch name an argument parser would read as an option is still refused,
// and that is not a narrowing: it never reaches a git or gh argument.
#[test]
fn a_branch_name_that_looks_like_an_option_is_reported_unread() {
    let fx = Fixture::new();
    fx.with_remote();
    fx.gh_no_pr();

    let (value, _) = fx.facts("--upload-pack=touch /tmp/pwned", None);

    assert_eq!(value["branch"], "", "{value}");
    assert!(
        value["unread"]
            .as_str()
            .expect("a String")
            .contains("not a valid branch name"),
        "the reason must name the gap: {value}"
    );
}

// #2819 R3: a state gh printed is reported, not blanked for failing a shape
// the collector invented. Which state names gh may return is the assessment's
// business; a reader handed an empty `pr_state` cannot tell a state it did not
// recognise from a pull request it could not read.
#[test]
fn a_pull_request_state_outside_the_old_shape_is_reported_as_gh_printed_it() {
    let fx = Fixture::new();
    fx.with_remote();
    fx.push();
    let head = fx.head();
    let mut body = open_pr(green_rollup());
    body["headRefOid"] = json!(head);
    body["state"] = json!("Open");
    fx.gh_view(&body);
    fx.gh_list(&json!([{"number": 7, "url": PR_URL, "isCrossRepository": false}]));

    let (value, _) = fx.facts(BRANCH, None);

    assert_eq!(
        value["pr_state"], "Open",
        "the state is reported as gh printed it: {value}"
    );
    assert_eq!(
        value["unread"], "",
        "a state that was read is not an unread fact: {value}"
    );
}

// #2819 R3: the host the run declared is reported as recorded, never renamed
// to `other`. The gate below it is unchanged -- anything but github is not
// asked for a pull request -- but a reader can now tell an unusable recorded
// value from a run that really recorded `other`.
#[test]
fn a_declared_remote_host_is_reported_as_the_run_recorded_it() {
    let fx = Fixture::new();
    fx.with_remote();
    fx.push();
    fx.gh_pr_at(&fx.head(), green_rollup());

    let (value, _) = fx.facts_for_host(BRANCH, None, "azure devops");

    assert_eq!(
        value["declared_remote_host"], "azure devops",
        "the recorded value reaches the reader: {value}"
    );
    assert_eq!(
        value["pr_number"], "",
        "a non-github remote is still never asked for a pull request: {value}"
    );
    assert!(
        value["unread"]
            .as_str()
            .expect("a String")
            .contains("azure devops"),
        "the reason must quote what the run recorded: {value}"
    );
}
