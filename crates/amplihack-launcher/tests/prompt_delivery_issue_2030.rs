//! Issue #2030 (red phase): launcher command builders never put a prompt in
//! argv under `Auto`, whatever its size.
//!
//! Spec: `docs/reference/agent-prompt-channels.md` ("Launch paths", "Delivery
//! policy"). Codex: `exec` reads stdin through a trailing `-`; interactive
//! Codex gets the file-reference pointer (D5). Copilot always gets the pointer
//! (D6). Claude reads stdin with a bare `-p`.

use std::path::Path;

use amplihack_launcher::flag_matrix::{AgentBinary, flags_for, prompt_delivery_caps_for};
use amplihack_launcher::prompt_delivery::build_tool_command_with_prompt_delivery;
use amplihack_utils::prompt_delivery::{DeliveryMode, FILE_REFERENCE_PREFIX, PromptDelivery};

const SENTINEL: &str = "SENTINEL-2030-short-brief";

fn argv(command: &std::process::Command) -> Vec<String> {
    command
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect()
}

fn assert_no_prompt(args: &[String]) {
    assert!(
        args.iter().all(|a| !a.contains(SENTINEL)),
        "prompt text in argv: {args:?}"
    );
}

#[test]
fn codex_row_advertises_stdin_and_file_reference() {
    let codex = prompt_delivery_caps_for(AgentBinary::Codex);
    assert!(codex.supports_stdin, "codex exec reads `-` from stdin");
    assert!(
        codex.supports_file_reference,
        "interactive codex takes the pointer"
    );
}

#[test]
fn claude_append_prompt_doc_names_the_file_flag() {
    // R6: the matrix comment on supports_append_prompt names the -file flag
    // and its minimum version.
    assert!(flags_for(AgentBinary::Claude).supports_append_prompt);
    let src =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/flag_matrix.rs"))
            .unwrap();
    assert!(src.contains("--append-system-prompt-file"));
    assert!(src.contains("2.1.319"));
}

#[test]
fn short_claude_prompt_goes_to_stdin() {
    let d = build_tool_command_with_prompt_delivery(
        AgentBinary::Claude,
        Path::new("."),
        &[],
        SENTINEL,
        PromptDelivery::Auto,
    )
    .unwrap();
    assert_eq!(d.selected_mode, DeliveryMode::Stdin);
    let args = argv(&d.command);
    assert_no_prompt(&args);
    assert_eq!(args.last().map(String::as_str), Some("-p"), "{args:?}");
    assert_eq!(d.stdin_payload.as_deref(), Some(SENTINEL.as_bytes()));
}

#[test]
fn short_copilot_and_amplifier_prompts_use_the_pointer() {
    for binary in [AgentBinary::Copilot, AgentBinary::Amplifier] {
        let d = build_tool_command_with_prompt_delivery(
            binary,
            Path::new("."),
            &[],
            SENTINEL,
            PromptDelivery::Auto,
        )
        .unwrap();
        assert_eq!(d.selected_mode, DeliveryMode::FileReference, "{binary}");
        let args = argv(&d.command);
        assert_no_prompt(&args);
        assert_eq!(
            args.iter()
                .filter(|a| a.starts_with(FILE_REFERENCE_PREFIX))
                .count(),
            1,
            "{binary}: {args:?}"
        );
    }
}

#[test]
fn codex_exec_ends_with_dash_and_reads_stdin() {
    let d = build_tool_command_with_prompt_delivery(
        AgentBinary::Codex,
        Path::new("."),
        &["exec".to_string()],
        SENTINEL,
        PromptDelivery::Auto,
    )
    .unwrap();
    let args = argv(&d.command);
    assert_no_prompt(&args);
    assert_eq!(args.last().map(String::as_str), Some("-"));
    assert_eq!(d.selected_mode, DeliveryMode::Stdin);
}

#[test]
fn interactive_codex_short_prompt_uses_the_pointer_not_argv() {
    let d = build_tool_command_with_prompt_delivery(
        AgentBinary::Codex,
        Path::new("."),
        &[],
        SENTINEL,
        PromptDelivery::Auto,
    )
    .expect("interactive codex has a safe channel");
    assert_eq!(d.selected_mode, DeliveryMode::FileReference);
    let args = argv(&d.command);
    assert_no_prompt(&args);
    assert!(
        args.iter().any(|a| a.starts_with(FILE_REFERENCE_PREFIX)),
        "{args:?}"
    );
    assert!(
        d.stdin_payload.is_none(),
        "interactive stdin belongs to the terminal"
    );
    let path = d.delivery_handle.tempfile_path().unwrap().to_path_buf();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), SENTINEL);
    drop(d);
    assert!(!path.exists());
}

#[test]
fn interactive_codex_large_prompt_no_longer_hits_an_argv_limit() {
    // The 96 KiB interactive cap existed only because the prompt rode argv.
    let prompt = format!("{SENTINEL}{}", "x".repeat(200 * 1024));
    let d = build_tool_command_with_prompt_delivery(
        AgentBinary::Codex,
        Path::new("."),
        &[],
        &prompt,
        PromptDelivery::Auto,
    )
    .expect("the pointer has no size limit");
    assert_eq!(d.selected_mode, DeliveryMode::FileReference);
    assert_no_prompt(&argv(&d.command));
}

#[test]
fn explicit_argv_request_for_claude_uses_stdin() {
    // R1.2: an `Argv` request is treated as `Auto`. Before R1.2 it was
    // honoured and put the prompt on Claude's command line.
    let d = build_tool_command_with_prompt_delivery(
        AgentBinary::Claude,
        Path::new("."),
        &[],
        SENTINEL,
        PromptDelivery::Argv,
    )
    .unwrap();
    assert_eq!(d.selected_mode, DeliveryMode::Stdin);
    assert_no_prompt(&argv(&d.command));
}
