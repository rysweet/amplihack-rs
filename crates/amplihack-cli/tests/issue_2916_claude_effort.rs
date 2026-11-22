//! Issue #2916 (TDD): a tier's reasoning effort reaches Claude Code as
//! `--effort <level>`, as Codex's reaches it as `-c model_reasoning_effort`.
//!
//! The real `amplihack` binary is started with the argv the recipe runner
//! builds, against a stand-in `claude` that records its own argv. Every path
//! is under one temporary home and every variable is set on the child only.
//!
//! A Claude Code whose `--help` does not list `--effort` gets the step without
//! it and a line saying so; nothing ends the launch. An instance can also
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
fn a_gateway_launch_still_carries_the_effort() {
    // The gateway chooses the model; the effort still belongs to the step.
    //
    // The fixture is `issue_1619_tier_propagation`'s: a gateway launch of
    // Claude Code wants one of `VERIFIED_CLAUDE_CODE_VERSIONS` (2.1.247),
    // while the contract file wants 2.1.319 or later (issue #2030), and no
    // version satisfies both. The stand-in reports the gateway-verified one
    // and takes the documented way out of the contract gate.
    let f = Fixture::new();
    crate::support::exec_stub::write_executable(
        &f.tools.join("claude"),
        crate::support::tier_harness::FAKE_HARNESS.replace("2.1.319", "2.1.247"),
        0o755,
    );
    let launch = f.finish(
        f.launch("claude", &["--model", "strong", "-p"])
            .env("AMPLIHACK_LITELLM_MODEL", "gateway-model")
            .env("AMPLIHACK_LITELLM_ENDPOINT", "http://127.0.0.1:1")
            .env("AMPLIHACK_LITELLM_API_KEY", "test-key")
            .env("AMPLIHACK_NO_SYSTEM_PROMPT_APPEND", "1"),
    );
    assert_eq!(
        launch.claude_effort(),
        Some("xhigh"),
        "argv: {:?}",
        launch.argv
    );
    assert_eq!(count_effort_flags(&launch), 1, "argv: {:?}", launch.argv);
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

/// An installed Claude Code too old to know `--effort`: the step reports it
/// and runs without the flag rather than failing on Claude's unknown option.
#[test]
fn a_claude_too_old_for_effort_runs_the_step_without_it_and_says_so() {
    let f = Fixture::new();
    // The stand-in's `--help` lists no `--effort`, and like an old Claude Code
    // it refuses the option outright if it is passed anyway.
    let old = crate::support::tier_harness::FAKE_HARNESS
        .replace("--effort <level>", "--model <model>")
        .replace(
            ": > \"$ARGV_LOG\"",
            "for a in \"$@\"; do [ \"$a\" = --effort ] && { echo \"error: unknown option '--effort'\" >&2; exit 1; }; done\n: > \"$ARGV_LOG\"",
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
    let stderr = launch.stderr();
    assert!(
        stderr.contains("does not list `--effort`")
            && stderr.contains("runs without `--effort xhigh`"),
        "the step says it ran without the effort:\n{stderr}"
    );
}

/// A launch that stops before the binary is resolved still writes the record
/// the tier decision made, as it did before the `--help` probe existed: the
/// record no longer waits on a probe the launch never reached.
#[test]
fn a_launch_that_stops_before_the_binary_is_resolved_still_writes_the_record() {
    let f = Fixture::new();
    let output = f
        .launch("claude", &["--model", "strong", "-p"])
        .env(
            "AMPLIHACK_CLAUDE_BINARY_PATH",
            f.tools.join("no-such-claude"),
        )
        .env("AMPLIHACK_RUN_DIR", &f.run_dir)
        .output()
        .expect("run amplihack");
    assert!(!output.status.success(), "a missing binary ends the launch");
    let text = std::fs::read_to_string(f.run_dir.join("model-tiers.jsonl")).unwrap_or_else(|e| {
        panic!(
            "no record: {e}. stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        )
    });
    let line: serde_json::Value =
        serde_json::from_str(text.lines().next().expect("one line")).expect("json");
    assert_eq!(line["tier"], "strong", "record: {text}");
    assert_eq!(line["effective_effort"], "xhigh", "record: {text}");
}
