//! TDD red-phase isolation test for SEC-10: no stale sink inheritance.
//!
//! When a run does NOT opt into the clean channel, the runner must actively
//! remove any inherited `AMPLIHACK_RESULT_SINK` from the child's environment so
//! a stale value from an ancestor process can never silently redirect capture.
//!
//! This test mutates process-global environment, so it lives in its own test
//! binary and is the ONLY test here — no sibling tests can race the mutation.
//!
//! It fails until the runner env_removes the variable on the no-opt-in path
//! (and until `ProcessResult.result` exists). See
//! docs/reference/clean-result-channel.md (SEC-10).

#![cfg(unix)]

use std::time::Duration;

use amplihack_orchestration::claude_process::{RunOptions, TokioProcessRunner};
use amplihack_utils::prompt_delivery::{DeliveryCaps, PromptDelivery};

/// Prints whatever `AMPLIHACK_RESULT_SINK` the child actually sees, or the
/// literal `UNSET` when it is absent/empty.
const REPORT_ENV_SCRIPT: &str = r#"printf '%s' "${AMPLIHACK_RESULT_SINK:-UNSET}""#;

#[tokio::test]
async fn no_opt_in_run_strips_inherited_result_sink_from_child() {
    // Simulate a stale ancestor value in this process's environment.
    let stale = "/tmp/stale-ancestor-result-sink-value";
    // SAFETY: single-test binary; no other thread reads/writes env concurrently.
    unsafe {
        std::env::set_var("AMPLIHACK_RESULT_SINK", stale);
    }

    let runner = TokioProcessRunner::new();
    let mut opts = RunOptions::new("unused-prompt".to_string(), "sec10".to_string());
    opts.timeout = Some(Duration::from_secs(10));
    // NOTE: no `with_result_sink` — this run does not opt in.

    let result = runner
        .run_with_prompt_delivery_for_test(
            opts,
            PromptDelivery::Argv,
            DeliveryCaps::argv_only(),
            "sh",
            ["-c", REPORT_ENV_SCRIPT, "sh"],
        )
        .await;

    unsafe {
        std::env::remove_var("AMPLIHACK_RESULT_SINK");
    }

    assert_eq!(
        result.output, "UNSET",
        "SEC-10: a no-opt-in run must env_remove the inherited AMPLIHACK_RESULT_SINK \
         so the child never sees the stale value {stale:?}"
    );
    assert!(
        result.result.is_none(),
        "no-opt-in run must yield result == None"
    );
}
