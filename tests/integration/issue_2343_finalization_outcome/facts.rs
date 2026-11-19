//! `finalization_outcome_facts` collects; it does not judge.
//!
//! These cases drive the reader directly. They assert the facts it reports
//! and the sentences it writes for a fact it could not read. None of them
//! asserts a verdict, a relation label or a terminal state, because the reader
//! produces none: a case here that started asserting one would be asserting
//! that the classifier came back.

use super::fixture_ext::*;
use crate::issue_1641_finalization_open_pr::builders::{green_rollup, open_pr};
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
    "unread",
];

fn assert_shape(value: &Value) {
    assert_eq!(value["schema_version"], json!(2), "schema version: {value}");
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

#[test]
fn the_reader_names_no_terminal_state_and_ends_nothing() {
    // Code only: the header says in prose that the reader reaches no verdict,
    // which is the point, not a violation of it.
    let text: String = tool_text("workflow_finalization_outcome.sh")
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n");
    for forbidden in [
        "EXISTING_OPEN_PR",
        "BLOCKED_CI",
        "terminal_state",
        "classify_",
        "verdict",
    ] {
        assert!(
            !text.contains(forbidden),
            "the facts reader must not contain `{forbidden}`"
        );
    }
    assert!(
        !std::path::Path::new(&tool_path("workflow_finalization_outcome_classify.sh")).exists(),
        "the bash outcome classifier must stay deleted (#2343)"
    );
}
