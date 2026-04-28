//! Tests for `amplihack pr watch-and-merge`.
//!
//! Uses `MockGhRunner` to simulate `gh` CLI responses without requiring
//! authentication or network access. Tests cover the full contract:
//! argument validation, check parsing, polling loop, merge invocation,
//! retry logic, and error reporting.

use super::*;
use std::cell::RefCell;
use std::collections::VecDeque;

// ── Mock infrastructure ─────────────────────────────────────────────

/// Mock `gh` runner that returns pre-programmed responses and records
/// every invocation for assertion.
struct MockGhRunner {
    /// Queue of responses to return in order. Each `run_gh` call pops
    /// the front. If the queue is empty, panics with "unexpected call".
    responses: RefCell<VecDeque<Result<GhOutput, String>>>,
    /// Recorded argument lists from each `run_gh` call.
    captured_calls: RefCell<Vec<Vec<String>>>,
}

impl MockGhRunner {
    fn new(responses: Vec<Result<GhOutput, String>>) -> Self {
        Self {
            responses: RefCell::new(responses.into()),
            captured_calls: RefCell::new(Vec::new()),
        }
    }

    /// Helper: create a successful GhOutput.
    fn ok(stdout: &str) -> Result<GhOutput, String> {
        Ok(GhOutput {
            stdout: stdout.to_string(),
            stderr: String::new(),
            success: true,
        })
    }

    /// Helper: create a failed GhOutput (gh returned non-zero).
    fn fail(stderr: &str) -> Result<GhOutput, String> {
        Ok(GhOutput {
            stdout: String::new(),
            stderr: stderr.to_string(),
            success: false,
        })
    }

    /// Helper: create a transient error (gh command itself failed to run).
    fn transient_err(msg: &str) -> Result<GhOutput, String> {
        Err(msg.to_string())
    }

    /// Return all captured argument lists.
    fn calls(&self) -> Vec<Vec<String>> {
        self.captured_calls.borrow().clone()
    }
}

impl GhRunner for MockGhRunner {
    fn run_gh(&self, args: &[&str]) -> Result<GhOutput> {
        self.captured_calls
            .borrow_mut()
            .push(args.iter().map(|s| s.to_string()).collect());
        let response = self
            .responses
            .borrow_mut()
            .pop_front()
            .expect("MockGhRunner: unexpected call — no more queued responses");
        match response {
            Ok(output) => Ok(output),
            Err(msg) => Err(anyhow::anyhow!(msg)),
        }
    }
}

// ── Test helpers ────────────────────────────────────────────────────

/// Default args for testing: PR #42, squash strategy, 5s interval.
fn default_args() -> WatchAndMergeArgs {
    WatchAndMergeArgs {
        pr_number: 42,
        strategy: "squash".to_string(),
        admin: false,
        delete_branch: false,
        interval: 5,
    }
}

/// JSON representing all checks passing.
fn all_checks_pass_json() -> &'static str {
    r#"[
        {"name": "build", "state": "SUCCESS", "detailsUrl": "https://github.com/run/1"},
        {"name": "test", "state": "SUCCESS", "detailsUrl": "https://github.com/run/2"}
    ]"#
}

/// JSON representing a check failure.
fn check_failure_json() -> &'static str {
    r#"[
        {"name": "build", "state": "SUCCESS", "detailsUrl": "https://github.com/run/1"},
        {"name": "lint", "state": "FAILURE", "detailsUrl": "https://github.com/run/3"}
    ]"#
}

/// JSON with pending checks.
fn checks_pending_json() -> &'static str {
    r#"[
        {"name": "build", "state": "SUCCESS", "detailsUrl": "https://github.com/run/1"},
        {"name": "deploy", "state": "IN_PROGRESS", "detailsUrl": "https://github.com/run/4"}
    ]"#
}

/// Empty checks array.
fn empty_checks_json() -> &'static str {
    "[]"
}

// ═══════════════════════════════════════════════════════════════════
// 1. Argument validation
// ═══════════════════════════════════════════════════════════════════

#[test]
fn test_validate_args_minimum_interval_enforced() {
    let mut args = default_args();
    args.interval = 4; // below MIN_INTERVAL_SECS (5)
    let err = validate_args(&args).unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("at least 5 seconds"),
        "Expected interval validation error, got: {msg}"
    );
}

#[test]
fn test_validate_args_accepts_minimum_interval() {
    let mut args = default_args();
    args.interval = 5;
    assert!(validate_args(&args).is_ok());
}

#[test]
fn test_validate_args_accepts_large_interval() {
    let mut args = default_args();
    args.interval = 300;
    assert!(validate_args(&args).is_ok());
}

// ═══════════════════════════════════════════════════════════════════
// 2. JSON check parsing
// ═══════════════════════════════════════════════════════════════════

#[test]
fn test_parse_checks_json_all_pass() {
    let result = parse_checks_json(all_checks_pass_json()).unwrap();
    assert!(result.all_passed, "Expected all checks to pass");
    assert!(!result.has_failures);
    assert!(!result.has_pending);
    assert_eq!(result.entries.len(), 2);
}

#[test]
fn test_parse_checks_json_with_failure() {
    let result = parse_checks_json(check_failure_json()).unwrap();
    assert!(!result.all_passed);
    assert!(result.has_failures);
    assert!(!result.has_pending);

    let failures: Vec<_> = result
        .entries
        .iter()
        .filter(|e| e.state == CheckState::Fail)
        .collect();
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].name, "lint");
    assert_eq!(failures[0].detail_url, "https://github.com/run/3");
}

#[test]
fn test_parse_checks_json_with_pending() {
    let result = parse_checks_json(checks_pending_json()).unwrap();
    assert!(!result.all_passed);
    assert!(!result.has_failures);
    assert!(result.has_pending);
}

#[test]
fn test_parse_checks_json_empty_is_all_passed() {
    let result = parse_checks_json(empty_checks_json()).unwrap();
    assert!(
        result.all_passed,
        "Empty check suite should be treated as all-passed"
    );
    assert!(result.entries.is_empty());
}

#[test]
fn test_parse_checks_json_neutral_and_skipped_are_passing() {
    let json = r#"[
        {"name": "optional-lint", "state": "NEUTRAL", "detailsUrl": "https://example.com/1"},
        {"name": "skipped-test", "state": "SKIPPED", "detailsUrl": "https://example.com/2"}
    ]"#;
    let result = parse_checks_json(json).unwrap();
    assert!(
        result.all_passed,
        "NEUTRAL and SKIPPED should count as passing"
    );
    assert!(!result.has_failures);
    assert!(!result.has_pending);
}

#[test]
fn test_parse_checks_json_error_state_is_failure() {
    let json = r#"[
        {"name": "infra", "state": "ERROR", "detailsUrl": "https://example.com/err"}
    ]"#;
    let result = parse_checks_json(json).unwrap();
    assert!(result.has_failures, "ERROR state should count as failure");
    assert_eq!(result.entries[0].state, CheckState::Fail);
}

#[test]
fn test_parse_checks_json_queued_is_pending() {
    let json = r#"[
        {"name": "deploy", "state": "QUEUED", "detailsUrl": "https://example.com/q"}
    ]"#;
    let result = parse_checks_json(json).unwrap();
    assert!(result.has_pending, "QUEUED state should count as pending");
    assert!(!result.all_passed);
}

#[test]
fn test_parse_checks_json_invalid_json_returns_error() {
    let result = parse_checks_json("not valid json {{{");
    assert!(result.is_err(), "Invalid JSON should return an error");
}

// ═══════════════════════════════════════════════════════════════════
// 3. Merge argument building
// ═══════════════════════════════════════════════════════════════════

#[test]
fn test_build_merge_args_squash_default() {
    let args = default_args();
    let merge_args = build_merge_args(&args);
    assert!(merge_args.contains(&"pr".to_string()));
    assert!(merge_args.contains(&"merge".to_string()));
    assert!(merge_args.contains(&"42".to_string()));
    assert!(merge_args.contains(&"--squash".to_string()));
    assert!(
        !merge_args.contains(&"--admin".to_string()),
        "Should not include --admin by default"
    );
    assert!(
        !merge_args.contains(&"--delete-branch".to_string()),
        "Should not include --delete-branch by default"
    );
}

#[test]
fn test_build_merge_args_rebase_strategy() {
    let mut args = default_args();
    args.strategy = "rebase".to_string();
    let merge_args = build_merge_args(&args);
    assert!(merge_args.contains(&"--rebase".to_string()));
    assert!(!merge_args.contains(&"--squash".to_string()));
}

#[test]
fn test_build_merge_args_merge_strategy() {
    let mut args = default_args();
    args.strategy = "merge".to_string();
    let merge_args = build_merge_args(&args);
    assert!(merge_args.contains(&"--merge".to_string()));
    assert!(!merge_args.contains(&"--squash".to_string()));
}

#[test]
fn test_build_merge_args_admin_flag() {
    let mut args = default_args();
    args.admin = true;
    let merge_args = build_merge_args(&args);
    assert!(
        merge_args.contains(&"--admin".to_string()),
        "Should include --admin when set"
    );
}

#[test]
fn test_build_merge_args_delete_branch_flag() {
    let mut args = default_args();
    args.delete_branch = true;
    let merge_args = build_merge_args(&args);
    assert!(
        merge_args.contains(&"--delete-branch".to_string()),
        "Should include --delete-branch when set"
    );
}

#[test]
fn test_build_merge_args_all_flags_combined() {
    let mut args = default_args();
    args.strategy = "rebase".to_string();
    args.admin = true;
    args.delete_branch = true;
    args.pr_number = 99;
    let merge_args = build_merge_args(&args);
    assert!(merge_args.contains(&"99".to_string()));
    assert!(merge_args.contains(&"--rebase".to_string()));
    assert!(merge_args.contains(&"--admin".to_string()));
    assert!(merge_args.contains(&"--delete-branch".to_string()));
}

// ═══════════════════════════════════════════════════════════════════
// 4. Retry logic
// ═══════════════════════════════════════════════════════════════════

#[test]
fn test_retry_succeeds_on_first_try() {
    let runner = MockGhRunner::new(vec![MockGhRunner::ok("ok")]);
    let result = run_gh_with_retry(&runner, &["pr", "checks", "42"]);
    assert!(result.is_ok());
    assert_eq!(runner.calls().len(), 1);
}

#[test]
fn test_retry_succeeds_after_transient_failure() {
    let runner = MockGhRunner::new(vec![
        MockGhRunner::transient_err("network timeout"),
        MockGhRunner::ok("recovered"),
    ]);
    let result = run_gh_with_retry(&runner, &["pr", "checks", "42"]);
    assert!(result.is_ok());
    let output = result.unwrap();
    assert_eq!(output.stdout, "recovered");
    assert_eq!(
        runner.calls().len(),
        2,
        "Should have retried once after transient failure"
    );
}

#[test]
fn test_retry_exhausted_after_max_retries() {
    let runner = MockGhRunner::new(vec![
        MockGhRunner::transient_err("fail 1"),
        MockGhRunner::transient_err("fail 2"),
        MockGhRunner::transient_err("fail 3"),
    ]);
    let result = run_gh_with_retry(&runner, &["pr", "checks", "42"]);
    assert!(result.is_err(), "Should fail after exhausting all retries");
    assert_eq!(
        runner.calls().len(),
        3,
        "Should have tried exactly MAX_RETRIES times"
    );
}

#[test]
fn test_retry_does_not_retry_on_check_failure() {
    // A non-success exit from `gh` (e.g., check failure) is NOT transient —
    // it should be returned immediately, not retried.
    let runner = MockGhRunner::new(vec![MockGhRunner::fail("checks failed")]);
    let result = run_gh_with_retry(&runner, &["pr", "checks", "42"]);
    assert!(
        result.is_ok(),
        "Non-transient gh failure should return Ok(GhOutput) with success=false"
    );
    let output = result.unwrap();
    assert!(!output.success);
    assert_eq!(
        runner.calls().len(),
        1,
        "Should NOT retry when gh returns a non-success exit code"
    );
}

// ═══════════════════════════════════════════════════════════════════
// 5. Poll-and-merge integration (happy path)
// ═══════════════════════════════════════════════════════════════════

#[test]
fn test_poll_and_merge_happy_path_squash() {
    // Sequence: checks pass immediately -> merge succeeds
    let runner = MockGhRunner::new(vec![
        // First call: gh pr checks 42 --json name,state,detailsUrl
        MockGhRunner::ok(all_checks_pass_json()),
        // Second call: gh pr merge 42 --squash
        MockGhRunner::ok("merged"),
    ]);

    let args = default_args();
    let mut stderr = Vec::new();
    let result = poll_and_merge(&runner, &args, &mut stderr);

    assert!(result.is_ok(), "Happy path should succeed: {result:?}");

    let calls = runner.calls();
    assert!(
        calls.len() >= 2,
        "Expected at least 2 gh calls (checks + merge)"
    );

    // Verify the checks call
    let checks_call = &calls[0];
    assert!(checks_call.contains(&"pr".to_string()));
    assert!(checks_call.contains(&"checks".to_string()));
    assert!(checks_call.contains(&"42".to_string()));

    // Verify the merge call
    let merge_call = &calls[1];
    assert!(merge_call.contains(&"pr".to_string()));
    assert!(merge_call.contains(&"merge".to_string()));
    assert!(merge_call.contains(&"42".to_string()));
    assert!(merge_call.contains(&"--squash".to_string()));
}

#[test]
fn test_poll_and_merge_check_failure_exits_with_error() {
    let runner = MockGhRunner::new(vec![
        // gh pr checks returns failure
        MockGhRunner::ok(check_failure_json()),
    ]);

    let args = default_args();
    let mut stderr = Vec::new();
    let result = poll_and_merge(&runner, &args, &mut stderr);

    assert!(result.is_err(), "Check failure should return Err");
    let err_msg = format!("{}", result.unwrap_err());
    assert!(
        err_msg.contains("lint"),
        "Error should mention the failing check name, got: {err_msg}"
    );

    // Should NOT have attempted a merge
    let calls = runner.calls();
    let merge_calls: Vec<_> = calls
        .iter()
        .filter(|c| c.contains(&"merge".to_string()))
        .collect();
    assert!(
        merge_calls.is_empty(),
        "Should not attempt merge when checks fail"
    );
}

#[test]
fn test_poll_and_merge_pending_then_pass() {
    // First poll: pending. Second poll: all pass. Then merge.
    let runner = MockGhRunner::new(vec![
        MockGhRunner::ok(checks_pending_json()),
        MockGhRunner::ok(all_checks_pass_json()),
        MockGhRunner::ok("merged"),
    ]);

    let mut args = default_args();
    args.interval = 5; // minimum interval; test won't actually sleep
    let mut stderr = Vec::new();
    let result = poll_and_merge(&runner, &args, &mut stderr);

    assert!(
        result.is_ok(),
        "Should succeed after pending resolves to pass: {result:?}"
    );
    assert!(
        runner.calls().len() >= 3,
        "Expected: check (pending) + check (pass) + merge = 3 calls minimum"
    );
}

#[test]
fn test_poll_and_merge_empty_checks_warns_and_merges() {
    let runner = MockGhRunner::new(vec![
        MockGhRunner::ok(empty_checks_json()),
        MockGhRunner::ok("merged"),
    ]);

    let args = default_args();
    let mut stderr = Vec::new();
    let result = poll_and_merge(&runner, &args, &mut stderr);

    assert!(
        result.is_ok(),
        "Empty checks should proceed to merge: {result:?}"
    );

    let stderr_str = String::from_utf8(stderr).unwrap();
    assert!(
        stderr_str.contains("No checks found") || stderr_str.contains("no checks"),
        "Should warn about empty checks on stderr, got: {stderr_str}"
    );
}

#[test]
fn test_poll_and_merge_rebase_strategy() {
    let runner = MockGhRunner::new(vec![
        MockGhRunner::ok(all_checks_pass_json()),
        MockGhRunner::ok("merged"),
    ]);

    let mut args = default_args();
    args.strategy = "rebase".to_string();
    let mut stderr = Vec::new();
    let result = poll_and_merge(&runner, &args, &mut stderr);

    assert!(result.is_ok());
    let merge_call = &runner.calls()[1];
    assert!(
        merge_call.contains(&"--rebase".to_string()),
        "Should use --rebase strategy"
    );
}

#[test]
fn test_poll_and_merge_admin_forwarded() {
    let runner = MockGhRunner::new(vec![
        MockGhRunner::ok(all_checks_pass_json()),
        MockGhRunner::ok("merged"),
    ]);

    let mut args = default_args();
    args.admin = true;
    let mut stderr = Vec::new();
    let result = poll_and_merge(&runner, &args, &mut stderr);

    assert!(result.is_ok());
    let merge_call = &runner.calls()[1];
    assert!(
        merge_call.contains(&"--admin".to_string()),
        "Should forward --admin flag to gh pr merge"
    );
}

#[test]
fn test_poll_and_merge_delete_branch_forwarded() {
    let runner = MockGhRunner::new(vec![
        MockGhRunner::ok(all_checks_pass_json()),
        MockGhRunner::ok("merged"),
    ]);

    let mut args = default_args();
    args.delete_branch = true;
    let mut stderr = Vec::new();
    let result = poll_and_merge(&runner, &args, &mut stderr);

    assert!(result.is_ok());
    let merge_call = &runner.calls()[1];
    assert!(
        merge_call.contains(&"--delete-branch".to_string()),
        "Should forward --delete-branch flag to gh pr merge"
    );
}

#[test]
fn test_poll_and_merge_merge_failure_surfaces_error() {
    // Checks pass, but merge itself fails (e.g., merge conflict)
    let runner = MockGhRunner::new(vec![
        MockGhRunner::ok(all_checks_pass_json()),
        MockGhRunner::fail("merge conflict: cannot merge"),
    ]);

    let args = default_args();
    let mut stderr = Vec::new();
    let result = poll_and_merge(&runner, &args, &mut stderr);

    assert!(result.is_err(), "Failed merge should return Err");
    let err_msg = format!("{}", result.unwrap_err());
    assert!(
        err_msg.contains("merge") || err_msg.contains("conflict"),
        "Error should mention merge failure, got: {err_msg}"
    );
}

// ═══════════════════════════════════════════════════════════════════
// 6. Error message quality
// ═══════════════════════════════════════════════════════════════════

#[test]
fn test_check_failure_reports_job_name_and_url() {
    let runner = MockGhRunner::new(vec![MockGhRunner::ok(check_failure_json())]);

    let args = default_args();
    let mut stderr = Vec::new();
    let result = poll_and_merge(&runner, &args, &mut stderr);

    assert!(result.is_err());
    let err_msg = format!("{}", result.unwrap_err());

    // Must include the failing job name
    assert!(
        err_msg.contains("lint"),
        "Error should include failing job name 'lint', got: {err_msg}"
    );
    // Must include the details URL
    assert!(
        err_msg.contains("https://github.com/run/3"),
        "Error should include failing job URL, got: {err_msg}"
    );
}

#[test]
fn test_stderr_shows_progress_during_polling() {
    // pending -> pass -> merge
    let runner = MockGhRunner::new(vec![
        MockGhRunner::ok(checks_pending_json()),
        MockGhRunner::ok(all_checks_pass_json()),
        MockGhRunner::ok("merged"),
    ]);

    let mut args = default_args();
    args.interval = 5;
    let mut stderr = Vec::new();
    let _ = poll_and_merge(&runner, &args, &mut stderr);

    let stderr_str = String::from_utf8(stderr).unwrap();
    assert!(
        !stderr_str.is_empty(),
        "Should output progress information to stderr during polling"
    );
}
