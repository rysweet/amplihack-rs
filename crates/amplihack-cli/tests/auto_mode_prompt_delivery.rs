//! Red-phase auto-mode prompt-delivery tests.
//!
//! Auto mode must stop inserting large prompts directly into child argv through
//! duplicate prompt construction. It should use the same launcher
//! prompt-delivery abstraction as normal launch paths.

use amplihack_cli::commands::auto_mode::{
    AutoModePromptDeliveryOptions, AutoModeTool, build_auto_command_with_prompt_delivery,
};
use amplihack_utils::prompt_delivery::{DeliveryMode, PromptDelivery};

const PAYLOAD_SIZE: usize = 64 * 1024;

fn synthetic_prompt() -> String {
    let pattern = "auto-mode issue 652: don't truncate 'quotes' or $(shell)\n";
    let mut prompt = String::with_capacity(PAYLOAD_SIZE);
    while prompt.len() < PAYLOAD_SIZE {
        prompt.push_str(pattern);
    }
    prompt.truncate(PAYLOAD_SIZE);
    prompt
}

fn argv(command: &std::process::Command) -> Vec<String> {
    command
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect()
}

#[test]
fn auto_mode_command_records_prompt_delivery_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let prompt = synthetic_prompt();

    let delivered = build_auto_command_with_prompt_delivery(AutoModePromptDeliveryOptions {
        tool: AutoModeTool::Copilot,
        execution_dir: dir.path().to_path_buf(),
        project_dir: dir.path().to_path_buf(),
        node_options: Some("--max-old-space-size=32768".to_string()),
        passthrough_args: vec!["--model".to_string(), "gpt-5.5".to_string()],
        prompt: prompt.clone(),
        requested_delivery: PromptDelivery::Tempfile,
    })
    .expect("auto-mode delivery-aware command should build");

    assert_eq!(delivered.selected_mode, DeliveryMode::Argv);
    let warnings = format!("{:?}", delivered.warnings);
    assert!(
        warnings.contains("degrading to argv") || warnings.contains("Argv"),
        "argv-only Copilot must produce a deterministic warning for tempfile requests"
    );
    assert_eq!(
        argv(&delivered.command)
            .iter()
            .filter(|arg| *arg == &prompt)
            .count(),
        1,
        "until Copilot has a verified long-form contract, auto mode must pass the prompt once as a structured argv element"
    );
}

#[test]
fn auto_mode_does_not_duplicate_prompt_between_wrapper_and_child_args() {
    let dir = tempfile::tempdir().unwrap();
    let prompt = synthetic_prompt();

    let delivered = build_auto_command_with_prompt_delivery(AutoModePromptDeliveryOptions {
        tool: AutoModeTool::Amplifier,
        execution_dir: dir.path().to_path_buf(),
        project_dir: dir.path().to_path_buf(),
        node_options: None,
        passthrough_args: vec![],
        prompt: prompt.clone(),
        requested_delivery: PromptDelivery::Auto,
    })
    .expect("amplifier auto-mode delivery-aware command should build");

    let args = argv(&delivered.command);
    assert_eq!(
        args.iter().filter(|arg| *arg == &prompt).count(),
        1,
        "auto mode must not pass the prompt once to amplihack and again to the nested child; args: {args:?}"
    );
}

#[test]
fn amplifier_auto_mode_uses_documented_run_positional_prompt_contract() {
    let dir = tempfile::tempdir().unwrap();
    let prompt = "issue #709: keep quotes, $PATH, and\nnewlines".to_string();

    let delivered = build_auto_command_with_prompt_delivery(AutoModePromptDeliveryOptions {
        tool: AutoModeTool::Amplifier,
        execution_dir: dir.path().to_path_buf(),
        project_dir: dir.path().to_path_buf(),
        node_options: None,
        passthrough_args: vec![
            "--provider".to_string(),
            "openai".to_string(),
            "--model".to_string(),
            "amp-test".to_string(),
        ],
        prompt: prompt.clone(),
        requested_delivery: PromptDelivery::Auto,
    })
    .expect("amplifier auto-mode delivery-aware command should build");

    let args = argv(&delivered.command);
    let passthrough_start = args
        .iter()
        .position(|arg| arg == "--")
        .expect("nested amplihack command must separate wrapper and Amplifier args")
        + 1;
    assert_eq!(
        &args[passthrough_start..],
        &[
            "run".to_string(),
            "--provider".to_string(),
            "openai".to_string(),
            "--model".to_string(),
            "amp-test".to_string(),
            prompt,
        ],
        "Amplifier auto-mode must use `amplifier run [OPTIONS] [PROMPT]` without a synthetic prompt flag"
    );
}

#[test]
fn amplifier_auto_mode_rejects_explicit_unsupported_prompt_delivery_modes() {
    let dir = tempfile::tempdir().unwrap();
    let prompt = synthetic_prompt();

    for requested_delivery in [PromptDelivery::Tempfile, PromptDelivery::Stdin] {
        let err = build_auto_command_with_prompt_delivery(AutoModePromptDeliveryOptions {
            tool: AutoModeTool::Amplifier,
            execution_dir: dir.path().to_path_buf(),
            project_dir: dir.path().to_path_buf(),
            node_options: None,
            passthrough_args: vec![],
            prompt: prompt.clone(),
            requested_delivery,
        })
        .expect_err("Amplifier auto-mode must reject unsupported explicit delivery modes");
        let message = format!("{err:#}");
        assert!(
            message.contains("Amplifier prompt delivery mode"),
            "error should identify Amplifier prompt-delivery policy; got: {message}"
        );
        assert!(
            message.contains(match requested_delivery {
                PromptDelivery::Tempfile => "tempfile",
                PromptDelivery::Stdin => "stdin",
                _ => unreachable!(),
            }),
            "error should name rejected mode; got: {message}"
        );
    }
}
