//! Issue #1619 (TDD): rewriting `--model` must not disturb the rest of the
//! command line.
//!
//! Resolving a tier means finding a `--model <word>` pair the recipe runner
//! wrote and replacing or deleting it. That is argv surgery on a command line
//! that also carries sandbox flags, permission flags, a prompt-file path and
//! the end-of-options marker. A naive scanner can delete a preceding option's
//! value, swallow a positional, or leave the rewrite where a later reader
//! does not see it — and each of those changes what the harness is told to do
//! without saying so.
//!
//! These tests launch the real binary with a fake harness that records its
//! own argv, and compare the recorded argv with and without a tier. Anything
//! that is not the `--model` pair must be byte-identical.

#![cfg(target_os = "linux")]

/// The fake harness and the isolated stage are
/// `support::tier_harness`'s: `issue_1619_tier_propagation` asks different
/// questions of the same recording, so the stage set is built once there
/// rather than copied into each.
use crate::support::tier_harness::Fixture;

/// Everything on the command line except the `--model` pair, so two argvs
/// can be compared without the part that is meant to differ.
fn without_model(argv: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut skip_next = false;
    for arg in argv {
        if skip_next {
            skip_next = false;
            continue;
        }
        if arg == "--model" {
            skip_next = true;
            continue;
        }
        if arg.starts_with("--model=") {
            continue;
        }
        out.push(arg.clone());
    }
    out
}

// -------------------------------------------------------------------------
// G2 — argv is byte-identical when nothing carries a tier
// -------------------------------------------------------------------------

#[test]
fn a_command_line_with_no_tier_is_untouched() {
    let f = Fixture::new();
    let before = f.argv("codex", &["exec", "--output-last-message", "/tmp/x", "-"]);
    // The same launch again: the tier machinery is in the path either way and
    // must change nothing when no tier word appears.
    let after = f.argv("codex", &["exec", "--output-last-message", "/tmp/x", "-"]);
    assert_eq!(
        before, after,
        "a launch with no tier must be byte-identical every time"
    );
    assert!(
        !before.iter().any(|a| a == "--model"),
        "amplihack adds no --model to codex; argv: {before:?}"
    );
}

// -------------------------------------------------------------------------
// G3 — the pair is replaced or deleted, never inserted, and nothing else moves
// -------------------------------------------------------------------------

#[test]
fn only_the_model_pair_differs_on_codex() {
    let f = Fixture::new();
    let plain = f.argv("codex", &["exec", "--output-last-message", "/tmp/x", "-"]);
    let tiered = f.argv(
        "codex",
        &[
            "exec",
            "--output-last-message",
            "/tmp/x",
            "--model",
            "standard",
            "-",
        ],
    );
    let tiered_rest: Vec<String> = without_model(&tiered)
        .into_iter()
        .filter(|a| !a.starts_with("model_reasoning_effort=") && a != "-c")
        .collect();
    assert_eq!(
        without_model(&plain),
        tiered_rest,
        "resolving a tier must change the model pair and the reasoning-effort setting \
         and nothing else.\nplain:  {plain:?}\ntiered: {tiered:?}"
    );
}

#[test]
fn the_sandbox_and_hook_trust_injections_are_unchanged_by_a_tier() {
    let f = Fixture::new();
    for passthrough in [vec!["exec", "-"], vec!["exec", "--model", "strong", "-"]] {
        let argv = f.argv("codex", &passthrough);
        assert!(
            argv.windows(2)
                .any(|w| w[0] == "--sandbox" && w[1] == "danger-full-access"),
            "the agent-step sandbox injection reads the same argv the tier rewrite \
             produced, and must still fire: {argv:?}"
        );
        assert!(
            argv.iter().any(|a| a == "--dangerously-bypass-hook-trust"),
            "argv: {argv:?}"
        );
    }
}

#[test]
fn a_caller_chosen_sandbox_is_still_honoured_with_a_tier() {
    let f = Fixture::new();
    let argv = f.argv(
        "codex",
        &["exec", "--sandbox", "read-only", "--model", "fast", "-"],
    );
    assert!(
        !argv
            .windows(2)
            .any(|w| w[0] == "--sandbox" && w[1] == "danger-full-access"),
        "a sandbox the caller chose is never widened, tier or no tier: {argv:?}"
    );
    assert_eq!(
        argv.iter().filter(|a| *a == "--sandbox").count(),
        1,
        "argv: {argv:?}"
    );
}

#[test]
fn copilot_allow_all_flags_survive_a_tier() {
    let f = Fixture::new();
    for passthrough in [vec!["-p", "go"], vec!["--model", "standard", "-p", "go"]] {
        let argv = f.argv("copilot", &passthrough);
        assert!(
            argv.iter().any(|a| a == "--allow-all"),
            "the Copilot allow-all injection reads the same argv and must still fire: \
             {argv:?}"
        );
    }
}

#[test]
fn the_end_of_options_marker_keeps_its_place_and_what_follows_it_is_not_a_tier() {
    // A tier word past `--` is the child's own argument, not an option of the
    // command amplihack is building, and must be forwarded as typed.
    let f = Fixture::new();
    let plain = f.argv("codex", &["exec", "--", "strong"]);
    assert!(
        plain.iter().any(|a| a == "--"),
        "the marker must survive: {plain:?}"
    );
    let after: Vec<&String> = plain.iter().skip_while(|a| *a != "--").skip(1).collect();
    assert!(
        !after.is_empty(),
        "something must still follow the marker: {plain:?}"
    );

    let tiered = f.argv("codex", &["exec", "--model", "fast", "--", "strong"]);
    let tiered_after: Vec<&String> = tiered.iter().skip_while(|a| *a != "--").skip(1).collect();
    assert_eq!(
        after, tiered_after,
        "resolving the tier before `--` must not disturb what follows it.\n\
         plain:  {plain:?}\ntiered: {tiered:?}"
    );
}

// -------------------------------------------------------------------------
// Arity: `--model` is recognised in flag position only
// -------------------------------------------------------------------------

#[test]
fn a_model_word_in_a_value_position_is_not_treated_as_a_flag() {
    let f = Fixture::new();
    // `--output-last-message` takes a value. A scanner that does not walk
    // arity would see the literal `--model` here as a flag and delete the
    // following argument, silently changing where codex writes its result.
    let argv = f.argv("codex", &["exec", "--output-last-message", "--model", "-"]);
    assert!(
        argv.windows(2)
            .any(|w| w[0] == "--output-last-message" && w[1] == "--model"),
        "an option's value is never read as a flag: {argv:?}"
    );
}

#[test]
fn a_trailing_model_with_no_value_is_still_refused_by_name() {
    // amplihack already refuses a Codex option that needs a value and has
    // none, before it launches anything. Resolving tiers must not turn that
    // clear refusal into a silent rewrite, and must not launch a command the
    // check would have stopped.
    let f = Fixture::new();
    let output = f
        .launch("codex", &["exec", "-", "--model"])
        .output()
        .expect("run amplihack");
    assert!(
        !output.status.success(),
        "a dangling --model is refused, not launched"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--model") && stderr.contains("value"),
        "the refusal names the option and what is missing: {stderr}"
    );
    assert!(
        !f.argv_log.exists(),
        "nothing was launched, so the fake harness recorded nothing"
    );
}

#[test]
fn the_attached_spelling_is_resolved_too() {
    let f = Fixture::new();
    let argv = f.argv("claude", &["--model=standard", "-p"]);
    let model: Option<&str> = argv.iter().enumerate().find_map(|(i, a)| {
        if a == "--model" {
            argv.get(i + 1).map(String::as_str)
        } else {
            a.strip_prefix("--model=")
        }
    });
    assert_eq!(
        model,
        Some("claude-sonnet-5-5"),
        "`--model=<tier>` is the same request as `--model <tier>`: {argv:?}"
    );
}

#[test]
fn several_model_occurrences_all_resolve_and_none_is_added() {
    let f = Fixture::new();
    let argv = f.argv("claude", &["--model", "fast", "--model", "strong", "-p"]);
    let models: Vec<&str> = argv
        .iter()
        .enumerate()
        .filter_map(|(i, a)| {
            if a == "--model" {
                argv.get(i + 1).map(String::as_str)
            } else {
                a.strip_prefix("--model=")
            }
        })
        .collect();
    assert_eq!(
        models.len(),
        2,
        "every occurrence is replaced in place; none is added and none is dropped: \
         {argv:?}"
    );
    assert!(
        !models
            .iter()
            .any(|m| ["fast", "standard", "strong"].contains(m)),
        "no occurrence may be left holding a tier word: {argv:?}"
    );
}

// -------------------------------------------------------------------------
// G1/G4 — a concrete id is forwarded exactly as typed
// -------------------------------------------------------------------------

#[test]
fn a_concrete_id_is_forwarded_untouched_on_every_harness() {
    let f = Fixture::new();
    let claude = f.argv("claude", &["--model", "claude-opus-5[1m]", "-p"]);
    assert!(
        claude
            .windows(2)
            .any(|w| w[0] == "--model" && w[1] == "claude-opus-5[1m]"),
        "issue #1527: a value the operator typed is left alone: {claude:?}"
    );
    let codex = f.argv("codex", &["exec", "--model", "gpt-6.1-sol", "-"]);
    assert!(
        codex
            .windows(2)
            .any(|w| w[0] == "--model" && w[1] == "gpt-6.1-sol"),
        "a concrete id reaches codex as typed: {codex:?}"
    );
    assert!(
        !codex
            .iter()
            .any(|a| a.starts_with("model_reasoning_effort=")),
        "a concrete id is not a tier, so it carries no effort of its own: {codex:?}"
    );
}

#[test]
fn no_argument_after_model_ever_begins_with_a_dash() {
    // A model value that could act as another argument is the hazard this
    // feature must not introduce: the value lands as the token right after
    // `--model`, and both codex and claude carry single-token
    // sandbox-bypass flags.
    let f = Fixture::new();
    for (harness, passthrough) in [
        ("claude", vec!["--model", "strong", "-p"]),
        ("claude", vec!["--model", "fast", "-p"]),
        ("codex", vec!["exec", "--model", "standard", "-"]),
        ("copilot", vec!["--model", "strong", "-p", "go"]),
    ] {
        let argv = f.argv(harness, &passthrough);
        for (i, arg) in argv.iter().enumerate() {
            if arg == "--model" {
                let value = argv.get(i + 1).map(String::as_str).unwrap_or("");
                assert!(
                    !value.starts_with('-'),
                    "{harness}: the token after --model must be a model id, never \
                     something that could act as another flag: {argv:?}"
                );
            }
        }
    }
}

#[test]
fn an_override_file_cannot_smuggle_an_argument_into_the_model_position() {
    let f = Fixture::new();
    for bad in [
        "--dangerously-skip-permissions",
        "-s",
        "a b",
        "a;b",
        "a$b",
        "a/b",
        "a\"b",
    ] {
        f.write_override(&format!(
            "version = 1\n\n[claude.fast]\nmodel = \"{bad}\"\n"
        ));
        let argv = f.argv("claude", &["--model", "fast", "-p"]);
        // Checked at the model position specifically: amplihack adds
        // `--dangerously-skip-permissions` of its own elsewhere in this argv,
        // and the question here is only what it was persuaded to ask for as a
        // model.
        for (i, arg) in argv.iter().enumerate() {
            if arg == "--model" {
                assert_ne!(
                    argv.get(i + 1).map(String::as_str),
                    Some(bad),
                    "an override model id that could act as another argument must be \
                     rejected before it reaches argv, and the shipped mapping used \
                     instead: {argv:?}"
                );
            }
            assert!(
                !arg.strip_prefix("--model=").is_some_and(|v| v == bad),
                "the attached spelling must be refused too: {argv:?}"
            );
        }
    }
}
