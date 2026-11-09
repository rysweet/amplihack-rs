//! Downstream prompt-delivery contracts for Simard/RabbitHole-style prompts.
//!
//! These scenarios treat prompts as data: multiline instructions, nested
//! markdown fences, quotes, shell metacharacters, and long payloads must not be
//! reinterpreted by a shell or split across argv elements.

use std::io::Write;
use std::process::{Command, Stdio};

use amplihack_utils::prompt_delivery::{
    DeliveryCaps, DeliveryMode, PromptDelivery, deliver, sanitize_prompt_nul, try_select_mode,
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

/// Issue #2030 R1.2: an explicit `Argv` request is treated as `Auto`, so on
/// a binary with argv and a `--prompt-file` flag the prompt goes to the
/// tempfile. Returns the handle, the file bytes and the argv.
fn deliver_argv_request(
    prompt: &str,
) -> (
    amplihack_utils::prompt_delivery::DeliveryHandle,
    Vec<u8>,
    Vec<String>,
) {
    let mut command = Command::new("/bin/true");
    let handle = deliver(
        &mut command,
        prompt,
        PromptDelivery::Argv,
        &DeliveryCaps::argv_and_tempfile("--prompt-file"),
    )
    .expect("an Argv request degrades to the tempfile, never argv");
    assert_eq!(handle.mode(), DeliveryMode::Tempfile);
    let bytes = std::fs::read(handle.tempfile_path().expect("tempfile path"))
        .expect("read live prompt tempfile");
    let args = argv(&command);
    (handle, bytes, args)
}

#[test]
fn argv_request_keeps_nested_shell_sensitive_prompt_out_of_argv() {
    let prompt = rabbithole_prompt();
    let (_handle, bytes, args) = deliver_argv_request(&prompt);
    assert_eq!(
        bytes,
        prompt.as_bytes(),
        "the prompt file holds the exact bytes"
    );
    assert!(
        args.iter().all(|a| !a.contains("RabbitHole workflow")),
        "no part of the prompt may reach argv: {args:?}"
    );
    assert_eq!(args.len(), 2, "only `--prompt-file <path>`: {args:?}");
    assert_eq!(args[0], "--prompt-file");
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
        supports_file_reference: false,
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
fn stdin_delivery_does_not_abort_on_nul_and_payload_is_spawn_safe() {
    // Issue #898: even the stdin path must not abort when the prompt carries a
    // NUL. `deliver` must succeed; consumers build the stdin payload via
    // `sanitize_prompt_nul`, which must yield NUL-free, spawn-safe bytes.
    let prompt = "stdin prompt with \0 embedded NUL byte";
    let caps = DeliveryCaps {
        supports_argv: true,
        supports_tempfile: false,
        supports_stdin: true,
        supports_file_reference: false,
        tempfile_flag: None,
    };
    let mut command = Command::new("/bin/cat");

    let handle = deliver(&mut command, prompt, PromptDelivery::Stdin, &caps)
        .expect("stdin delivery must not abort on a NUL byte");
    assert_eq!(handle.mode(), DeliveryMode::Stdin);
    command.stdout(Stdio::piped());

    // Consumers sanitize the payload they write to the child's stdin.
    let payload = sanitize_prompt_nul(prompt).into_owned();
    assert!(
        !payload.as_bytes().contains(&0),
        "the stdin payload must be NUL-free before it reaches the child"
    );

    let mut child = command.spawn().expect("spawn cat");
    child
        .stdin
        .as_mut()
        .expect("stdin must be piped")
        .write_all(payload.as_bytes())
        .expect("write sanitized prompt to child stdin");
    drop(child.stdin.take());

    let output = child.wait_with_output().expect("wait for cat output");
    assert!(output.status.success());
    assert_eq!(output.stdout, payload.as_bytes());
}

#[test]
fn auto_promotes_long_downstream_prompt_to_tempfile_when_supported() {
    // The selector takes no prompt size: a long prompt gets the same answer.
    let caps = DeliveryCaps::argv_and_tempfile("--prompt-file");

    assert_eq!(
        try_select_mode(PromptDelivery::Auto, &caps).unwrap(),
        DeliveryMode::Tempfile,
        "long downstream prompts must leave argv when a verified tempfile contract exists"
    );
}

#[test]
fn argv_request_strips_nul_bytes_and_continues() {
    // Issue #898: a single NUL byte in agent/bash step output must NOT abort the
    // whole run. `deliver` strips NUL bytes and continues. Since #2030 the
    // prompt goes to a file even when argv is requested.
    let prompt = "RabbitHole prompt with embedded NUL: \0 must be stripped, not fatal";
    let expected = "RabbitHole prompt with embedded NUL:  must be stripped, not fatal";
    let (_handle, bytes, args) = deliver_argv_request(prompt);
    assert!(
        !bytes.contains(&0),
        "the delivered file must be free of NUL bytes"
    );
    assert_eq!(
        bytes,
        expected.as_bytes(),
        "only NUL bytes may be removed; every other byte must be preserved in order"
    );
    assert!(args.iter().all(|a| !a.contains("RabbitHole")), "{args:?}");
}

#[test]
fn argv_request_strips_multiple_nul_bytes_preserving_order() {
    let (_handle, bytes, args) = deliver_argv_request("a\0b\0c");
    assert_eq!(bytes, b"abc");
    assert!(args.iter().all(|a| a != "abc"), "{args:?}");
}

#[test]
fn tempfile_delivery_strips_nul_bytes_from_written_prompt() {
    // The tempfile path must also be NUL-free so downstream consumers reading the
    // file never see a stray NUL. Strip uniformly in `deliver`, all modes.
    let prompt = format!("{}\0embedded-nul-tail", long_simard_prompt());
    let caps = DeliveryCaps::argv_and_tempfile("--prompt-file");
    let mut command = Command::new("/bin/true");

    let handle = deliver(&mut command, &prompt, PromptDelivery::Tempfile, &caps)
        .expect("tempfile delivery must strip NUL bytes and succeed");

    assert_eq!(handle.mode(), DeliveryMode::Tempfile);
    let path = handle
        .tempfile_path()
        .expect("tempfile mode must expose the temporary prompt file path");
    let file_bytes = std::fs::read(path).expect("read live prompt tempfile");
    assert!(
        !file_bytes.contains(&0),
        "the written tempfile must be free of NUL bytes"
    );
    assert_eq!(
        file_bytes,
        prompt.replace('\0', "").as_bytes(),
        "tempfile contents must equal the prompt with only NUL bytes removed"
    );
}

#[test]
fn deliver_is_lossless_for_prompts_without_nul() {
    // Prompts with no NUL must be delivered byte-for-byte unchanged.
    let prompt = rabbithole_prompt();
    let (_handle, bytes, _args) = deliver_argv_request(&prompt);
    assert_eq!(
        bytes,
        prompt.as_bytes(),
        "a NUL-free prompt must be delivered byte-for-byte unchanged"
    );
}

#[test]
fn sanitize_prompt_nul_is_zero_copy_when_no_nul_present() {
    use std::borrow::Cow;

    let prompt = "no nul bytes here — leave untouched";
    match sanitize_prompt_nul(prompt) {
        Cow::Borrowed(s) => assert_eq!(s, prompt),
        Cow::Owned(_) => panic!("sanitize_prompt_nul must take the zero-copy borrowed fast path"),
    }
}

#[test]
fn sanitize_prompt_nul_removes_only_nul_bytes_in_order() {
    use std::borrow::Cow;

    let prompt = "\0start\0mid\0end\0";
    match sanitize_prompt_nul(prompt) {
        Cow::Owned(s) => assert_eq!(s, "startmidend"),
        Cow::Borrowed(_) => panic!("sanitize_prompt_nul must allocate when NUL bytes are present"),
    }
}
