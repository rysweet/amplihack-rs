//! #2784: with base_branch empty, step-16 resolves the base from origin/HEAD.
//! An open pull request from this branch into another base must be adopted,
//! not duplicated by a second `gh pr create` into the default branch.
//!
//! Every case runs the shipped step-16 body through the unchanged
//! `PublishFixture`: a local repository whose origin/HEAD is origin/main, a
//! stub `gh` that logs each call, and no network. origin/dev is created at
//! origin/main, as in a clone that has fetched `dev`.
use super::fixture::{BRANCH, PublishFixture, describe, json};
use serde_json::Value;
use std::fs;
use std::process::Output;

const KEPT_DEV: &str = "existing open PR #615 targets dev; the resolved base was main; kept dev";
const COLLISION: &str = "adopted existing open PR after create collision";

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
        .expect("read recorded gh pr create arguments")
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

// When two open pull requests on other bases appear between the first lookup
// and `gh pr create`, the collision lookup cannot choose either: the create
// failure stands, with the lookup's failure appended, and nothing is labelled.
#[test]
fn create_collision_with_two_open_prs_on_other_bases_fails_the_create() {
    let f = fixture();
    f.write(
        "create.stderr",
        &format!("a pull request for branch \"{BRANCH}\" into branch \"main\" already exists"),
    );
    f.write(
        "collision.json",
        &Value::Array(vec![open_pr(&f, 615, "dev"), open_pr(&f, 616, "staging")]).to_string(),
    );
    let output = f
        .publish_command()
        .env("CREATE_STATUS", "23")
        .env("WORKFLOW_PR_LABELS", "self-merge-eligible")
        .output()
        .unwrap();
    assert!(!output.status.success(), "{}", describe(&output));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("FAILED_PR_CREATE")
            && stderr.contains("; scoped collision lookup failed (exit 1)"),
        "{}",
        describe(&output)
    );
    assert!(
        !String::from_utf8_lossy(&output.stdout).contains("\"terminal_status\":\"success\""),
        "{}",
        describe(&output)
    );
    assert_eq!(gh_calls(&f, "pr create"), 1, "{}", f.read("gh.log"));
    assert_eq!(gh_calls(&f, "pr edit"), 0, "{}", f.read("gh.log"));
}

/// Step-16 with WORKFLOW_PR_LABELS set and the given forbidden_base_branches,
/// with PR #615 into dev as the branch's only pull request.
fn publish_labelled(f: &PublishFixture, forbidden: &str) -> Output {
    listed(f, &[open_pr(f, 615, "dev")]);
    f.publish_command()
        .env("WORKFLOW_PR_LABELS", "self-merge-eligible")
        .env("RECIPE_VAR_forbidden_base_branches", forbidden)
        .output()
        .unwrap()
}

// #2784 preserves the existing label contract: base policy belongs to
// finalization. A configured label alone does not authorize a merge.
#[test]
fn labels_apply_to_an_adopted_pr_even_when_its_base_is_forbidden() {
    let f = fixture();
    let output = publish_labelled(&f, "dev");
    assert_adopted(&output, 615, "dev", KEPT_DEV);
    assert_eq!(gh_calls(&f, "pr edit"), 1, "{}", f.read("gh.log"));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("best-effort labels"),
        "{}",
        describe(&output)
    );
}

// Fetch availability also does not change publish-time labeling. Finalize
// still refuses a recorded base that has no remote-tracking reference.
#[test]
fn labels_apply_to_an_adopted_pr_even_when_its_base_is_unfetched() {
    let f = PublishFixture::new();
    let output = publish_labelled(&f, "");
    assert_adopted(&output, 615, "dev", KEPT_DEV);
    assert_eq!(gh_calls(&f, "pr edit"), 1, "{}", f.read("gh.log"));
}

// The allowed-base control. The fixture's gh answers `pr edit` with exit 96,
// which the label step reports as a warning without changing the result.
#[test]
fn labels_apply_to_an_adopted_pr_whose_base_is_allowed() {
    let f = fixture();
    let output = publish_labelled(&f, "release/*");
    assert_adopted(&output, 615, "dev", KEPT_DEV);
    assert!(
        f.read("gh.log").lines().any(
            |l| l == "pr edit 615 --repo github.com/owner/repo --add-label self-merge-eligible"
        ),
        "gh.log:\n{}",
        f.read("gh.log")
    );
    assert!(
        !String::from_utf8_lossy(&output.stderr).contains("not applied to adopted PR"),
        "{}",
        describe(&output)
    );
}
