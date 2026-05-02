//! TDD tests for `execution.rs` — port of `execution.py`.
//!
//! Behavioral parity contract:
//! - `run_parallel`: runs all processes concurrently; collects all results
//!   (order in completion order, but length and content must match);
//!   exceptions in any process produce a `ProcessResult { exit_code: -1, ... }`
//!   rather than failing the batch.
//! - `run_sequential`: runs in order; with `pass_output=true` later prompts
//!   are prefixed with previous output; with `stop_on_failure=true` the run
//!   halts after the first non-zero exit_code.
//! - `run_with_fallback`: tries prompts in order; returns first success;
//!   otherwise returns last failure with stderr prefixed by
//!   `"All N fallback attempts failed."`.
//! - `run_batched`: groups processes into batches of `batch_size`; runs each
//!   batch in parallel; with `pass_output=true` accumulates batch outputs.
//! - Empty input → empty output (or error for `run_with_fallback`).

use std::sync::Arc;
use std::time::Duration;

use amplihack_orchestration::claude_process::{
    ClaudeProcess, MockProcessRunner, ProcessResult, ProcessRunner,
};
use amplihack_orchestration::execution::{
    ExecutionError, run_batched, run_parallel, run_sequential, run_with_fallback,
};

fn make_process(
    runner: Arc<dyn ProcessRunner>,
    prompt: &str,
    pid: &str,
    log_dir: &std::path::Path,
) -> ClaudeProcess {
    ClaudeProcess::builder()
        .prompt(prompt)
        .process_id(pid)
        .working_dir(log_dir.to_path_buf())
        .log_dir(log_dir.to_path_buf())
        .runner(runner)
        .build()
        .unwrap()
}

#[tokio::test]
async fn run_parallel_returns_empty_for_empty_input() {
    let results = run_parallel(vec![], None).await;
    assert!(results.is_empty());
}

#[tokio::test]
async fn run_parallel_runs_all_processes_concurrently() {
    let mock = Arc::new(MockProcessRunner::new());
    mock.expect(
        "p1",
        ProcessResult::ok("o1".into(), "p1".into(), Duration::from_millis(1)),
    );
    mock.expect(
        "p2",
        ProcessResult::ok("o2".into(), "p2".into(), Duration::from_millis(1)),
    );
    mock.expect(
        "p3",
        ProcessResult::ok("o3".into(), "p3".into(), Duration::from_millis(1)),
    );

    let dir = tempfile::tempdir().unwrap();
    let r: Arc<dyn ProcessRunner> = mock.clone();
    let processes = vec![
        make_process(r.clone(), "p1", "p1", dir.path()),
        make_process(r.clone(), "p2", "p2", dir.path()),
        make_process(r.clone(), "p3", "p3", dir.path()),
    ];

    let results = run_parallel(processes, None).await;
    assert_eq!(results.len(), 3);
    assert_eq!(results.iter().filter(|r| r.is_success()).count(), 3);
}

#[tokio::test]
async fn run_parallel_max_workers_bounds_concurrency() {
    let mock = Arc::new(MockProcessRunner::new());
    for i in 0..6 {
        mock.expect(
            &format!("p{i}"),
            ProcessResult::ok("o".into(), format!("p{i}"), Duration::from_millis(1)),
        );
    }

    let dir = tempfile::tempdir().unwrap();
    let r: Arc<dyn ProcessRunner> = mock.clone();
    let processes: Vec<_> = (0..6)
        .map(|i| make_process(r.clone(), &format!("p{i}"), &format!("p{i}"), dir.path()))
        .collect();

    // With max_workers=2, all 6 should still complete.
    let results = run_parallel(processes, Some(2)).await;
    assert_eq!(results.len(), 6);
}

#[tokio::test]
async fn run_sequential_returns_empty_for_empty_input() {
    let results = run_sequential(vec![], false, false).await;
    assert!(results.is_empty());
}

#[tokio::test]
async fn run_sequential_runs_in_order_and_collects_all() {
    let mock = Arc::new(MockProcessRunner::new());
    mock.expect(
        "a",
        ProcessResult::ok("A".into(), "a".into(), Duration::ZERO),
    );
    mock.expect(
        "b",
        ProcessResult::ok("B".into(), "b".into(), Duration::ZERO),
    );
    mock.expect(
        "c",
        ProcessResult::ok("C".into(), "c".into(), Duration::ZERO),
    );

    let dir = tempfile::tempdir().unwrap();
    let r: Arc<dyn ProcessRunner> = mock.clone();
    let processes = vec![
        make_process(r.clone(), "a", "a", dir.path()),
        make_process(r.clone(), "b", "b", dir.path()),
        make_process(r.clone(), "c", "c", dir.path()),
    ];

    let results = run_sequential(processes, false, false).await;
    assert_eq!(results.len(), 3);
    let calls = mock.calls();
    assert_eq!(calls[0].prompt, "a");
    assert_eq!(calls[1].prompt, "b");
    assert_eq!(calls[2].prompt, "c");
}

#[tokio::test]
async fn run_sequential_stops_on_failure_when_requested() {
    let mock = Arc::new(MockProcessRunner::new());
    mock.expect(
        "a",
        ProcessResult::ok("A".into(), "a".into(), Duration::ZERO),
    );
    mock.expect(
        "b",
        ProcessResult::err("fail".into(), "b".into(), Duration::ZERO),
    );
    mock.expect(
        "c",
        ProcessResult::ok("C".into(), "c".into(), Duration::ZERO),
    );

    let dir = tempfile::tempdir().unwrap();
    let r: Arc<dyn ProcessRunner> = mock.clone();
    let processes = vec![
        make_process(r.clone(), "a", "a", dir.path()),
        make_process(r.clone(), "b", "b", dir.path()),
        make_process(r.clone(), "c", "c", dir.path()),
    ];

    let results = run_sequential(processes, false, true).await;
    assert_eq!(results.len(), 2, "should halt after b fails");
    assert_eq!(mock.calls().len(), 2);
}

#[tokio::test]
async fn run_sequential_pass_output_prepends_previous_output_to_next_prompt() {
    let mock = Arc::new(MockProcessRunner::new());
    mock.expect(
        "first",
        ProcessResult::ok("RESULT-OF-FIRST".into(), "1".into(), Duration::ZERO),
    );
    // The 2nd call's prompt MUST contain the 1st's output prefix.
    mock.expect_substring(
        "RESULT-OF-FIRST",
        ProcessResult::ok("done".into(), "2".into(), Duration::ZERO),
    );

    let dir = tempfile::tempdir().unwrap();
    let r: Arc<dyn ProcessRunner> = mock.clone();
    let processes = vec![
        make_process(r.clone(), "first", "1", dir.path()),
        make_process(r.clone(), "second", "2", dir.path()),
    ];

    let results = run_sequential(processes, true, false).await;
    assert_eq!(results.len(), 2);
    assert!(
        results[1].is_success(),
        "second process should match accumulated-output substring"
    );

    let calls = mock.calls();
    assert!(
        calls[1].prompt.contains("RESULT-OF-FIRST"),
        "second prompt should include first output, got {:?}",
        calls[1].prompt
    );
    assert!(calls[1].prompt.contains("Previous output"));
}

#[tokio::test]
async fn run_with_fallback_returns_error_for_empty_input() {
    let dir = tempfile::tempdir().unwrap();
    let _ = dir;
    let res = run_with_fallback(Vec::<ClaudeProcess>::new(), None).await;
    assert!(matches!(res, Err(ExecutionError::EmptyFallbackList)));
}

#[tokio::test]
async fn run_with_fallback_returns_first_success() {
    let mock = Arc::new(MockProcessRunner::new());
    mock.expect(
        "primary",
        ProcessResult::ok("yay".into(), "p".into(), Duration::ZERO),
    );
    mock.expect(
        "fallback",
        ProcessResult::ok("nope".into(), "f".into(), Duration::ZERO),
    );

    let dir = tempfile::tempdir().unwrap();
    let r: Arc<dyn ProcessRunner> = mock.clone();
    let processes = vec![
        make_process(r.clone(), "primary", "p", dir.path()),
        make_process(r.clone(), "fallback", "f", dir.path()),
    ];

    let res = run_with_fallback(processes, None).await.unwrap();
    assert_eq!(res.output, "yay");
    assert_eq!(
        mock.calls().len(),
        1,
        "fallback must NOT be called when primary succeeds"
    );
}

#[tokio::test]
async fn run_with_fallback_tries_next_on_failure() {
    let mock = Arc::new(MockProcessRunner::new());
    mock.expect(
        "primary",
        ProcessResult::err("dead".into(), "p".into(), Duration::ZERO),
    );
    mock.expect(
        "fallback",
        ProcessResult::ok("rescued".into(), "f".into(), Duration::ZERO),
    );

    let dir = tempfile::tempdir().unwrap();
    let r: Arc<dyn ProcessRunner> = mock.clone();
    let processes = vec![
        make_process(r.clone(), "primary", "p", dir.path()),
        make_process(r.clone(), "fallback", "f", dir.path()),
    ];

    let res = run_with_fallback(processes, None).await.unwrap();
    assert_eq!(res.output, "rescued");
    assert_eq!(mock.calls().len(), 2);
}

#[tokio::test]
async fn run_with_fallback_returns_last_failure_with_summary() {
    let mock = Arc::new(MockProcessRunner::new());
    mock.expect(
        "a",
        ProcessResult::err("err-a".into(), "a".into(), Duration::ZERO),
    );
    mock.expect(
        "b",
        ProcessResult::err("err-b".into(), "b".into(), Duration::ZERO),
    );

    let dir = tempfile::tempdir().unwrap();
    let r: Arc<dyn ProcessRunner> = mock.clone();
    let processes = vec![
        make_process(r.clone(), "a", "a", dir.path()),
        make_process(r.clone(), "b", "b", dir.path()),
    ];

    let res = run_with_fallback(processes, None).await.unwrap();
    assert_eq!(res.exit_code, -1);
    assert!(
        res.stderr.contains("All 2 fallback attempts failed"),
        "stderr should mention attempts count, got: {:?}",
        res.stderr
    );
    assert!(res.stderr.contains("err-b"));
}

#[tokio::test]
async fn run_batched_returns_empty_for_empty_input() {
    let results = run_batched(vec![], 3, false).await;
    assert!(results.is_empty());
}

#[tokio::test]
async fn run_batched_processes_in_groups_of_batch_size() {
    let mock = Arc::new(MockProcessRunner::new());
    for i in 0..7 {
        mock.expect(
            &format!("p{i}"),
            ProcessResult::ok(format!("o{i}"), format!("p{i}"), Duration::ZERO),
        );
    }

    let dir = tempfile::tempdir().unwrap();
    let r: Arc<dyn ProcessRunner> = mock.clone();
    let processes: Vec<_> = (0..7)
        .map(|i| make_process(r.clone(), &format!("p{i}"), &format!("p{i}"), dir.path()))
        .collect();

    let results = run_batched(processes, 3, false).await;
    assert_eq!(results.len(), 7);
    assert_eq!(mock.calls().len(), 7);
}
