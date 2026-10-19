//! Issue #1570 (D1/R5): when a child does not read its whole stdin prompt,
//! the child's exit status decides the result, never the unfinished write.
//!
//! Each child exits without reading a 1 MiB prompt, far more than a pipe
//! buffer holds, so the writer sees `BrokenPipe` or is still pending when
//! the child exits.

#![cfg(unix)]

use std::time::Duration;

use amplihack_orchestration::claude_process::{
    build_command_with_prompt_delivery, run_delivered_command_for_test,
};
use amplihack_utils::prompt_delivery::{DeliveryCaps, DeliveryMode, PromptDelivery};

async fn run_without_reading_stdin(
    exit_code: i32,
) -> amplihack_orchestration::claude_process::ProcessResult {
    let prompt = "p".repeat(1024 * 1024);
    let script = format!("printf done; exit {exit_code}");
    let delivered = build_command_with_prompt_delivery(
        "sh",
        ["-c", script.as_str()],
        &prompt,
        PromptDelivery::Stdin,
        DeliveryCaps::argv_only().with_stdin(),
    )
    .expect("stdin-capable delivered command should build");
    assert_eq!(delivered.selected_mode, DeliveryMode::Stdin);
    tokio::time::timeout(
        Duration::from_secs(5),
        run_delivered_command_for_test(delivered, Duration::from_secs(10)),
    )
    .await
    .expect("an exited child must not leave the runner waiting")
    .expect("test runner should return a process result")
}

#[tokio::test]
async fn a_child_that_exits_0_without_reading_stdin_succeeds() {
    let result = run_without_reading_stdin(0).await;
    assert_eq!(result.exit_code, 0, "stderr: {:?}", result.stderr);
    assert_eq!(result.output, "done");
}

#[tokio::test]
async fn a_child_that_exits_3_without_reading_stdin_reports_exit_3() {
    let result = run_without_reading_stdin(3).await;
    assert_eq!(result.exit_code, 3, "stderr: {:?}", result.stderr);
    assert!(
        !result.stderr.contains("prompt"),
        "the child's status, not a prompt delivery error, must be reported; got {:?}",
        result.stderr
    );
}
