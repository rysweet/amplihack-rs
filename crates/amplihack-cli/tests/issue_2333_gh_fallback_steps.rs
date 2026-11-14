//! The bash read sites and the round evidence (D5, D7, D8): each runs the
//! real tool or recipe step body under the #2333 fake `gh`.

use std::process::Output;

use serde_json::{Value, json};

use crate::support::auto_drive_light::{show, stderr, stdout, tools_dir};
use crate::support::autodrive_gh_fake::{BRANCH, GhRepo};

fn last_json(o: &Output) -> Value {
    stdout(o)
        .lines()
        .rev()
        .find_map(|l| serde_json::from_str::<Value>(l).ok())
        .unwrap_or_else(|| panic!("no JSON line: {}", show(o)))
}

fn tool(g: &GhRepo, name: &str, args: &[&str]) -> Output {
    g.cmd("bash")
        .arg(tools_dir().join(name))
        .args(args)
        .output()
        .unwrap()
}

// ── autodrive_progress.sh check: the 7th argument ───────────────────────────

fn progress(g: &GhRepo, args: &[&str]) -> Output {
    tool(g, "autodrive_progress.sh", args)
}

fn progress_setup(g: &GhRepo, ci: &str) -> (std::path::PathBuf, String) {
    let state = g.tmp.path().join("state");
    crate::support::auto_drive_light::private_dir(&state);
    let findings = state.join("findings");
    crate::support::auto_drive_light::private_file(&findings, "approval-facts-unreadable\n");
    let s = state.to_str().unwrap().to_owned();
    let o = progress(
        g,
        &[
            "record",
            &s,
            "merge-ready",
            "round-1",
            &g.head(),
            "0",
            findings.to_str().unwrap(),
            ci,
        ],
    );
    assert!(o.status.success(), "{}", show(&o));
    (findings, s)
}

#[test]
fn progress_check_never_stops_a_round_the_record_calls_waiting() {
    for ci in ["UNREADABLE", "QUEUED", "expected-checks-unreadable"] {
        let g = GhRepo::new();
        let (findings, s) = progress_setup(&g, ci);
        let f = findings.to_str().unwrap();
        let head = g.head();
        let o = progress(
            &g,
            &["check", &s, "merge-ready", "round-2", &head, f, ci, "true"],
        );
        assert_eq!(
            o.status.code(),
            Some(0),
            "{ci}: round_waiting=true is no stop: {}",
            show(&o)
        );
        let o = progress(
            &g,
            &["check", &s, "merge-ready", "round-2", &head, f, ci, "false"],
        );
        assert_eq!(
            o.status.code(),
            Some(10),
            "{ci}: a non-waiting repeat still stops: {}",
            show(&o)
        );
    }
}

#[test]
fn progress_check_with_six_arguments_keeps_the_pending_only_rule() {
    let g = GhRepo::new();
    let (findings, s) = progress_setup(&g, "PENDING");
    let f = findings.to_str().unwrap();
    let o = progress(
        &g,
        &[
            "check",
            &s,
            "merge-ready",
            "round-2",
            &g.head(),
            f,
            "PENDING",
        ],
    );
    assert_eq!(
        o.status.code(),
        Some(0),
        "old callers keep the PENDING exemption: {}",
        show(&o)
    );
}

// ── autodrive_round_evidence.sh: fixable-only .findings, .waits, record ─────

struct Round {
    g: GhRepo,
    record: std::path::PathBuf,
    qa_hash: String,
    qa_file: std::path::PathBuf,
}

impl Round {
    fn new() -> Self {
        let g = GhRepo::new();
        let state = g.tmp.path().join("state");
        crate::support::auto_drive_light::private_dir(&state);
        let qa_file = state.join("qa.json");
        crate::support::auto_drive_light::private_file(&qa_file, "{\"qa_status\":\"PASS\"}\n");
        let o = tool(
            &g,
            "autodrive_round_evidence.sh",
            &["qa-evidence-hash", qa_file.to_str().unwrap()],
        );
        let qa_hash = last_json(&o).to_string();
        Self {
            record: state.join("merge-ready-round-1.json"),
            g,
            qa_hash,
            qa_file,
        }
    }

    fn run(
        &self,
        args: &[&str],
        ci: &str,
        qa: &str,
        approval: &str,
        merge_state: &str,
        stdin: &str,
    ) -> Output {
        use std::io::Write;
        let head = self.g.head();
        let mut c = self.g.cmd("bash");
        c.arg(tools_dir().join("autodrive_round_evidence.sh"))
            .args(args)
            .env("AUTODRIVE_ROUND_RECORD", &self.record)
            .env("AUTODRIVE_ROUND_LABEL", "round-1")
            .env("AUTODRIVE_QA_EVIDENCE", &self.qa_file)
            .env("REPO_PATH", &self.g.repo)
            .env("PR_NUMBER", "7")
            .env(
                "RECIPE_VAR_qa_evidence",
                json!({"qa_status": qa}).to_string(),
            )
            .env("RECIPE_VAR_qa_evidence_hash", &self.qa_hash)
            .env(
                "RECIPE_VAR_ci_evidence",
                json!({"ci_status": ci, "ci_signal": ci, "head_sha": head}).to_string(),
            )
            .env(
                "RECIPE_VAR_merge_sync",
                json!({"conflict": "false", "head_sha": head, "synced": "false"}).to_string(),
            )
            .env(
                "RECIPE_VAR_platform_facts",
                json!({"unresolved_threads": "0", "approval_status": approval,
                       "merge_state": merge_state, "mergeable": "MERGEABLE"})
                .to_string(),
            )
            .env(
                "RECIPE_VAR_crusty_evidence",
                json!({"crusty_status": "DONE_CLEAN"}).to_string(),
            )
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut child = c.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stdin.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }

    fn verdict(
        &self,
        ci: &str,
        qa: &str,
        approval: &str,
        merge_state: &str,
        blockers: Value,
    ) -> Output {
        let o = self.run(
            &["merge-ready-verdict", "NOT_MERGE_READY", "review"],
            ci,
            qa,
            approval,
            merge_state,
            &blockers.to_string(),
        );
        assert!(o.status.success(), "{}", show(&o));
        o
    }

    fn record(&self, ci: &str, qa: &str, approval: &str, merge_state: &str) -> Value {
        let mut c = self.run(&["round-record"], ci, qa, approval, merge_state, "");
        assert!(c.status.success(), "{}", show(&c));
        c.stdout = std::fs::read(&self.record).unwrap();
        last_json(&c)
    }

    fn findings(&self) -> Vec<String> {
        crate::support::autodrive_gh_fake::lines(&self.record.with_extension("json.findings"))
            .into_iter()
            .filter(|l| !l.trim().is_empty())
            .collect()
    }

    fn waits(&self) -> Value {
        let p = self.record.with_extension("json.waits");
        let text = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
        let mode =
            std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(&p).unwrap().permissions());
        assert_eq!(mode & 0o777, 0o600, "the .waits file is private");
        serde_json::from_str(text.trim()).unwrap()
    }
}

#[test]
fn a_pending_ci_round_writes_no_fixable_findings_and_a_waiting_record() {
    let r = Round::new();
    r.verdict(
        "PENDING",
        "PASS",
        "PENDING",
        "BLOCKED",
        json!([{"id": "ci-pending"}]),
    );
    assert!(
        r.findings().is_empty(),
        "the hashed .findings holds fixable ids only: {:?}",
        r.findings()
    );
    let w = r.waits();
    assert_eq!(w["round_waiting"], true, "{w}");
    assert_eq!(w["wait_target"], format!("ci@{}", r.g.head()), "{w}");
    let rec = r.record("PENDING", "PASS", "PENDING", "BLOCKED");
    assert_eq!(rec["round_waiting"], true, "{rec}");
    assert_eq!(rec["wait_target"], format!("ci@{}", r.g.head()), "{rec}");
    assert!(
        rec["waits"].as_array().is_some_and(|a| !a.is_empty()),
        "{rec}"
    );
    assert_eq!(rec["merge_state"], "BLOCKED", "{rec}");
}

#[test]
fn a_round_with_a_qa_failure_keeps_measured_evidence_disagrees_fixable() {
    let r = Round::new();
    r.verdict(
        "PENDING",
        "FAIL",
        "PENDING",
        "BLOCKED",
        json!([{"id": "ci-pending"}]),
    );
    assert!(
        r.findings()
            .contains(&"measured-evidence-disagrees".to_string()),
        "{:?}",
        r.findings()
    );
    assert!(
        !r.findings().contains(&"ci-pending".to_string()),
        "{:?}",
        r.findings()
    );
    let rec = r.record("PENDING", "FAIL", "PENDING", "BLOCKED");
    assert_eq!(
        rec["round_waiting"], false,
        "a mixed round is not waiting: {rec}"
    );
}

#[test]
fn an_unknown_merge_state_round_is_waiting_on_mergeability() {
    let r = Round::new();
    r.verdict("GREEN", "PASS", "PENDING", "UNKNOWN", json!([]));
    let rec = r.record("GREEN", "PASS", "PENDING", "UNKNOWN");
    assert_eq!(rec["merge_state"], "UNKNOWN", "{rec}");
    assert_eq!(rec["round_waiting"], true, "{rec}");
    assert_eq!(
        rec["wait_target"],
        format!("mergeability@{}", r.g.head()),
        "{rec}"
    );
}

#[test]
fn agent_ids_the_classifier_does_not_know_stay_fixable() {
    let r = Round::new();
    r.verdict(
        "PENDING",
        "PASS",
        "PENDING",
        "BLOCKED",
        json!([{"id": "ci-pending"}, {"id": "flaky-ish"}]),
    );
    assert_eq!(r.findings(), ["flaky-ish"], "{:?}", r.findings());
    let rec = r.record("PENDING", "PASS", "PENDING", "BLOCKED");
    assert_eq!(rec["round_waiting"], false, "{rec}");
}

// ── autodrive_pr_head.sh ─────────────────────────────────────────────────────

#[test]
fn pr_head_falls_back_to_rest_when_graphql_is_refused() {
    let g = GhRepo::new();
    g.mode("gql", "refuse");
    let o = tool(
        &g,
        "autodrive_pr_head.sh",
        &["--pr", "7", "--repo", g.repo.to_str().unwrap()],
    );
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    let v = last_json(&o);
    assert_eq!(v["head_sha"], g.head().as_str(), "{v}");
    assert_eq!(v["head_ref"], BRANCH, "{v}");
}

#[test]
fn pr_head_exits_4_only_when_every_path_is_unavailable_and_says_so() {
    let g = GhRepo::new();
    g.mode("gql", "refuse");
    g.mode("rest", "refuse");
    let o = tool(
        &g,
        "autodrive_pr_head.sh",
        &["--pr", "7", "--repo", g.repo.to_str().unwrap()],
    );
    assert_eq!(o.status.code(), Some(4), "{}", show(&o));
    assert!(stderr(&o).contains("unavailable"), "{}", show(&o));
    assert!(
        stderr(&o).contains("secondary rate limit"),
        "the helper's reason is printed: {}",
        show(&o)
    );
}

// ── merge-evidence step-01 and step-03, through the helpers (A8) ────────────
//
// #2340 turned merge-evidence step-01 and step-03 into agent steps, so their
// bash is gone and a prompt cannot run in a test. The behaviour those steps
// rely on lives in the helpers they are told to call, and is tested here
// through the fake `gh`: the base comes from REST, update-branch takes no
// base and writes only after a qualifying previous-round sidecar, and the
// CI read is a single call that reports pending as an answer.

fn ub_puts(g: &GhRepo) -> Vec<String> {
    crate::support::autodrive_gh_fake::lines(&g.log.join("update-branch.log"))
}

#[test]
fn step01_pr_read_gives_base_and_merge_state_over_rest_when_graphql_is_refused() {
    let g = GhRepo::new();
    let mut pr: Value =
        serde_json::from_str(&std::fs::read_to_string(g.fix.join("rest-pr.json")).unwrap())
            .unwrap();
    pr["base"]["ref"] = json!("develop");
    pr["mergeable"] = Value::Null;
    pr["mergeable_state"] = json!("unknown");
    g.fixture("rest-pr.json", &pr);
    g.mode("gql", "refuse");
    let o = g.read(&["pr", "7"]);
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    let v = last_json(&o);
    assert_eq!(v["source"], "rest", "{v}");
    assert_eq!(
        v["base_ref"], "develop",
        "the PR's own base, never main: {v}"
    );
    assert_eq!(v["merge_state"], "UNKNOWN", "{v}");
    assert_eq!(v["mergeable"], "UNKNOWN", "{v}");
}

#[test]
fn step01_pr_read_with_every_path_refused_names_no_base() {
    let g = GhRepo::new();
    g.mode("gql", "refuse");
    g.mode("rest", "refuse");
    let o = g.read(&["pr", "7"]);
    assert_eq!(o.status.code(), Some(3), "{}", show(&o));
    let v = last_json(&o);
    assert!(
        v.get("base_ref").is_none_or(Value::is_null),
        "an unavailable read never guesses a base: {v}"
    );
}

#[test]
fn update_branch_with_a_base_flag_is_a_usage_error_and_makes_no_gh_call() {
    let g = GhRepo::new();
    let head = g.head();
    let pw = g.prior_wait(&head);
    let pw = pw.to_str().unwrap();
    for args in [
        &[
            "7",
            "--expected-head-sha",
            &head,
            "--prior-wait",
            pw,
            "--base",
            "main",
        ][..],
        &[
            "7",
            "--expected-head-sha",
            &head,
            "--prior-wait",
            pw,
            "--base=main",
        ],
        &["7", "--expected-head-sha", &head],
        &["7", "--prior-wait", pw],
    ] {
        let o = g.update_branch(args);
        assert_eq!(o.status.code(), Some(2), "{args:?}: {}", show(&o));
    }
    assert!(g.calls().is_empty(), "{:?}", g.calls());
    assert!(ub_puts(&g).is_empty());
}

#[test]
fn update_branch_with_a_mismatched_expected_head_is_refused_once() {
    let g = GhRepo::new();
    let head = g.head();
    let pw = g.prior_wait(&head);
    g.mode("ub", "422-moved");
    let o = g.update_branch(&[
        "7",
        "--expected-head-sha",
        &head,
        "--prior-wait",
        pw.to_str().unwrap(),
    ]);
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    let v = last_json(&o);
    assert_eq!(v["action"], "update-branch", "{v}");
    assert_eq!(
        v["result"], "refused",
        "a 422 SHA mismatch is never a wait: {v}"
    );
    assert_eq!(ub_puts(&g).len(), 1, "never retried: {:?}", ub_puts(&g));
}

/// Runs update-branch against a previous-round sidecar and asserts the
/// helper answered `first-read` with `reason`, and made no `gh` call.
fn assert_first_read(g: &GhRepo, head: &str, prior: &str, reason: &str, case: &str) {
    let o = g.update_branch(&["7", "--expected-head-sha", head, "--prior-wait", prior]);
    assert_eq!(o.status.code(), Some(0), "{case}: {}", show(&o));
    let v = last_json(&o);
    assert_eq!(
        v,
        json!({"pr": 7, "action": "none", "result": "first-read",
               "expected_head_sha": head, "reason": reason}),
        "{case}"
    );
    assert!(g.calls().is_empty(), "{case}: no gh call: {:?}", g.calls());
    assert!(ub_puts(g).is_empty(), "{case}: no write");
}

#[test]
fn update_branch_first_read_cases_make_no_gh_call() {
    let other = "0123456789abcdef0123456789abcdef01234567";

    let g = GhRepo::new();
    let head = g.head();
    assert_first_read(&g, &head, "", "prior wait: none", "empty path");

    let g = GhRepo::new();
    let head = g.head();
    let missing = g.tmp.path().join("state/merge-ready-round-0.json.wait");
    assert_first_read(
        &g,
        &head,
        missing.to_str().unwrap(),
        "prior wait: none",
        "missing file",
    );

    let g = GhRepo::new();
    let head = g.head();
    let real = g.prior_wait(&head);
    let link = real.with_file_name("link.json.wait");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    assert_first_read(
        &g,
        &head,
        link.to_str().unwrap(),
        "prior wait: invalid",
        "symlink",
    );

    // A FIFO with no writer would block a plain open forever; the reader
    // opens it non-blocking and refuses it as not a regular file.
    let g = GhRepo::new();
    let head = g.head();
    let fifo = g.prior_wait(&head).with_file_name("fifo.json.wait");
    let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
    // SAFETY: `c` is a valid NUL-terminated path.
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0, "mkfifo");
    assert_first_read(
        &g,
        &head,
        fifo.to_str().unwrap(),
        "prior wait: invalid",
        "fifo",
    );

    let g = GhRepo::new();
    let head = g.head();
    let mut big = crate::support::autodrive_gh_fake::unknown_sidecar(&head);
    big.truncate(big.len() - 1);
    big.push_str(&format!(",\"pad\":\"{}\"}}", "x".repeat(4096)));
    let p = g.prior_wait_raw(&big);
    assert_first_read(
        &g,
        &head,
        p.to_str().unwrap(),
        "prior wait: invalid",
        "over 4 KiB",
    );

    let g = GhRepo::new();
    let head = g.head();
    let p = g.prior_wait_raw(
        &json!({"wait": format!("mergeability@{head}"), "head_sha": head,
                "merge_state": "UNKNOWN", "extra": 1})
        .to_string(),
    );
    assert_first_read(
        &g,
        &head,
        p.to_str().unwrap(),
        "prior wait: invalid",
        "unknown key",
    );

    let g = GhRepo::new();
    let head = g.head();
    let p = g.prior_wait_raw(
        &json!({"wait": format!("mergeability@{head}"), "head_sha": head.to_uppercase(),
                "merge_state": "UNKNOWN"})
        .to_string(),
    );
    assert_first_read(
        &g,
        &head,
        p.to_str().unwrap(),
        "prior wait: invalid",
        "bad head_sha",
    );

    let g = GhRepo::new();
    let head = g.head();
    let p = g.prior_wait_raw("not json");
    assert_first_read(
        &g,
        &head,
        p.to_str().unwrap(),
        "prior wait: invalid",
        "not JSON",
    );

    let g = GhRepo::new();
    let head = g.head();
    let p = g.prior_wait_raw(
        &json!({"wait": format!("ci@{head}"), "head_sha": head, "merge_state": "BLOCKED"})
            .to_string(),
    );
    assert_first_read(
        &g,
        &head,
        p.to_str().unwrap(),
        "prior wait: not unknown",
        "BLOCKED",
    );

    let g = GhRepo::new();
    let head = g.head();
    let p = g.prior_wait(other);
    assert_first_read(
        &g,
        &head,
        p.to_str().unwrap(),
        "prior wait: head differs",
        "moved head",
    );
}

#[test]
fn update_branch_with_a_qualifying_prior_wait_makes_exactly_one_put() {
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
    let v = last_json(&o);
    assert_eq!(
        v,
        json!({"pr": 7, "action": "update-branch", "result": "ok", "expected_head_sha": head})
    );
    let puts = ub_puts(&g);
    assert_eq!(puts.len(), 1, "{puts:?}");
    assert!(puts[0].starts_with("PUT "), "{puts:?}");
    assert!(
        puts[0].contains(&format!("expected_head_sha={head}")),
        "{puts:?}"
    );
    assert!(!puts[0].contains("base"), "no base is sent: {puts:?}");
    assert!(pw.exists(), "the helper never deletes the previous sidecar");
}

// ── step-03: one CI read at the head, pending is an answer ──────────────────

#[test]
fn step03_checks_by_sha_is_one_gh_call_and_reports_pending() {
    for gql in ["ok", "refuse"] {
        let g = GhRepo::new();
        g.mode("gql", gql);
        let head = g.head();
        let o = g.read(&["checks", &head]);
        assert_eq!(o.status.code(), Some(0), "{gql}: {}", show(&o));
        let v = last_json(&o);
        assert_eq!(v["source"], "rest", "{gql}: a SHA read goes to REST: {v}");
        assert!(
            v["checks"]
                .as_array()
                .is_some_and(|c| c.iter().any(|c| c["bucket"] == "pending")),
            "{gql}: pending checks are reported, not UNREADABLE: {v}"
        );
        let check_reads = g
            .calls()
            .iter()
            .filter(|c| c.contains("/check-runs"))
            .count();
        assert_eq!(
            check_reads,
            1,
            "{gql}: read once, never polled: {:?}",
            g.calls()
        );
    }
}

// ── autodrive_platform_facts.sh: the REST fallback states no review decision ─

fn platform_facts(g: &GhRepo) -> Value {
    let o = tool(g, "autodrive_platform_facts.sh", &["7"]);
    assert!(o.status.success(), "{}", show(&o));
    last_json(&o)
}

#[test]
fn rest_fallback_with_no_review_rule_and_no_reviews_is_not_a_missing_approval() {
    // #2333 review: REST counted approvals against an assumed requirement of
    // one, so a repo that requires no review got required-approval-missing
    // on a clean PR, a STUCK finding on the fallback path.
    let g = GhRepo::new();
    g.mode("gql", "refuse");
    g.fixture("rest-reviews.json", &json!([]));
    let v = platform_facts(&g);
    assert_eq!(v["review_decision"], "", "{v}");
    assert_eq!(v["required_approvals"], "0", "{v}");
    assert_eq!(v["approval_status"], "MET", "{v}");
    assert_eq!(v["approval_source"], "required-count", "{v}");
    assert_eq!(v["approval_blocker"], "", "{v}");
}

#[test]
fn rest_fallback_never_calls_one_approval_enough_when_two_are_required() {
    let g = GhRepo::new();
    g.mode("gql", "refuse");
    g.fixture(
        "rest-rules.json",
        &json!([{"type": "pull_request", "parameters": {"required_approving_review_count": 2}}]),
    );
    // alice's one approval (the default fixture) is not GitHub's decision.
    let v = platform_facts(&g);
    assert_eq!(v["review_decision"], "", "{v}");
    assert_eq!(v["required_approvals"], "2", "{v}");
    assert_eq!(v["approval_status"], "NOT_MET", "{v}");
    assert_eq!(v["approval_source"], "required-count", "{v}");
    assert_eq!(v["approval_blocker"], "required-approval-missing", "{v}");
}

#[test]
fn bad_credentials_on_every_path_reach_the_waits_classifier_as_refused() {
    // Review round 17: a 401 everywhere was a wait, so the drive never
    // reached STUCK. The unavailable entry now says refused.
    let g = GhRepo::new();
    g.mode("gql", "auth");
    g.mode("rest", "auth");
    let v = platform_facts(&g);
    let u = v["unavailable"].as_array().unwrap_or_else(|| panic!("{v}"));
    assert!(!u.is_empty(), "{v}");
    assert!(
        u.iter()
            .all(|r| r["refused"] == true && r["rate_limited"] == false),
        "{v}"
    );
}
