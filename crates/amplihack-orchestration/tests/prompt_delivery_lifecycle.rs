//! Red-phase async lifecycle tests for orchestration prompt delivery.
//!
//! The green phase must adapt `amplihack-utils::prompt_delivery` for Tokio
//! process orchestration without duplicating mode-selection logic. The returned
//! delivery handle has to live until the child exits, and stdin delivery must
//! write, flush, close, then await.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use amplihack_orchestration::claude_process::{
    RunOptions, TokioProcessRunner, build_command_with_prompt_delivery,
    run_delivered_command_for_test,
};
use amplihack_utils::prompt_delivery::{DeliveryCaps, DeliveryMode, PromptDelivery};

const PAYLOAD_SIZE: usize = 64 * 1024;

struct KillPidFile(PathBuf);

impl Drop for KillPidFile {
    fn drop(&mut self) {
        if let Ok(pid) = std::fs::read_to_string(&self.0) {
            let _ = std::process::Command::new("kill")
                .arg("-KILL")
                .arg(pid.trim())
                .stderr(std::process::Stdio::null())
                .status();
        }
    }
}

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
async fn stdin_delivery_drains_large_child_output_while_writing_prompt() {
    let prompt = "p".repeat(1024 * 1024);
    // Emit more than a typical OS pipe can hold on stdout and stderr before
    // reading stdin. The parent must drain both streams while writing this
    // large prompt, or both sides can block forever.
    let command = r"head -c 131072 /dev/zero | tr '\000' o; head -c 131072 /dev/zero | tr '\000' e >&2; cat >/dev/null; printf done";
    let delivered = build_command_with_prompt_delivery(
        "sh",
        ["-c", command],
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

    let result = tokio::time::timeout(
        Duration::from_secs(10),
        run_delivered_command_for_test(delivered, Duration::from_secs(5)),
    )
    .await
    .expect("stdin writer and output drain must not deadlock")
    .expect("test child should run to completion");

    assert_eq!(result.exit_code, 0);
    assert_eq!(result.output.len(), 131072 + "done".len());
    assert!(result.output.starts_with(&"o".repeat(131072)));
    assert_eq!(result.stderr.len(), 131072);
    assert!(result.stderr.bytes().all(|byte| byte == b'e'));
}

#[tokio::test]
async fn stdin_write_failure_kills_and_reaps_output_streaming_child() {
    let temp = tempfile::tempdir().expect("temporary directory should be available");
    let pid_file = temp.path().join("child.pid");
    let pid_file_arg = pid_file.to_str().expect("temporary path should be UTF-8");
    let prompt = "p".repeat(1024 * 1024);
    // Close stdin, then keep stdout active. The failed write must cancel the
    // output-liveness wait instead of waiting forever for an idle interval.
    let command = r#"printf '%s\n' "$$" > "$1"; exec 0<&-; while :; do printf x; sleep 0.01; done"#;
    let delivered = build_command_with_prompt_delivery(
        "sh",
        ["-c", command, "sh", pid_file_arg],
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

    let result = tokio::time::timeout(
        Duration::from_secs(3),
        run_delivered_command_for_test(delivered, Duration::from_secs(5)),
    )
    .await
    .expect("stdin failure must not wait for an output-idle interval")
    .expect("test runner should return a process result");

    assert_eq!(result.exit_code, -1);
    assert!(
        result
            .stderr
            .contains("failed to write prompt to child stdin:"),
        "expected the prompt delivery error, got {:?}",
        result.stderr
    );
    let pid = std::fs::read_to_string(&pid_file)
        .expect("child should record its pid before closing stdin")
        .trim()
        .parse::<u32>()
        .expect("child pid should be numeric");
    let still_running = std::process::Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .stderr(std::process::Stdio::null())
        .status()
        .expect("Unix kill utility should be available")
        .success();
    assert!(
        !still_running,
        "failed child {pid} must be reaped before return"
    );
}

#[tokio::test]
async fn stdin_writer_is_cancelled_when_exited_child_leaves_nonreader() {
    let temp = tempfile::tempdir().expect("temporary directory should be available");
    let pid_file = temp.path().join("descendant.pid");
    let _cleanup = KillPidFile(pid_file.clone());
    let pid_file_arg = pid_file.to_str().expect("temporary path should be UTF-8");
    let prompt = "p".repeat(1024 * 1024);
    // Keep the inherited stdin read end open in a background process that
    // never reads. The direct child exits, so the watchdog finishes while the
    // prompt writer is still blocked on the full pipe.
    let command = r#"sleep 30 <&0 >/dev/null 2>&1 & printf '%s\n' "$!" > "$1"; exit 0"#;
    let delivered = build_command_with_prompt_delivery(
        "sh",
        ["-c", command, "sh", pid_file_arg],
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

    let result = tokio::time::timeout(
        Duration::from_secs(3),
        run_delivered_command_for_test(delivered, Duration::from_secs(5)),
    )
    .await
    .expect("writer must be cancelled after the direct child exits")
    .expect("test runner should return a process result");

    assert_eq!(result.exit_code, -1);
    assert!(
        result
            .stderr
            .contains("failed to write prompt to child stdin:"),
        "expected incomplete-prompt error, got {:?}",
        result.stderr
    );
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
