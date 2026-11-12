//! Unit contract for `finalization_open_pr_signal` (workflow_finalization_pr_signal.sh):
//! it reports facts as one compact JSON line, or exits 1 printing nothing.

use serde_json::{Value, json};

use super::fixture::*;

fn signal_json(outcome: &Outcome) -> Value {
    assert!(
        outcome.success(),
        "signal must report\n{}",
        outcome.describe()
    );
    assert_eq!(
        outcome.stdout.trim_end().lines().count(),
        1,
        "signal prints exactly one line\n{}",
        outcome.describe()
    );
    let value: Value = serde_json::from_str(outcome.stdout.trim()).expect("signal JSON");
    let keys: Vec<&str> = value
        .as_object()
        .expect("signal is an object")
        .keys()
        .map(String::as_str)
        .collect();
    for key in ["pr_number", "pr_url", "checks"] {
        assert!(keys.contains(&key), "signal must carry `{key}`: {value}");
    }
    value
}

#[test]
fn signal_reports_number_url_and_check_classification() {
    let cases = [
        (green_rollup(), "green"),
        (json!([check_run("COMPLETED", json!("FAILURE"))]), "failing"),
        (json!([status_context("ERROR")]), "failing"),
        (json!([check_run("IN_PROGRESS", json!(null))]), "pending"),
        (json!([status_context("PENDING")]), "pending"),
        (json!([]), "none"),
        (json!([{"__typename": "MysteryCheck"}]), "unknown"),
    ];
    for (rollup, expected) in cases {
        let fx = Fixture::new();
        fx.gh_view(&open_pr(rollup.clone()));
        let value = signal_json(&fx.signal(BRANCH, Some("7")));
        assert_eq!(value["pr_number"], "7", "rollup {rollup}");
        assert_eq!(value["pr_url"], PR_URL, "rollup {rollup}");
        assert_eq!(value["checks"], expected, "rollup {rollup}");
        fx.assert_gh_reads_only();
    }
}

#[test]
fn signal_accepts_an_exact_pr_url_reference() {
    let fx = Fixture::new();
    fx.gh_view(&open_pr(green_rollup()));
    let value = signal_json(&fx.signal(BRANCH, Some(PR_URL)));
    assert_eq!(value["pr_number"], "7");
    assert!(fx.gh_calls().iter().any(|c| c.starts_with("pr view 7 ")));
}

#[test]
fn signal_without_a_reference_uses_a_single_same_repo_pr_list_match() {
    let fx = Fixture::new();
    fx.gh_list(&json!([{"number": 7, "url": PR_URL, "isCrossRepository": false}]));
    fx.gh_view(&open_pr(green_rollup()));
    let value = signal_json(&fx.signal(BRANCH, None));
    assert_eq!(value["checks"], "green");
    let calls = fx.gh_calls();
    assert!(
        calls
            .iter()
            .any(|c| c.starts_with("pr list ") && c.contains("--limit 2")),
        "pr list must fetch at most two results to detect ambiguity: {calls:?}"
    );
    fx.assert_gh_reads_only();
}

#[test]
fn signal_exits_one_and_prints_nothing_without_a_usable_signal() {
    let closed = {
        let mut view = open_pr(green_rollup());
        view["state"] = json!("CLOSED");
        view
    };
    let fx = Fixture::new();
    fx.gh_view(&closed);
    let outcome = fx.signal(BRANCH, Some("7"));
    assert!(
        !outcome.success(),
        "closed PR is no signal\n{}",
        outcome.describe()
    );
    assert_eq!(outcome.stdout, "", "no-signal prints nothing on stdout");

    let fx = Fixture::new();
    fx.gh_fail("view", 1);
    let outcome = fx.signal(BRANCH, Some("7"));
    assert!(!outcome.success(), "gh failure is no signal");
    assert_eq!(outcome.stdout, "");
    assert_no_leak(&outcome);

    for bad_ref in ["0", "07", "7; echo pwned", "12345678901", "-7"] {
        let fx = Fixture::new();
        fx.gh_view(&open_pr(green_rollup()));
        let outcome = fx.signal(BRANCH, Some(bad_ref));
        assert!(!outcome.success(), "bad ref {bad_ref:?} is no signal");
        assert_eq!(outcome.stdout, "");
        fx.assert_gh_not_called(bad_ref);
    }

    for bad_branch in ["main", "master", "HEAD", "-x", "feat/other", ""] {
        let fx = Fixture::new();
        fx.gh_view(&open_pr(green_rollup()));
        let outcome = fx.signal(bad_branch, Some("7"));
        assert!(!outcome.success(), "branch {bad_branch:?} is no signal");
        fx.assert_gh_not_called(bad_branch);
    }
}

#[test]
fn signal_fails_closed_for_a_non_github_origin() {
    let fx = Fixture::new();
    fx.git(&[
        "remote",
        "set-url",
        "origin",
        "https://dev.azure.com/org/proj/_git/fixture",
    ]);
    fx.gh_view(&open_pr(green_rollup()));
    let outcome = fx.signal(BRANCH, Some("7"));
    assert!(!outcome.success(), "non-GitHub origin is no signal");
    fx.assert_gh_not_called("non-GitHub origin");

    // A non-GitHub host whose path contains `@github.com/` is not github.com.
    let fx = Fixture::new();
    fx.git(&[
        "remote",
        "set-url",
        "origin",
        "https://evil.example/x@github.com/rysweet/fixture",
    ]);
    fx.gh_view(&open_pr(green_rollup()));
    let outcome = fx.signal(BRANCH, Some("7"));
    assert!(!outcome.success(), "host-confused origin is no signal");
    fx.assert_gh_not_called("host-confused origin");

    let fx = Fixture::new();
    fx.git(&["remote", "remove", "origin"]);
    let outcome = fx.signal(BRANCH, Some("7"));
    assert!(!outcome.success(), "missing origin is no signal");
    fx.assert_gh_not_called("missing origin");
}

#[test]
fn signal_accepts_ssh_origin_forms() {
    for origin in [
        "git@github.com:rysweet/fixture.git",
        "ssh://git@github.com/rysweet/fixture",
    ] {
        let fx = Fixture::new();
        fx.git(&["remote", "set-url", "origin", origin]);
        fx.gh_view(&open_pr(green_rollup()));
        let value = signal_json(&fx.signal(BRANCH, Some("7")));
        assert_eq!(value["checks"], "green", "origin {origin}");
        fx.assert_gh_reads_only();
    }
}
