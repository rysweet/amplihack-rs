//! T2 (dispatch half) and T5: the completion dispatch's head read has three
//! outcomes. `head-matches` proceeds, `head-differs` is refused as before, and
//! `head-unavailable` is WAITING with exit 76: never DONE and never
//! INVALID_DECISION. Contract:
//! docs/reference/autodrive-crusty-completion.md#head-check-outcomes.
//!
//! Built on the #2336 `Drive` fixture. Its own fake `gh` writes the round's
//! evidence; the #2333 fake replaces it before the dispatch, so only the
//! dispatch's head read meets a refused GraphQL.

use std::ffi::OsStr;
use std::process::Output;

use serde_json::{Value, json};

use crate::support::auto_drive_light::{private_file, show, stderr, tools_dir};
use crate::support::autodrive_gh_fake::{FAKE_GH, write_fixtures};

#[path = "support/autodrive_completion.rs"]
#[allow(dead_code, clippy::duplicate_mod)] // also loaded by issue_2336_autodrive_completion
mod fixture;
use fixture::{Drive, assert_complete, json_output};

const EXIT_WAITING: i32 = 76;

struct Case {
    d: Drive,
    p: Value,
    fix: std::path::PathBuf,
}

impl Case {
    /// A complete-able round (MINOR_ONLY, recorded at the head), prepared
    /// with `--pr <pr>` when given, a `complete` decision written, and the
    /// #2333 fake `gh` installed.
    fn new(pr: Option<&str>) -> Self {
        let d = Drive::new();
        d.record(1);
        let p = prepare(&d, 1, pr);
        d.decide(&p, "complete");
        let fix = d.tmp.path().join("gh-fix");
        crate::support::auto_drive_light::private_dir(&fix);
        write_fixtures(&fix, &d.head());
        crate::support::exec_stub::write_executable(&d.fakes.bin.join("gh"), FAKE_GH, 0o755);
        Self { d, p, fix }
    }

    fn mode(&self, file: &str, v: &str) {
        std::fs::write(self.fix.join(file), v).unwrap();
    }

    fn rest_head(&self, sha: &str) {
        let mut pr: Value =
            serde_json::from_str(&std::fs::read_to_string(self.fix.join("rest-pr.json")).unwrap())
                .unwrap();
        pr["head"]["sha"] = json!(sha);
        std::fs::write(self.fix.join("rest-pr.json"), pr.to_string()).unwrap();
    }

    fn dispatch(&self, rc: i32) -> Output {
        self.d
            .cmd(crate::support::amplihack_bin::amplihack_bin())
            .args([
                "orch",
                "helper",
                "autodrive-completion-dispatch",
                "--private-state",
            ])
            .arg(self.p["private_state"].as_str().unwrap())
            .args(["--child-exit-code", &rc.to_string()])
            .env("FAKE_FIX", &self.fix)
            .output()
            .unwrap()
    }

    fn private_state(&self) -> std::path::PathBuf {
        std::path::PathBuf::from(self.p["private_state"].as_str().unwrap())
    }

    fn dispatched_marker(&self) -> bool {
        self.private_state().with_extension("dispatched").exists()
    }
}

/// `Drive::prepare` plus `--pr`.
fn prepare(d: &Drive, n: usize, pr: Option<&str>) -> Value {
    let record = d.p(&format!("crusty-round-{n}.json"));
    let context = d.p("evaluator-context.json");
    private_file(
        &context,
        &json!({
            "loop_name":"auto-drive:crusty", "loop_round_label":format!("round-{n}"),
            "loop_history":"", "loop_repo_path":d.repo, "loop_agent_working_dir":d.review,
            "loop_baseline_ref":d.head(), "loop_child_exit_code":"0",
            "loop_findings_current":d.read(&format!("crusty-round-{n}.json.findings")),
            "loop_findings_previous":"", "loop_test_signal":"", "loop_test_signal_previous":"",
            "loop_ci_signal":"", "loop_ci_signal_previous":"", "loop_clean_token":"CLEAN",
            "loop_round_verdict":"CONCERNS", "loop_last_round_output":"fixture review"
        })
        .to_string(),
    );
    let label = format!("round-{n}");
    let tools = tools_dir();
    let mut args: Vec<&OsStr> = vec![
        OsStr::new("--state-dir"),
        d.state.as_os_str(),
        OsStr::new("--round-label"),
        OsStr::new(&label),
        OsStr::new("--record"),
        record.as_os_str(),
        OsStr::new("--repo"),
        d.repo.as_os_str(),
        OsStr::new("--review-dir"),
        d.review.as_os_str(),
        OsStr::new("--launch-tools"),
        tools.as_os_str(),
        OsStr::new("--evaluator-context-file"),
        context.as_os_str(),
    ];
    if let Some(pr) = pr {
        args.push(OsStr::new("--pr"));
        args.push(OsStr::new(pr));
    }
    let o = d.helper("prepare", &args);
    assert!(o.status.success(), "prepare --pr is required: {}", show(&o));
    json_output(&o)
}

fn assert_waiting(c: &Case, o: &Output) -> Value {
    assert_eq!(
        o.status.code(),
        Some(EXIT_WAITING),
        "head-unavailable exits 76 (WAITING), not 1 (INVALID_DECISION) and not 75 (ORPHANED): {}",
        show(o)
    );
    let v = json_output(o);
    assert_eq!(v["loop_result"], "WAITING", "{}", show(o));
    assert_eq!(v["wait_reason"], "head-unavailable", "{v}");
    assert_eq!(v["evaluator_invoked"], false, "{v}");
    assert!(
        c.d.evaluators().is_empty(),
        "WAITING never runs the evaluator"
    );
    assert!(
        !c.dispatched_marker(),
        "WAITING creates no .dispatched marker"
    );
    assert!(!c.d.marked(), "WAITING never marks the phase");
    v
}

// ── T5: head-matches through REST ───────────────────────────────────────────

#[test]
fn a_clean_dispatch_with_graphql_refused_and_a_matching_rest_head_completes() {
    let c = Case::new(Some("7"));
    c.mode("gql", "refuse");
    let o = c.dispatch(0);
    assert_complete(&c.d, &o);
    let calls = c.d.fakes.gh_calls();
    assert!(
        calls
            .iter()
            .any(|a| a.first().map(String::as_str) == Some("api")
                && a.iter().any(|s| s.contains("pulls/7"))),
        "the head came from REST: {calls:?}"
    );
}

#[test]
fn a_clean_dispatch_reads_the_head_through_ls_remote_when_rest_is_refused_too() {
    let c = Case::new(Some("7"));
    c.d.git(
        &c.d.repo,
        &["push", "-q", "origin", "HEAD:refs/pull/7/head"],
    );
    c.mode("gql", "refuse");
    c.mode("rest", "refuse");
    assert_complete(&c.d, &c.dispatch(0));
}

#[test]
fn without_state_pr_the_dispatch_resolves_the_pr_from_the_branch() {
    // Older state files have no `pr`; the number comes from pulls?head=.
    let c = Case::new(None);
    c.mode("gql", "refuse");
    assert_complete(&c.d, &c.dispatch(0));
    assert!(
        c.d.fakes
            .gh_calls()
            .iter()
            .any(|a| a.iter().any(|s| s.contains("quick/demo"))),
        "PR resolved from the branch: {:?}",
        c.d.fakes.gh_calls()
    );
}

// ── T2/T5: head-unavailable is WAITING, exit 76 ─────────────────────────────

#[test]
fn both_head_reads_failing_is_waiting_with_exit_76_not_invalid_decision() {
    let c = Case::new(Some("7"));
    c.mode("gql", "refuse");
    c.mode("rest", "refuse");
    let o = c.dispatch(0);
    let v = assert_waiting(&c, &o);
    let target = v["wait_target"].as_str().unwrap_or_default();
    assert!(
        target.starts_with("ratelimit-reset@"),
        "a rate-limited head read waits for the reset: {v}"
    );
    let text = format!(
        "{}{}",
        crate::support::auto_drive_light::stdout(&o),
        stderr(&o)
    );
    assert!(!text.contains("INVALID_DECISION"), "{text}");
    assert!(
        !text.contains("differs"),
        "unavailable is not 'differs': {text}"
    );
}

#[test]
fn an_unresolvable_pr_number_is_waiting_too() {
    let c = Case::new(None);
    c.mode("gql", "refuse");
    c.mode("rest", "refuse");
    let v = assert_waiting(&c, &c.dispatch(0));
    assert_eq!(v["wait_reason"], "head-unavailable", "{v}");
}

#[test]
fn no_open_pr_for_the_branch_is_an_error_not_a_wait() {
    // A definite "no open PR" (merged or closed between rounds) never clears
    // by waiting, so the dispatch fails rather than WAITING on every drive.
    let c = Case::new(None);
    c.mode("gql", "refuse");
    std::fs::write(c.fix.join("rest-pulls.json"), "[]").unwrap();
    let o = c.dispatch(0);
    assert!(!o.status.success(), "{}", show(&o));
    assert_ne!(o.status.code(), Some(EXIT_WAITING), "{}", show(&o));
    assert!(stderr(&o).contains("not a wait"), "{}", show(&o));
    assert!(!c.dispatched_marker() && !c.d.marked());
}

#[test]
fn a_non_rate_limited_head_failure_waits_on_read_pr_head() {
    let c = Case::new(Some("7"));
    c.mode("gql", "fail");
    c.mode("rest", "fail");
    let v = assert_waiting(&c, &c.dispatch(0));
    assert_eq!(v["wait_target"], "read@pr-head", "{v}");
}

#[test]
fn waiting_leaves_the_sealed_evidence_unchanged_and_the_round_resumable() {
    let c = Case::new(Some("7"));
    let evidence = c.private_state().with_extension("evidence");
    let before = std::fs::read(&evidence).unwrap();
    let decision = std::fs::read(fixture::decision_file(&c.p)).unwrap();
    c.mode("gql", "refuse");
    c.mode("rest", "refuse");
    assert_waiting(&c, &c.dispatch(0));
    assert_eq!(std::fs::read(&evidence).unwrap(), before, "seal unchanged");
    assert_eq!(
        std::fs::read(fixture::decision_file(&c.p)).unwrap(),
        decision,
        "decision unchanged"
    );
    // The reset passed: the same round dispatches DONE.
    c.mode("rest", "ok");
    assert_complete(&c.d, &c.dispatch(0));
}

// ── T2: head-differs stays a finding and fails closed ───────────────────────

#[test]
fn a_rest_head_that_differs_is_refused_as_before_not_waiting() {
    let c = Case::new(Some("7"));
    c.mode("gql", "refuse");
    c.rest_head(&"b".repeat(40));
    let o = c.dispatch(0);
    assert!(!o.status.success(), "{}", show(&o));
    assert_ne!(
        o.status.code(),
        Some(EXIT_WAITING),
        "differs is never a wait: {}",
        show(&o)
    );
    assert!(
        stderr(&o).contains("current PR head differs from reviewed evidence"),
        "{}",
        show(&o)
    );
    assert!(
        !crate::support::auto_drive_light::stdout(&o).contains("WAITING"),
        "{}",
        show(&o)
    );
    assert!(c.d.evaluators().is_empty());
    assert!(!c.d.marked());
}

#[test]
fn a_graphql_head_that_differs_is_not_rescued_by_rest() {
    // GraphQL answered; its answer is the finding. Falling through to a
    // matching REST head would turn a real difference into completion.
    let c = Case::new(Some("7"));
    let mut gql: Value =
        serde_json::from_str(&std::fs::read_to_string(c.fix.join("gql-pr.json")).unwrap()).unwrap();
    gql["headRefOid"] = json!("c".repeat(40));
    std::fs::write(c.fix.join("gql-pr.json"), gql.to_string()).unwrap();
    let o = c.dispatch(0);
    assert!(!o.status.success(), "{}", show(&o));
    assert!(stderr(&o).contains("differs"), "{}", show(&o));
}

// ── Fail-closed ordering ─────────────────────────────────────────────────────

#[test]
fn an_invalid_record_with_the_head_unavailable_is_still_rejected_not_waiting() {
    // Record checks run before the remote read, so a rate limit cannot turn
    // an invalid record into a wait.
    let c = Case::new(Some("7"));
    c.d.git(
        &c.d.repo,
        &["commit", "--allow-empty", "-qm", "moved after review"],
    );
    c.mode("gql", "refuse");
    c.mode("rest", "refuse");
    let o = c.dispatch(0);
    assert!(!o.status.success(), "{}", show(&o));
    assert_ne!(o.status.code(), Some(EXIT_WAITING), "{}", show(&o));
    assert!(
        !crate::support::auto_drive_light::stdout(&o).contains("WAITING"),
        "{}",
        show(&o)
    );
}

#[test]
fn a_child_that_exits_76_is_not_treated_as_waiting() {
    let c = Case::new(Some("7"));
    let o = c.dispatch(EXIT_WAITING);
    assert!(!o.status.success(), "{}", show(&o));
    assert_ne!(
        o.status.code(),
        Some(EXIT_WAITING),
        "76 is not a pass-through child code: {}",
        show(&o)
    );
    assert!(
        !crate::support::auto_drive_light::stdout(&o).contains("WAITING"),
        "{}",
        show(&o)
    );
    assert!(!c.dispatched_marker());
}

#[test]
fn prepare_refuses_a_pr_number_that_is_not_a_positive_integer() {
    let d = Drive::new();
    d.record(1);
    for bad in ["-1", "7; rm", "abc"] {
        let record = d.p("crusty-round-1.json");
        let o = d.helper(
            "prepare",
            &[
                OsStr::new("--state-dir"),
                d.state.as_os_str(),
                OsStr::new("--round-label"),
                OsStr::new("round-1"),
                OsStr::new("--record"),
                record.as_os_str(),
                OsStr::new("--repo"),
                d.repo.as_os_str(),
                OsStr::new("--review-dir"),
                d.review.as_os_str(),
                OsStr::new("--launch-tools"),
                tools_dir().as_os_str(),
                OsStr::new("--pr"),
                OsStr::new(bad),
            ],
        );
        assert!(!o.status.success(), "--pr {bad}: {}", show(&o));
    }
}
