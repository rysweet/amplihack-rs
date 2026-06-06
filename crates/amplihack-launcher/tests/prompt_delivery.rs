//! Red-phase prompt-delivery integration contracts for launcher command builders.
//!
//! These tests intentionally name the public API expected from the green phase:
//! static capability metadata plus delivery-aware command builders that preserve
//! the selected mode, warnings, and RAII handle alongside the `Command`.

use std::ffi::OsStr;
use std::io::ErrorKind;
use std::path::Path;

use amplihack_launcher::flag_matrix::{
    ALL_BINARIES, AgentBinary, prompt_delivery_caps_for, prompt_delivery_report_for,
};
use amplihack_launcher::prompt_delivery::{
    DeliveryWarning, build_command_with_prompt_delivery, build_tool_command_with_prompt_delivery,
};
use amplihack_utils::prompt_delivery::{DeliveryMode, PromptDelivery};

const PAYLOAD_SIZE: usize = 64 * 1024;

fn synthetic_prompt() -> String {
    let pattern = "issue 652: don't shell-quote `$PATH`; keep \"bytes\" intact\n";
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
fn capability_matrix_covers_all_prompt_binaries_without_speculation() {
    assert!(
        ALL_BINARIES.contains(&AgentBinary::Amplifier),
        "Amplifier must be represented in launcher prompt-delivery metadata"
    );

    for binary in [
        AgentBinary::Claude,
        AgentBinary::Copilot,
        AgentBinary::Codex,
        AgentBinary::Amplifier,
    ] {
        let caps = prompt_delivery_caps_for(binary);
        assert!(
            caps.supports_argv,
            "{binary} must support structured argv prompt delivery"
        );
    }

    let claude = prompt_delivery_caps_for(AgentBinary::Claude);
    assert!(
        !claude.supports_tempfile,
        "Claude task-prompt tempfile support must stay disabled until a verified task-prompt file contract exists; --append-system-prompt is a different prompt role"
    );
    assert!(
        !claude.supports_stdin,
        "Claude stdin task-prompt support is not verified"
    );

    let copilot = prompt_delivery_caps_for(AgentBinary::Copilot);
    assert!(
        !copilot.supports_tempfile && !copilot.supports_stdin,
        "Copilot must remain argv-only until a prompt-file or stdin contract is verified"
    );

    let codex = prompt_delivery_caps_for(AgentBinary::Codex);
    assert!(
        !codex.supports_tempfile && !codex.supports_stdin,
        "Codex stdin support is pending a named verified command contract and must not be enabled speculatively"
    );

    let amplifier = prompt_delivery_caps_for(AgentBinary::Amplifier);
    assert!(
        !amplifier.supports_tempfile && !amplifier.supports_stdin,
        "Amplifier must be migrated through prompt_delivery as argv-only unless a long-form contract is verified"
    );
}

#[test]
fn argv_only_builders_still_use_one_structured_argv_element_for_raw_prompt() {
    let prompt = synthetic_prompt();

    for binary in [
        AgentBinary::Claude,
        AgentBinary::Copilot,
        AgentBinary::Codex,
        AgentBinary::Amplifier,
    ] {
        let delivered = build_tool_command_with_prompt_delivery(
            binary,
            Path::new("/tmp"),
            &[],
            &prompt,
            PromptDelivery::Auto,
        )
        .expect("delivery-aware launcher command should build");

        assert_eq!(
            delivered.selected_mode,
            DeliveryMode::Argv,
            "{binary} has no verified long-form delivery contract yet"
        );
        assert!(
            delivered.delivery_handle.tempfile_path().is_none(),
            "{binary} argv mode must not create a tempfile"
        );

        let args = argv(&delivered.command);
        assert_eq!(
            args.iter().filter(|arg| *arg == &prompt).count(),
            1,
            "{binary} must pass the raw prompt exactly once as one argv element, not via shell splitting. Args: {args:?}"
        );
    }
}

#[test]
fn unsupported_tempfile_request_degrades_to_argv_with_warning() {
    let prompt = synthetic_prompt();

    let delivered = build_tool_command_with_prompt_delivery(
        AgentBinary::Copilot,
        Path::new("/tmp"),
        &[],
        &prompt,
        PromptDelivery::Tempfile,
    )
    .expect("unsupported explicit mode should degrade, not fail");

    assert_eq!(delivered.selected_mode, DeliveryMode::Argv);
    assert!(
        delivered.warnings.iter().any(|warning| {
            matches!(
                warning,
                DeliveryWarning::UnsupportedMode {
                    requested: PromptDelivery::Tempfile,
                    effective: DeliveryMode::Argv,
                    ..
                }
            )
        }),
        "explicit tempfile on an argv-only binary must produce a deterministic degradation warning; got {:?}",
        delivered.warnings
    );
}

#[test]
fn amplifier_argv_uses_documented_positional_prompt_contract() {
    let prompt = "issue #709: keep `$PATH`, quotes, and\nnewlines as one argv element";
    let extra_args = vec![
        "--dry-run".to_string(),
        "--model".to_string(),
        "amp-test".to_string(),
    ];

    let delivered = build_tool_command_with_prompt_delivery(
        AgentBinary::Amplifier,
        Path::new("/tmp"),
        &extra_args,
        prompt,
        PromptDelivery::Auto,
    )
    .expect("Amplifier argv prompt delivery should build");

    assert_eq!(delivered.selected_mode, DeliveryMode::Argv);
    assert!(
        delivered.delivery_handle.tempfile_path().is_none(),
        "Amplifier argv mode must not create a tempfile"
    );

    let args = argv(&delivered.command);
    let expected_args: Vec<String> = ["run", "--dry-run", "--model", "amp-test", prompt]
        .into_iter()
        .map(String::from)
        .collect();
    assert_eq!(
        args, expected_args,
        "Amplifier must use the documented `amplifier run [OPTIONS] [PROMPT]` shape without a synthetic --prompt flag"
    );
}

#[test]
fn amplifier_rejects_explicit_tempfile_and_stdin_before_launch() {
    let prompt = synthetic_prompt();

    for (requested, mode_name) in [
        (PromptDelivery::Tempfile, "tempfile"),
        (PromptDelivery::Stdin, "stdin"),
    ] {
        let err = build_tool_command_with_prompt_delivery(
            AgentBinary::Amplifier,
            Path::new("/tmp"),
            &[],
            &prompt,
            requested,
        )
        .expect_err("Amplifier must reject unsupported explicit long-form delivery modes");

        assert_eq!(
            err.kind(),
            ErrorKind::InvalidInput,
            "unsupported Amplifier prompt delivery should be a deterministic caller error"
        );
        let message = err.to_string();
        assert!(
            message.contains("Amplifier")
                && message.contains("unsupported")
                && message.contains(mode_name),
            "error should name the unsupported Amplifier mode without leaking the prompt body; got: {message}"
        );
        assert!(
            !message.contains(&prompt[..256]),
            "unsupported-mode error must not leak prompt content"
        );
    }
}

#[test]
fn generic_long_form_builder_keeps_large_prompt_out_of_argv_when_caps_support_tempfile() {
    let prompt = synthetic_prompt();

    let delivered = build_command_with_prompt_delivery(
        OsStr::new("cat"),
        std::iter::empty::<&str>(),
        &prompt,
        PromptDelivery::Tempfile,
        amplihack_utils::prompt_delivery::DeliveryCaps::argv_and_tempfile(""),
    )
    .expect("generic delivery-aware command should build");

    assert_eq!(delivered.selected_mode, DeliveryMode::Tempfile);
    assert!(
        delivered.delivery_handle.tempfile_path().is_some(),
        "tempfile mode must return a live RAII handle"
    );
    assert!(
        !argv(&delivered.command)
            .iter()
            .any(|arg| arg.contains(&prompt[..256])),
        "large prompt must not appear in argv when tempfile is selected"
    );
}

#[test]
fn prompt_delivery_report_matches_doctor_order_and_effective_modes() {
    let report = prompt_delivery_report_for(PromptDelivery::Tempfile, PAYLOAD_SIZE);

    let binaries: Vec<_> = report.entries.iter().map(|entry| entry.binary).collect();
    assert_eq!(
        binaries,
        vec![
            AgentBinary::Claude,
            AgentBinary::Copilot,
            AgentBinary::Codex,
            AgentBinary::Amplifier,
        ],
        "doctor and launcher reports must use a stable binary order"
    );
    assert!(
        report
            .entries
            .iter()
            .all(|entry| entry.effective_mode == DeliveryMode::Argv),
        "current verified launcher contracts are argv-only, so tempfile requests degrade to argv"
    );
    assert!(
        report.entries.iter().all(|entry| !entry.warning.is_empty()),
        "every argv-only binary should explain an unsupported tempfile request"
    );
}
