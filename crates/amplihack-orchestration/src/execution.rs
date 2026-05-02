//! Execution helpers for orchestrating multiple `ClaudeProcess` instances.
//!
//! Native Rust port of `execution.py`. Mirrors the four primitives:
//! `run_parallel`, `run_sequential`, `run_with_fallback`, `run_batched`.

use std::sync::Arc;
use std::time::Duration;

use thiserror::Error;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::claude_process::{ClaudeProcess, ProcessResult};

/// Errors that can occur in the execution helpers.
#[derive(Debug, Error)]
pub enum ExecutionError {
    #[error("run_with_fallback requires at least one process")]
    EmptyFallbackList,
}

/// Run multiple `ClaudeProcess` instances concurrently.
///
/// `max_workers` bounds concurrency via a `tokio::sync::Semaphore`; `None`
/// means unbounded. Results are returned in completion order, mirroring the
/// Python `as_completed` semantics.
pub async fn run_parallel(
    processes: Vec<ClaudeProcess>,
    max_workers: Option<usize>,
) -> Vec<ProcessResult> {
    if processes.is_empty() {
        return Vec::new();
    }

    let semaphore: Option<Arc<Semaphore>> = max_workers.map(|n| Arc::new(Semaphore::new(n.max(1))));
    let mut set: JoinSet<ProcessResult> = JoinSet::new();

    for process in processes {
        let sem = semaphore.clone();
        set.spawn(async move {
            // Acquire a permit if a bound is configured. Permit is dropped
            // when this task exits.
            let _permit = match sem {
                Some(s) => Some(s.acquire_owned().await.expect("semaphore not closed")),
                None => None,
            };
            process.run().await
        });
    }

    let mut results = Vec::new();
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok(r) => results.push(r),
            Err(e) => results.push(ProcessResult::err(
                format!("Parallel execution exception: {e}"),
                "join_error".to_string(),
                Duration::ZERO,
            )),
        }
    }
    results
}

/// Run processes sequentially in order.
///
/// - `pass_output` — when true, prepends a `"Previous output"` block built
///   from accumulated previous outputs to each subsequent prompt (matching
///   Python's exact wording).
/// - `stop_on_failure` — when true, halts after the first non-zero exit.
pub async fn run_sequential(
    mut processes: Vec<ClaudeProcess>,
    pass_output: bool,
    stop_on_failure: bool,
) -> Vec<ProcessResult> {
    if processes.is_empty() {
        return Vec::new();
    }

    let mut results = Vec::new();
    let mut accumulated = String::new();

    for (i, process) in processes.iter_mut().enumerate() {
        if pass_output && i > 0 && !accumulated.is_empty() {
            let context =
                format!("\n\n--- Previous output ---\n{accumulated}\n\n--- New task ---\n");
            let new_prompt = format!("{}{}", context, process.prompt());
            process.set_prompt(new_prompt);
        }

        let result = process.run().await;
        let succeeded = result.is_success();
        if pass_output {
            accumulated.push_str(&result.output);
        }
        results.push(result);

        if stop_on_failure && !succeeded {
            process.log("Stopping sequential execution due to failure", "WARNING");
            break;
        }
    }
    results
}

/// Try each process in order, returning the first success.
///
/// If `timeout` is set, each attempt's timeout is overridden. When all
/// attempts fail, returns the last failure with stderr prefixed by
/// `"All N fallback attempts failed."`.
pub async fn run_with_fallback(
    mut processes: Vec<ClaudeProcess>,
    timeout: Option<Duration>,
) -> Result<ProcessResult, ExecutionError> {
    if processes.is_empty() {
        return Err(ExecutionError::EmptyFallbackList);
    }

    let n = processes.len();
    let mut last: Option<ProcessResult> = None;

    for process in processes.iter_mut() {
        if let Some(t) = timeout {
            process.set_timeout(Some(t));
        }
        process.log("Attempting process (fallback strategy)", "INFO");
        let result = process.run().await;
        if result.is_success() {
            process.log("Process succeeded, skipping remaining fallbacks", "INFO");
            return Ok(result);
        }
        process.log(
            &format!(
                "Process failed (exit_code={}), trying next fallback",
                result.exit_code
            ),
            "WARNING",
        );
        last = Some(result);
    }

    let mut last = last.expect("loop guarantees at least one assignment when n > 0");
    last.stderr = format!(
        "All {n} fallback attempts failed. Last error: {}",
        last.stderr
    );
    Ok(last)
}

/// Run processes in batches, with each batch executed in parallel.
///
/// When `pass_output` is true, the concatenated successful outputs of the
/// previous batch are prefixed to each prompt in the next batch (matching
/// Python's exact wording).
pub async fn run_batched(
    mut processes: Vec<ClaudeProcess>,
    batch_size: usize,
    pass_output: bool,
) -> Vec<ProcessResult> {
    if processes.is_empty() {
        return Vec::new();
    }
    let batch_size = batch_size.max(1);

    let mut all = Vec::new();
    let mut accumulated = String::new();

    while !processes.is_empty() {
        let take = batch_size.min(processes.len());
        let mut batch: Vec<ClaudeProcess> = processes.drain(..take).collect();

        if pass_output && !accumulated.is_empty() {
            for p in batch.iter_mut() {
                let context = format!(
                    "\n\n--- Previous batch output ---\n{accumulated}\n\n--- New task ---\n"
                );
                let new_prompt = format!("{}{}", context, p.prompt());
                p.set_prompt(new_prompt);
            }
        }

        let batch_results = run_parallel(batch, None).await;
        if pass_output {
            let mut joined = String::new();
            for r in batch_results.iter().filter(|r| r.is_success()) {
                if !joined.is_empty() {
                    joined.push_str("\n\n");
                }
                joined.push_str(&r.output);
            }
            if !joined.is_empty() {
                if !accumulated.is_empty() {
                    accumulated.push_str("\n\n");
                }
                accumulated.push_str(&joined);
            }
        }
        all.extend(batch_results);
    }
    all
}

#[cfg(test)]
mod inline_tests {
    use super::*;
    use crate::claude_process::{MockProcessRunner, ProcessRunner};

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
    async fn run_parallel_empty() {
        assert!(run_parallel(vec![], None).await.is_empty());
    }

    #[tokio::test]
    async fn run_with_fallback_empty_errs() {
        let r = run_with_fallback(Vec::<ClaudeProcess>::new(), None).await;
        assert!(matches!(r, Err(ExecutionError::EmptyFallbackList)));
    }

    #[tokio::test]
    async fn run_batched_empty() {
        assert!(run_batched(vec![], 3, false).await.is_empty());
    }

    #[tokio::test]
    async fn run_sequential_empty() {
        assert!(run_sequential(vec![], false, false).await.is_empty());
    }

    #[tokio::test]
    async fn batched_zero_size_is_treated_as_one() {
        let dir = tempfile::tempdir().unwrap();
        let mock = Arc::new(MockProcessRunner::new());
        mock.expect_any(ProcessResult::ok("o".into(), "p".into(), Duration::ZERO));
        let processes = vec![make_process(
            mock as Arc<dyn ProcessRunner>,
            "p",
            "p",
            dir.path(),
        )];
        let r = run_batched(processes, 0, false).await;
        assert_eq!(r.len(), 1);
    }
}
