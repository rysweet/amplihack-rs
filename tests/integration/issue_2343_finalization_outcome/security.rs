//! The trust boundary (#2343, #2809).
//!
//! Everything finalization is handed -- the assessment's own JSON, the run's
//! recorded branch name, the worktree's origin URL -- comes from outside it and
//! none of it is trusted. These are the cases the security review named: S1
//! (what an assessment may and may not put into the result), S2 (a GitHub
//! origin whose pull request cannot be read), S4 (a branch name git itself
//! rejects) and S5 (an origin URL carrying userinfo).
//!
//! S1 changed shape with the design it covers. The assessment now decides the
//! terminal state, so the old case -- a claimed success that a mechanical
//! re-read overruled -- no longer describes anything in the tree. What is still
//! true, and is what these cases hold it to, is that the assessment supplies a
//! state and words and nothing else: the pull request the result names is
//! built from the remote the facts report, so no text the assessment returns
//! can put a repository, a URL or a commit into the result.

use super::fixture_ext::*;
use crate::issue_1641_finalization_open_pr::builders::{
    as_refs, assert_no_leak, green_rollup, set_branch,
};
use crate::issue_1641_finalization_open_pr::fixture::*;
use serde_json::json;

fn resumed(fx: &Fixture) -> Vec<(&'static str, String)> {
    crate::issue_1641_finalization_open_pr::builders::resumed_env(&fx.repo_path())
}

/// Neither stream may carry `text`, whatever the outcome was.
fn assert_absent(outcome: &Outcome, text: &str, why: &str) {
    for stream in [&outcome.stdout, &outcome.stderr] {
        assert!(
            !stream.contains(text),
            "{why}: `{text}` must never be emitted\n{}",
            outcome.describe()
        );
    }
}

/// An assessment carrying hostile identity fields beside its decision.
fn hostile_assessment(terminal_state: &str, pr_number: &str) -> (&'static str, String) {
    (
        "RECIPE_VAR_finalization_assessment",
        json!({
            "terminal_state": terminal_state,
            "action_taken": "opened_pull_request",
            "pr_number": pr_number,
            "pr_url": "https://evil.example.com/rysweet/fixture/pull/9",
            "remote": "attacker/repo",
            "reason": "opened the pull request at https://evil.example.com/rysweet/fixture/pull/9",
            "next_action": "merge it",
        })
        .to_string(),
    )
}

// S1a. The URL in the result is built from the remote the facts report. A URL
// the assessment supplies is never adopted, whatever it claims.
//
// `terminal_reason` and `required_next_action` ARE the assessment's own
// sentences -- that is what it is for -- so they are not asserted clean here;
// they are cleaned of control characters and capped in length, and they drive
// nothing. The identity fields and the evidence record are the ones no text
// from the assessment may reach.
#[test]
fn an_assessment_cannot_put_its_own_pull_request_url_into_the_result() {
    let fx = Fixture::new();
    fx.with_remote();
    fx.push();
    fx.gh_pr_at(&fx.head(), green_rollup());
    let mut envs = finished_env(&fx.repo_path());
    envs.push(hostile_assessment("EXISTING_OPEN_PR", "7"));

    let outcome = fx.validate(&as_refs(&envs));

    assert!(outcome.success(), "{}", outcome.describe());
    assert_eq!(outcome.field("pr_url"), PR_URL);
    assert_eq!(outcome.field("pr_number"), "7");
    for field in ["pr_url", "pr_number", "evidence_used"] {
        let value = outcome.field(field);
        for hostile in ["evil.example.com", "attacker/repo"] {
            assert!(
                !value.contains(hostile),
                "`{field}` is built from the facts, not from the assessment: {value}"
            );
        }
    }
}

// S1b. A pull request number that is not a canonical number names no pull
// request. Finalization does not substitute the one the facts happen to have
// read: the result is about the request the decision was about, or about none.
#[test]
fn a_malformed_pull_request_number_names_no_pull_request() {
    let fx = Fixture::new();
    fx.with_remote();
    fx.push();
    fx.gh_pr_at(&fx.head(), green_rollup());
    let mut envs = finished_env(&fx.repo_path());
    envs.push(hostile_assessment("EXISTING_OPEN_PR", "7 OR 1=1"));

    let outcome = fx.validate(&as_refs(&envs));

    // Nothing is derived from the hostile value: the result names no pull
    // request of the assessment's, and `pr_url` stays the one the run itself
    // recorded, which is the reporting default and predates this change.
    assert_eq!(outcome.field("pr_number"), "");
    assert_eq!(outcome.field("pr_url"), PR_URL);
    // The value is quoted back once, in the human reason, exactly as an
    // unusable `terminal_state` is (#2819 R1 asked for a reason that names
    // what came back). That is assessment text, which `terminal_reason` has
    // always carried: cleaned of control characters and cut to 40 characters.
    // What must not happen is that it becomes an identity, and it does not --
    // it reaches neither `pr_number` nor `pr_url`, and gh is never asked about
    // it.
    let reason = outcome.field("terminal_reason");
    assert!(reason.contains("7 OR 1=1"), "{reason}");
    assert!(
        !outcome.field("evidence_used").contains("1=1"),
        "the record of the facts is not the assessment's text: {}",
        outcome.field("evidence_used")
    );
    for call in fx.gh_calls() {
        assert!(
            !call.contains("1=1"),
            "a malformed number must never reach gh: {call}"
        );
    }
    // Because the state it claimed IS an open pull request, a decision that
    // named none of them is not published at all. The run goes to its recovery
    // agent instead of ending a success that showed nothing.
    assert!(!outcome.success(), "{}", outcome.describe());
    assert_eq!(
        outcome.field("terminal_state"),
        "FAILED_MISSING_TERMINAL_EVIDENCE",
        "{}",
        outcome.describe()
    );
    assert_eq!(outcome.field("terminal_success"), "false");
    // The facts still carry the request that was read, so the record shows
    // which one the decision was made beside.
    assert!(
        outcome.field("evidence_used").contains("outcome.pr=7"),
        "the gathered facts stay in the record: {}",
        outcome.field("evidence_used")
    );
}

// S1c. With nothing published there is no pull request to name, so a success
// the assessment decides names none, and the hand-off brick has nothing to
// hand on. The assessment's claim about having opened one changes nothing.
#[test]
fn an_assessment_that_names_a_pull_request_the_facts_never_saw_names_none() {
    let fx = Fixture::new();
    fx.with_remote();
    fx.gh_no_pr();
    let mut envs = resumed(fx_ref(&fx));
    envs.push((
        "RECIPE_VAR_finalization_assessment",
        json!({
            "terminal_state": "FAILED_IMPLEMENTATION",
            "action_taken": "opened_pull_request",
            "pr_number": "",
            "reason": "nothing was published for this run",
            "next_action": "Push the branch and open a pull request.",
        })
        .to_string(),
    ));

    let outcome = fx.validate(&as_refs(&envs));

    assert!(!outcome.success(), "{}", outcome.describe());
    assert_eq!(outcome.field("terminal_state"), "FAILED_IMPLEMENTATION");
    assert_eq!(outcome.field("pr_number"), "");
    assert_eq!(outcome.field("pr_url"), "");
    fx.assert_gh_reads_only();
}

fn fx_ref(fx: &Fixture) -> &Fixture {
    fx
}

// S2. A GitHub origin whose pull request cannot be read has an unread fact,
// not an absent one. Nothing stands in for it: with no decision the run ends
// with none, and the reason names what could not be read.
#[test]
fn an_unreadable_github_outcome_never_becomes_a_success() {
    let fx = Fixture::new();
    fx.with_remote();
    fx.push();
    fx.gh_unreachable();
    let mut envs = resumed(&fx);
    envs.push(("IMPLEMENTATION_COMPLETED", "true".to_owned()));
    envs.push(("VERIFICATION_COMPLETED", "true".to_owned()));

    let outcome = fx.validate(&as_refs(&envs));

    assert!(
        !outcome.success(),
        "an unread GitHub outcome is a gap, not a success\n{}",
        outcome.describe()
    );
    assert_ne!(outcome.field("terminal_state"), "IMPLEMENTED_VERIFIED");
    assert_eq!(outcome.field("terminal_success"), "false");
    let reason = outcome.field("terminal_reason");
    assert!(
        reason.contains("could not be") && reason.contains(BRANCH),
        "the unread fact must be named, not the missing bookkeeping: {reason}\n{}",
        outcome.describe()
    );
    assert_no_leak(&outcome);
}

// S4. A branch name git itself rejects never reaches a git or gh argument.
// The read fails soft -- one JSON object, no crash -- and nothing from the
// name appears in it, at the facts level or in the result.
#[test]
fn a_branch_name_git_rejects_reaches_neither_git_nor_the_result() {
    for hostile in ["feat/evilmarker..x", "-evilmarker", "feat/evilmarker\nx"] {
        let fx = Fixture::new();
        fx.with_remote();

        let (value, facts) = fx.facts(hostile, None);
        assert!(
            facts.success(),
            "the reader must fail soft, not abort\n{}",
            facts.describe()
        );
        assert_eq!(
            value["branch"], "",
            "a rejected name is not reported as the branch"
        );
        assert!(
            value["unread"]
                .as_str()
                .expect("unread is a String")
                .contains("valid branch name"),
            "the reader must say why it could not read: {value}"
        );
        assert_absent(&facts, "evilmarker", "a rejected branch name");
        fx.assert_gh_not_called("a branch name git rejects is never asked about");

        let mut envs = resumed(&fx);
        set_branch(&mut envs, hostile);
        let outcome = fx.validate(&as_refs(&envs));
        assert_eq!(outcome.field("terminal_success"), "false");
        assert_eq!(
            outcome.output.status.code(),
            Some(1),
            "the step is left failing\n{}",
            outcome.describe()
        );
        assert_absent(&outcome, "evilmarker", "a rejected branch name");
    }
}

// S5. The origin URL is never reported: the remote is named as owner/repo
// through parse_github_repo_identity, which discards the userinfo.
#[test]
fn userinfo_in_the_origin_url_never_reaches_the_result() {
    let fx = Fixture::new();
    fx.with_remote();
    fx.push();
    // Only after the push: the fixture's `insteadOf` rewrite matches the bare
    // https://github.com/ prefix, so a userinfo-bearing origin would send the
    // push to the real host.
    fx.origin_with_userinfo();
    fx.gh_pr_at(&fx.head(), green_rollup());

    let (_, facts) = fx.facts(BRANCH, None);
    let outcome = fx.validate(&as_refs(&decided(&fx, "EXISTING_OPEN_PR", "7")));

    assert!(
        outcome.success(),
        "an open pull request at the local head is still the success\n{}",
        outcome.describe()
    );
    assert_eq!(outcome.field("terminal_state"), "EXISTING_OPEN_PR");
    for secret in [ORIGIN_USERINFO, "ghp-s3cr3t-2343", "ci-bot"] {
        assert_absent(&outcome, secret, "origin userinfo");
        assert_absent(&facts, secret, "origin userinfo in the facts");
    }
}

// The same holds when the read fails: a failure's own explanation is built
// from the facts, and the origin is not one of them.
#[test]
fn userinfo_stays_out_of_a_failing_result_too() {
    let fx = Fixture::new();
    fx.with_remote();
    fx.origin_with_userinfo();
    fx.gh_unreachable();

    let outcome = fx.validate(&as_refs(&resumed(&fx)));

    assert!(!outcome.success(), "{}", outcome.describe());
    for secret in [ORIGIN_USERINFO, "ghp-s3cr3t-2343", "ci-bot"] {
        assert_absent(&outcome, secret, "origin userinfo on the failure path");
    }
}
