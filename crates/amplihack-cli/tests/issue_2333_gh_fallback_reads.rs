//! T1, T2 (helper half) and T4 (helper): `autodrive-gh-read` and
//! `autodrive-gh-update-branch` under a fake `gh` that refuses GraphQL.

use serde_json::{Value, json};

use crate::support::auto_drive_light::show;
use crate::support::autodrive_gh_fake::{BRANCH, GhRepo, one_json, without_source};

const SECONDARY: &str = "secondary rate limit";

fn ok(o: &std::process::Output) -> Value {
    assert_eq!(o.status.code(), Some(0), "read must succeed: {}", show(o));
    one_json(o)
}

fn unavailable(o: &std::process::Output) -> Value {
    assert_eq!(
        o.status.code(),
        Some(3),
        "a read that fails on every path exits 3 (unavailable): {}",
        show(o)
    );
    let v = one_json(o);
    assert!(v["source"].is_null(), "{v}");
    assert!(v["unavailable"].is_object(), "{v}");
    v
}

fn graphql_calls(g: &GhRepo) -> usize {
    g.calls()
        .iter()
        .filter(|c| {
            c.starts_with("pr\u{1f}")
                || c.starts_with("api\u{1f}graphql")
                || c.starts_with("repo\u{1f}")
        })
        .count()
}

fn rest_calls(g: &GhRepo) -> usize {
    g.calls()
        .iter()
        .filter(|c| c.starts_with("api\u{1f}") && c.contains("repos/"))
        .count()
}

// ── T1: each read falls back and returns the GraphQL-equivalent values ──────

#[test]
fn pr_read_uses_graphql_when_it_answers() {
    let g = GhRepo::new();
    let v = ok(&g.read(&["pr", "7"]));
    assert_eq!(v["source"], "graphql", "{v}");
    assert_eq!(v["read"], "pr");
    assert_eq!(v["pr"], 7);
    assert_eq!(rest_calls(&g), 0, "no REST call when GraphQL answers");
}

#[test]
fn pr_read_falls_back_to_rest_with_the_graphql_spelling() {
    let g = GhRepo::new();
    let via_graphql = ok(&g.read(&["pr", "7"]));
    g.mode("gql", "refuse");
    let via_rest = ok(&g.read(&["pr", "7"]));
    assert_eq!(via_rest["source"], "rest", "{via_rest}");
    assert_eq!(
        via_rest,
        json!({
            "read": "pr", "pr": 7, "source": "rest", "state": "OPEN", "is_draft": false,
            "head_sha": g.head(), "head_ref": BRANCH, "base_ref": "main",
            "merge_state": "BLOCKED", "mergeable": "MERGEABLE", "merged_at": null
        })
    );
    assert_eq!(
        without_source(&via_graphql),
        without_source(&via_rest),
        "REST and GraphQL reads of one PR give equal output apart from source"
    );
    assert!(graphql_calls(&g) >= 2, "GraphQL is tried first every time");
}

#[test]
fn pr_read_maps_rest_merged_closed_draft_and_unknown_mergeability() {
    let g = GhRepo::new();
    g.mode("gql", "refuse");
    let mut pr: Value =
        serde_json::from_str(&std::fs::read_to_string(g.fix.join("rest-pr.json")).unwrap())
            .unwrap();
    pr["state"] = json!("closed");
    pr["merged_at"] = json!("2026-11-12T10:00:00Z");
    pr["draft"] = json!(true);
    pr["mergeable"] = Value::Null;
    pr["mergeable_state"] = json!("something-new");
    g.fixture("rest-pr.json", &pr);
    let v = ok(&g.read(&["pr", "7"]));
    assert_eq!(v["state"], "MERGED", "merged_at wins over closed: {v}");
    assert_eq!(v["is_draft"], true);
    assert_eq!(v["mergeable"], "UNKNOWN", "null mergeable is UNKNOWN: {v}");
    assert_eq!(
        v["merge_state"], "UNKNOWN",
        "an unlisted state is UNKNOWN: {v}"
    );
    assert_eq!(v["merged_at"], "2026-11-12T10:00:00Z");

    pr["merged_at"] = Value::Null;
    pr["mergeable"] = json!(false);
    pr["mergeable_state"] = json!("dirty");
    g.fixture("rest-pr.json", &pr);
    let v = ok(&g.read(&["pr", "7"]));
    assert_eq!(v["state"], "CLOSED", "{v}");
    assert_eq!(v["mergeable"], "CONFLICTING", "{v}");
    assert_eq!(v["merge_state"], "DIRTY", "{v}");
}

#[test]
fn pr_read_never_defaults_a_missing_draft_flag_to_not_draft() {
    let g = GhRepo::new();
    let load = |n: &str| -> Value {
        serde_json::from_str(&std::fs::read_to_string(g.fix.join(n)).unwrap()).unwrap()
    };
    let mut gql = load("gql-pr.json");
    gql.as_object_mut().unwrap().remove("isDraft");
    g.fixture("gql-pr.json", &gql);
    let mut rest = load("rest-pr.json");
    rest["draft"] = json!(true);
    g.fixture("rest-pr.json", &rest);
    let v = ok(&g.read(&["pr", "7"]));
    assert_eq!(
        v["source"], "rest",
        "GraphQL without isDraft falls through: {v}"
    );
    assert_eq!(v["is_draft"], true, "{v}");

    rest.as_object_mut().unwrap().remove("draft");
    g.fixture("rest-pr.json", &rest);
    let o = g.read(&["pr", "7"]);
    unavailable(&o);
    assert!(
        !String::from_utf8_lossy(&o.stdout).contains("is_draft"),
        "no draft flag is ever invented: {}",
        show(&o)
    );
}

#[test]
fn head_read_falls_back_to_rest() {
    let g = GhRepo::new();
    g.mode("gql", "refuse");
    let v = ok(&g.read(&["head", "7"]));
    assert_eq!(v["source"], "rest", "{v}");
    assert_eq!(v["head_sha"], g.head().as_str());
    assert_eq!(v["read"], "head");
}

#[test]
fn head_read_falls_back_to_ls_remote_when_rest_also_fails() {
    let g = GhRepo::new();
    g.publish_pull_ref();
    g.mode("gql", "refuse");
    g.mode("rest", "refuse");
    let v = ok(&g.read(&["head", "7"]));
    assert_eq!(v["source"], "ls-remote", "{v}");
    assert_eq!(v["head_sha"], g.head().as_str());
}

#[test]
fn head_read_via_graphql_equals_rest_and_ls_remote() {
    let g = GhRepo::new();
    g.publish_pull_ref();
    let a = ok(&g.read(&["head", "7"]));
    g.mode("gql", "refuse");
    let b = ok(&g.read(&["head", "7"]));
    g.mode("rest", "refuse");
    let c = ok(&g.read(&["head", "7"]));
    assert_eq!(
        [&a["source"], &b["source"], &c["source"]],
        [&json!("graphql"), &json!("rest"), &json!("ls-remote")]
    );
    assert_eq!(a["head_sha"], b["head_sha"]);
    assert_eq!(b["head_sha"], c["head_sha"]);
}

#[test]
fn checks_read_falls_back_to_rest_check_runs_and_legacy_statuses() {
    let g = GhRepo::new();
    let via_graphql = ok(&g.read(&["checks", "7"]));
    g.mode("gql", "refuse");
    let via_rest = ok(&g.read(&["checks", "7"]));
    assert_eq!(via_rest["source"], "rest", "{via_rest}");
    let buckets = |v: &Value| {
        let mut b: Vec<(String, String)> = v["checks"]
            .as_array()
            .unwrap_or_else(|| panic!("checks must be a list: {v}"))
            .iter()
            .map(|c| {
                (
                    c["name"].as_str().unwrap().to_owned(),
                    c["bucket"].as_str().unwrap().to_owned(),
                )
            })
            .collect();
        b.sort();
        b
    };
    let expected: Vec<(String, String)> = [
        ("build", "pass"),
        ("docs", "skipping"),
        ("e2e", "pending"),
        ("legacy/ci", "fail"),
        ("lint", "pending"),
    ]
    .iter()
    .map(|(a, b)| (a.to_string(), b.to_string()))
    .collect();
    assert_eq!(buckets(&via_rest), expected, "{via_rest}");
    assert_eq!(buckets(&via_graphql), buckets(&via_rest));
    assert_eq!(via_rest["head_sha"], g.head().as_str());
    assert!(
        g.calls()
            .iter()
            .any(|c| c.contains(&format!("commits/{}/check-runs", g.head()))),
        "REST checks are read at the head SHA: {:?}",
        g.calls()
    );
}

#[test]
fn checks_read_by_sha_uses_rest_directly() {
    let g = GhRepo::new();
    g.mode("gql", "refuse");
    let head = g.head();
    let v = ok(&g.read(&["checks", &head]));
    assert_eq!(v["head_sha"], head.as_str(), "{v}");
    assert_eq!(v["checks"].as_array().map(Vec::len), Some(5), "{v}");
}

#[test]
fn reviews_read_falls_back_to_rest_with_latest_review_per_user() {
    let g = GhRepo::new();
    let via_graphql = ok(&g.read(&["reviews", "7"]));
    assert_eq!(
        via_graphql["review_decision"], "APPROVED",
        "GitHub's own: {via_graphql}"
    );
    g.mode("gql", "refuse");
    let v = ok(&g.read(&["reviews", "7"]));
    assert_eq!(v["source"], "rest", "{v}");
    // alice: APPROVED (her COMMENTED is ignored); bob: DISMISSED is his latest.
    assert_eq!(v["approvals"], 1, "{v}");
    assert_eq!(v["changes_requested"], 0, "{v}");
    assert_eq!(
        v["unresolved_threads"], "unavailable",
        "threads have no REST path; a refused GraphQL read is a wait, never 0: {v}"
    );
}

#[test]
fn rest_reviews_read_never_states_a_review_decision() {
    // REST has no reviewDecision. Counting approvals against a guessed
    // requirement would call a PR with no reviews REVIEW_REQUIRED in a repo
    // that requires none, and one approval APPROVED in a repo that requires
    // two or a code owner. The decision stays empty for the caller's
    // required-count logic.
    let g = GhRepo::new();
    g.mode("gql", "refuse");
    for reviews in [
        json!([]),
        json!([{"user": {"login": "alice"}, "state": "APPROVED"}]),
        json!([
            {"user": {"login": "alice"}, "state": "APPROVED"},
            {"user": {"login": "dave"}, "state": "APPROVED"},
            {"user": {"login": "dave"}, "state": "CHANGES_REQUESTED"},
        ]),
    ] {
        g.fixture("rest-reviews.json", &reviews);
        let v = ok(&g.read(&["reviews", "7"]));
        assert_eq!(v["source"], "rest", "{v}");
        assert_eq!(v["review_decision"], "", "{reviews}: {v}");
    }
    let v = ok(&g.read(&["reviews", "7"]));
    assert_eq!(
        (v["approvals"].clone(), v["changes_requested"].clone()),
        (json!(1), json!(1)),
        "{v}"
    );
}

#[test]
fn graphql_reviews_read_keeps_an_empty_decision_empty() {
    let g = GhRepo::new();
    let head = g.head();
    g.fixture(
        "gql-pr.json",
        &json!({"number": 7, "state": "OPEN", "headRefOid": head, "reviewDecision": "",
            "latestReviews": [{"author": {"login": "alice"}, "state": "APPROVED"}]}),
    );
    let v = ok(&g.read(&["reviews", "7"]));
    assert_eq!(v["source"], "graphql", "{v}");
    assert_eq!(v["review_decision"], "", "{v}");
    assert_eq!(v["approvals"], 1, "{v}");
}

#[test]
fn pr_for_branch_falls_back_to_rest() {
    let g = GhRepo::new();
    g.mode("gql", "refuse");
    let v = ok(&g.read(&["pr-for-branch", BRANCH]));
    assert_eq!(v["source"], "rest", "{v}");
    assert_eq!(v["number"], 7, "{v}");
    assert!(
        g.calls()
            .iter()
            .any(|c| c.contains("pulls") && c.contains(BRANCH)),
        "the REST lookup filters on the branch: {:?}",
        g.calls()
    );
}

#[test]
fn pr_for_branch_with_no_open_pr_is_an_answer_not_unavailable() {
    let g = GhRepo::new();
    g.mode("gql", "refuse");
    g.fixture("rest-pulls.json", &json!([]));
    let v = ok(&g.read(&["pr-for-branch", BRANCH]));
    assert!(v["number"].is_null(), "{v}");
    assert!(v.get("unavailable").is_none(), "{v}");
}

#[test]
fn a_graphql_answer_with_an_invalid_head_falls_through_to_rest() {
    let g = GhRepo::new();
    let mut pr: Value =
        serde_json::from_str(&std::fs::read_to_string(g.fix.join("gql-pr.json")).unwrap()).unwrap();
    pr["headRefOid"] = json!("not-a-sha; rm -rf /");
    g.fixture("gql-pr.json", &pr);
    let v = ok(&g.read(&["head", "7"]));
    assert_eq!(v["source"], "rest", "{v}");
    assert_eq!(v["head_sha"], g.head().as_str());
}

// ── T2: both paths fail → unavailable, distinct from any finding ────────────

#[test]
fn every_read_reports_unavailable_when_both_paths_are_refused() {
    let g = GhRepo::new();
    g.mode("gql", "refuse");
    g.mode("rest", "refuse");
    for args in [
        &["pr", "7"][..],
        &["head", "7"],
        &["checks", "7"],
        &["reviews", "7"],
        &["pr-for-branch", BRANCH],
    ] {
        let v = unavailable(&g.read(args));
        let u = &v["unavailable"];
        assert_eq!(u["rate_limited"], true, "{args:?}: {v}");
        assert_eq!(
            u["reset_epoch"].as_i64(),
            g.reset_epoch(),
            "{args:?}: the reset comes from gh api rate_limit: {v}"
        );
        let reason = u["reason"].as_str().unwrap();
        assert!(reason.contains(SECONDARY), "{args:?}: {reason}");
        let text = v.to_string().to_lowercase();
        assert!(
            !text.contains("differs") && !text.contains("mismatch"),
            "an unavailable read never reads as a finding: {text}"
        );
    }
}

#[test]
fn head_unavailable_names_every_path_tried() {
    let g = GhRepo::new(); // no refs/pull/7/head on origin: ls-remote finds nothing
    g.mode("gql", "refuse");
    g.mode("rest", "refuse");
    let v = unavailable(&g.read(&["head", "7"]));
    let reason = v["unavailable"]["reason"].as_str().unwrap();
    for path in ["graphql", "rest", "ls-remote"] {
        assert!(reason.contains(path), "reason names {path}: {reason}");
    }
}

#[test]
fn a_failure_without_a_rate_limit_is_unavailable_with_no_reset_lookup() {
    let g = GhRepo::new();
    g.mode("gql", "fail");
    g.mode("rest", "fail");
    let v = unavailable(&g.read(&["pr", "7"]));
    assert_eq!(v["unavailable"]["rate_limited"], false, "{v}");
    assert!(v["unavailable"]["reset_epoch"].is_null(), "{v}");
    assert!(
        !g.calls().iter().any(|c| c.contains("rate_limit")),
        "the reset is looked up only after a rate limit: {:?}",
        g.calls()
    );
}

#[test]
fn a_reset_epoch_far_from_now_is_reported_as_unknown() {
    let g = GhRepo::new();
    g.mode("gql", "refuse");
    g.mode("rest", "refuse");
    g.mode("reset-abs", "1000");
    let v = unavailable(&g.read(&["pr", "7"]));
    assert_eq!(v["unavailable"]["rate_limited"], true);
    assert!(
        v["unavailable"]["reset_epoch"].is_null(),
        "a reset more than a day from now is null: {v}"
    );
}

#[test]
fn the_unavailable_reason_redacts_tokens_and_stays_short() {
    let g = GhRepo::new();
    g.mode("gql", "refuse");
    g.mode("rest", "refuse");
    let filler = "x".repeat(400);
    g.mode(
        "extra",
        &format!("Authorization: Bearer ghp_SYNTHETICfixture0123456789 github_pat_SYNTHETIC_fixture {filler}"),
    );
    let o = g.read(&["pr", "7"]);
    let text = format!(
        "{}{}",
        crate::support::auto_drive_light::stdout(&o),
        crate::support::auto_drive_light::stderr(&o)
    );
    for secret in [
        "ghp_SYNTHETICfixture0123456789",
        "github_pat_SYNTHETIC_fixture",
    ] {
        assert!(!text.contains(secret), "token value leaked: {text}");
    }
    let v = unavailable(&o);
    for part in v["unavailable"]["reason"].as_str().unwrap().split("; ") {
        assert!(
            part.len() <= 220,
            "each excerpt is at most 200 bytes: {part}"
        );
        assert!(part.bytes().all(|b| (0x20..0x7f).contains(&b)), "{part}");
    }
}

// ── Usage and process environment ───────────────────────────────────────────

#[test]
fn bad_arguments_exit_2_before_any_gh_call() {
    let g = GhRepo::new();
    for args in [
        &["pr", "0"][..],
        &["pr", "12abc"],
        &["pr", "-7"],
        &["pr", "99999999999"],
        &["head", "--repo=x"],
        &["checks", "deadbeef"],
        &["checks", "ZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZ"],
        &["reviews", "7", "--required", "2"],
        &["pr-for-branch", "--upload-pack=touch pwned"],
        &["pr-for-branch", "a..b"],
        &["pr-for-branch", "-x"],
        &["pr", "7", "--repo", "o/r/../x"],
        &["nonsense", "7"],
    ] {
        let o = g.read(args);
        assert_eq!(o.status.code(), Some(2), "{args:?}: {}", show(&o));
    }
    assert!(
        g.calls().is_empty(),
        "nothing runs on a usage error: {:?}",
        g.calls()
    );
}

#[test]
fn gh_runs_without_gh_repo_and_without_prompts() {
    let g = GhRepo::new();
    g.mode("gql", "refuse");
    let o = g
        .cmd(crate::support::amplihack_bin::amplihack_bin())
        .args(["orch", "helper", "autodrive-gh-read", "pr", "7"])
        .env("GH_REPO", "attacker/elsewhere")
        .output()
        .unwrap();
    ok(&o);
    let env = std::fs::read_to_string(g.log.join("gh-env.log")).unwrap();
    assert!(!env.is_empty());
    for line in env.lines() {
        assert!(line.starts_with("GH_REPO= "), "GH_REPO is removed: {line}");
        assert!(line.contains("GH_PROMPT_DISABLED=1"), "{line}");
        assert!(line.contains("GH_PAGER=cat"), "{line}");
    }
}

// ── T4 (helper): update-branch with expected_head_sha ───────────────────────

fn ub_calls(g: &GhRepo) -> Vec<String> {
    crate::support::autodrive_gh_fake::lines(&g.log.join("update-branch.log"))
}

#[test]
fn update_branch_sends_one_put_with_the_expected_head_and_never_merges() {
    let g = GhRepo::new();
    let head = g.head();
    let pw = g.prior_wait(&head);
    let o = g.update_branch(&[
        "7",
        "--expected-head-sha",
        &head,
        "--prior-wait",
        pw.to_str().unwrap(),
    ]);
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    assert_eq!(
        one_json(&o),
        json!({"pr": 7, "action": "update-branch", "result": "ok", "expected_head_sha": head})
    );
    let puts = ub_calls(&g);
    assert_eq!(puts.len(), 1, "exactly one update-branch call: {puts:?}");
    assert!(puts[0].starts_with("PUT "), "{puts:?}");
    assert!(
        puts[0].contains(&format!("expected_head_sha={head}")),
        "{puts:?}"
    );
    assert!(
        !g.log.join("merge-attempt").exists(),
        "update-branch never merges"
    );
    assert!(
        !g.calls().iter().any(|c| c.starts_with("pr\u{1f}merge")),
        "{:?}",
        g.calls()
    );
}

#[test]
fn update_branch_422_no_new_commits_is_noop_up_to_date() {
    let g = GhRepo::new();
    g.mode("ub", "422-noop");
    let head = g.head();
    let pw = g.prior_wait(&head);
    let o = g.update_branch(&[
        "7",
        "--expected-head-sha",
        &head,
        "--prior-wait",
        pw.to_str().unwrap(),
    ]);
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    let v = one_json(&o);
    assert_eq!(v["action"], "noop-up-to-date", "{v}");
    assert_eq!(v["result"], "ok", "{v}");
    assert_eq!(ub_calls(&g).len(), 1, "never retried");
}

#[test]
fn update_branch_with_a_moved_head_is_refused_and_recorded() {
    let g = GhRepo::new();
    g.mode("ub", "422-moved");
    let head = g.head();
    let pw = g.prior_wait(&head);
    let o = g.update_branch(&[
        "7",
        "--expected-head-sha",
        &head,
        "--prior-wait",
        pw.to_str().unwrap(),
    ]);
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    let v = one_json(&o);
    assert_eq!(
        (v["action"].as_str(), v["result"].as_str()),
        (Some("update-branch"), Some("refused")),
        "{v}"
    );
}

#[test]
fn update_branch_rate_limited_is_unavailable_with_the_reset() {
    let g = GhRepo::new();
    g.mode("ub", "refuse");
    let head = g.head();
    let pw = g.prior_wait(&head);
    let o = g.update_branch(&[
        "7",
        "--expected-head-sha",
        &head,
        "--prior-wait",
        pw.to_str().unwrap(),
    ]);
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    let v = one_json(&o);
    assert_eq!(v["result"], "unavailable", "{v}");
    assert_eq!(v["reset_epoch"].as_i64(), g.reset_epoch(), "{v}");
    assert_eq!(ub_calls(&g).len(), 1, "no retry after a limit");
}

#[test]
fn update_branch_requires_a_valid_expected_head_sha() {
    let g = GhRepo::new();
    let pw = g.prior_wait(&"a".repeat(40));
    let pw = pw.to_str().unwrap();
    for args in [
        &["7", "--prior-wait", pw][..],
        &["7", "--expected-head-sha", "abc", "--prior-wait", pw],
        &[
            "0",
            "--expected-head-sha",
            &"a".repeat(40),
            "--prior-wait",
            pw,
        ],
    ] {
        let o = g.update_branch(args);
        assert_eq!(o.status.code(), Some(2), "{args:?}: {}", show(&o));
    }
    assert!(g.calls().is_empty(), "{:?}", g.calls());
}

// ── IV2: the ls-remote head route reads only the repository named ───────────
//
// `git ls-remote origin` reads `origin`, whatever `--repo` says. The route is
// used only when `--repo` is absent or `origin` resolves to the same
// owner/name; `origin` is resolved with `git remote get-url origin`, which
// contacts no server. The fake `git` answers that one call and logs every
// call, so the tests can count `ls-remote` calls. `refs/pull/7/head` is
// published on the local origin in every case, so a route that wrongly ran
// would answer, not fail.

fn ls_remote_calls(g: &GhRepo) -> usize {
    g.git_calls()
        .iter()
        .filter(|c| c.split('\u{1f}').any(|a| a == "ls-remote"))
        .count()
}

fn head_only_by_ls_remote(origin_url: &str) -> GhRepo {
    let g = GhRepo::new();
    g.publish_pull_ref();
    g.fake_git(origin_url);
    g.mode("gql", "refuse");
    g.mode("rest", "refuse");
    g
}

#[test]
fn head_with_a_repo_other_than_origin_makes_no_ls_remote_call_and_is_unavailable() {
    for url in [
        "https://github.com/other/r.git",
        "git@github.com:o/elsewhere.git",
        "https://github.com/o/r-fork",
    ] {
        let g = head_only_by_ls_remote(url);
        let o = g.read(&["head", "7", "--repo", "o/r"]);
        let v = unavailable(&o);
        assert_eq!(
            ls_remote_calls(&g),
            0,
            "{url}: origin is another repository, so ls-remote never runs: {:?}",
            g.git_calls()
        );
        let reason = v["unavailable"]["reason"].as_str().unwrap_or_default();
        assert!(
            reason.contains("ls-remote: origin is not o/r"),
            "{url}: the route names why it was skipped: {v}"
        );
        assert!(v.get("head_sha").is_none_or(Value::is_null), "{url}: {v}");
    }
}

#[test]
fn head_with_a_repo_equal_to_origin_still_uses_ls_remote() {
    for url in [
        "https://github.com/o/r.git",
        "https://github.com/o/r",
        "git@github.com:o/r.git",
        "ssh://git@github.com/o/r.git",
    ] {
        let g = head_only_by_ls_remote(url);
        let o = g.read(&["head", "7", "--repo", "o/r"]);
        let v = ok(&o);
        assert_eq!(v["source"], "ls-remote", "{url}: {v}");
        assert_eq!(v["head_sha"], g.head().as_str(), "{url}: {v}");
        assert_eq!(ls_remote_calls(&g), 1, "{url}: {:?}", g.git_calls());
    }
}

#[test]
fn head_without_repo_uses_ls_remote_on_origin_as_before() {
    let g = head_only_by_ls_remote("https://github.com/anyone/anything.git");
    let o = g.read(&["head", "7"]);
    let v = ok(&o);
    assert_eq!(v["source"], "ls-remote", "{v}");
    assert_eq!(ls_remote_calls(&g), 1, "{:?}", g.git_calls());
}

#[test]
fn a_head_answered_by_graphql_never_runs_ls_remote() {
    // While GraphQL answers, the origin is never resolved and git is never run.
    let g = GhRepo::new();
    g.fake_git("https://github.com/other/r.git");
    let o = g.read(&["head", "7", "--repo", "o/r"]);
    let v = ok(&o);
    assert_eq!(v["source"], "graphql", "{v}");
    assert_eq!(ls_remote_calls(&g), 0, "{:?}", g.git_calls());
}

// ── Review round 17: refusals and the checks race ───────────────────────────

#[test]
fn a_read_refused_on_every_path_says_refused() {
    let g = GhRepo::new();
    g.mode("gql", "auth");
    g.mode("rest", "auth");
    for args in [&["pr", "7"][..], &["reviews", "7"], &["checks", "7"]] {
        let v = unavailable(&g.read(args));
        assert_eq!(v["unavailable"]["refused"], true, "{args:?}: {v}");
        assert_eq!(v["unavailable"]["rate_limited"], false, "{args:?}: {v}");
    }
    // A 404 on REST after a 401 on GraphQL: still nothing a wait clears.
    g.mode("rest", "fail");
    let v = unavailable(&g.read(&["reviews", "7"]));
    assert_eq!(v["unavailable"]["refused"], true, "{v}");
}

#[test]
fn a_rate_limit_on_any_path_is_not_a_refusal() {
    let g = GhRepo::new();
    g.mode("gql", "refuse");
    g.mode("rest", "fail");
    let v = unavailable(&g.read(&["reviews", "7"]));
    assert_eq!(v["unavailable"]["refused"], false, "{v}");
    g.mode("gql", "fail");
    g.mode("rest", "auth");
    let v = unavailable(&g.read(&["reviews", "7"]));
    assert_eq!(v["unavailable"]["refused"], false, "a 502 may clear: {v}");
}

#[test]
fn a_push_during_the_graphql_checks_read_falls_to_rest_at_one_sha() {
    let g = GhRepo::new();
    let pr = std::fs::read_to_string(g.fix.join("gql-pr.json")).unwrap();
    let moved = "0123456789abcdef0123456789abcdef01234567";
    std::fs::write(
        g.fix.join("push-during-checks"),
        pr.replace(&g.head(), moved),
    )
    .unwrap();
    let v = ok(&g.read(&["checks", "7"]));
    assert_eq!(
        v["source"], "rest",
        "checks read while the head moved are not pinned to either SHA: {v}"
    );
    assert_eq!(v["head_sha"], g.head().as_str(), "{v}");
    assert_ne!(v["head_sha"], moved, "{v}");
}
