//! Issue #3162: which launch forms take operator guidance, and how.
//!
//! `guidance_route` is pure: it takes whether the caller's stdin may hold a
//! prompt instead of looking at the process. Whether it did is decided later
//! by reading it (the forwarder's `saw_text`). The runner starts a Claude agent step as
//! `claude -- --system-prompt-file <file> -p --add-dir .` with the prompt
//! already on stdin, so the rewrite produces no `stdin_payload` and the block
//! has to follow the caller's stdin (`StdinTrailer`), as it already does for
//! Codex `exec … -`.
//!
//! Contract used here:
//! - `GuidanceRoute::{None, Prompt, StdinTrailer, Undeliverable(DropReason)}`;
//! - `guidance_route(tool, &guidance, &rewritten, &args, stdin_may_hold_prompt)`;
//! - `DropReason::{NoPromptOnStdin, LaunchFormUnsupported, TrailerNotWritten,
//!   PromptFileNotCopied}` with `code()` returning `no_prompt_on_stdin`,
//!   `launch_form_unsupported`, `trailer_not_written` and
//!   `prompt_file_not_copied`;
//! - `step_label(tool, &args) -> StepLabel` with `agent: Option<String>`.
//!
//! Each case is rewritten by the rewriter production uses for its tool
//! (`launch/mod.rs`), with the guidance block passed to the Copilot rewrite
//! as the launcher passes it.
//!
//! The tests compare enum values and strings that are identifiers. None reads
//! prose.

use super::codex_copilot_rewrite::{rewrite_codex_passthrough, rewrite_copilot_passthrough};
use super::passthrough_rewrite::{PromptBlock, rewrite_claude_passthrough};
use super::*;
use crate::run_guidance::{DropReason, GuidanceForLaunch, append_entry};
use amplihack_utils::prompt_delivery::{FILE_REFERENCE_PREFIX, FILE_REFERENCE_TEMPLATE};
use std::os::unix::fs::PermissionsExt;

fn s(items: &[&str]) -> Vec<String> {
    items.iter().map(|a| (*a).to_string()).collect()
}

/// A guidance folder with one entry, and a launch holding it.
fn with_guidance() -> (tempfile::TempDir, GuidanceForLaunch) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let dir = root.join("runs").join("r-1").join("guidance");
    for d in [&root, &root.join("runs"), &root.join("runs").join("r-1")] {
        std::fs::create_dir_all(d).unwrap();
        std::fs::set_permissions(d, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    append_entry(&dir, "place and launch nothing in this pass", "alice").unwrap();
    (tmp, GuidanceForLaunch::from_dirs(vec![dir]))
}

/// The arguments as the launcher rewrites them for `tool`, with Copilot
/// able to open a private file (`--add-dir`, no gateway).
fn rewrite(tool: &str, args: &[String], g: &GuidanceForLaunch) -> passthrough_rewrite::Rewritten {
    match tool {
        "codex" => rewrite_codex_passthrough(args).unwrap(),
        "copilot" => rewrite_copilot_passthrough(args, true, false, g.block()).unwrap(),
        _ => rewrite_claude_passthrough(args).unwrap(),
    }
}

fn route(tool: &str, args: &[&str], stdin_may_hold_prompt: bool) -> GuidanceRoute {
    let (_tmp, g) = with_guidance();
    let args = s(args);
    let rewritten = rewrite(tool, &args, &g);
    guidance_route(tool, &g, &rewritten, &args, stdin_may_hold_prompt)
}

const RUNNER_FORM: [&str; 5] = ["--system-prompt-file", "/f", "-p", "--add-dir", "."];

#[test]
fn the_runner_form_with_a_prompt_on_stdin_takes_a_stdin_trailer() {
    assert_eq!(
        route("claude", &RUNNER_FORM, true),
        GuidanceRoute::StdinTrailer
    );
    assert_eq!(
        route("claude", &["--print", "--add-dir", "."], true),
        GuidanceRoute::StdinTrailer
    );
}

#[test]
fn the_runner_form_with_nothing_on_stdin_is_a_recorded_drop() {
    assert_eq!(
        route("claude", &RUNNER_FORM, false),
        GuidanceRoute::Undeliverable(DropReason::NoPromptOnStdin)
    );
}

#[test]
fn a_prompt_in_argv_still_goes_through_the_prompt_route_whatever_stdin_holds() {
    for stdin in [true, false] {
        assert_eq!(
            route("claude", &["-p", "do the thing"], stdin),
            GuidanceRoute::Prompt
        );
    }
}

#[test]
fn claude_without_print_mode_cannot_take_guidance() {
    for args in [&["--add-dir", "."][..], &[][..], &["--resume", "x"][..]] {
        assert!(
            matches!(route("claude", args, true), GuidanceRoute::Undeliverable(_)),
            "{args:?}"
        );
    }
}

#[test]
fn codex_exec_dash_keeps_its_trailer_and_other_codex_forms_are_dropped() {
    assert_eq!(
        route("codex", &["exec", "--output-last-message", "/m", "-"], true),
        GuidanceRoute::StdinTrailer
    );
    assert!(matches!(
        route("codex", &["exec", "do the thing"], true),
        GuidanceRoute::Undeliverable(_)
    ));
}

#[test]
fn copilot_with_prompt_text_in_argv_takes_the_prompt_route() {
    assert_eq!(
        route("copilot", &["-p", "do the thing"], true),
        GuidanceRoute::Prompt
    );
}

#[test]
fn copilot_without_prompt_text_is_never_reported_as_delivered() {
    // With nothing on stdin, neither form has a prompt the block could
    // follow: `-i` is an interactive session, and a launch with no prompt
    // flag reads its prompt from stdin. The same launch with a prompt on
    // stdin takes a `StdinTrailer` (`guidance_route.rs`, the Copilot rows of
    // the table), covered with a stand-in Copilot by
    // `tests/issue_3162_copilot_stdin_launcher.rs`.
    for args in [&["-i", "talk"][..], &["--model", "m"][..]] {
        assert!(
            matches!(
                route("copilot", args, false),
                GuidanceRoute::Undeliverable(_)
            ),
            "{args:?}"
        );
    }
}

/// A prompt file as the runner writes it, and the arguments the pinned
/// runner (2833a0b `cli_subprocess.rs`) starts a Copilot step with.
fn runner_copilot_step(dir: &std::path::Path, prompt: &str) -> (std::path::PathBuf, Vec<String>) {
    let file = dir.join("agent-system-prompt.md");
    std::fs::write(&file, prompt).unwrap();
    let pointer = FILE_REFERENCE_TEMPLATE.replace("{path}", file.to_str().unwrap());
    let args = s(&[
        "--allow-all-tools",
        "--add-dir",
        dir.to_str().unwrap(),
        "-p",
        &pointer,
        "--add-dir",
        ".",
    ]);
    (file, args)
}

/// The prompt file the `-p` pointer in `args` names.
fn pointed_file(args: &[String]) -> std::path::PathBuf {
    let at = args.iter().position(|a| a == "-p").expect("a -p flag");
    let (_, suffix) = FILE_REFERENCE_TEMPLATE.split_once("{path}").unwrap();
    let path = args[at + 1]
        .strip_prefix(FILE_REFERENCE_PREFIX)
        .and_then(|rest| rest.strip_suffix(suffix))
        .expect("a pointer sentence");
    std::path::PathBuf::from(path)
}

#[test]
fn the_runner_copilot_form_takes_the_block_in_a_private_copy_of_its_prompt_file() {
    let (_tmp, g) = with_guidance();
    let runner_dir = tempfile::tempdir().unwrap();
    let prompt = "SYSTEM PART\n\n# Task instructions\n\nRUNNER-TASK do the step";
    let (file, args) = runner_copilot_step(runner_dir.path(), prompt);

    let rewritten = rewrite_copilot_passthrough(&args, true, false, g.block()).unwrap();
    assert_eq!(rewritten.prompt_block, PromptBlock::Added);
    assert_eq!(
        guidance_route("copilot", &g, &rewritten, &args, false),
        GuidanceRoute::Prompt
    );

    // Copilot is pointed at a private copy, in a folder it is granted.
    let copy = pointed_file(&rewritten.args);
    assert_ne!(copy, file);
    assert_eq!(rewritten.args[0], "--add-dir");
    assert_eq!(
        std::path::Path::new(&rewritten.args[1]),
        copy.parent().unwrap()
    );
    let mode = std::fs::metadata(&copy).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);

    // The copy holds the step's prompt, then the block, and nothing after it.
    let block = g.block().unwrap();
    let text = std::fs::read_to_string(&copy).unwrap();
    assert!(text.starts_with(prompt), "{text:?}");
    assert!(text.trim_end().ends_with(block.trim_end()), "{text:?}");
    assert_eq!(text.matches(block.trim_end()).count(), 1, "{text:?}");

    // The runner's own file is never written.
    assert_eq!(std::fs::read_to_string(&file).unwrap(), prompt);
    // The runner's other flags are kept, in order.
    let kept: Vec<&str> = rewritten.args[2..]
        .iter()
        .map(String::as_str)
        .filter(|a| !a.starts_with(FILE_REFERENCE_PREFIX))
        .collect();
    assert_eq!(
        kept,
        [
            "--allow-all-tools",
            "--add-dir",
            runner_dir.path().to_str().unwrap(),
            "-p",
            "--add-dir",
            "."
        ]
    );
}

#[test]
fn a_runner_copilot_pointer_without_guidance_is_passed_on_unchanged() {
    let runner_dir = tempfile::tempdir().unwrap();
    let (_, args) = runner_copilot_step(runner_dir.path(), "RUNNER-TASK");
    let rewritten = rewrite_copilot_passthrough(&args, true, false, None).unwrap();
    assert_eq!(rewritten.args, args);
    assert_eq!(rewritten.prompt_block, PromptBlock::Absent);
}

#[test]
fn a_copilot_pointer_to_a_file_that_cannot_be_read_is_a_recorded_drop_and_unchanged() {
    let (_tmp, g) = with_guidance();
    let runner_dir = tempfile::tempdir().unwrap();
    let (file, args) = runner_copilot_step(runner_dir.path(), "RUNNER-TASK");
    std::fs::remove_file(&file).unwrap();

    let rewritten = rewrite_copilot_passthrough(&args, true, false, g.block()).unwrap();
    assert_eq!(
        rewritten.args, args,
        "the launch goes on as the caller gave it"
    );
    assert_eq!(
        guidance_route("copilot", &g, &rewritten, &args, false),
        GuidanceRoute::Undeliverable(DropReason::PromptFileNotCopied)
    );
}

#[test]
fn a_copilot_pointer_with_no_private_file_copilot_can_open_is_a_recorded_drop() {
    // Under the external gateway (no --add-dir) and without path access,
    // Copilot could not open a private copy: the pointer is passed on.
    let (_tmp, g) = with_guidance();
    let runner_dir = tempfile::tempdir().unwrap();
    let (_, args) = runner_copilot_step(runner_dir.path(), "RUNNER-TASK");
    let rewritten = rewrite_copilot_passthrough(&args, false, false, g.block()).unwrap();
    assert_eq!(rewritten.args, args);
    assert_eq!(
        guidance_route("copilot", &g, &rewritten, &args, false),
        GuidanceRoute::Undeliverable(DropReason::LaunchFormUnsupported)
    );
}

#[test]
fn a_copilot_pointer_with_path_access_and_no_add_dir_takes_the_block() {
    let (_tmp, g) = with_guidance();
    let runner_dir = tempfile::tempdir().unwrap();
    let (file, args) = runner_copilot_step(runner_dir.path(), "RUNNER-TASK");
    let rewritten = rewrite_copilot_passthrough(&args, false, true, g.block()).unwrap();
    assert_eq!(rewritten.prompt_block, PromptBlock::Added);
    assert_ne!(pointed_file(&rewritten.args), file);
    assert_eq!(
        rewritten.args[0], "--allow-all-tools",
        "no --add-dir is added"
    );
}

#[test]
fn claude_print_mode_with_a_json_input_stream_is_a_recorded_drop() {
    // Plain text after a stream of JSON lines would corrupt the stream.
    for args in [
        &[
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
        ][..],
        &["--print", "--input-format=stream-json"][..],
        &[
            "--system-prompt-file",
            "/f",
            "-p",
            "--input-format",
            "stream-json",
        ][..],
    ] {
        for stdin in [true, false] {
            assert_eq!(
                route("claude", args, stdin),
                GuidanceRoute::Undeliverable(DropReason::LaunchFormUnsupported),
                "{args:?}"
            );
        }
    }
    // An explicit text input format is the ordinary print-mode route.
    assert_eq!(
        route("claude", &["-p", "--input-format", "text"], true),
        GuidanceRoute::StdinTrailer
    );
}

#[test]
fn launch_form_labels_name_the_copilot_and_json_forms_without_argv_text() {
    let runner_dir = tempfile::tempdir().unwrap();
    let (_, args) = runner_copilot_step(runner_dir.path(), "RUNNER-TASK");
    let l = step_label("copilot", &args);
    assert_eq!(l.launch_form, "copilot -p <prompt file>");
    assert!(!l.launch_form.contains(runner_dir.path().to_str().unwrap()));
    assert_eq!(
        step_label("copilot", &s(&["-i", "talk"])).launch_form,
        "copilot -i"
    );
    assert_eq!(
        step_label("claude", &s(&["-p", "--input-format", "stream-json"])).launch_form,
        "claude, input format other than text"
    );
}

#[test]
fn no_guidance_is_no_route() {
    let tmp = tempfile::tempdir().unwrap();
    let empty = GuidanceForLaunch::from_dirs(vec![tmp.path().join("none")]);
    let args = s(&RUNNER_FORM);
    let rewritten = rewrite_claude_passthrough(&args).unwrap();
    assert_eq!(
        guidance_route("claude", &empty, &rewritten, &args, true),
        GuidanceRoute::None
    );
}

#[test]
fn drop_reason_codes_are_the_documented_identifiers() {
    assert_eq!(DropReason::NoPromptOnStdin.code(), "no_prompt_on_stdin");
    assert_eq!(
        DropReason::LaunchFormUnsupported.code(),
        "launch_form_unsupported"
    );
    assert_eq!(DropReason::TrailerNotWritten.code(), "trailer_not_written");
    assert_eq!(
        DropReason::PromptFileNotCopied.code(),
        "prompt_file_not_copied"
    );
    for r in [
        DropReason::NoPromptOnStdin,
        DropReason::LaunchFormUnsupported,
        DropReason::TrailerNotWritten,
        DropReason::PromptFileNotCopied,
    ] {
        assert!(!r.detail().is_empty());
        assert!(!r.detail().contains('\n'));
    }
}

#[test]
fn step_label_takes_the_agent_from_the_flags_and_sanitises_it() {
    let l = step_label("claude", &s(&["--agent", "amplihack:core:builder", "-p"]));
    assert_eq!(l.agent.as_deref(), Some("amplihack_core_builder"));
    let l = step_label(
        "claude",
        &s(&["--system-prompt-file", "/x/y/builder.md", "-p"]),
    );
    assert_eq!(l.agent.as_deref(), Some("builder"));
    let l = step_label("claude", &s(&["-p"]));
    assert_eq!(l.agent, None);
    // Hostile or oversized names never reach a log line unfiltered.
    let hostile = format!("{}\x1b[31m\n", "a".repeat(200));
    let l = step_label("claude", &s(&["--agent", &hostile, "-p"]));
    let agent = l.agent.unwrap();
    assert!(agent.len() <= 64, "{agent}");
    assert!(
        agent
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-')),
        "{agent}"
    );
}

#[test]
fn step_label_launch_form_is_a_fixed_label_with_no_argv_text() {
    let l = step_label("claude", &s(&["-p", "SECRET-PROMPT-TEXT", "--agent", "b"]));
    assert!(
        !l.launch_form.contains("SECRET-PROMPT-TEXT"),
        "{}",
        l.launch_form
    );
    assert_eq!(l.tool, "claude");
}
