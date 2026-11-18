//! #2784: with base_branch empty, step-16 resolves the base from origin/HEAD.
//! An open pull request from this branch into another base must be adopted,
//! not duplicated by a second `gh pr create` into the default branch.
//!
//! Every case runs the shipped step-16 body through the unchanged
//! `PublishFixture`: a local repository whose origin/HEAD is origin/main, a
//! stub `gh` that logs each call, and no network. origin/dev is created at
//! origin/main so an adopted `dev` base passes the base checks for the right
//! reason.
use super::fixture::{BRANCH, PublishFixture, describe, json};
use serde_json::Value;
use std::fs;
use std::process::Output;

const KEPT_DEV: &str = "existing open PR #615 targets dev; the resolved base was main; kept dev";
const COLLISION: &str = "adopted existing open PR after create collision";
const BASE_CHECK_FAILED: &str =
    "existing open PR #615 targets dev, which fails the base checks; no pull request was created";

/// The fixture with origin/dev beside origin/main (origin/HEAD stays main).
fn fixture() -> PublishFixture {
    let f = PublishFixture::new();
    f.git(&[
        "update-ref",
        "refs/remotes/origin/dev",
        "refs/remotes/origin/main",
    ]);
    f
}

/// A same-repository pull request from this branch with its own number, URL,
/// state and base.
fn pr_on(f: &PublishFixture, state: &str, number: u64, base: &str) -> Value {
    let mut pr = f.pr(state, BRANCH);
    pr["number"] = number.into();
    pr["url"] = format!("https://github.com/owner/repo/pull/{number}").into();
    pr["baseRefName"] = base.into();
    pr
}

fn open_pr(f: &PublishFixture, number: u64, base: &str) -> Value {
    pr_on(f, "OPEN", number, base)
}

/// What `gh pr list --head <branch>` answers before any create.
fn listed(f: &PublishFixture, prs: &[Value]) {
    f.write("prs.json", &Value::Array(prs.to_vec()).to_string());
}

fn gh_calls(f: &PublishFixture, operation: &str) -> usize {
    f.read("gh.log")
        .lines()
        .filter(|line| line.starts_with(operation))
        .count()
}

fn create_args(f: &PublishFixture) -> Vec<Vec<u8>> {
    fs::read(f.tmp.path().join("create.argv"))
        .unwrap_or_default()
        .split(|b| *b == 0)
        .map(<[u8]>::to_vec)
        .collect()
}

fn assert_no_create(f: &PublishFixture, output: &Output) {
    assert_eq!(
        gh_calls(f, "pr create"),
        0,
        "no pull request may be created:\n{}\ngh.log:\n{}",
        describe(output),
        f.read("gh.log")
    );
    assert!(
        !f.tmp.path().join("create.argv").exists(),
        "gh pr create must not be reached: {}",
        describe(output)
    );
}

/// Exit 0 with the existing pull request recorded as this run's pull request.
fn assert_adopted(output: &Output, number: u64, base: &str, message: &str) -> Value {
    assert!(output.status.success(), "{}", describe(output));
    let result = json(output);
    assert_eq!(result["state"], "existing-open-pr", "{result}");
    assert_eq!(result["legacy_state"], "existing-open-pr", "{result}");
    assert_eq!(result["terminal_status"], "success", "{result}");
    assert_eq!(result["pr_number"], number.to_string(), "{result}");
    assert_eq!(
        result["pr_url"],
        format!("https://github.com/owner/repo/pull/{number}"),
        "{result}"
    );
    assert_eq!(result["base_branch"], base, "{result}");
    assert_eq!(result["message"], message, "{result}");
    result
}

// The issue's case (acceptance 1): PR #615 from this branch targets dev,
// origin/HEAD is main and base_branch is empty. Before the fix the lookup was
// scoped to main, found nothing, and `gh pr create` opened a second PR.
#[test]
fn open_pr_on_another_base_is_adopted_and_nothing_is_created() {
    let f = fixture();
    listed(&f, &[open_pr(&f, 615, "dev")]);
    let output = f.publish();
    assert_adopted(&output, 615, "dev", KEPT_DEV);
    assert_no_create(&f, &output);
    assert!(
        f.read("gh.log")
            .lines()
            .any(|l| l.starts_with("pr list") && l.contains(&format!("--head {BRANCH}"))),
        "the branch's pull requests must be listed:\n{}",
        f.read("gh.log")
    );
}

// Same-repository identity is unchanged: a pull request from a fork whose
// branch has the same name is never this run's pull request.
#[test]
fn fork_pr_on_another_base_is_not_adopted() {
    let f = fixture();
    let mut fork = open_pr(&f, 615, "dev");
    fork["isCrossRepository"] = true.into();
    fork["headRepositoryOwner"] = serde_json::json!({"login": "fork"});
    listed(&f, &[fork]);
    let result = f.assert_created(&f.publish());
    assert_eq!(gh_calls(&f, "pr create"), 1, "{}", f.read("gh.log"));
    assert!(
        !create_args(&f).iter().any(|a| a.starts_with(b"--base")),
        "without base_branch, gh pr create names no base, as before"
    );
    assert_eq!(result["base_branch"], "", "{result}");
}

// Two open pull requests from one head on two other bases share a head SHA,
// so nothing can choose between them: the step stops and creates nothing.
#[test]
fn two_open_prs_on_other_bases_stop_without_a_create() {
    let f = fixture();
    f.git(&[
        "update-ref",
        "refs/remotes/origin/release",
        "refs/remotes/origin/main",
    ]);
    listed(&f, &[open_pr(&f, 615, "dev"), open_pr(&f, 616, "release")]);
    let output = f.publish();
    assert!(!output.status.success(), "{}", describe(&output));
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("FAILED_PR_IDENTITY (exit 1): scoped PR lookup failed: multiple_scoped_prs"),
        "{}",
        describe(&output)
    );
    assert!(
        !String::from_utf8_lossy(&output.stdout).contains("\"terminal_status\":\"success\""),
        "{}",
        describe(&output)
    );
    assert_no_create(&f, &output);
}

// An open pull request on another base takes precedence over a closed one
// into the resolved base, which used to end as FAILED_CLOSED_UNMERGED.
#[test]
fn open_pr_on_another_base_wins_over_a_closed_pr_on_the_resolved_base() {
    let f = fixture();
    listed(
        &f,
        &[pr_on(&f, "CLOSED", 99, "main"), open_pr(&f, 615, "dev")],
    );
    let output = f.publish();
    assert_adopted(&output, 615, "dev", KEPT_DEV);
    assert_no_create(&f, &output);
}

// An open pull request into the resolved base is still used first, with the
// message and recorded base unchanged.
#[test]
fn open_pr_on_the_resolved_base_wins_over_one_on_another_base() {
    let f = fixture();
    listed(&f, &[open_pr(&f, 99, "main"), open_pr(&f, 615, "dev")]);
    let output = f.publish();
    assert_adopted(&output, 99, "main", "existing open PR found for branch");
    assert_no_create(&f, &output);
}

// With base_branch set there is no discovery, as before: an open pull
// request on another base does not stop the create into base_branch.
#[test]
fn base_branch_set_creates_into_it_despite_an_open_pr_on_another_base() {
    let f = fixture();
    listed(&f, &[open_pr(&f, 615, "dev")]);
    let output = f
        .publish_command()
        .env("RECIPE_VAR_base_branch", "main")
        .output()
        .unwrap();
    let result = f.assert_created(&output);
    assert_eq!(gh_calls(&f, "pr create"), 1, "{}", f.read("gh.log"));
    assert!(
        create_args(&f).contains(&b"--base=main".to_vec()),
        "gh pr create must name base_branch"
    );
    assert_eq!(result["base_branch"], "main", "{result}");
}

// The post-create collision lookup uses the same rule. A collision with a
// pull request into the resolved base keeps today's message; one into
// another base records that base and names it.
#[test]
fn create_collision_adopts_an_open_pr_on_another_base() {
    for (number, base, message) in [
        (99, "main", COLLISION.to_string()),
        (615, "dev", format!("{COLLISION}; {KEPT_DEV}")),
    ] {
        let f = fixture();
        f.write(
            "create.stderr",
            &format!("a pull request for branch \"{BRANCH}\" into branch \"main\" already exists"),
        );
        f.write(
            "collision.json",
            &Value::Array(vec![open_pr(&f, number, base)]).to_string(),
        );
        let output = f
            .publish_command()
            .env("CREATE_STATUS", "23")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "collision with a PR into {base}: {}",
            describe(&output)
        );
        assert_adopted(&output, number, base, &message);
        assert_eq!(gh_calls(&f, "pr create"), 1, "{}", f.read("gh.log"));
    }
}

// An adopted base comes from GitHub, so it passes the same checks as an
// explicit base_branch before it is recorded or labelled: a forbidden base,
// or one with no origin/<base>, stops the step and nothing is created.
#[test]
fn adopted_base_that_fails_the_base_checks_stops_without_a_create() {
    for (forbidden, with_dev, reason) in [
        ("dev", true, "matches forbidden_base_branches pattern 'dev'"),
        ("", false, "has no remote-tracking ref origin/dev"),
    ] {
        let f = if with_dev {
            fixture()
        } else {
            PublishFixture::new()
        };
        listed(&f, &[open_pr(&f, 615, "dev")]);
        let output = f
            .publish_command()
            .env("RECIPE_VAR_forbidden_base_branches", forbidden)
            .env("WORKFLOW_PR_LABELS", "ready-for-review")
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1), "{}", describe(&output));
        let result = json(&output);
        assert_eq!(result["state"], "FAILED_PR_IDENTITY", "{result}");
        assert_eq!(result["legacy_state"], "invalid-pr-identity", "{result}");
        assert_eq!(result["terminal_status"], "failure", "{result}");
        assert_eq!(result["base_branch"], "", "{result}");
        assert_eq!(result["message"], BASE_CHECK_FAILED, "{result}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(reason),
            "expected {reason:?}: {}",
            describe(&output)
        );
        assert_no_create(&f, &output);
        assert_eq!(
            gh_calls(&f, "pr edit"),
            0,
            "no label may be applied to a pull request whose base failed the checks:\n{}",
            f.read("gh.log")
        );
    }
}
