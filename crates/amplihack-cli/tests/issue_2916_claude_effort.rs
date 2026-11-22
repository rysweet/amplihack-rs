//! Issue #2916 (TDD): a tier's reasoning effort reaches Claude Code as
//! `--effort <level>`, as Codex's reaches it as `-c model_reasoning_effort`.
//!
//! The real `amplihack` binary is started with the argv the recipe runner
//! builds, against a stand-in `claude` that records its own argv. Every path
//! is under one temporary home and every variable is set on the child only.
//!
//! A Claude Code older than 2.1.329, or whose version cannot be read, gets the
//! step without `--effort` and a line saying so; nothing ends the launch and
//! the stand-in is started exactly once. An instance can also
//! clear the cell's effort in its own override file, which R9 pins.

#![cfg(target_os = "linux")]

use crate::support::tier_harness::{Fixture, Launch};

const STRONG: &str = "claude-opus-5[1m]";

fn count_effort_flags(launch: &Launch) -> usize {
    launch
        .argv
        .iter()
        .filter(|a| *a == "--effort" || a.starts_with("--effort="))
        .count()
}

/// R8
#[test]
fn a_strong_tier_step_launches_with_xhigh_and_the_tier_model() {
    let f = Fixture::new();
    let launch = f.run("claude", &["--model", "strong", "-p"]);
    assert_eq!(
        launch.claude_effort(),
        Some("xhigh"),
        "argv: {:?}",
        launch.argv
    );
    assert_eq!(launch.model(), Some(STRONG), "argv: {:?}", launch.argv);
    assert_eq!(count_effort_flags(&launch), 1, "argv: {:?}", launch.argv);
    assert_eq!(f.launches(), 1, "one launch, no probe");
    assert!(
        launch.output.status.success(),
        "stderr:\n{}",
        launch.stderr()
    );
}

#[test]
fn standard_and_fast_carry_no_effort() {
    let f = Fixture::new();
    for tier in ["standard", "fast"] {
        let launch = f.run("claude", &["--model", tier, "-p"]);
        assert_eq!(launch.claude_effort(), None, "{tier}: {:?}", launch.argv);
    }
}

#[test]
fn a_concrete_model_gets_no_effort() {
    let f = Fixture::new();
    let launch = f.run("claude", &["--model", STRONG, "-p"]);
    assert_eq!(launch.claude_effort(), None, "argv: {:?}", launch.argv);
}

#[test]
fn the_launch_adds_nothing_else_but_the_model_and_the_effort() {
    let f = Fixture::new();
    let plain = f.argv("claude", &["--model", STRONG, "-p"]);
    let tier = f.argv("claude", &["--model", "strong", "-p"]);
    let strip = |argv: &[String]| -> Vec<String> {
        let mut out = Vec::new();
        let mut it = argv.iter();
        while let Some(a) = it.next() {
            if a == "--effort" {
                it.next();
            } else {
                out.push(a.clone());
            }
        }
        out
    };
    assert_eq!(
        strip(&tier),
        strip(&plain),
        "no permission or sandbox flag moves"
    );
}

/// R9: an override cell that names a model and no effort clears the shipped
/// effort, so the flag is never written.
#[test]
fn an_override_cell_with_no_effort_launches_without_the_flag() {
    let f = Fixture::new();
    f.write_override("version = 1\n[claude.strong]\nmodel = \"claude-opus-5[1m]\"\n");
    let launch = f.run("claude", &["--model", "strong", "-p"]);
    assert_eq!(launch.model(), Some(STRONG), "argv: {:?}", launch.argv);
    assert_eq!(count_effort_flags(&launch), 0, "argv: {:?}", launch.argv);
    assert!(
        launch.output.status.success(),
        "stderr:\n{}",
        launch.stderr()
    );
}

/// R10
#[test]
fn a_callers_own_effort_is_kept_and_amplihack_says_it_left_it_alone() {
    let f = Fixture::new();
    for tail in [
        &["--effort", "medium", "--model", "strong", "-p"][..],
        &["--effort=medium", "--model", "strong", "-p"][..],
    ] {
        let launch = f.run("claude", tail);
        assert_eq!(
            launch.claude_effort(),
            Some("medium"),
            "argv: {:?}",
            launch.argv
        );
        assert_eq!(count_effort_flags(&launch), 1, "argv: {:?}", launch.argv);
        assert_eq!(launch.model(), Some(STRONG));
        let stderr = launch.stderr();
        assert!(
            stderr.contains("--effort") && stderr.contains("alone"),
            "amplihack says it left the caller's flag alone:\n{stderr}"
        );
        assert!(
            !stderr.contains("-c model_reasoning_effort"),
            "the Claude line names Claude's flag:\n{stderr}"
        );
    }
}

/// R11
#[test]
fn an_override_can_raise_strong_to_max() {
    let f = Fixture::new();
    f.write_override(
        "version = 1\n[claude.strong]\nmodel = \"claude-opus-5[1m]\"\neffort = \"max\"\n",
    );
    let launch = f.run("claude", &["--model", "strong", "-p"]);
    assert_eq!(
        launch.claude_effort(),
        Some("max"),
        "argv: {:?}",
        launch.argv
    );
}

/// R11
#[test]
fn a_word_outside_claudes_list_is_reported_and_dropped_and_the_model_applies() {
    let f = Fixture::new();
    f.write_override(
        "version = 1\n[claude.strong]\nmodel = \"claude-opus-5[1m]\"\neffort = \"minimal\"\n",
    );
    let launch = f.run("claude", &["--model", "strong", "-p"]);
    assert!(
        launch.output.status.success(),
        "stderr:\n{}",
        launch.stderr()
    );
    assert_eq!(count_effort_flags(&launch), 0, "argv: {:?}", launch.argv);
    assert_eq!(launch.model(), Some(STRONG), "argv: {:?}", launch.argv);
    let stderr = launch.stderr();
    let line = stderr
        .lines()
        .find(|l| l.contains("not an accepted reasoning effort"))
        .unwrap_or_else(|| panic!("a BadEffort line is printed:\n{stderr}"));
    assert!(
        line.contains("claude.strong"),
        "names column and tier: {line}"
    );
    assert!(
        line.contains("low, medium, high, xhigh, max"),
        "lists Claude's words: {line}"
    );
    assert!(
        !line.chars().any(|c| c.is_control()),
        "no raw control characters in the line: {line:?}"
    );
}

#[test]
fn a_hostile_effort_value_never_reaches_the_command_line() {
    let f = Fixture::new();
    f.write_override(
        "version = 1\n[claude.strong]\nmodel = \"claude-opus-5[1m]\"\n\
         effort = \"xhigh --dangerously-skip-permissions\"\n",
    );
    let launch = f.run("claude", &["--model", "strong", "-p"]);
    assert_eq!(count_effort_flags(&launch), 0, "argv: {:?}", launch.argv);
    // No token of the refused value reaches the command line: not the word
    // it tried to pass off as an effort, and not the option it carried
    // behind it. The cell replaced the shipped one whole, so the shipped
    // `xhigh` is gone too. amplihack adds a bare
    // `--dangerously-skip-permissions` to every Claude launch itself, so the
    // check looks for the `=` form a smuggled value would have to take.
    assert!(
        !launch
            .argv
            .iter()
            .any(|a| a.contains("xhigh") || a.contains("--dangerously-skip-permissions=")),
        "argv: {:?}",
        launch.argv
    );
    assert_eq!(launch.model(), Some(STRONG));
}

#[test]
fn a_ceiling_lowers_the_effort_and_the_line_says_so() {
    let f = Fixture::new();
    f.write_override("version = 1\n[claude]\nmax_effort = \"high\"\n");
    let launch = f.run("claude", &["--model", "strong", "-p"]);
    assert_eq!(
        launch.claude_effort(),
        Some("high"),
        "argv: {:?}",
        launch.argv
    );
    assert!(
        launch.stderr().contains("max_effort"),
        "stderr:\n{}",
        launch.stderr()
    );
}

#[test]
fn the_applied_line_names_claudes_flag_and_never_the_argv_tail() {
    let f = Fixture::new();
    let launch = f.run(
        "claude",
        &["--model", "strong", "-p", "a-secret-prompt-word"],
    );
    let stderr = launch.stderr();
    assert!(stderr.contains("--effort xhigh"), "stderr:\n{stderr}");
    assert!(
        !stderr.contains("a-secret-prompt-word"),
        "stderr:\n{stderr}"
    );
}

#[test]
fn a_gateway_launch_withholds_the_effort_its_pinned_claude_code_predates() {
    // The gateway chooses the model; the pinned Claude Code decides the effort.
    //
    // The fixture is `issue_1619_tier_propagation`'s: a gateway launch of
    // Claude Code wants one of `VERIFIED_CLAUDE_CODE_VERSIONS` (2.1.247),
    // while the contract file wants 2.1.319 or later (issue #2030), and no
    // version satisfies both. The stand-in reports the gateway-verified one
    // and takes the documented way out of the contract gate.
    let f = Fixture::new();
    crate::support::exec_stub::write_executable(
        &f.tools.join("claude"),
        crate::support::tier_harness::FAKE_HARNESS.replace("2.1.329", "2.1.247"),
        0o755,
    );
    let launch = f.finish(
        f.launch("claude", &["--model", "strong", "-p"])
            .env("AMPLIHACK_LITELLM_MODEL", "gateway-model")
            .env("AMPLIHACK_LITELLM_ENDPOINT", "http://127.0.0.1:1")
            .env("AMPLIHACK_LITELLM_API_KEY", "test-key")
            .env("AMPLIHACK_NO_SYSTEM_PROMPT_APPEND", "1"),
    );
    // The gateway pins Claude Code 2.1.247, which predates `--effort`.
    assert_eq!(count_effort_flags(&launch), 0, "argv: {:?}", launch.argv);
    assert_eq!(f.launches(), 1, "one launch");
    assert!(launch.stderr().contains("2.1.247"), "{}", launch.stderr());
    assert_eq!(
        launch.model(),
        Some("gateway-model"),
        "the gateway still chooses the model. argv: {:?}",
        launch.argv
    );
    let stderr = launch.stderr();
    assert!(
        stderr.contains("the LiteLLM gateway routes on the model name"),
        "the gateway line keeps printing:\n{stderr}"
    );
    assert!(
        !stderr.contains("the model stays"),
        "and the effort line claims nothing about the model:\n{stderr}"
    );
}

/// `rusty` and `amplifier` are names amplihack launches like Claude Code but
/// has no subcommand for, so `rustyclawd` is the one this suite can start
/// end to end. That the whole `is_claude_compatible_tool` set shares the
/// column, and that the dispatch reads the column rather than the binary
/// name, is pinned by `tests_claude_effort` and `tests_table`.
#[test]
fn rustyclawd_travels_with_the_claude_column() {
    let f = Fixture::new();
    crate::support::exec_stub::write_executable(
        &f.tools.join("rustyclawd"),
        crate::support::tier_harness::FAKE_HARNESS,
        0o755,
    );
    let launch = f.run("rustyclawd", &["--model", "strong", "-p"]);
    assert_eq!(
        launch.claude_effort(),
        Some("xhigh"),
        "argv: {:?}",
        launch.argv
    );
    assert_eq!(launch.model(), Some(STRONG), "argv: {:?}", launch.argv);
}

#[test]
fn a_codex_launch_is_unchanged_and_carries_no_claude_flag() {
    let f = Fixture::new();
    let launch = f.run("codex", &["exec", "--model", "strong", "-"]);
    assert_eq!(launch.effort(), Some("high"), "argv: {:?}", launch.argv);
    assert_eq!(count_effort_flags(&launch), 0, "argv: {:?}", launch.argv);
}

/// An installed Claude Code older than 2.1.329: the step runs without the
/// effort, once, and says so.
#[test]
fn a_claude_too_old_for_effort_runs_the_step_without_it_and_says_so() {
    let f = Fixture::new();
    // Like an old Claude Code, the stand-in refuses `--effort` outright.
    let old = crate::support::tier_harness::FAKE_HARNESS
        .replace("2.1.329 (Claude Code)", "2.1.320 (Claude Code)")
        .replace(
            "echo launch >>",
            "for a in \"$@\"; do [ \"$a\" = --effort ] && { echo \"error: unknown option '--effort'\" >&2; exit 1; }; done\necho launch >>",
        );
    crate::support::exec_stub::write_executable(&f.tools.join("claude"), old, 0o755);
    let launch = f.run("claude", &["--model", "strong", "-p"]);
    assert!(
        launch.output.status.success(),
        "stderr:\n{}",
        launch.stderr()
    );
    assert_eq!(count_effort_flags(&launch), 0, "argv: {:?}", launch.argv);
    assert_eq!(launch.model(), Some(STRONG), "argv: {:?}", launch.argv);
    assert_eq!(f.launches(), 1, "one launch, no retry");
    let stderr = launch.stderr();
    assert!(
        stderr.contains("2.1.320")
            && stderr.contains("2.1.329")
            && stderr.contains("runs without `--effort xhigh`"),
        "the step says why it ran without the effort:\n{stderr}"
    );
}

/// A caller's own `--effort` is passed through unchanged, once.
#[test]
fn a_callers_effort_low_is_passed_through_with_no_duplicate() {
    let f = Fixture::new();
    let launch = f.run("claude", &["--effort", "low", "--model", "strong", "-p"]);
    assert_eq!(launch.claude_effort(), Some("low"), "{:?}", launch.argv);
    assert_eq!(count_effort_flags(&launch), 1, "{:?}", launch.argv);
    assert_eq!(f.launches(), 1);
}

/// An override file's effort is the one passed.
#[test]
fn an_override_files_high_effort_is_passed() {
    let f = Fixture::new();
    f.write_override(
        "version = 1\n[claude.strong]\nmodel = \"claude-opus-5[1m]\"\neffort = \"high\"\n",
    );
    let launch = f.run("claude", &["--model", "strong", "-p"]);
    assert_eq!(launch.claude_effort(), Some("high"), "{:?}", launch.argv);
    assert_eq!(f.launches(), 1);
}

// A version that cannot be read never reaches the tier step on the real
// launch path: the binary health gate refuses an unparseable `--version`
// first. The rule's answer for it is pinned by the unit tests in
// `model_tiers/tests_claude_effort.rs`.

/// The record a launch that stops early leaves, and its stderr.
///
/// Each early exit below ends the launch before the stand-in Claude runs. The
/// tier decision is made, recorded and announced first, so the record and the
/// applied line must still be there, once.
fn record_of_a_stopped_launch(
    f: &Fixture,
    command: &mut std::process::Command,
    expected_effort: serde_json::Value,
) -> String {
    let output = command
        .env("AMPLIHACK_RUN_DIR", &f.run_dir)
        .output()
        .expect("run amplihack");
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        !output.status.success(),
        "the launch stops early:\n{stderr}"
    );
    assert!(
        !f.argv_log.exists(),
        "the stand-in Claude never ran:\n{stderr}"
    );
    let text = std::fs::read_to_string(f.run_dir.join("model-tiers.jsonl"))
        .unwrap_or_else(|e| panic!("no record: {e}. stderr:\n{stderr}"));
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 1, "exactly one record: {text}");
    let line: serde_json::Value = serde_json::from_str(lines[0]).expect("json");
    assert_eq!(line["tier"], "strong", "record: {text}");
    assert_eq!(line["effective_effort"], expected_effort, "record: {text}");
    assert_eq!(
        stderr
            .lines()
            .filter(|l| l.contains("`--effort xhigh`") && !l.contains("runs without"))
            .count(),
        usize::from(expected_effort == "xhigh"),
        "the applied line prints once when the effort applied:\n{stderr}"
    );
    stderr
}

/// A missing Claude binary ends the launch after the tier was resolved.
#[test]
fn a_launch_whose_binary_cannot_be_resolved_still_writes_the_record() {
    let f = Fixture::new();
    let mut command = f.launch("claude", &["--model", "strong", "-p"]);
    command.env(
        "AMPLIHACK_CLAUDE_BINARY_PATH",
        f.tools.join("no-such-claude"),
    );
    record_of_a_stopped_launch(&f, &mut command, serde_json::json!("xhigh"));
}

/// A Claude Code older than the contract gate's minimum is refused before it
/// is spawned (issue #2030); the tier decision was already recorded.
#[test]
fn a_launch_the_contract_gate_refuses_still_writes_the_record() {
    let f = Fixture::new();
    let old = crate::support::tier_harness::FAKE_HARNESS
        .replace("2.1.329 (Claude Code)", "2.1.100 (Claude Code)");
    crate::support::exec_stub::write_executable(&f.tools.join("claude"), &old, 0o755);
    let stderr = record_of_a_stopped_launch(
        &f,
        &mut f.launch("claude", &["--model", "strong", "-p"]),
        serde_json::Value::Null,
    );
    assert!(
        stderr.contains("2.1.100"),
        "the refusal names the installed version:\n{stderr}"
    );
}

/// The Docker hand-off returns before the binary is resolved. A stand-in
/// `docker` that fails `docker info` makes the hand-off end at once with
/// "Docker is not running", so nothing real is started.
#[test]
fn a_docker_launch_still_writes_the_record() {
    let f = Fixture::new();
    crate::support::exec_stub::write_executable(
        &f.tools.join("docker"),
        "#!/bin/sh\nexit 1\n",
        0o755,
    );
    let stderr = record_of_a_stopped_launch(
        &f,
        &mut f.launch_with_flags("claude", &["--docker"], &["--model", "strong", "-p"]),
        serde_json::json!("xhigh"),
    );
    assert!(
        stderr.contains("Docker is not running"),
        "the launch took the Docker hand-off:\n{stderr}"
    );
}
