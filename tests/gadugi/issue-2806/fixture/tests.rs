//! Contracts for the test-only #2806 driver, not the CI scenario.
//!
//! The example executable uses the same test-only fixture modules. This file is also a module of cli_integration, so ordinary local
//! testing exercises the harness without running recipes or Gadugi scenarios.
//! Function/type names below propose internal fixture APIs, not runner metadata.
//! No regression allowance is justified by a compile failure in these tests.

#![cfg(unix)]
#![allow(dead_code)]

#[path = "agent.rs"]
mod agent;
#[path = "environment.rs"]
mod environment;
#[path = "evidence.rs"]
mod evidence;
#[path = "hosting.rs"]
mod hosting;
#[path = "lifecycle.rs"]
mod lifecycle;

use std::collections::BTreeMap;
use std::fs;
use std::io::BufReader;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

use agent::{Contract, Receipt, select_contract, validate_receipt};
use environment::{IsolatedCase, confined_path};
use evidence::{Allowance, Assertion, FailureKind, Outcome, Report};
use hosting::{Checks, HostingStore};
use lifecycle::{Event, TrackedChild, read_event};

const BASE: &str = "3af9633bd9244169c16434a5fc59a88af3c9bb24";

fn case() -> (tempfile::TempDir, IsolatedCase) {
    let temp = tempfile::tempdir().unwrap();
    let fixture = IsolatedCase::create(&temp.path().join("case")).unwrap();
    (temp, fixture)
}

fn git(case: &IsolatedCase, cwd: &Path, args: &[&str]) -> String {
    let mut command = amplihack_git::command_in(cwd);
    // env -i is explicit even for local Git fixture setup. No global process
    // environment or real user configuration is modified by these tests.
    command.env_clear().env("PATH", "/usr/bin:/bin");
    let executable = command.get_program().to_owned();
    let output = Command::new("/usr/bin/env")
        .env_clear()
        .args(["-i", "PATH=/usr/bin:/bin"])
        .arg(format!("HOME={}", case.home.display()))
        .args([
            "GIT_CONFIG_GLOBAL=/dev/null",
            "GIT_CONFIG_SYSTEM=/dev/null",
            "GIT_CONFIG_NOSYSTEM=1",
            "GIT_TERMINAL_PROMPT=0",
            "GIT_ALLOW_PROTOCOL=file",
        ])
        .arg(executable)
        .args([
            "-c",
            "user.name=Issue 2806",
            "-c",
            "user.email=qa@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "init.templateDir=",
        ])
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "local Git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

struct Repository {
    _temp: tempfile::TempDir,
    case: IsolatedCase,
    origin: PathBuf,
    checkout: PathBuf,
    head: String,
}

impl Repository {
    fn published() -> Self {
        let (temp, case) = case();
        let origin = case.root.join("origin.git");
        let checkout = case.root.join("checkout");
        git(
            &case,
            &case.root,
            &[
                "init",
                "--bare",
                "-q",
                "-b",
                "main",
                origin.to_str().unwrap(),
            ],
        );
        git(
            &case,
            &case.root,
            &[
                "clone",
                "-q",
                origin.to_str().unwrap(),
                checkout.to_str().unwrap(),
            ],
        );
        fs::write(checkout.join("answer.txt"), "before\n").unwrap();
        git(&case, &checkout, &["add", "answer.txt"]);
        git(&case, &checkout, &["commit", "-qm", "baseline"]);
        git(&case, &checkout, &["push", "-q", "-u", "origin", "main"]);
        git(&case, &checkout, &["checkout", "-qb", "feat/answer"]);
        fs::write(checkout.join("answer.txt"), "42\n").unwrap();
        git(&case, &checkout, &["commit", "-qam", "implement answer"]);
        git(
            &case,
            &checkout,
            &["push", "-q", "-u", "origin", "feat/answer"],
        );
        let head = git(&case, &checkout, &["rev-parse", "HEAD"]);
        Self {
            _temp: temp,
            case,
            origin,
            checkout,
            head,
        }
    }

    fn hosting(&self) -> HostingStore {
        HostingStore::create(
            &self.case.evidence.join("hosting.json"),
            "fixture/project",
            &self.origin,
        )
        .unwrap()
    }

    fn open_pr(&self, hosting: &mut HostingStore) -> u64 {
        let response = hosting
            .invoke(
                &self.checkout,
                &[
                    "pr",
                    "create",
                    "--repo",
                    "fixture/project",
                    "--head",
                    "feat/answer",
                    "--base",
                    "main",
                    "--title",
                    "Implement answer",
                    "--body",
                    "Closes #7",
                ],
            )
            .unwrap();
        assert_eq!(response.exit_code, 0);
        hosting
            .open_prs()
            .unwrap()
            .into_iter()
            .next()
            .unwrap()
            .number
    }
}

#[test]
fn environment_roots_are_private_and_evidence_is_outside_cleanup_candidates() {
    let (_temp, fixture) = case();
    for path in [
        &fixture.root,
        &fixture.home,
        &fixture.work,
        &fixture.tmp,
        &fixture.evidence,
    ] {
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o700,
            "{}",
            path.display()
        );
    }
    assert!(!fixture.evidence.starts_with(&fixture.work));
}

#[test]
fn command_runs_with_private_configuration_and_only_allowlisted_environment() {
    let (_temp, fixture) = case();
    let output = fixture
        .command(Path::new("/usr/bin/env"))
        .unwrap()
        .output()
        .unwrap();
    assert!(output.status.success());
    let observed: BTreeMap<_, _> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| {
            let (key, value) = line.split_once('=').unwrap();
            (key.to_owned(), value.to_owned())
        })
        .collect();
    assert_eq!(
        observed.get("HOME"),
        Some(&fixture.home.to_string_lossy().into_owned())
    );
    for key in [
        "CODEX_HOME",
        "CLAUDE_CONFIG_DIR",
        "XDG_CONFIG_HOME",
        "AMPLIHACK_HOME",
    ] {
        assert!(
            Path::new(observed.get(key).unwrap()).starts_with(&fixture.root),
            "{key}"
        );
    }
    assert_eq!(
        observed.get("GIT_ALLOW_PROTOCOL").map(String::as_str),
        Some("file")
    );
    assert_eq!(
        observed.get("GIT_CONFIG_GLOBAL").map(String::as_str),
        Some("/dev/null")
    );
    assert_eq!(
        observed.get("GIT_CONFIG_SYSTEM").map(String::as_str),
        Some("/dev/null")
    );
    for key in observed.keys() {
        assert!(
            fixture.allowed_environment_keys().contains(key),
            "unexpected key: {key}"
        );
    }
    for key in [
        "GH_TOKEN",
        "GITHUB_TOKEN",
        "ANTHROPIC_API_KEY",
        "OPENAI_API_KEY",
        "SSH_AUTH_SOCK",
        "GIT_CONFIG_COUNT",
        "GIT_DIR",
        "GIT_WORK_TREE",
    ] {
        assert!(
            !observed.contains_key(key),
            "inherited credential/configuration: {key}"
        );
    }
}

#[test]
fn confined_paths_reject_parent_absolute_and_symlink_escapes_before_writing() {
    let (temp, fixture) = case();
    let outside = temp.path().join("outside");
    fs::create_dir(&outside).unwrap();
    symlink(&outside, fixture.root.join("escape")).unwrap();
    for path in [
        Path::new("../outside/response"),
        outside.as_path(),
        Path::new("escape/response"),
    ] {
        assert!(
            confined_path(&fixture.root, path).is_err(),
            "{}",
            path.display()
        );
    }
    assert!(fs::read_dir(outside).unwrap().next().is_none());
    assert_eq!(
        confined_path(&fixture.root, Path::new("response.json")).unwrap(),
        fixture.root.join("response.json")
    );
}

#[test]
fn response_selection_uses_the_declared_contract_independent_of_request_order() {
    // The actual autodrive-build no-merge step declares these two output
    // fields. No invented runner step-id or invocation ordinal is supplied.
    let directive =
        Contract::json_object("no-merge-directive", &["task_description", "requirements"]);
    let preflight = Contract::json_object(
        "build-preflight",
        &["pr_found", "pr_state", "repo", "blockers"],
    );
    let contracts = [preflight, directive];
    for task in [
        "Return JSON {\"task_description\":\"...\",\"requirements\":\"...\"}.",
        "Task: preserve Unicode λ and a leading --option.\nReturn JSON {\"requirements\":\"...\",\"task_description\":\"...\"}.",
    ] {
        assert_eq!(
            select_contract(task, &contracts).unwrap().name,
            "no-merge-directive"
        );
    }
    assert!(select_contract("An unsupported task with no output contract", &contracts).is_err());
}

#[test]
fn ambiguous_contracts_and_malformed_responses_are_harness_errors() {
    let task = "Return JSON {\"task_description\":\"...\",\"requirements\":\"...\"}.";
    let contracts = [
        Contract::json_object("first", &["task_description", "requirements"]),
        Contract::json_object("second", &["task_description", "requirements"]),
    ];
    assert!(select_contract(task, &contracts).is_err());
    for response in [
        "not-json",
        "[]",
        "{}",
        "{\"task_description\":42,\"requirements\":\"x\"}",
    ] {
        assert!(
            contracts[0].validate_response(response).is_err(),
            "{response}"
        );
    }
}

#[test]
fn implementation_receipt_requires_observed_file_and_successful_verification() {
    let repo = Repository::published();
    let expected = repo.case.root.join("expected-answer");
    fs::write(&expected, "42\n").unwrap();
    let plan = serde_json::from_value(json!({
        "files": [{"path": "answer.txt", "content": "42\n"}],
        "checks": [{"executable": "/usr/bin/cmp", "args": [expected, "answer.txt"]}]
    }))
    .unwrap();
    // The adapter must perform its actions, including the real check, before
    // issuing a receipt. A claimed exit code is not a verification action.
    let receipt = agent::execute_plan(&repo.case, &repo.checkout, &plan).unwrap();
    assert!(validate_receipt(&repo.checkout, &receipt).is_ok());
    fs::write(repo.checkout.join("answer.txt"), "not implemented\n").unwrap();
    assert!(validate_receipt(&repo.checkout, &receipt).is_err());
    fs::write(repo.checkout.join("answer.txt"), "42\n").unwrap();
    fs::write(&expected, "different answer\n").unwrap();
    assert!(agent::execute_plan(&repo.case, &repo.checkout, &plan).is_err());
    fs::remove_file(repo.checkout.join("answer.txt")).unwrap();
    assert!(validate_receipt(&repo.checkout, &receipt).is_err());
}

#[test]
fn codex_transport_writes_the_final_message_only_inside_the_private_root() {
    let (temp, fixture) = case();
    let final_message = fixture.tmp.join("final-message.txt");
    let response = "{\"requirements\":\"λ\",\"task_description\":\"--unchanged\"}\n";
    agent::write_codex_final(&fixture, &final_message, response).unwrap();
    assert_eq!(fs::read_to_string(final_message).unwrap(), response);
    let outside = temp.path().join("outside-response");
    assert!(agent::write_codex_final(&fixture, &outside, response).is_err());
    assert!(!outside.exists());
}

#[test]
fn clean_review_receipt_requires_the_observed_head_and_zero_commits() {
    let repo = Repository::published();
    let clean = Receipt::clean_review(repo.head.clone(), repo.head.clone());
    assert!(validate_receipt(&repo.checkout, &clean).is_ok());
    git(
        &repo.case,
        &repo.checkout,
        &["commit", "--allow-empty", "-qm", "unexpected review edit"],
    );
    assert!(validate_receipt(&repo.checkout, &clean).is_err());
}

#[test]
fn hosting_state_persists_and_pending_checks_do_not_become_green_on_read() {
    let repo = Repository::published();
    let mut hosting = repo.hosting();
    let number = repo.open_pr(&mut hosting);
    let state_path = hosting.path().to_owned();
    drop(hosting);
    let mut reloaded = HostingStore::open(&state_path).unwrap();
    for _ in 0..2 {
        let response = reloaded
            .invoke(
                &repo.checkout,
                &[
                    "pr",
                    "checks",
                    &number.to_string(),
                    "--json",
                    "name,state,bucket",
                ],
            )
            .unwrap();
        assert_ne!(response.exit_code, 0);
        assert_eq!(reloaded.pr(number).unwrap().checks, Checks::Pending);
        assert_eq!(reloaded.pr(number).unwrap().head_sha, repo.head);
    }
    assert_eq!(reloaded.open_prs().unwrap().len(), 1);
}

#[test]
fn hosting_refuses_unpushed_heads_and_unknown_operations_without_state_mutation() {
    let repo = Repository::published();
    let mut hosting = repo.hosting();
    let number = repo.open_pr(&mut hosting);
    let before = fs::read(hosting.path()).unwrap();
    for args in [
        vec![
            "pr",
            "create",
            "--head",
            "feat/not-pushed",
            "--base",
            "main",
            "--title",
            "bad",
            "--body",
            "bad",
        ],
        vec!["api", "unsupported/endpoint"],
        vec!["pr", "view", "999999", "--json", "state"],
    ] {
        let response = hosting.invoke(&repo.checkout, &args);
        assert!(response.is_err() || response.unwrap().exit_code != 0);
        assert_eq!(fs::read(hosting.path()).unwrap(), before);
    }
    assert_eq!(hosting.pr(number).unwrap().state, "OPEN");
}

#[test]
fn hosting_merges_only_after_explicit_green_and_matching_requested_pr_and_origin_heads() {
    let repo = Repository::published();
    let mut hosting = repo.hosting();
    let number = repo.open_pr(&mut hosting);
    let number = number.to_string();
    let args = [
        "pr",
        "merge",
        &number,
        "--squash",
        "--match-head-commit",
        &repo.head,
    ];
    assert_ne!(hosting.invoke(&repo.checkout, &args).unwrap().exit_code, 0);
    hosting
        .set_checks(number.parse().unwrap(), Checks::Green)
        .unwrap();
    let base = git(&repo.case, &repo.checkout, &["rev-parse", "main"]);
    let stale = [
        "pr",
        "merge",
        &number,
        "--squash",
        "--match-head-commit",
        &base,
    ];
    assert_ne!(hosting.invoke(&repo.checkout, &stale).unwrap().exit_code, 0);
    assert_ne!(
        hosting
            .invoke(&repo.checkout, &["pr", "merge", &number, "--squash"])
            .unwrap()
            .exit_code,
        0
    );
    // The adapter must read origin again, rather than trusting its stored PR
    // state. Move origin forward through an ordinary commit and push.
    git(
        &repo.case,
        &repo.checkout,
        &["commit", "--allow-empty", "-qm", "new remote head"],
    );
    git(
        &repo.case,
        &repo.checkout,
        &["push", "-q", "origin", "feat/answer"],
    );
    assert_ne!(hosting.invoke(&repo.checkout, &args).unwrap().exit_code, 0);
    assert_eq!(hosting.pr(number.parse().unwrap()).unwrap().state, "OPEN");
}

#[test]
fn hosting_successful_merge_has_fresh_merged_readback_and_recorded_invocation() {
    let repo = Repository::published();
    let mut hosting = repo.hosting();
    let number = repo.open_pr(&mut hosting);
    hosting.set_checks(number, Checks::Green).unwrap();
    let merged = hosting
        .invoke(
            &repo.checkout,
            &[
                "pr",
                "merge",
                &number.to_string(),
                "--squash",
                "--match-head-commit",
                &repo.head,
            ],
        )
        .unwrap();
    assert_eq!(merged.exit_code, 0);
    let readback = hosting
        .invoke(
            &repo.checkout,
            &[
                "pr",
                "view",
                &number.to_string(),
                "--json",
                "state,headRefOid",
            ],
        )
        .unwrap();
    let readback: Value = serde_json::from_slice(&readback.stdout).unwrap();
    assert_eq!(readback["state"], "MERGED");
    assert_eq!(readback["headRefOid"], repo.head);
    assert!(hosting.invocations().unwrap().iter().any(|call| {
        call.args
            .windows(2)
            .any(|pair| pair == ["--match-head-commit", repo.head.as_str()])
    }));
}

#[test]
fn separate_drive_fixtures_do_not_share_origins_or_hosting_state() {
    let control = Repository::published();
    let regression = Repository::published();
    assert_ne!(control.origin, regression.origin);
    let mut control_hosting = control.hosting();
    let mut regression_hosting = regression.hosting();
    let control_pr = control.open_pr(&mut control_hosting);
    let regression_pr = regression.open_pr(&mut regression_hosting);
    control_hosting
        .set_checks(control_pr, Checks::Green)
        .unwrap();
    assert_eq!(
        regression_hosting.pr(regression_pr).unwrap().checks,
        Checks::Pending
    );
    assert_eq!(
        git(
            &regression.case,
            &regression.origin,
            &["rev-parse", "refs/heads/feat/answer"]
        ),
        regression.head
    );
}

#[test]
fn lifecycle_pipe_events_require_complete_records_and_do_not_treat_eof_as_ready() {
    let event = b"{\"event\":\"work_ready\",\"run_id\":\"run-a\",\"pid\":123,\"commit\":\"0123456789012345678901234567890123456789\",\"dirty_path\":\"continue.txt\"}\n";
    assert!(matches!(
        read_event(&mut BufReader::new(event.as_slice())).unwrap(),
        Event::WorkReady { .. }
    ));
    for bytes in [
        b"".as_slice(),
        b"{}\n",
        b"not json\n",
        b"{\"event\":\"work_ready\",\"pid\":0}\n",
    ] {
        assert!(read_event(&mut BufReader::new(bytes)).is_err());
    }
}

#[test]
fn lifecycle_interrupts_only_its_recorded_child_after_explicit_pipe_readiness() {
    let (_temp, fixture) = case();
    let pid_record = fixture.evidence.join("child.pid");
    let mut command = fixture.command(Path::new("/bin/sh")).unwrap();
    command
        .args([
            "-c",
            "printf '{\"event\":\"process_ready\",\"pid\":%s}\\n' \"$$\"; read -r release",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());
    let mut child = TrackedChild::spawn(&mut command, &pid_record).unwrap();
    let pid = child.id();
    assert_eq!(
        fs::read_to_string(&pid_record)
            .unwrap()
            .trim()
            .parse::<u32>()
            .unwrap(),
        pid
    );
    let event = read_event(&mut BufReader::new(child.take_stdout().unwrap())).unwrap();
    assert_eq!(event, Event::ProcessReady { pid });
    let status = child.interrupt_and_wait().unwrap();
    assert!(!status.success());
    // Reaping seals signaling authority, even if the OS later reuses the PID.
    assert!(child.interrupt_and_wait().is_err());
}

fn worktree_allowance() -> Allowance {
    Allowance {
        assertion_id: "drive-missing.worktree".into(),
        issue: 2802,
        base_sha: BASE.into(),
        evidence_path: "baseline/2802.json".into(),
        failure_fingerprint: "no-worktree".into(),
    }
}

fn observed_failure(kind: FailureKind, fingerprint: &str) -> Assertion {
    Assertion::failed("drive-missing", "drive-missing.worktree", kind, fingerprint)
}

fn write_baseline_unit_input(root: &Path) {
    // Synthetic reporter input only. This is not a reproduction of #2802 and
    // must never be copied into the scenario's actual expected-failure file.
    let baseline = root.join("baseline");
    fs::create_dir(&baseline).unwrap();
    fs::write(baseline.join("2802.json"), serde_json::to_vec(&json!({
        "tested_sha": BASE, "assertion_id": "drive-missing.worktree", "issue": 2802,
        "executed": true, "prerequisites_satisfied": true,
        "failure_kind": "regression", "failure_fingerprint": "no-worktree",
        "observations": {"pr_state": "OPEN", "private_repository": true, "worktrees_at_pr_head": []}
    })).unwrap()).unwrap();
}

#[test]
fn allowances_require_observed_base_evidence_and_preserve_newly_passing_assertions() {
    let (temp, fixture) = case();
    let mut report = Report::new(&fixture.evidence, BASE).unwrap();
    let allowance = worktree_allowance();
    assert!(report.add_allowance(allowance.clone()).is_err());
    write_baseline_unit_input(&fixture.evidence);
    report.add_allowance(allowance).unwrap();
    assert_eq!(
        report
            .classify(observed_failure(FailureKind::Regression, "no-worktree"))
            .unwrap(),
        Outcome::Xfail { issue: 2802 }
    );
    assert_eq!(
        report
            .classify(Assertion::passed("drive-missing", "drive-missing.worktree"))
            .unwrap(),
        Outcome::Pass
    );
    assert_eq!(
        report
            .classify(observed_failure(FailureKind::Regression, "unrelated-error"))
            .unwrap(),
        Outcome::Fail
    );
    // Baseline references cannot escape the retained evidence root.
    let mut escaped = worktree_allowance();
    escaped.evidence_path = temp.path().join("elsewhere.json");
    assert!(report.add_allowance(escaped).is_err());
}

#[test]
fn harness_failures_and_unexecuted_assertions_never_receive_expected_failure_credit() {
    let (_temp, fixture) = case();
    let mut report = Report::new(&fixture.evidence, BASE).unwrap();
    write_baseline_unit_input(&fixture.evidence);
    report.add_allowance(worktree_allowance()).unwrap();
    for kind in [
        FailureKind::Harness,
        FailureKind::Isolation,
        FailureKind::Prerequisite,
        FailureKind::MalformedResponse,
    ] {
        assert_eq!(
            report
                .classify(observed_failure(kind, "no-worktree"))
                .unwrap(),
            Outcome::Fail
        );
    }
    let blocked = Assertion::blocked(
        "drive-missing",
        "drive-missing.merge",
        "drive-missing.worktree",
        "build never entered",
    );
    assert_eq!(
        report.classify(blocked).unwrap(),
        Outcome::NotExecuted {
            blocking_assertion: "drive-missing.worktree".into(),
            reason: "build never entered".into(),
        }
    );
}

#[test]
fn aggregate_results_require_every_case_and_reject_cross_case_blocking() {
    let (_temp, fixture) = case();
    let mut report = Report::new(&fixture.evidence, BASE).unwrap();
    report
        .record(Assertion::failed(
            "build",
            "build.terminal",
            FailureKind::Harness,
            "agent did not start",
        ))
        .unwrap();
    // A failed build must not prevent independently seeded drive or cleanup
    // evidence from being recorded; omissions cannot be a green aggregate.
    assert!(
        report
            .record(Assertion::blocked(
                "drive-control",
                "drive-control.merge",
                "build.terminal",
                "build failed"
            ))
            .is_err()
    );
    report
        .record(Assertion::passed("drive-control", "drive-control.merge"))
        .unwrap();
    report
        .record(Assertion::passed(
            "cleanup-interrupted",
            "cleanup-interrupted.commit",
        ))
        .unwrap();
    report.persist().unwrap();
    let reloaded = Report::open(&fixture.evidence).unwrap();
    assert_eq!(
        reloaded.outcome("drive-control.merge").unwrap(),
        Outcome::Pass
    );
    assert_eq!(
        reloaded.outcome("cleanup-interrupted.commit").unwrap(),
        Outcome::Pass
    );
    assert!(!reloaded.success());
    assert!(
        !Report::new(&fixture.evidence.join("empty"), BASE)
            .unwrap()
            .success()
    );
    let mut incomplete = Report::new(&fixture.evidence.join("incomplete"), BASE).unwrap();
    incomplete
        .record(Assertion::passed("build", "build.complete"))
        .unwrap();
    assert!(
        !incomplete.success(),
        "one passing case cannot substitute for omitted cases"
    );
}

#[test]
fn build_acceptance_requires_local_commit_origin_and_open_pr_independently() {
    let repo = Repository::published();
    let mut hosting = repo.hosting();
    let number = repo.open_pr(&mut hosting);
    let observation =
        evidence::observe_build(&repo.checkout, &repo.origin, &hosting, number, "answer.txt")
            .unwrap();
    assert!(
        observation.local_committed && observation.remote_matches && observation.open_pr_matches
    );
    // Updating only the checkout must not fabricate a push or a hosting update.
    fs::write(repo.checkout.join("answer.txt"), "43\n").unwrap();
    git(
        &repo.case,
        &repo.checkout,
        &["commit", "-qam", "unpublished work"],
    );
    let observation =
        evidence::observe_build(&repo.checkout, &repo.origin, &hosting, number, "answer.txt")
            .unwrap();
    assert!(observation.local_committed);
    assert!(!observation.remote_matches);
    assert!(!observation.open_pr_matches);
}

#[test]
fn pending_finalization_acceptance_requires_real_state_and_consistent_typed_outcome() {
    // These are evaluator unit inputs, not baseline reproductions or XFAIL
    // evidence. CI must obtain each observation from the real finalizer.
    for (context, read_error) in [
        ("valid", Value::Null),
        ("missing", json!("NotFound")),
        ("unreadable", json!("PermissionDenied")),
    ] {
        let mut raw = json!({
            "context": context, "context_read_error": read_error,
            "fault_before_consumption": context != "valid",
            "origin_head": BASE, "pr_head": BASE, "pr_state": "OPEN", "checks": "pending",
            "terminal_state": "BLOCKED_CI", "terminal_success": false, "exit_code": 1
        });
        let observation = serde_json::from_value(raw.clone()).unwrap();
        assert!(
            evidence::validate_pending_finalization(&observation).is_ok(),
            "{context}"
        );
        raw["terminal_state"] = json!("FAILED_IMPLEMENTATION");
        let observation = serde_json::from_value(raw.clone()).unwrap();
        assert!(
            evidence::validate_pending_finalization(&observation).is_err(),
            "{context}"
        );
        raw["terminal_state"] = json!("BLOCKED_CI");
        raw["exit_code"] = json!(0);
        let observation = serde_json::from_value(raw.clone()).unwrap();
        assert!(evidence::validate_pending_finalization(&observation).is_err());
        raw["exit_code"] = json!(1);
        raw["pr_head"] = json!("0123456789012345678901234567890123456789");
        let observation = serde_json::from_value(raw).unwrap();
        assert!(evidence::validate_pending_finalization(&observation).is_err());
    }
}

#[test]
fn retention_observer_checks_unpushed_commit_dirty_content_and_branch_continuation() {
    let repo = Repository::published();
    git(
        &repo.case,
        &repo.checkout,
        &["commit", "--allow-empty", "-qm", "unpublished work"],
    );
    let unpushed = git(&repo.case, &repo.checkout, &["rev-parse", "HEAD"]);
    fs::write(repo.checkout.join("continue.txt"), "unfinished work\n").unwrap();
    let observed = evidence::observe_retention(
        &repo.checkout,
        &repo.origin,
        &unpushed,
        "continue.txt",
        b"unfinished work\n",
    )
    .unwrap();
    assert!(
        observed.commit_reachable
            && observed.dirty_content_intact
            && observed.branch_usable
            && observed.unpushed
    );
    // Demonstrate actual continuation before publication, rather than merely
    // accepting a model's claim that a branch could be continued.
    git(&repo.case, &repo.checkout, &["add", "continue.txt"]);
    git(
        &repo.case,
        &repo.checkout,
        &["commit", "-qm", "continue retained work"],
    );
    assert_eq!(
        git(&repo.case, &repo.checkout, &["show", "HEAD:continue.txt"]),
        "unfinished work"
    );
    fs::remove_file(repo.checkout.join("continue.txt")).unwrap();
    let observed = evidence::observe_retention(
        &repo.checkout,
        &repo.origin,
        &unpushed,
        "continue.txt",
        b"unfinished work\n",
    )
    .unwrap();
    assert!(observed.commit_reachable);
    assert!(!observed.dirty_content_intact);
}

#[test]
fn private_execution_evidence_rejects_caller_and_detached_launch_as_pr_worktrees() {
    let (_temp, fixture) = case();
    let head = "0123456789012345678901234567890123456789";
    let mut observation = evidence::DriveObservation {
        caller_common_dir: fixture.root.join("caller/.git"),
        private_common_dir: fixture.work.join("exec/copy/repository.git"),
        execution_common_dir: fixture.work.join("exec/copy/repository.git"),
        launch_checkout: fixture.work.join("exec/copy/launch"),
        resolved_worktree: fixture.work.join("exec/copy/worktrees/pr-7"),
        observed_head: head.into(),
        worktree_head: head.into(),
        worktree_branch: Some("feat/answer".into()),
        registered_linked_worktree: true,
    };
    assert!(evidence::validate_drive_location(&observation).is_ok());
    observation.execution_common_dir = observation.caller_common_dir.clone();
    assert!(evidence::validate_drive_location(&observation).is_err());
    observation.execution_common_dir = observation.private_common_dir.clone();
    observation.resolved_worktree = observation.launch_checkout.clone();
    assert!(evidence::validate_drive_location(&observation).is_err());
}

#[test]
fn aggregate_never_accepts_case_names_without_each_required_observation() {
    let (_temp, fixture) = case();
    let mut report = Report::new(&fixture.evidence, BASE).unwrap();
    for name in evidence::CASES {
        report
            .record(Assertion::passed(name, &format!("{name}.complete")))
            .unwrap();
    }
    assert!(
        !report.success(),
        "case labels cannot replace required behavioral observations"
    );
}

#[test]
fn hosting_rejects_cross_repository_identity_without_mutating_the_pr() {
    let repo = Repository::published();
    let mut hosting = repo.hosting();
    repo.open_pr(&mut hosting);
    let before = fs::read(hosting.path()).unwrap();
    assert!(
        hosting
            .invoke(
                &repo.checkout,
                &[
                    "pr",
                    "view",
                    "1",
                    "--repo",
                    "other/project",
                    "--json",
                    "state"
                ]
            )
            .is_err()
    );
    assert_eq!(fs::read(hosting.path()).unwrap(), before);
}

#[test]
fn process_network_view_requires_complete_namespace_observations() {
    let header = "Inter-| Receive | Transmit\n face |bytes packets errs drop fifo frame compressed multicast|bytes packets errs drop fifo colls carrier compressed\n";
    let counts = "0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0";
    assert_eq!(
        environment::network_interfaces(&format!("{header} lo: {counts}\n")).unwrap(),
        vec!["lo"]
    );
    assert_eq!(
        environment::network_interfaces(&format!("{header} lo: {counts}\n eth0: {counts}\n"))
            .unwrap(),
        vec!["eth0", "lo"]
    );
    assert!(environment::network_interfaces(header).is_err());
    assert!(environment::network_interfaces(&format!("{header} lo: 0 0\n")).is_err());
}

#[test]
fn hosting_reads_the_real_publish_body_file_and_reports_failed_checks_as_failed() {
    let repo = Repository::published();
    let mut hosting = repo.hosting();
    let path = repo.case.tmp.join("publish-body.md");
    fs::write(&path, "Closes #7\n\nVerified answer implementation.\n").unwrap();
    let response = hosting
        .invoke(
            &repo.checkout,
            &[
                "pr",
                "create",
                "--head",
                "feat/answer",
                "--base",
                "main",
                "--title",
                "Implement answer",
                "--body-file",
                path.to_str().unwrap(),
            ],
        )
        .unwrap();
    assert_eq!(response.exit_code, 0);
    assert_eq!(
        hosting.pr(1).unwrap().body,
        fs::read_to_string(path).unwrap()
    );
    hosting.set_checks(1, Checks::Failed).unwrap();
    let response = hosting
        .invoke(
            &repo.checkout,
            &["pr", "view", "1", "--json", "statusCheckRollup"],
        )
        .unwrap();
    let value: Value = serde_json::from_slice(&response.stdout).unwrap();
    assert_eq!(value["statusCheckRollup"][0]["conclusion"], "FAILURE");
}

#[test]
fn recovered_pending_finalization_can_pass_with_a_consistent_build_success() {
    for state in ["IMPLEMENTED_VERIFIED", "EXISTING_OPEN_PR"] {
        let observed=serde_json::from_value(json!({"context":"valid","context_read_error":null,"fault_before_consumption":false,"origin_head":BASE,"pr_head":BASE,"pr_state":"OPEN","checks":"pending","terminal_state":state,"terminal_success":true,"exit_code":0})).unwrap();
        assert!(evidence::validate_pending_finalization(&observed).is_ok());
    }
}

#[test]
fn agent_reads_rendered_fields_and_json_without_shell_context_aliases() {
    let prompt = "Prefer the provided state dir ; otherwise create one.\nRunner-captured invocation: {\"run_id\":\"real-run\",\"selected_pr\":1}\nMore instructions";
    assert_eq!(
        agent::task_field(prompt, "Prefer the provided state dir ", ";").unwrap(),
        ""
    );
    assert_eq!(
        agent::task_json(prompt, "Runner-captured invocation: ").unwrap()["run_id"],
        "real-run"
    );
    assert!(agent::task_json("Return invalid", "Return ").is_err());
    assert!(agent::task_field(prompt, "absent", ";").is_err());
    assert!(agent::task_field("path is unterminated", "path is ", ";").is_err());
}

#[test]
fn required_checks_api_is_head_bound_and_keeps_pending_until_explicit_transition() {
    let repo = Repository::published();
    let mut hosting = repo.hosting();
    let number = repo.open_pr(&mut hosting);
    let endpoint = format!(
        "repos/{{owner}}/{{repo}}/commits/{}/check-runs?filter=all",
        repo.head
    );
    let response = hosting
        .invoke(&repo.checkout, &["api", "--paginate", &endpoint])
        .unwrap();
    let pending: Value = serde_json::from_slice(&response.stdout).unwrap();
    assert_eq!(pending["check_runs"][0]["head_sha"], repo.head);
    assert_eq!(pending["check_runs"][0]["status"], "in_progress");
    assert!(pending["check_runs"][0]["conclusion"].is_null());
    let stale = format!(
        "repos/fixture/project/commits/{}/check-runs",
        "0".repeat(40)
    );
    let response = hosting.invoke(&repo.checkout, &["api", &stale]).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&response.stdout).unwrap()["total_count"],
        0
    );
    hosting.set_checks(number, Checks::Green).unwrap();
    let response = hosting
        .invoke(&repo.checkout, &["api", &endpoint, "--paginate"])
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&response.stdout).unwrap()["check_runs"][0]["conclusion"],
        "success"
    );
    let response = hosting
        .invoke(
            &repo.checkout,
            &["api", "repos/{owner}/{repo}/branches/main"],
        )
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&response.stdout).unwrap()["protection"]["required_status_checks"]
            ["contexts"],
        json!(["fixture-verification"])
    );
    let readback = hosting
        .invoke(&repo.checkout, &["pr", "view", "1", "--json", "createdAt"])
        .unwrap();
    let readback: Value = serde_json::from_slice(&readback.stdout).unwrap();
    assert!(chrono::DateTime::parse_from_rfc3339(readback["createdAt"].as_str().unwrap()).is_ok());
    assert!(readback["mergedAt"].is_null());
}
