//! Red-phase CLI doctor tests for prompt-delivery diagnostics.
//!
//! The doctor surface must report modes, static capabilities, effective
//! degradation, and warnings without printing raw prompt data.

use amplihack_cli::commands::doctor::{
    PromptDeliveryDoctorOptions, render_prompt_delivery_diagnostics,
};
use amplihack_utils::prompt_delivery::PromptDelivery;

#[test]
fn doctor_reports_requested_mode_capabilities_effective_modes_and_warnings() {
    let output = render_prompt_delivery_diagnostics(PromptDeliveryDoctorOptions {
        requested: PromptDelivery::Tempfile,
    });

    assert!(output.contains("Prompt delivery"));
    assert!(output.contains("requested: tempfile"));
    assert!(output.contains("auto: stdin, then file-reference, then tempfile; never argv"));

    for binary in ["claude", "copilot", "codex", "amplifier"] {
        assert!(
            output.contains(binary),
            "doctor prompt-delivery section must include {binary}; output:\n{output}"
        );
        assert!(
            output.contains("capabilities: argv"),
            "current verified contracts must be reported as argv-only; output:\n{output}"
        );
    }

    // Issue #2030: an unsupported tempfile request degrades only to channels
    // that keep the prompt out of argv.
    assert!(
        !output.contains("effective: argv"),
        "no binary degrades to argv; output:\n{output}"
    );
    assert!(
        !output.contains("effective: unavailable"),
        "every verified binary has a safe channel; output:\n{output}"
    );
    assert!(output.contains("effective: stdin"));
    assert!(output.contains("effective: file-reference"));
    assert!(output.contains("requested tempfile is unsupported; degrading to stdin"));
}

#[test]
fn doctor_output_never_contains_raw_prompt_material() {
    let output = render_prompt_delivery_diagnostics(PromptDeliveryDoctorOptions {
        requested: PromptDelivery::Stdin,
    });

    for forbidden in [
        "don't shell-quote",
        "$PATH",
        "apostrophe",
        "64 KiB prompt",
        "simard-prompt-",
    ] {
        assert!(
            !output.contains(forbidden),
            "doctor diagnostics must not print prompt bytes or tempfile paths; found {forbidden:?} in:\n{output}"
        );
    }
}

#[test]
fn doctor_help_lists_prompt_delivery_environment_override() {
    let help = amplihack_cli::commands::doctor::render_doctor_help();

    assert!(help.contains("AMPLIHACK_PROMPT_DELIVERY"));
    assert!(help.contains("auto"));
    assert!(help.contains("tempfile"));
    assert!(help.contains("stdin"));
    // Issue #2030 R1.2: `argv` is no longer an accepted value; the help must
    // not offer it.
    assert!(
        help.contains("AMPLIHACK_PROMPT_DELIVERY=auto|tempfile|stdin"),
        "{help}"
    );
    assert!(!help.contains("auto|argv"), "{help}");
}
