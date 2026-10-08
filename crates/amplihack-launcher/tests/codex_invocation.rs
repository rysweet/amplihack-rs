//! Issue #1538: executable red contracts against the existing production builder.
//! Exec is selected by native `exec` passthrough, never by changing interactive stdin.
use amplihack_launcher::flag_matrix::AgentBinary;
use amplihack_launcher::prompt_delivery::build_tool_command_with_prompt_delivery;
use amplihack_utils::prompt_delivery::{DeliveryMode, PromptDelivery};
use std::path::Path;

fn build(
    args: &[&str],
    prompt: &str,
) -> std::io::Result<amplihack_launcher::prompt_delivery::DeliveredCommand> {
    build_tool_command_with_prompt_delivery(
        AgentBinary::Codex,
        Path::new("."),
        &args.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        prompt,
        PromptDelivery::Auto,
    )
}
fn argv(d: &amplihack_launcher::prompt_delivery::DeliveredCommand) -> Vec<String> {
    d.command
        .get_args()
        .map(|a| a.to_str().unwrap().to_owned())
        .collect()
}
#[test]
fn interactive_positional_prompt_preserves_profile_model_and_terminal_input() {
    let prompt = "--literal $HOME `echo no`\n日本語";
    let d = build(&["-p", "review", "--model", "caller-choice"], prompt).unwrap();
    assert_eq!(
        argv(&d),
        ["-p", "review", "--model", "caller-choice", "--", prompt]
    );
    assert_eq!(d.selected_mode, DeliveryMode::Argv);
    assert!(d.stdin_payload.is_none());
}
#[test]
fn interactive_resume_uses_native_subcommand() {
    let d = build(&["resume", "--last"], "continue").unwrap();
    assert_eq!(argv(&d), ["resume", "--last", "--", "continue"]);
    assert!(d.stdin_payload.is_none());
}
#[test]
fn exec_full_large_envelope_is_stdin_without_added_writable_scope() {
    let prompt = format!(
        "recipe-run provenance\nSYSTEM\nPERSONA\n{}\nTASK-END",
        "λ\n".repeat(100_000)
    );
    let d = build(&["exec", "--output-last-message", "/d0/final.txt"], &prompt).unwrap();
    assert_eq!(
        argv(&d),
        ["exec", "--output-last-message", "/d0/final.txt", "-"]
    );
    assert_eq!(d.selected_mode, DeliveryMode::Stdin);
    assert_eq!(d.stdin_payload.as_deref(), Some(prompt.as_bytes()));
    assert!(d.delivery_handle.tempfile_path().is_none());
}
#[test]
fn exec_resume_keeps_session_and_output_before_stdin_marker() {
    let d = build(
        &[
            "exec",
            "resume",
            "--output-last-message",
            "/d0/final.txt",
            "session-123",
        ],
        "task",
    )
    .unwrap();
    assert_eq!(
        argv(&d),
        [
            "exec",
            "resume",
            "--output-last-message",
            "/d0/final.txt",
            "session-123",
            "-"
        ]
    );
    assert_eq!(d.stdin_payload.as_deref(), Some(b"task".as_slice()));
}
#[test]
fn invalid_prompt_and_conflicting_mode_options_fail_before_spawn() {
    for (args, prompt) in [
        (vec![], ""),
        (vec![], "secret\0tail"),
        (vec!["--prompt", "other"], "task"),
        (vec!["exec", "resume", "--add-dir", "/d0", "--last"], "task"),
        (vec!["resume", "--last", "session-123"], "task"),
        (vec!["--", "exec"], "task"),
        (
            vec!["--remote-auth-token-env", "exec", "--", "resume"],
            "task",
        ),
    ] {
        let error = build(&args, prompt).expect_err("invalid Codex request must be rejected");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert!(!error.to_string().contains("secret"));
    }
}
#[test]
fn unproven_oversized_interactive_transport_fails_instead_of_truncating() {
    assert!(build(&[], &"x".repeat(2 * 1024 * 1024)).is_err());
}
#[test]
fn copilot_retains_prompt_flag_and_raw_bytes() {
    let d = build_tool_command_with_prompt_delivery(
        AgentBinary::Copilot,
        Path::new("."),
        &[],
        "hi\nλ",
        PromptDelivery::Auto,
    )
    .unwrap();
    assert_eq!(argv(&d), ["-p", "hi\nλ"]);
    assert!(d.stdin_payload.is_none());
}

#[test]
fn supported_root_options_preserve_exact_exec_arguments_and_complete_stdin() {
    let prompt = format!("SYSTEM\nPERSONA\n{}\nTASK-END", "λ\n".repeat(40_000));
    for args in [
        vec!["--no-daemon", "--remote", "exec", "e"],
        vec!["--remote=resume", "--remote-auth-token-env", "exec", "exec"],
        vec!["--remote", "resume", "--remote-auth-token-env=exec", "exec"],
        vec![
            "--model",
            "resume",
            "--profile=exec",
            "--config",
            "key=value",
            "exec",
        ],
    ] {
        let d = build(&args, &prompt).unwrap();
        let mut expected: Vec<String> = args.iter().map(|s| (*s).into()).collect();
        expected.push("-".into());
        assert_eq!(argv(&d), expected);
        assert_eq!(d.selected_mode, DeliveryMode::Stdin);
        assert_eq!(d.stdin_payload.as_deref(), Some(prompt.as_bytes()));
    }
}

#[test]
fn supported_resume_and_command_like_values_retain_terminal_transport() {
    for args in [
        vec!["resume", "--last", "--include-non-interactive"],
        vec!["resume", "session-123", "--include-non-interactive"],
        vec!["--remote", "exec"],
        vec!["--remote-auth-token-env=resume", "--no-daemon"],
    ] {
        let d = build(&args, "continue λ").unwrap();
        let mut expected: Vec<String> = args.iter().map(|s| (*s).into()).collect();
        expected.extend(["--".into(), "continue λ".into()]);
        assert_eq!(argv(&d), expected);
        assert_eq!(d.selected_mode, DeliveryMode::Argv);
        assert!(
            d.stdin_payload.is_none(),
            "terminal stdin must remain inherited"
        );
    }
}

#[test]
fn supported_exec_resume_preserves_stdin_and_delimiter() {
    let args = ["exec", "resume", "--last", "--all", "--"];
    let d = build(&args, "continue").unwrap();
    assert_eq!(argv(&d), ["exec", "resume", "--last", "--all", "--", "-"]);
    assert_eq!(d.stdin_payload.as_deref(), Some(b"continue".as_slice()));
    for args in [
        vec!["--remote"],
        vec!["--remote-auth-token-env"],
        vec!["--unknown", "exec"],
    ] {
        let error = build(&args, "private task").unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert!(!error.to_string().contains("private task"));
    }
}

#[cfg(unix)]
#[path = "codex_invocation/subprocess.rs"]
mod subprocess;

#[cfg(unix)]
#[test]
fn remote_exec_subprocess_receives_complete_stdin() {
    subprocess::remote_exec_subprocess_receives_complete_stdin();
}

#[cfg(unix)]
#[test]
fn interactive_resume_subprocess_inherits_real_terminal_stdin() {
    subprocess::interactive_resume_subprocess_inherits_real_terminal_stdin();
}
