//! The bash read sites and the round evidence (D5, D7, D8): each runs the
//! real tool or recipe step body under the #2333 fake `gh`.

use std::process::Output;

use serde_json::{Value, json};

use crate::support::auto_drive_light::{show, stderr, stdout, step_command, tools_dir};
use crate::support::autodrive_gh_fake::{BRANCH, GhRepo, git_in};

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

// ── autodrive-merge-evidence.yaml step-01: no merge from a guessed base ─────

fn step(g: &GhRepo, step_id: &str) -> Output {
    let script = step_command("autodrive-merge-evidence", step_id);
    g.cmd("bash")
        .args(["-c", &script])
        .env("REPO_PATH", &g.repo)
        .env("PR_NUMBER", "7")
        .env(
            "AMPLIHACK_HOME",
            crate::support::auto_drive_light::repo_root(),
        )
        .env("RECIPE_VAR_autodrive_tools_dir", tools_dir())
        .output()
        .unwrap()
}

/// origin/main moves one commit ahead of the PR head, so a step that merged
/// from a base would change HEAD.
fn advance_main(g: &GhRepo) {
    git_in(&g.repo, &["commit", "--allow-empty", "-qm", "base moved"]);
    git_in(&g.repo, &["push", "-q", "origin", "HEAD:refs/heads/main"]);
    git_in(&g.repo, &["reset", "-q", "--hard", "HEAD~1"]);
}

#[test]
fn sync_with_base_reads_the_base_through_rest() {
    // The PR's base is `develop`, which is ahead; `main` is not. A step that
    // fell back to `main` would report base main and nothing to merge.
    let g = GhRepo::new();
    git_in(
        &g.repo,
        &["commit", "--allow-empty", "-qm", "develop moved"],
    );
    git_in(
        &g.repo,
        &["push", "-q", "origin", "HEAD:refs/heads/develop"],
    );
    git_in(&g.repo, &["reset", "-q", "--hard", "HEAD~1"]);
    let mut pr: Value =
        serde_json::from_str(&std::fs::read_to_string(g.fix.join("rest-pr.json")).unwrap())
            .unwrap();
    pr["base"]["ref"] = json!("develop");
    g.fixture("rest-pr.json", &pr);
    g.mode("gql", "refuse");
    let o = step(&g, "step-01-sync-with-base");
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    let v = last_json(&o);
    assert_eq!(v["base"], "develop", "the base came from REST: {v}");
    assert_eq!(v["synced"], "true", "{v}");
}

#[test]
fn sync_with_base_never_merges_a_guessed_main_when_the_base_is_unavailable() {
    let g = GhRepo::new();
    advance_main(&g);
    let before = g.head();
    g.mode("gql", "refuse");
    g.mode("rest", "refuse");
    let o = step(&g, "step-01-sync-with-base");
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    let v = last_json(&o);
    assert_eq!(v["reason"], "base-unavailable", "{v}");
    assert_ne!(v["base"], "main", "the silent main default is gone: {v}");
    assert_ne!(v["synced"], "true", "{v}");
    assert_eq!(g.head(), before, "nothing was merged");
}

// ── step-03: one CI read, no sleep loop ─────────────────────────────────────

/// A `sleep` that records the call and stops its caller, so a step that still
/// polls fails at once instead of hanging the test.
const TRIP_SLEEP: &str = "#!/usr/bin/env bash\necho \"sleep $*\" >> \"$FAKE_LOG/sleep.log\"\nkill -TERM \"$PPID\" 2>/dev/null\nexit 1\n";

#[test]
fn wait_for_ci_reads_once_and_records_pending_without_sleeping() {
    for gql in ["ok", "refuse"] {
        let g = GhRepo::new();
        crate::support::exec_stub::write_executable(&g.bin.join("sleep"), TRIP_SLEEP, 0o755);
        g.mode("gql", gql);
        let o = step(&g, "step-03-wait-for-ci");
        assert!(
            !g.log.join("sleep.log").exists(),
            "{gql}: the sleep 60 loop is removed: {}",
            show(&o)
        );
        assert_eq!(o.status.code(), Some(0), "{gql}: {}", show(&o));
        let v = last_json(&o);
        assert_eq!(
            v["ci_status"], "PENDING",
            "{gql}: pending is a recorded wait, not UNREADABLE: {v}"
        );
    }
}
