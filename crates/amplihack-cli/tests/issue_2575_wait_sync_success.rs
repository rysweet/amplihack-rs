//! #2575, Crusty's review of PR #2637: B1, B2 and M1.
//!
//! Contract: docs/reference/autodrive-merge-ready-round.md.
//!
//! - B2: a required check still running is a wait, not a blocker. The outcome
//!   reader may answer `waiting`; `autodrive_round_word` maps it to `WAITING`
//!   with a `ci_signal` of `<state>@<head>`; a `WAITING` round ends the loop
//!   before the builder and the evaluator; finalisation reports
//!   `checks-pending` (exit 7, no hand-off, no draft) and the next wake merges
//!   once the check has passed. A pending signal at an unchanged head is
//!   `CONTINUE`, never `STUCK`.
//! - M1: success means merged. `autodrive_merge` reads `mergeStateStatus`;
//!   `DIRTY` before the merge or `BEHIND`/`DIRTY` after a refusal is
//!   `base-behind` (exit 8). The merge loop syncs the base by merging (never
//!   rebasing or forcing), runs one more round through
//!   `autodrive_merge_loop_run.sh 1` and finalises again. The report step
//!   reads JSON only and says `completed` only for a merge.
//! - B1: a missing Crusty record asks for a Crusty run (`no-record`), and the
//!   read-only `crusty-covers` range check replaces `crusty-evidence`.
//! - Minor 3: a PR with no checks at all is not refused for that reason
//!   (`no_checks_at_all_merges` in the parent module).
//!
//! A child of issue_2575_agentic_merge_ready.rs: it uses that module's fakes
//! (`FAKE_GH`, `FAKE_AMPLIHACK`, `FAKE_LOOP_DRIVER`, `DRIVER_AMPLIHACK`) and
//! fixtures (`Fx`, `Drive`). Every process is isolated the same way; nothing
//! reaches GitHub or an agent. tests/gadugi/* is not run here.
//!
//! The agent steps (the Crusty verdict comment writer, the step-01b reader,
//! the base-sync builder) are not run. Their tool-visible effects are played
//! by stand-ins, and no prompt wording is asserted.

use super::*;

// ===========================================================================
// autodrive_round_word / autodrive_round_file: `waiting` and `ci_signal`
// ===========================================================================

fn word_ci(fx: &Fx, answer: &str, ci: &str, head: &str) -> Value {
    let out = fx.round_tool(
        "autodrive_round_word \"$1\" \"$2\" \"$3\"",
        &[answer, ci, head],
        b"",
    );
    assert_eq!(out.status.code(), Some(0), "{answer:?}: {}", show(&out));
    last_json(&out, "merge_ready_verdict")
}

#[test]
fn waiting_word_maps_to_waiting_with_the_ci_signal() {
    let Some(fx) = Fx::new() else { return };
    for (answer, ci) in [
        ("waiting", "ci: pending"),
        ("Waiting.", "CI: Pending"),
        ("  WAITING\n", "pending"),
        ("`waiting`", "ci:pending"),
    ] {
        assert_eq!(
            word_ci(&fx, answer, ci, SHA_A),
            json!({
                "word": "waiting",
                "merge_ready_verdict": "WAITING",
                "ci_signal": format!("pending@{SHA_A}")
            }),
            "{answer:?} {ci:?}"
        );
    }
    let sha256 = "c".repeat(64);
    assert_eq!(
        word_ci(&fx, "ready", "ci: pass", &sha256),
        json!({"word": "ready", "merge_ready_verdict": "MERGE_READY", "ci_signal": format!("pass@{sha256}")})
    );
    for state in ["fail", "none"] {
        assert_eq!(
            word_ci(&fx, "not-ready", &format!("ci: {state}"), SHA_A)["ci_signal"],
            json!(format!("{state}@{SHA_A}"))
        );
    }
    // hand-off keeps its meaning with the extra arguments.
    assert_eq!(
        word_ci(&fx, "hand-off", "ci: fail", SHA_A)["merge_ready_verdict"],
        json!("HANDOFF")
    );
}

#[test]
fn ci_signal_is_left_out_unless_the_state_and_head_are_valid() {
    let Some(fx) = Fx::new() else { return };
    let upper = SHA_A.to_uppercase();
    for (ci, head) in [
        ("ci: passed", SHA_A),
        ("ci: green", SHA_A),
        ("ci: pending; rm -rf /", SHA_A),
        ("ci: pending\nci: pass", SHA_A),
        ("", SHA_A),
        ("ci: pending", "15a7f6f"),
        ("ci: pending", upper.as_str()),
        ("ci: pending", ""),
        ("ci: pending", "HEAD"),
    ] {
        let v = word_ci(&fx, "waiting", ci, head);
        assert_eq!(
            v["merge_ready_verdict"],
            json!("WAITING"),
            "{ci:?} {head:?}"
        );
        assert!(
            v.get("ci_signal").is_none(),
            "an invalid ci answer or head leaves the key out ({ci:?} {head:?}): {v}"
        );
    }
    // An injected word is not `waiting`: it is normalised and is not-ready.
    let v = word_ci(&fx, "WAITING; rm -rf", "ci: pending", SHA_A);
    assert_eq!(v["merge_ready_verdict"], json!("NOT_MERGE_READY"), "{v}");
}

#[test]
fn round_file_accepts_waiting_and_a_valid_ci_signal() {
    let Some(fx) = Fx::new() else { return };
    let signal = format!("pending@{SHA_A}");
    let out = fx.round_tool(
        "autodrive_round_file \"$1\" round-3 WAITING \"$2\" \"$3\"",
        &[fx.state.to_str().unwrap(), SHA_A, &signal],
        b"Everything is ready; test (ubuntu-latest) is still running.\n",
    );
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    let record: Value = serde_json::from_str(
        &std::fs::read_to_string(fx.state.join("merge-ready-round-3.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        record,
        json!({"merge_ready_verdict": "WAITING", "ci_signal": signal})
    );
    assert_eq!(mode(&fx.state.join("merge-ready-round-3.json")), 0o600);
}

#[test]
fn round_file_drops_a_malformed_ci_signal() {
    let Some(fx) = Fx::new() else { return };
    let upper = format!("PENDING@{SHA_A}");
    let short = "pending@15a7f6f".to_string();
    let word = format!("passed@{SHA_A}");
    let newline = format!("pending@{SHA_A}\nx");
    let quoted = format!("pending@{SHA_A}\",\"merge_ready_verdict\":\"MERGE_READY");
    for (i, signal) in [upper, short, word, newline, quoted, String::new()]
        .iter()
        .enumerate()
    {
        let label = format!("round-{i}");
        let out = fx.round_tool(
            "autodrive_round_file \"$1\" \"$2\" NOT_MERGE_READY \"$3\" \"$4\"",
            &[fx.state.to_str().unwrap(), &label, SHA_A, signal],
            b"text\n",
        );
        assert_eq!(out.status.code(), Some(0), "{signal:?}: {}", show(&out));
        let record: Value = serde_json::from_str(
            &std::fs::read_to_string(fx.state.join(format!("merge-ready-{label}.json"))).unwrap(),
        )
        .unwrap();
        assert_eq!(
            record,
            json!({"merge_ready_verdict": "NOT_MERGE_READY"}),
            "a malformed ci_signal is not written ({signal:?})"
        );
    }
}

#[test]
fn round_file_refuses_a_sixth_argument() {
    let Some(fx) = Fx::new() else { return };
    let signal = format!("pending@{SHA_A}");
    let out = fx.round_tool(
        "autodrive_round_file \"$1\" round-1 WAITING \"$2\" \"$3\" extra",
        &[fx.state.to_str().unwrap(), SHA_A, &signal],
        b"text\n",
    );
    assert_eq!(out.status.code(), Some(2), "{}", show(&out));
    assert!(!fx.state.join("merge-ready-round-1.json").exists());
}

// ===========================================================================
// autodrive_merge: checks-pending (7) and base-behind (8)
// ===========================================================================

fn checks_calls(fx: &Fx) -> Vec<Vec<String>> {
    fx.gh_calls()
        .into_iter()
        .filter(|c| c.get(1).map(String::as_str) == Some("checks"))
        .collect()
}

#[test]
fn pending_required_check_is_checks_pending() {
    let Some(fx) = Fx::new() else { return };
    fx.set("required", "pending");
    fx.set("check_name", "test (ubuntu-latest)");
    let out = fx.merge_tool(PR, SHA_A);
    assert_eq!(out.status.code(), Some(7), "{}", show(&out));
    let v = last_json(&out, "result");
    assert_eq!(v["result"], json!("checks-pending"), "{v}");
    assert_eq!(v["expected_head"], json!(SHA_A));
    let why = v["reason"].as_str().unwrap_or_default();
    assert!(
        why.contains("test (ubuntu-latest)"),
        "the reason names the pending check: {v}"
    );
    assert_no_merge_attempt(&fx);
    assert_eq!(checks_calls(&fx).len(), 1, "read once, no polling");
    for c in fx.gh_calls() {
        assert!(!c.contains(&"--watch".to_string()), "no gh --watch: {c:?}");
    }
}

#[test]
fn pending_check_after_the_fallback_is_checks_pending() {
    let Some(fx) = Fx::new() else { return };
    fx.set("required", "none");
    fx.set("all", "pending");
    let out = fx.merge_tool(PR, SHA_A);
    assert_eq!(out.status.code(), Some(7), "{}", show(&out));
    assert_eq!(last_json(&out, "result")["result"], json!("checks-pending"));
    assert_no_merge_attempt(&fx);
}

#[test]
fn pending_check_names_are_cleaned_and_capped() {
    let Some(fx) = Fx::new() else { return };
    fx.set("required", "pending");
    let name = format!("evil\u{1b}[31mname\u{7}\r{}", "x".repeat(2000));
    fx.set("check_name", &name);
    let out = fx.merge_tool(PR, SHA_A);
    assert_eq!(out.status.code(), Some(7), "{}", show(&out));
    let v = last_json(&out, "result");
    let why = v["reason"].as_str().expect("reason is text");
    assert!(
        !why.chars().any(|c| c.is_control()),
        "control characters are removed: {why:?}"
    );
    assert!(
        why.len() <= 300 + 200,
        "check names are cut at 300 bytes: {} bytes",
        why.len()
    );
}

#[test]
fn merge_state_is_read_in_the_one_pr_view_before_the_merge() {
    let Some(fx) = Fx::new() else { return };
    let out = fx.merge_tool(PR, SHA_A);
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    let calls = fx.gh_calls();
    let first_checks = calls
        .iter()
        .position(|c| c.get(1).map(String::as_str) == Some("checks"))
        .expect("checks are read");
    let views: Vec<_> = calls[..first_checks]
        .iter()
        .filter(|c| c.get(1).map(String::as_str) == Some("view"))
        .collect();
    assert_eq!(views.len(), 1, "one PR read before the checks: {calls:?}");
    let fields = views[0]
        .iter()
        .position(|a| a == "--json")
        .and_then(|i| views[0].get(i + 1))
        .cloned()
        .unwrap_or_default();
    assert!(
        fields.contains("mergeStateStatus") && fields.contains("state"),
        "mergeStateStatus comes in the same read as the state: {fields:?}"
    );
}

#[test]
fn dirty_before_the_merge_is_base_behind_without_a_merge_request() {
    let Some(fx) = Fx::new() else { return };
    fx.set("merge_state", "DIRTY");
    let out = fx.merge_tool(PR, SHA_A);
    assert_eq!(out.status.code(), Some(8), "{}", show(&out));
    assert_eq!(last_json(&out, "result")["result"], json!("base-behind"));
    assert_no_merge_attempt(&fx);
}

#[test]
fn behind_before_the_merge_still_asks_github() {
    // BEHIND alone does not stop the request: GitHub decides whether an
    // out-of-date branch may merge.
    let Some(fx) = Fx::new() else { return };
    fx.set("merge_state", "BEHIND");
    let out = fx.merge_tool(PR, SHA_A);
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    assert_eq!(last_json(&out, "result")["result"], json!("merged"));
    assert_eq!(fx.merge_calls().len(), 1);
}

#[test]
fn refusal_on_an_out_of_date_branch_is_base_behind() {
    let Some(fx) = Fx::new() else { return };
    fx.set("merge_rc", "1");
    for after in ["BEHIND", "DIRTY"] {
        let _ = std::fs::remove_file(fx.fake.join("gh.log"));
        let _ = std::fs::remove_file(fx.fake.join("merge-body"));
        fx.set("merge_state_after", after);
        let out = fx.merge_tool(PR, SHA_A);
        assert_eq!(out.status.code(), Some(8), "{after}: {}", show(&out));
        assert_eq!(
            last_json(&out, "result")["result"],
            json!("base-behind"),
            "{after}"
        );
        assert_eq!(fx.merge_calls().len(), 1, "one request, no retry ({after})");
        let last = fx.gh_calls().last().cloned().unwrap();
        assert_eq!(
            &last[..2],
            ["pr", "view"],
            "the state is read again: {last:?}"
        );
    }
}

#[test]
fn refusal_with_any_other_merge_state_is_refused() {
    let Some(fx) = Fx::new() else { return };
    fx.set("merge_rc", "1");
    for after in [
        "CLEAN",
        "BLOCKED",
        "UNKNOWN",
        "",
        "behind",
        "BEHIND; rm -rf /",
        "DIRTY\nBEHIND",
    ] {
        let _ = std::fs::remove_file(fx.fake.join("merge-body"));
        fx.set("merge_state_after", after);
        let out = fx.merge_tool(PR, SHA_A);
        assert_eq!(out.status.code(), Some(4), "{after:?}: {}", show(&out));
        assert_eq!(
            last_json(&out, "result")["result"],
            json!("refused"),
            "an unknown merge state is a refusal: {after:?}"
        );
    }
}

#[test]
fn the_merge_request_never_forces_or_bypasses() {
    let Some(fx) = Fx::new() else { return };
    fx.set("merge_state", "BEHIND");
    let out = fx.merge_tool(PR, SHA_A);
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    for c in fx.gh_calls() {
        for bad in ["--admin", "--force", "--rebase", "--auto"] {
            assert!(!c.contains(&bad.to_string()), "{bad} in {c:?}");
        }
    }
    let m = &fx.merge_calls()[0];
    assert!(m.contains(&"--squash".to_string()), "{m:?}");
    assert!(m.contains(&"--match-head-commit".to_string()), "{m:?}");
}

// ===========================================================================
// autodrive-finalize: checks-pending and base-behind are not hand-offs
// ===========================================================================

#[test]
fn finalize_pending_check_is_not_a_handoff() {
    let Some((fx, head)) = finalize_fixture() else {
        return;
    };
    fx.set("required", "pending");
    let out = finalize_merge(&fx, &AUTHORISED);
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    let v = finalize_result(&out);
    assert_eq!(v["result"], json!("checks-pending"), "{v}");
    assert_eq!(v["handoff"], json!("false"), "no draft, no hand-off: {v}");
    assert_eq!(v["expected_head"], json!(head));
    assert_no_merge_attempt(&fx);
}

#[test]
fn finalize_base_behind_is_not_a_handoff() {
    let Some((fx, _)) = finalize_fixture() else {
        return;
    };
    fx.set("merge_state", "DIRTY");
    let out = finalize_merge(&fx, &AUTHORISED);
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    let v = finalize_result(&out);
    assert_eq!(v["result"], json!("base-behind"), "{v}");
    assert_eq!(v["handoff"], json!("false"), "{v}");
    assert_no_merge_attempt(&fx);
}

/// #2575's behaviour test: a drive whose only open item is a pending required
/// check is not merged and not handed off; the next wake, once the check has
/// passed, merges. No sleep, no polling: each wake reads the checks once.
#[test]
fn pending_check_merges_on_the_next_wake_once_it_passes() {
    let Some((fx, head)) = finalize_fixture() else {
        return;
    };
    fx.set("required", "pending");
    let first = finalize_merge(&fx, &AUTHORISED);
    assert_eq!(first.status.code(), Some(0), "{}", show(&first));
    let v = finalize_result(&first);
    assert_eq!(v["result"], json!("checks-pending"), "{v}");
    assert_eq!(v["handoff"], json!("false"));
    assert_no_merge_attempt(&fx);
    for c in fx.gh_calls() {
        assert!(
            !(c.get(1).map(String::as_str) == Some("ready")),
            "the PR is not drafted: {c:?}"
        );
    }
    let report = report_step(&fx, &[("finalize_merge", &v.to_string())]);
    assert_eq!(report.status.code(), Some(0), "{}", show(&report));
    let r = last_json(&report, "stop_reason");
    assert_eq!(r["stop_reason"], json!("waiting_for_checks"), "{r}");
    assert!(
        stdout(&report).contains("not merged: waiting for"),
        "{}",
        show(&report)
    );

    // The check run completes; the next wake reads it and merges.
    fx.set("required", "pass");
    let second = finalize_merge(&fx, &AUTHORISED);
    assert_eq!(second.status.code(), Some(0), "{}", show(&second));
    let v = finalize_result(&second);
    assert_eq!(v["result"], json!("merged"), "{v}");
    assert_eq!(v["expected_head"], json!(head));
    assert_eq!(fx.merge_calls().len(), 1);
    let report = report_step(&fx, &[("finalize_merge", &v.to_string())]);
    assert_eq!(
        last_json(&report, "stop_reason")["stop_reason"],
        json!("completed")
    );
}

// ===========================================================================
// The loop: WAITING ends it; a pending signal is never STUCK
// ===========================================================================

#[test]
fn waiting_round_ends_the_loop_before_the_builder_and_evaluator() {
    let Some(d) = Drive::new() else { return };
    d.plan(&["waiting|1|ci: pending"]);
    d.writeup(
        1,
        "All criteria met except CI, still running. Outcome: waiting.\n",
    );
    let head = d.head();
    let out = d.run();
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    let r = d.result();
    assert_eq!(r["loop_result"], json!("WAITING"), "{r}");
    assert_eq!(r["round_verdict"], json!("WAITING"), "{r}");
    assert_eq!(r["ci_signal"], json!(format!("pending@{head}")), "{r}");
    assert!(
        d.lines("evaluator-calls").is_empty(),
        "no evaluator for WAITING"
    );
    assert!(
        d.lines("builder-calls").is_empty(),
        "no builder for WAITING"
    );
    assert_eq!(d.head(), head, "nothing was committed");
    assert_eq!(d.lines("round-calls"), ["round-1"]);
    assert_eq!(
        std::fs::read_to_string(d.state.join("merge-ready-latest.head"))
            .unwrap()
            .trim(),
        head,
        "finalisation merges only the assessed head"
    );
}

#[test]
fn pending_ci_at_an_unchanged_head_is_continue_not_stuck() {
    let Some(d) = Drive::new() else { return };
    d.plan(&[
        "not-ready|0|ci: pending",
        "not-ready|0|ci: pending",
        "ready|0|ci: pass",
    ]);
    for n in 1..=2 {
        d.writeup(n, "CI is still running. Outcome: not-ready.\n");
    }
    d.writeup(3, "CI is green. Outcome: ready.\n");
    let head = d.head();
    let out = d.run();
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    assert_eq!(d.result()["loop_result"], json!("DONE"));
    assert_eq!(d.lines("round-calls"), ["round-1", "round-2", "round-3"]);
    assert!(
        d.lines("evaluator-ci")
            .iter()
            .any(|s| *s == format!("pending@{head}")),
        "the evaluator gets loop_ci_signal: {:?}",
        d.lines("evaluator-ci")
    );
}

#[test]
fn failing_ci_at_an_unchanged_head_is_still_stuck() {
    let Some(d) = Drive::new() else { return };
    d.plan(&["not-ready|0|ci: fail"]);
    d.writeup(1, "test fails. Outcome: not-ready.\n");
    let out = d.run();
    assert_eq!(out.status.code(), Some(1), "{}", show(&out));
    assert_eq!(d.result()["loop_result"], json!("STUCK"), "{}", show(&out));
    assert_eq!(d.lines("round-calls"), ["round-1", "round-2"]);
}

// ===========================================================================
// autodrive-merge-loop: WAITING finalises; the run tool; the base sync
// ===========================================================================

fn loop_env(fx: &Fx, c: &mut Command) {
    let review = fx.root.join("review");
    std::fs::create_dir_all(&review).unwrap();
    let preflight = json!({
        "should_run": "true", "pr": PR, "pr_url": "", "pr_state": "OPEN",
        "branch": "feat/issue-2575", "head_sha": SHA_A, "base_branch": "main",
        "base_sha": SHA_B, "state_dir": fx.state, "repo": fx.repo,
        "review_dir": review, "reason": "fixture", "blockers": [], "attempts": []
    })
    .to_string();
    c.env("RECIPE_VAR_merge_loop_preflight", preflight)
        .env("RECIPE_VAR_task_description", "issue 2575 fixture task")
        .env("RECIPE_VAR_autodrive_state_dir", &fx.state)
        .env("RECIPE_VAR_pr_number", PR)
        .env("RECIPE_VAR_no_merge", "false")
        .env("RECIPE_VAR_autodrive_dry_run", "false")
        .env(
            "RECIPE_VAR_autodrive_invocation_file",
            fx.state.join("invocation.json"),
        )
        .env("REPO_PATH", &fx.repo)
        .current_dir(&fx.repo)
        .stdin(Stdio::null());
}

fn fake_loop(fx: &Fx, rc: &str, result: Option<&str>) {
    write_executable(
        &fx.tools().join("autodrive_loop.sh"),
        FAKE_LOOP_DRIVER,
        0o755,
    );
    fx.set("loop_rc", rc);
    let _ = std::fs::remove_file(fx.fake.join("loop_result"));
    let _ = std::fs::remove_file(fx.fake.join("loop-argv"));
    let _ = std::fs::remove_file(fx.state.join("merge-ready-result.json"));
    if let Some(r) = result {
        fx.set("loop_result", r);
    }
}

fn loop_cmd(fx: &Fx, step: &str, vars: &[(&str, &str)]) -> Output {
    let mut c = fx.bash();
    c.arg("-c").arg(command_of("autodrive-merge-loop", step));
    loop_env(fx, &mut c);
    for (k, v) in vars {
        c.env(format!("RECIPE_VAR_{k}"), v);
    }
    c.output().expect("run a merge loop step")
}

fn run_tool(fx: &Fx, args: &[&str], vars: &[(&str, &str)]) -> Output {
    let mut c = fx.bash();
    c.arg(fx.tools().join("autodrive_merge_loop_run.sh"))
        .args(args);
    loop_env(fx, &mut c);
    for (k, v) in vars {
        c.env(format!("RECIPE_VAR_{k}"), v);
    }
    c.output().expect("run autodrive_merge_loop_run.sh")
}

fn loop_argv(fx: &Fx) -> Vec<String> {
    std::fs::read_to_string(fx.fake.join("loop-argv"))
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

#[test]
fn step02_waiting_leads_to_finalisation() {
    let Some(fx) = loop_fixture() else { return };
    for (result, finalise) in [("WAITING", "true"), ("DONE", "true")] {
        fake_loop(&fx, "0", Some(result));
        let out = loop_cmd(&fx, "step-02", &[]);
        assert_eq!(out.status.code(), Some(0), "{result}: {}", show(&out));
        let classified = loop_cmd(&fx, "step-02a", &[("merge_loop_result", &stdout(&out))]);
        let v = last_json(&classified, "outcome");
        assert_eq!(v["outcome"], json!("done"), "{result}: {v}");
        assert_eq!(v["finalise"], json!(finalise), "{result}: {v}");
    }
    let finalize = step_by_prefix("autodrive-merge-loop", "step-03");
    assert_eq!(
        finalize["condition"].as_str(),
        Some("merge_loop_outcome.finalise == 'true'")
    );
}

#[test]
fn step02a_never_finalises_a_handoff() {
    let Some(fx) = loop_fixture() else { return };
    for result in ["STUCK", "BUDGET_EXHAUSTED", "HANDOFF"] {
        fake_loop(&fx, "1", Some(result));
        let out = loop_cmd(&fx, "step-02", &[]);
        let classified = loop_cmd(&fx, "step-02a", &[("merge_loop_result", &stdout(&out))]);
        let v = last_json(&classified, "outcome");
        assert_eq!(v["outcome"], json!("handoff"), "{result}: {v}");
        assert_eq!(v["finalise"], json!("false"), "{result}: {v}");
    }
}

#[test]
fn waiting_with_a_failed_driver_exit_is_handed_off() {
    // The driver writes WAITING only on its exit-0 path.
    let Some(fx) = loop_fixture() else { return };
    fake_loop(&fx, "1", Some("WAITING"));
    let out = loop_cmd(&fx, "step-02", &[]);
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    assert!(stdout(&out).contains("UNREADABLE"), "{}", show(&out));
    let classified = loop_cmd(&fx, "step-02a", &[("merge_loop_result", &stdout(&out))]);
    let v = last_json(&classified, "outcome");
    assert_eq!(v["outcome"], json!("handoff"), "{v}");
    assert_eq!(v["finalise"], json!("false"), "{v}");
}

#[test]
fn step02_without_the_run_tool_stops_with_tools_not_found() {
    let Some(fx) = loop_fixture() else { return };
    fake_loop(&fx, "0", Some("DONE"));
    std::fs::remove_file(fx.tools().join("autodrive_merge_loop_run.sh"))
        .expect("autodrive_merge_loop_run.sh is in the bundle");
    let out = loop_cmd(&fx, "step-02", &[]);
    assert_eq!(out.status.code(), Some(1), "{}", show(&out));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("autodrive-tools-not-found"),
        "{}",
        show(&out)
    );
    assert!(loop_argv(&fx).is_empty(), "the loop never ran");
}

#[test]
fn merge_loop_run_refuses_a_bad_round_count() {
    let Some(fx) = loop_fixture() else { return };
    fake_loop(&fx, "0", Some("DONE"));
    for bad in ["0", "-1", "abc", "1;id", "", " 1", "1 2", "+1"] {
        let out = run_tool(&fx, &[bad], &[]);
        assert_eq!(out.status.code(), Some(2), "{bad:?}: {}", show(&out));
        assert!(loop_argv(&fx).is_empty(), "{bad:?}: the loop never ran");
    }
}

#[test]
fn merge_loop_run_asks_for_n_more_rounds() {
    let Some(fx) = loop_fixture() else { return };
    fake_loop(&fx, "0", Some("DONE"));
    let out = run_tool(&fx, &["1"], &[("autodrive_budget_loop_rounds", "7")]);
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    assert_eq!(last_json(&out, "loop_result")["loop_result"], json!("DONE"));
    let argv = loop_argv(&fx);
    assert!(
        argv.iter().any(|a| a == "autodrive_budget_loop_rounds=1"),
        "no rounds used yet, so one more round is a budget of 1: {argv:?}"
    );
    // Without the argument the operator's budget passes through unchanged.
    fake_loop(&fx, "0", Some("DONE"));
    let out = run_tool(&fx, &[], &[("autodrive_budget_loop_rounds", "7")]);
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    assert!(
        loop_argv(&fx)
            .iter()
            .any(|a| a == "autodrive_budget_loop_rounds=7"),
        "{:?}",
        loop_argv(&fx)
    );
}

#[test]
fn merge_loop_run_keeps_exit_79() {
    let Some(fx) = loop_fixture() else { return };
    fake_loop(&fx, "79", None);
    let out = run_tool(&fx, &["1"], &[]);
    assert_eq!(out.status.code(), Some(79), "{}", show(&out));
    assert!(stdout(&out).contains("TERMINAL_REFUSAL"), "{}", show(&out));
}

/// git with a fixed identity and nothing else changed (hooks stay on).
fn git_in(dir: &Path, args: &[&str]) -> String {
    let out = amplihack_git::command_in(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", dir)
        .output()
        .expect("run git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

/// A PR worktree whose branch is behind a `main` that moved on `origin`.
fn behind_base_fixture() -> Option<(Fx, String)> {
    let (fx, _) = finalize_fixture()?;
    let origin = fx.root.join("origin.git");
    git_in(
        &fx.root,
        &["init", "-q", "--bare", origin.to_str().unwrap()],
    );
    let r = &fx.repo;
    git_in(r, &["branch", "-M", "feat/issue-2575"]);
    git_in(r, &["remote", "add", "origin", origin.to_str().unwrap()]);
    git_in(r, &["push", "-q", "origin", "HEAD:refs/heads/main"]);
    std::fs::write(r.join("feature.txt"), "feature\n").unwrap();
    git_in(r, &["add", "feature.txt"]);
    git_in(r, &["commit", "-q", "-m", "feature"]);
    git_in(
        r,
        &["push", "-q", "origin", "HEAD:refs/heads/feat/issue-2575"],
    );
    let other = fx.root.join("other");
    git_in(
        &fx.root,
        &[
            "clone",
            "-q",
            origin.to_str().unwrap(),
            other.to_str().unwrap(),
        ],
    );
    git_in(&other, &["checkout", "-q", "-B", "main", "origin/main"]);
    std::fs::write(other.join("base.txt"), "base moved\n").unwrap();
    git_in(&other, &["add", "base.txt"]);
    git_in(&other, &["commit", "-q", "-m", "base moved"]);
    git_in(&other, &["push", "-q", "origin", "HEAD:refs/heads/main"]);
    let head = git_in(r, &["rev-parse", "HEAD"]);
    fx.set("server_head", &head);
    private_file(
        &fx.state.join("merge-ready-latest.head"),
        &format!("{head}\n"),
    );
    Some((fx, head))
}

/// M1's behaviour test: GitHub refuses the merge because the branch is out of
/// date; the drive merges the base into the branch (a merge commit, pushed as
/// a fast-forward), runs one more round and finalises again, and merges.
#[test]
fn out_of_date_branch_is_synced_by_a_merge_then_merged() {
    let Some((fx, head1)) = behind_base_fixture() else {
        return;
    };
    let origin = fx.root.join("origin.git");
    // 1. Finalisation: GitHub refuses; the read-back says BEHIND.
    fx.set("merge_rc", "1");
    fx.set("merge_state_after", "BEHIND");
    let out = finalize_merge(&fx, &AUTHORISED);
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    let first = finalize_result(&out);
    assert_eq!(first["result"], json!("base-behind"), "{first}");
    assert_eq!(first["handoff"], json!("false"), "{first}");
    let sync = step_by_prefix("autodrive-merge-loop", "step-03b");
    assert!(
        sync["condition"]
            .as_str()
            .is_some_and(|c| c.contains("base-behind")),
        "the base sync runs only on base-behind: {sync:?}"
    );

    // 2. Stand-in for the step-03b builder: the documented commands only.
    git_in(&fx.repo, &["fetch", "-q", "origin", "--", "main"]);
    git_in(&fx.repo, &["merge", "-q", "--no-edit", "origin/main"]);
    git_in(&fx.repo, &["push", "-q", "origin", "HEAD:feat/issue-2575"]);
    let head2 = git_in(&fx.repo, &["rev-parse", "HEAD"]);
    assert_ne!(head2, head1);
    let parents = git_in(&fx.repo, &["show", "-s", "--format=%P", &head2]);
    assert_eq!(parents.split_whitespace().count(), 2, "a merge commit");
    let remote = git_in(&origin, &["rev-parse", "refs/heads/feat/issue-2575"]);
    assert_eq!(remote, head2);
    // A fast-forward of the remote branch: nothing was rewritten.
    git_in(&origin, &["merge-base", "--is-ancestor", &head1, &head2]);
    let reflog = git_in(&fx.repo, &["reflog", "--format=%gs"]);
    assert!(!reflog.contains("rebase"), "{reflog}");

    // 3. step-03c: one more round through the run tool.
    fx.set("server_head", &head2);
    for f in ["merge_rc", "merge_state_after", "merge-body"] {
        let _ = std::fs::remove_file(fx.fake.join(f));
    }
    fake_loop(&fx, "0", Some("DONE"));
    let step03c = step_by_prefix("autodrive-merge-loop", "step-03c");
    assert_eq!(step03c["output"].as_str(), Some("sync_loop_result"));
    let round = loop_cmd(&fx, "step-03c", &[]);
    assert_eq!(round.status.code(), Some(0), "{}", show(&round));
    assert!(
        loop_argv(&fx)
            .iter()
            .any(|a| a == "autodrive_budget_loop_rounds=1"),
        "exactly one more round: {:?}",
        loop_argv(&fx)
    );
    assert_eq!(
        std::fs::read_to_string(fx.state.join("merge-ready-latest.head"))
            .unwrap()
            .trim(),
        head2,
        "the round assessed the synced head"
    );

    // 4. step-03d reads the sync round's result, not the first loop's.
    let first_loop =
        json!({"loop":"merge-ready","loop_result":"STUCK","round_label":"round-1"}).to_string();
    let mapped = loop_cmd(
        &fx,
        "step-03d",
        &[
            ("sync_loop_result", &stdout(&round)),
            ("merge_loop_result", &first_loop),
        ],
    );
    assert_eq!(mapped.status.code(), Some(0), "{}", show(&mapped));
    let outcome = last_json(&mapped, "outcome");
    assert_eq!(outcome["outcome"], json!("done"), "{outcome}");
    assert_eq!(outcome["finalise"], json!("true"), "{outcome}");

    // 5. The second finalisation merges at the synced head.
    let out = finalize_merge(&fx, &AUTHORISED);
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    let second = finalize_result(&out);
    assert_eq!(second["result"], json!("merged"), "{second}");
    assert_eq!(second["expected_head"], json!(head2));
    let merges = fx.merge_calls();
    let m = merges.last().unwrap();
    let i = m.iter().position(|a| a == "--match-head-commit").unwrap();
    assert_eq!(m[i + 1], head2);
    for c in fx.gh_calls() {
        let line = c.join(" ");
        assert!(
            !line.contains("--force") && !line.contains("+refs") && !line.contains("--admin"),
            "{line}"
        );
    }
    let report = report_step(&fx, &[("finalize_merge", &second.to_string())]);
    assert_eq!(
        last_json(&report, "stop_reason")["stop_reason"],
        json!("completed")
    );
}

#[test]
fn a_sync_round_that_stops_is_handed_off_not_finalised() {
    let Some(fx) = loop_fixture() else { return };
    fake_loop(&fx, "1", Some("STUCK"));
    let round = loop_cmd(&fx, "step-03c", &[]);
    assert_eq!(round.status.code(), Some(0), "{}", show(&round));
    let done =
        json!({"loop":"merge-ready","loop_result":"DONE","round_label":"round-1"}).to_string();
    let mapped = loop_cmd(
        &fx,
        "step-03d",
        &[
            ("sync_loop_result", &stdout(&round)),
            ("merge_loop_result", &done),
        ],
    );
    let outcome = last_json(&mapped, "outcome");
    assert_eq!(outcome["outcome"], json!("handoff"), "{outcome}");
    assert_eq!(outcome["finalise"], json!("false"), "{outcome}");
    let again = step_by_prefix("autodrive-merge-loop", "step-03e");
    assert_eq!(
        again["condition"].as_str(),
        Some("sync_loop_outcome.finalise == 'true'")
    );
}

// ===========================================================================
// auto-drive-to-merge report step: JSON only; Success means merged
// ===========================================================================

fn report_step(fx: &Fx, vars: &[(&str, &str)]) -> Output {
    let mut c = fx.bash();
    c.arg("-c")
        .arg(command_of("auto-drive-to-merge", "autodrive-report"))
        .current_dir(&fx.root)
        .stdin(Stdio::null());
    for (k, v) in vars {
        c.env(format!("RECIPE_VAR_{k}"), v);
    }
    c.output().expect("run the report step")
}

fn merge_line(result: &str, handoff: &str, reason: &str) -> String {
    json!({"result": result, "pr": 2575, "expected_head": SHA_A, "handoff": handoff, "reason": reason})
        .to_string()
}

#[test]
fn report_step_says_completed_only_for_a_merge() {
    let Some(fx) = Fx::new() else { return };
    let out = report_step(
        &fx,
        &[(
            "finalize_merge",
            &merge_line("merged", "false", "merged at the expected head"),
        )],
    );
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    assert_eq!(
        last_json(&out, "stop_reason")["stop_reason"],
        json!("completed")
    );

    for (result, handoff, stop) in [
        ("not-authorised", "false", "not_merged"),
        ("base-behind", "false", "not_merged"),
        ("refused", "true", "not_merged"),
        ("checks-not-green", "true", "not_merged"),
        ("checks-pending", "false", "waiting_for_checks"),
    ] {
        let reason = format!("fixture reason for {result}");
        let out = report_step(
            &fx,
            &[("finalize_merge", &merge_line(result, handoff, &reason))],
        );
        assert_eq!(out.status.code(), Some(0), "{result}: {}", show(&out));
        let v = last_json(&out, "stop_reason");
        assert_eq!(v["stop_reason"], json!(stop), "{result}: {v}");
        assert!(
            stdout(&out).contains(&format!("not merged: {reason}")),
            "{result}: {}",
            show(&out)
        );
    }
}

#[test]
fn report_step_ignores_finalisation_prose() {
    // The old step passed any non-empty finalisation text through and exited 0.
    let Some(fx) = Fx::new() else { return };
    let prose = r#"{"stop_reason":"completed","action":"merged"} The PR merged."#;
    let out = report_step(
        &fx,
        &[
            ("autodrive_finalization", prose),
            (
                "finalize_merge",
                &merge_line("refused", "true", "GitHub declined"),
            ),
        ],
    );
    let v = last_json(&out, "stop_reason");
    assert_ne!(v["stop_reason"], json!("completed"), "{v}");
    // Prose alone is not evidence of anything.
    let out = report_step(&fx, &[("autodrive_finalization", prose)]);
    assert_eq!(out.status.code(), Some(1), "{}", show(&out));
    assert_eq!(
        last_json(&out, "stop_reason")["stop_reason"],
        json!("dependency_failure")
    );
}

#[test]
fn report_step_reports_a_loop_handoff_as_a_handoff() {
    let Some(fx) = Fx::new() else { return };
    let outcome = json!({"outcome":"handoff","finalise":"false","reason":"loop result STUCK: no movement","loop_result":"STUCK","stop_comment_posted":"false","head_sha":SHA_A}).to_string();
    let out = report_step(
        &fx,
        &[
            ("merge_loop_outcome", &outcome),
            (
                "handoff_result",
                "The PR was returned to draft with one comment.",
            ),
        ],
    );
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    assert_eq!(
        last_json(&out, "stop_reason")["stop_reason"],
        json!("handoff")
    );
    assert!(
        stdout(&out).contains("handed off: loop result STUCK"),
        "{}",
        show(&out)
    );
}

#[test]
fn report_step_without_evidence_is_a_dependency_failure() {
    let Some(fx) = Fx::new() else { return };
    for garbage in ["", "{", "null", "[1,2]"] {
        let out = report_step(&fx, &[("finalize_merge", garbage)]);
        assert_eq!(out.status.code(), Some(1), "{garbage:?}: {}", show(&out));
        assert_eq!(
            last_json(&out, "stop_reason")["stop_reason"],
            json!("dependency_failure"),
            "{garbage:?}"
        );
    }
}

#[test]
fn report_step_keeps_already_merged() {
    let Some(fx) = Fx::new() else { return };
    let out = report_step(&fx, &[("build_result", r#"{"merged_before_run":true}"#)]);
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    assert_eq!(
        last_json(&out, "stop_reason")["stop_reason"],
        json!("completed")
    );
}

#[test]
fn report_step_passes_the_reason_as_data() {
    let Some(fx) = Fx::new() else { return };
    let marker = fx.root.join("reason-ran");
    let reason = format!(
        "\"}}; touch {} ; $(touch {}) `touch {}`",
        marker.display(),
        marker.display(),
        marker.display()
    );
    let out = report_step(
        &fx,
        &[("finalize_merge", &merge_line("refused", "true", &reason))],
    );
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    assert!(!marker.exists(), "the reason was executed");
    let v = last_json(&out, "stop_reason");
    assert_eq!(v["stop_reason"], json!("not_merged"), "{v}");
    assert!(
        v.to_string().contains("touch"),
        "the reason is kept as text inside valid JSON: {v}"
    );
}

// ===========================================================================
// B1: no Crusty record runs Crusty; the read-only crusty-covers check
// ===========================================================================

fn rereview(fx: &Fx) -> Value {
    let mut c = fx.bash();
    c.arg("-c")
        .arg(
            "set -u\nT=\"$AMPLIHACK_HOME/amplifier-bundle/tools\"\n\
             . \"$T/autodrive_state.sh\" || exit 90\n. \"$T/autodrive_trust.sh\" || exit 91\n\
             autodrive_rereview_decision \"$1\" \"$2\" \"\"",
        )
        .arg("bash")
        .arg(&fx.state)
        .arg(&fx.repo)
        .current_dir(&fx.repo);
    let out = feed(c, b"");
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    last_json(&out, "rereview")
}

#[test]
fn rereview_with_no_crusty_record_runs_crusty() {
    let Some(fx) = Fx::new() else { return };
    fx.git_repo();
    // No record at all, then an empty record list; a stale crusty-loop marker
    // is cleared so the before-round step runs the Crusty round.
    for records in [None, Some("")] {
        private_file(&fx.state.join("phases.tsv"), "crusty-loop\tdone\n");
        if let Some(body) = records {
            private_file(&fx.state.join("crusty-records.tsv"), body);
        }
        let v = rereview(&fx);
        assert_eq!(v["rereview"], json!("true"), "{records:?}: {v}");
        assert_eq!(v["range"], json!("no-record"), "{records:?}: {v}");
        let phases = std::fs::read_to_string(fx.state.join("phases.tsv")).unwrap_or_default();
        assert!(!phases.contains("crusty-loop\t"), "{records:?}: {phases:?}");
    }
    // An empty state directory with no phases file at all.
    let _ = std::fs::remove_file(fx.state.join("phases.tsv"));
    let _ = std::fs::remove_file(fx.state.join("crusty-records.tsv"));
    let v = rereview(&fx);
    assert_eq!(v["rereview"], json!("true"), "{v}");
    assert_eq!(v["range"], json!("no-record"), "{v}");
}

#[test]
fn rereview_with_records_but_none_final_is_not_checked() {
    let Some(fx) = Fx::new() else { return };
    fx.git_repo();
    private_file(
        &fx.state.join("crusty-records.tsv"),
        &format!("round-1\tcrusty-round-1.json\t{SHA_A}\n"),
    );
    let v = rereview(&fx);
    assert_eq!(v["rereview"], json!("false"), "{v}");
    assert_eq!(v["range"], json!("not-checked"), "{v}");
}

#[test]
fn crusty_range_from_an_empty_state_dir_asks_for_crusty() {
    let Some(fx) = Fx::new() else { return };
    fx.git_repo();
    let mut c = fx.bash();
    c.arg(fx.tools().join("autodrive_round_evidence.sh"))
        .arg("crusty-range")
        .env("AUTODRIVE_STATE_DIR", &fx.state)
        .env("PR_NUMBER", PR)
        .current_dir(&fx.repo);
    let out = feed(c, b"");
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    let v = last_json(&out, "rereview");
    assert_eq!(v["rereview"], json!("true"), "{v}");
    assert_eq!(v["range"], json!("no-record"), "{v}");
}

fn covers(fx: &Fx, sha: &str) -> Output {
    let mut c = fx.bash();
    c.arg(fx.tools().join("autodrive_round_evidence.sh"))
        .args(["crusty-covers", sha])
        .env("PR_NUMBER", PR)
        .env_remove("AUTODRIVE_BASE_BRANCH")
        .current_dir(&fx.repo);
    feed(c, b"")
}

fn commit_file(fx: &Fx, path: &str) -> String {
    let file = fx.repo.join(path);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, format!("{path}\n")).unwrap();
    let git = |args: &[&str]| crate::support::quick_change_repo::git(&fx.repo, args);
    git(&["add", path]);
    git(&["commit", "-q", "-m", path]);
    git(&["rev-parse", "HEAD"]).trim().to_string()
}

#[test]
fn crusty_covers_refuses_a_malformed_sha() {
    let Some(fx) = Fx::new() else { return };
    fx.git_repo();
    let (upper, long) = (SHA_A.to_uppercase(), "a".repeat(41));
    for sha in [
        "",
        "15a7f6f",
        upper.as_str(),
        "HEAD",
        "--all",
        long.as_str(),
    ] {
        let out = covers(&fx, sha);
        assert_eq!(out.status.code(), Some(2), "{sha:?}: {}", show(&out));
        assert_eq!(stdout(&out).trim(), "crusty-range-unreadable", "{sha:?}");
    }
}

#[test]
fn crusty_covers_the_head_it_names() {
    let Some(fx) = Fx::new() else { return };
    let head = fx.git_repo();
    let out = covers(&fx, &head);
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    assert_eq!(stdout(&out).trim(), "ok");
}

#[test]
fn crusty_covers_an_evidence_only_change_but_not_code() {
    let Some(fx) = Fx::new() else { return };
    let reviewed = fx.git_repo();
    commit_file(&fx, ".autodrive/evidence/qa.json");
    let out = covers(&fx, &reviewed);
    assert_eq!(stdout(&out).trim(), "ok", "{}", show(&out));
    let code = commit_file(&fx, "src/lib.rs");
    let out = covers(&fx, &reviewed);
    assert_eq!(
        stdout(&out).trim(),
        format!("crusty-unreviewed-commits:{code}"),
        "{}",
        show(&out)
    );
    // It reads git only: a record in a state directory changes nothing.
    assert!(
        !fx.state.join("phases.tsv").exists(),
        "crusty-covers never writes state"
    );
}

#[test]
fn crusty_evidence_subcommand_is_retired() {
    let Some(fx) = Fx::new() else { return };
    fx.git_repo();
    let mut c = fx.bash();
    c.arg(fx.tools().join("autodrive_round_evidence.sh"))
        .arg("crusty-evidence")
        .env("AUTODRIVE_STATE_DIR", &fx.state)
        .current_dir(&fx.repo);
    let out = feed(c, b"");
    assert_eq!(out.status.code(), Some(2), "{}", show(&out));
    assert!(
        !stdout(&out).contains("crusty_status"),
        "criterion 3 is no longer read from crusty-records.tsv: {}",
        show(&out)
    );
}

// ===========================================================================
// #2663: GitHub's merge state decides the outcome, in both directions
// ===========================================================================

/// #2663's behaviour test: GitHub merged the PR, which moved the base, and gh
/// still reported an error. The read-back says MERGED at the expected head
/// and the base now reads BEHIND: the run is merged, and the report says
/// `completed`. No later base or readiness comparison turns it into a failure.
#[test]
fn merged_then_base_moved_is_merged_and_completed() {
    let Some((fx, head)) = finalize_fixture() else {
        return;
    };
    fx.set("merge_rc_after", "1");
    fx.set("merge_state_after", "BEHIND");
    let out = finalize_merge(&fx, &AUTHORISED);
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    let v = finalize_result(&out);
    assert_eq!(v["result"], json!("merged"), "{v}");
    assert_eq!(v["expected_head"], json!(head));
    assert_eq!(v["handoff"], json!("false"), "{v}");
    assert_eq!(fx.merge_calls().len(), 1, "one request, no retry");

    let report = report_step(&fx, &[("finalize_merge", &v.to_string())]);
    assert_eq!(report.status.code(), Some(0), "{}", show(&report));
    assert_eq!(
        last_json(&report, "stop_reason")["stop_reason"],
        json!("completed"),
        "{}",
        show(&report)
    );
}

#[test]
fn a_pr_already_merged_at_the_expected_head_is_merged_without_a_request() {
    let Some(fx) = Fx::new() else { return };
    fx.set("state", "MERGED");
    let out = fx.merge_tool(PR, SHA_A);
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    assert_eq!(last_json(&out, "result")["result"], json!("merged"));
    assert_no_merge_attempt(&fx);
}

/// The other direction: gh reporting success while GitHub does not read the
/// PR merged is never a merge, so the run never reports `completed`.
#[test]
fn a_merge_github_does_not_read_back_is_refused() {
    let Some((fx, _)) = finalize_fixture() else {
        return;
    };
    // The merge request "succeeds" but the state file is reset afterwards by
    // a read-back that still says OPEN.
    let gh = fx.bin.join("gh");
    let real = fx.bin.join("gh-real");
    std::fs::rename(&gh, &real).unwrap();
    write_executable(
        &gh,
        format!(
            "#!/usr/bin/env bash\n\"{}\" \"$@\"; rc=$?\n\
             [ \"${{1:-}} ${{2:-}}\" = \"pr merge\" ] && printf OPEN > \"$FAKE_STATE/state\"\n\
             exit $rc\n",
            real.display()
        ),
        0o755,
    );
    let out = finalize_merge(&fx, &AUTHORISED);
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    let v = finalize_result(&out);
    assert_eq!(v["result"], json!("refused"), "{v}");
    let report = report_step(&fx, &[("finalize_merge", &v.to_string())]);
    assert_ne!(
        last_json(&report, "stop_reason")["stop_reason"],
        json!("completed"),
        "{}",
        show(&report)
    );
}
