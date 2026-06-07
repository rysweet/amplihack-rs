//! Downstream prompt-delivery contracts for Simard/RabbitHole-style prompts.
//!
//! These scenarios treat prompts as data: multiline instructions, nested
//! markdown fences, quotes, shell metacharacters, and long payloads must not be
//! reinterpreted by a shell or split across argv elements.

use std::io::Write;
use std::process::{Command, Stdio};

use amplihack_utils::prompt_delivery::{
    DeliveryCaps, DeliveryMode, PromptDelivery, deliver, select_mode,
};

fn rabbithole_prompt() -> String {
    [
        "# RabbitHole workflow",
        "",
        "Do not execute this as shell:",
        "```bash",
        "echo '$PATH' && rm -rf /tmp/not-real && printf \"%s\\n\" \"$(whoami)\"",
        "```",
        "",
        "Nested JSON payload:",
        "```json",
        r#"{"quote":"'","dollar":"$HOME","backtick":"`uname`","lines":["a","b"]}"#,
        "```",
    ]
    .join("\n")
}

fn long_simard_prompt() -> String {
    let pattern =
        "Simard/RabbitHole payload: keep '$PATH', `backticks`, \"quotes\", and newlines intact.\n";
    let mut prompt = String::with_capacity(32 * 1024);
    while prompt.len() < 32 * 1024 {
        prompt.push_str(pattern);
    }
    prompt
}

fn argv(command: &Command) -> Vec<String> {
    command
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect()
}

#[test]
fn argv_delivery_preserves_nested_shell_sensitive_prompt_as_one_argument() {
    let prompt = rabbithole_prompt();
    let mut command = Command::new("/bin/true");
    let handle = deliver(
        &mut command,
        &prompt,
        PromptDelivery::Argv,
        &DeliveryCaps::argv_only(),
    )
    .expect("argv delivery should accept shell-sensitive prompt text as data");

    assert_eq!(handle.mode(), DeliveryMode::Argv);
    assert_eq!(
        argv(&command).iter().filter(|arg| *arg == &prompt).count(),
        1,
        "the full nested prompt must be passed exactly once as one argv element"
    );
}

#[test]
fn tempfile_delivery_preserves_long_prompt_bytes_and_keeps_prompt_out_of_argv() {
    let prompt = long_simard_prompt();
    let caps = DeliveryCaps::argv_and_tempfile("--prompt-file");
    let mut command = Command::new("/bin/true");

    let handle = deliver(&mut command, &prompt, PromptDelivery::Tempfile, &caps)
        .expect("tempfile delivery should support long prompt payloads");

    assert_eq!(handle.mode(), DeliveryMode::Tempfile);
    let path = handle
        .tempfile_path()
        .expect("tempfile mode must expose the temporary prompt file path");
    let file_bytes = std::fs::read(path).expect("read live prompt tempfile");
    assert_eq!(file_bytes, prompt.as_bytes());
    assert!(
        !argv(&command)
            .iter()
            .any(|arg| arg.contains("Simard/RabbitHole payload")),
        "tempfile mode must keep the prompt body out of argv"
    );
}

#[test]
fn stdin_delivery_can_round_trip_representative_prompt_without_shell_quoting() {
    let prompt = rabbithole_prompt();
    let caps = DeliveryCaps {
        supports_argv: true,
        supports_tempfile: false,
        supports_stdin: true,
        tempfile_flag: None,
    };
    let mut command = Command::new("/bin/cat");
    let handle =
        deliver(&mut command, &prompt, PromptDelivery::Stdin, &caps).expect("stdin delivery");
    assert_eq!(handle.mode(), DeliveryMode::Stdin);
    command.stdout(Stdio::piped());

    let mut child = command.spawn().expect("spawn cat");
    child
        .stdin
        .as_mut()
        .expect("stdin must be piped")
        .write_all(prompt.as_bytes())
        .expect("write prompt to child stdin");
    drop(child.stdin.take());

    let output = child.wait_with_output().expect("wait for cat output");
    assert!(output.status.success());
    assert_eq!(output.stdout, prompt.as_bytes());
}

#[test]
fn auto_promotes_long_downstream_prompt_to_tempfile_when_supported() {
    let prompt = long_simard_prompt();
    let caps = DeliveryCaps::argv_and_tempfile("--prompt-file");

    assert_eq!(
        select_mode(PromptDelivery::Auto, prompt.len(), &caps),
        DeliveryMode::Tempfile,
        "long downstream prompts must leave argv when a verified tempfile contract exists"
    );
}

#[test]
fn argv_delivery_rejects_nul_bytes_before_child_spawn() {
    let prompt = "RabbitHole prompt with embedded NUL: \0 must not reach argv";
    let mut command = Command::new("/bin/true");

    let error = deliver(
        &mut command,
        prompt,
        PromptDelivery::Argv,
        &DeliveryCaps::argv_only(),
    )
    .expect_err("argv delivery must reject NUL bytes before the later child spawn fails");

    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    assert!(
        error.to_string().contains("NUL"),
        "error should name the invalid NUL byte without leaking the full prompt: {error}"
    );
}
