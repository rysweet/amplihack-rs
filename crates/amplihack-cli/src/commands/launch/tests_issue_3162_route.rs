//! Issue #3162: which launch forms take operator guidance, and how.
//!
//! `guidance_route` is pure: it takes whether the caller's stdin has input
//! instead of looking at the process. The runner starts a Claude agent step as
//! `claude -- --system-prompt-file <file> -p --add-dir .` with the prompt
//! already on stdin, so the rewrite produces no `stdin_payload` and the block
//! has to follow the caller's stdin (`StdinTrailer`), as it already does for
//! Codex `exec … -`.
//!
//! Contract used here:
//! - `GuidanceRoute::{None, Prompt, StdinTrailer, Undeliverable(DropReason)}`;
//! - `guidance_route(tool, &guidance, &rewritten, &args, stdin_has_input)`;
//! - `DropReason::{NoPromptOnStdin, LaunchFormUnsupported, TrailerNotWritten}`
//!   with `code()` returning `no_prompt_on_stdin`, `launch_form_unsupported`
//!   and `trailer_not_written`;
//! - `step_label(tool, &args) -> StepLabel` with `agent: Option<String>`.
//!
//! The tests compare enum values and strings that are identifiers. None reads
//! prose.

use super::passthrough_rewrite::rewrite_claude_passthrough;
use super::*;
use crate::run_guidance::{DropReason, GuidanceForLaunch, append_entry};
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

fn route(tool: &str, args: &[&str], stdin_has_input: bool) -> GuidanceRoute {
    let (_tmp, g) = with_guidance();
    let args = s(args);
    let rewritten = rewrite_claude_passthrough(&args).unwrap();
    guidance_route(tool, &g, &rewritten, &args, stdin_has_input)
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
    // Whether Copilot reads a prompt from stdin is settled by the pinned CLI
    // (recorded in a comment beside the route). Either answer is acceptable
    // here; claiming delivery for an interactive `-i` launch is not.
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
    for r in [
        DropReason::NoPromptOnStdin,
        DropReason::LaunchFormUnsupported,
        DropReason::TrailerNotWritten,
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
