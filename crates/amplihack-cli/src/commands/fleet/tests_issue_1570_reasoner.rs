//! Issue #1570 (finding 2): the fleet reasoner bounds its untrusted input in
//! `to_prompt_context` and hands the prompt to `claude -p` on stdin.
//!
//! Contract: `docs/reference/large-payload-delivery.md`, "Fleet reasoner
//! bounds".

use super::*;
use crate::test_support::{ClearedGraphDbEnv, home_env_lock};
use amplihack_utils::root_sandbox::{SKIP_PERMISSIONS_FLAG, SkipPermissionsEnv};
use std::os::unix::fs::PermissionsExt;

const ARG_MAX_STRLEN: usize = 131_072;
/// Room for the fixed labels and short fields around a bounded section.
const SLACK: usize = 4 * 1024;

fn write_executable(path: &Path, content: &str) {
    fs::write(path, content).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn context() -> SessionContext {
    SessionContext::new("vm-1", "sess-1", "task", "").unwrap()
}

/// 5,000 numbered lines, about 1 MiB in total.
fn huge_scrollback() -> String {
    let pad = "x".repeat(200);
    let text = (0..5000)
        .map(|i| format!("line-{i:05} {pad}"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.len() >= 1024 * 1024);
    text
}

#[test]
fn reasoner_bound_constants_match_the_documented_values() {
    assert_eq!(REASONER_CAPTURE_LINES, 2000);
    assert_eq!(REASONER_SCROLLBACK_MAX_BYTES, 48 * 1024);
    assert_eq!(REASONER_TRANSCRIPT_MAX_MESSAGES, 200);
    assert_eq!(REASONER_TRANSCRIPT_MESSAGE_MAX_BYTES, 4 * 1024);
    assert_eq!(REASONER_TRANSCRIPT_MAX_BYTES, 64 * 1024);
}

#[test]
fn gather_captures_only_the_last_2000_lines() {
    let temp = tempfile::tempdir().unwrap();
    let azlin = temp.path().join("azlin");
    let record = temp.path().join("argv");
    write_executable(
        &azlin,
        &format!(
            "#!/bin/sh\nfor a in \"$@\"; do last=$a; done\nprintf '%s' \"$last\" > '{}'\nprintf '===TMUX===\\nhello\\n===END===\\n'\n",
            record.display()
        ),
    );
    gather_session_context(&azlin, "vm-1", "sess-1", "", "", None).unwrap();
    let gather_cmd = fs::read_to_string(&record).unwrap();
    assert!(gather_cmd.contains("capture-pane"), "{gather_cmd}");
    assert!(gather_cmd.contains("-p -S -2000"), "{gather_cmd}");
    assert!(
        !gather_cmd.contains("-S - "),
        "unbounded capture: {gather_cmd}"
    );
}

#[test]
fn scrollback_is_bounded_to_its_tail_with_a_bounded_label() {
    let capture = huge_scrollback();
    let mut ctx = context();
    ctx.tmux_capture = capture.clone();
    let prompt = ctx.to_prompt_context();
    assert!(prompt.contains("recent scrollback (bounded)"), "label");
    assert!(!prompt.contains("full scrollback"), "old label");
    assert!(
        prompt.len() <= REASONER_SCROLLBACK_MAX_BYTES + SLACK,
        "prompt is {} bytes",
        prompt.len()
    );
    assert!(
        prompt.contains(&capture[capture.len() - 1000..]),
        "tail kept"
    );
    assert!(prompt.contains("line-04999"), "most recent line kept");
    assert!(!prompt.contains("line-00000"), "oldest line dropped");
}

#[test]
fn multibyte_scrollback_is_bounded_without_panicking() {
    let mut ctx = context();
    ctx.tmux_capture = "é日🚀".repeat(200_000);
    let prompt = ctx.to_prompt_context();
    assert!(prompt.len() <= REASONER_SCROLLBACK_MAX_BYTES + SLACK);
    assert!(prompt.ends_with("é日🚀") || prompt.contains("é日🚀é日🚀"));
}

#[test]
fn cached_tmux_capture_is_bounded_as_well() {
    let temp = tempfile::tempdir().unwrap();
    let azlin = temp.path().join("azlin");
    write_executable(&azlin, "#!/bin/sh\nexit 1\n");
    let capture = huge_scrollback();
    let ctx = gather_session_context(&azlin, "vm-1", "sess-1", "", "", Some(&capture)).unwrap();
    let prompt = ctx.to_prompt_context();
    assert!(
        prompt.len() <= REASONER_SCROLLBACK_MAX_BYTES + SLACK,
        "cached capture produced a {} byte prompt",
        prompt.len()
    );
    assert!(prompt.contains("line-04999"));
}

#[test]
fn transcript_keeps_only_the_most_recent_200_messages() {
    let mut ctx = context();
    ctx.transcript_summary = (0..5000)
        .map(|i| format!("msg-{i:05} short message"))
        .collect::<Vec<_>>()
        .join("\n");
    let prompt = ctx.to_prompt_context();
    assert!(prompt.contains("msg-04999"), "newest message kept");
    assert!(prompt.contains("msg-04800"), "200th newest message kept");
    assert!(
        !prompt.contains("msg-04799"),
        "201st newest message dropped"
    );
    assert!(!prompt.contains("msg-00000"), "oldest message dropped");
    assert!(prompt.matches("msg-").count() <= REASONER_TRANSCRIPT_MAX_MESSAGES);
}

#[test]
fn each_transcript_message_keeps_only_its_head() {
    let mut ctx = context();
    ctx.transcript_summary = format!("HEAD-MARK{}TAIL-MARK", "y".repeat(100 * 1024));
    let prompt = ctx.to_prompt_context();
    assert!(prompt.contains("HEAD-MARK"), "head of the message kept");
    assert!(
        !prompt.contains("TAIL-MARK"),
        "message must be cut to its head"
    );
    assert!(prompt.len() <= REASONER_TRANSCRIPT_MESSAGE_MAX_BYTES + SLACK);
}

#[test]
fn whole_transcript_keeps_its_most_recent_64_kib() {
    let mut ctx = context();
    let body = "z".repeat(10 * 1024);
    ctx.transcript_summary = (0..300)
        .map(|i| format!("msg-{i:05} {body}"))
        .collect::<Vec<_>>()
        .join("\n");
    let prompt = ctx.to_prompt_context();
    assert!(
        prompt.len() <= REASONER_TRANSCRIPT_MAX_BYTES + SLACK,
        "prompt is {} bytes",
        prompt.len()
    );
    assert!(prompt.contains("msg-00299"), "most recent message kept");
    assert!(!prompt.contains("msg-00100"), "older messages dropped");
}

#[test]
fn reasoner_argv_is_exactly_skip_permissions_and_print() {
    let path = Path::new("/usr/bin/claude");
    for decision in [
        SkipPermissionsEnv::NotRoot,
        SkipPermissionsEnv::AlreadySandboxed,
        SkipPermissionsEnv::SetSandbox {
            signal: "/.dockerenv",
        },
    ] {
        let command = reasoning::reasoner_command(path, &decision).unwrap();
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(args, [SKIP_PERMISSIONS_FLAG, "-p"], "{decision:?}");
    }
}

#[test]
fn complete_sends_the_prompt_to_claude_on_stdin() {
    let _home_guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _graph_guard = ClearedGraphDbEnv::new();
    let temp = tempfile::tempdir().unwrap();
    let claude = temp.path().join("claude");
    let out = temp.path().join("out");
    fs::create_dir_all(&out).unwrap();
    write_executable(
        &claude,
        &format!(
            "#!/bin/sh\nout='{}'\nprintf '%s\\0' \"$@\" > \"$out/argv\"\ncat > \"$out/stdin\"\nprintf '%s\\n' '{{\"action\":\"wait\"}}'\n",
            out.display()
        ),
    );
    let mut prompt = String::from("REASONER-1570 '; touch /tmp/pwned; ' é🚀\n");
    while prompt.len() < 200 * 1024 {
        prompt.push_str("untrusted scrollback line with $VAR and `ticks`\n");
    }
    let cwd = crate::test_support::CwdGuard::set(temp.path()).unwrap();
    let result =
        NativeReasonerBackend::Claude(claude, || SkipPermissionsEnv::NotRoot).complete(&prompt);
    drop(cwd);
    let output = result.expect("a 200 KiB prompt must reach claude (no E2BIG)");
    assert!(output.contains("\"action\":\"wait\""), "{output}");

    let raw_argv = fs::read(out.join("argv")).unwrap();
    let argv = raw_argv
        .split(|b| *b == 0)
        .filter(|arg| !arg.is_empty())
        .map(|arg| String::from_utf8_lossy(arg).into_owned())
        .collect::<Vec<_>>();
    assert_eq!(argv, [SKIP_PERMISSIONS_FLAG, "-p"]);
    assert!(raw_argv.len() < ARG_MAX_STRLEN);
    let stdin = fs::read(out.join("stdin")).unwrap();
    assert_eq!(stdin.len(), prompt.len(), "stdin length");
    assert!(
        stdin == prompt.as_bytes(),
        "stdin must be the prompt, byte for byte"
    );
}
