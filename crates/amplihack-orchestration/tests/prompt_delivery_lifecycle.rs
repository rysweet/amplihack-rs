//! Red-phase async lifecycle tests for orchestration prompt delivery.
//!
//! The green phase must adapt `amplihack-utils::prompt_delivery` for Tokio
//! process orchestration without duplicating mode-selection logic. The returned
//! delivery handle has to live until the child exits, and stdin delivery must
//! write, flush, close, then await.

#![cfg(unix)]

use std::path::Path;
use std::time::Duration;

use amplihack_orchestration::claude_process::{
    RunOptions, TokioProcessRunner, build_command_with_prompt_delivery,
    run_delivered_command_for_test,
};
use amplihack_utils::prompt_delivery::{DeliveryCaps, DeliveryMode, PromptDelivery};

const PAYLOAD_SIZE: usize = 64 * 1024;

fn synthetic_prompt() -> String {
    let pattern = "stdin/tempfile lifecycle: don't truncate 'apostrophes' or $DOLLARS\n";
    let mut prompt = String::with_capacity(PAYLOAD_SIZE);
    while prompt.len() < PAYLOAD_SIZE {
        prompt.push_str(pattern);
    }
    prompt.truncate(PAYLOAD_SIZE);
    prompt
}

#[tokio::test]
async fn tokio_stdin_delivery_writes_flushes_closes_and_awaits_child() {
    let prompt = synthetic_prompt();
    let delivered = build_command_with_prompt_delivery(
        "cat",
        std::iter::empty::<&str>(),
        &prompt,
        PromptDelivery::Stdin,
        DeliveryCaps {
            supports_argv: true,
            supports_tempfile: false,
            supports_stdin: true,
            tempfile_flag: None,
        },
    )
    .expect("stdin-capable delivered command should build");

    assert_eq!(delivered.selected_mode, DeliveryMode::Stdin);
    assert_eq!(
        delivered.stdin_payload.as_deref(),
        Some(prompt.as_bytes()),
        "stdin mode must expose the exact payload bytes for the Tokio spawn path"
    );

    let result = run_delivered_command_for_test(delivered, Duration::from_secs(5))
        .await
        .expect("cat should exit after stdin is closed");

    assert_eq!(result.exit_code, 0);
    assert_eq!(result.output, prompt);
}

#[tokio::test]
async fn tokio_tempfile_delivery_keeps_handle_alive_until_after_wait() {
    let prompt = synthetic_prompt();
    let delivered = build_command_with_prompt_delivery(
        "cat",
        std::iter::empty::<&str>(),
        &prompt,
        PromptDelivery::Tempfile,
        DeliveryCaps::argv_and_tempfile(""),
    )
    .expect("tempfile-capable delivered command should build");

    let path = delivered
        .delivery_handle
        .tempfile_path()
        .expect("tempfile mode must expose a path")
        .to_path_buf();
    assert!(path.exists(), "tempfile must exist before spawn");

    let result = run_delivered_command_for_test(delivered, Duration::from_secs(5))
        .await
        .expect("cat should read the tempfile before the handle is dropped");

    assert_eq!(result.exit_code, 0);
    assert_eq!(result.output, prompt);
    assert!(
        !path.exists(),
        "orchestration must drop the DeliveryHandle only after the child exits"
    );
}

#[tokio::test]
async fn process_runner_uses_delivery_path_for_large_delegate_prompt() {
    let runner = TokioProcessRunner::new();
    let prompt = synthetic_prompt();

    let result = runner
        .run_with_prompt_delivery_for_test(
            RunOptions {
                prompt: prompt.clone(),
                process_id: "prompt-delivery-lifecycle".to_string(),
                timeout: Some(Duration::from_secs(5)),
                model: None,
                working_dir: Some(Path::new("/tmp").to_path_buf()),
                result_sink: None,
            },
            PromptDelivery::Auto,
            DeliveryCaps {
                supports_argv: true,
                supports_tempfile: false,
                supports_stdin: true,
                tempfile_flag: None,
            },
            "cat",
            std::iter::empty::<&str>(),
        )
        .await;

    assert_eq!(result.exit_code, 0);
    assert_eq!(result.output, prompt);
}
