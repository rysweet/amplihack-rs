//! Issue #1619, B2 (TDD): an operator's `AMPLIHACK_DEFAULT_MODEL` still
//! chooses the model, with the tier table in place.
//!
//! The condition Ryan set on 16 November is that amplihack runs in other
//! contexts and harnesses, and that Opus and Sonnet will not exist in some of
//! them. Before this feature `AMPLIHACK_DEFAULT_MODEL` chose every launch's
//! model, and it is the only setting an operator on such a host has. A tier
//! table that outranked it would ask every tiered step for a Claude id the
//! account does not have, and the operator would have no way to say so.
//!
//! These run the real `amplihack` binary against an argv-recording fake
//! harness: the question is what the harness is actually told, not what any
//! constant is spelled. Every path a run may read or write is under one temp
//! directory and every variable is set on the child only.

#![cfg(target_os = "linux")]

use crate::support::tier_harness::Fixture;

/// An id no column of the shipped mapping names, so a reading of it can only
/// have come from the operator's own variable.
const HOUSE_MODEL: &str = "house-model-7";

// -------------------------------------------------------------------------
// It replaces a tier word, on a Claude-compatible harness and on Codex
// -------------------------------------------------------------------------

#[test]
fn an_operator_default_outranks_the_shipped_claude_tier() {
    let f = Fixture::new();
    let mut command = f.launch("claude", &["--model", "strong", "-p"]);
    command.env("AMPLIHACK_DEFAULT_MODEL", HOUSE_MODEL);
    let launch = f.finish(&mut command);
    assert_eq!(
        launch.model(),
        Some(HOUSE_MODEL),
        "a set AMPLIHACK_DEFAULT_MODEL outranks the shipped tier column. argv: {:?}",
        launch.argv
    );
}

#[test]
fn an_operator_default_covers_every_tier() {
    let f = Fixture::new();
    for tier in ["fast", "standard", "strong"] {
        let mut command = f.launch("claude", &["--model", tier, "-p"]);
        command.env("AMPLIHACK_DEFAULT_MODEL", HOUSE_MODEL);
        let launch = f.finish(&mut command);
        assert_eq!(
            launch.model(),
            Some(HOUSE_MODEL),
            "`{tier}` takes the operator's model like every other tier. argv: {:?}",
            launch.argv
        );
    }
}

/// The shipped Codex column names no model at all, because Opus and Sonnet do
/// not exist there and guessing at the account would be worse. An operator
/// who does know their account's id must be able to supply it, and the tier's
/// own reasoning effort must still reach the harness, because a model was
/// chosen rather than fallen back from.
#[test]
fn an_operator_default_reaches_a_harness_with_no_opus_and_no_sonnet() {
    let f = Fixture::new();
    let mut command = f.launch("codex", &["exec", "--model", "strong", "-"]);
    command.env("AMPLIHACK_DEFAULT_MODEL", "gpt-6.1-sol");
    let launch = f.finish(&mut command);
    assert_eq!(
        launch.model(),
        Some("gpt-6.1-sol"),
        "argv: {:?}",
        launch.argv
    );
    assert_eq!(
        launch.effort(),
        Some("xhigh"),
        "the tier's Codex reasoning effort survives the substitution, as it does \
         under `model.force`. argv: {:?}",
        launch.argv
    );
}

#[test]
fn no_tier_word_survives_an_operator_default_on_any_harness() {
    let f = Fixture::new();
    for harness in ["claude", "codex", "copilot"] {
        for tier in ["fast", "standard", "strong"] {
            let passthrough: Vec<&str> = if harness == "codex" {
                vec!["exec", "--model", tier, "-"]
            } else {
                vec!["--model", tier, "-p"]
            };
            let mut command = f.launch(harness, &passthrough);
            command.env("AMPLIHACK_DEFAULT_MODEL", HOUSE_MODEL);
            let launch = f.finish(&mut command);
            assert_ne!(
                launch.model(),
                Some(tier),
                "{harness}: a tier word asked for as a model breaks the step. argv: {:?}",
                launch.argv
            );
        }
    }
}

// -------------------------------------------------------------------------
// Where it sits, and what it must not disturb
// -------------------------------------------------------------------------

#[test]
fn a_forced_model_still_outranks_an_operator_default() {
    let f = Fixture::new();
    f.write_user_config("[model]\nforce = \"forced-model-9\"\n");
    let mut command = f.launch("claude", &["--model", "strong", "-p"]);
    command.env("AMPLIHACK_DEFAULT_MODEL", HOUSE_MODEL);
    let launch = f.finish(&mut command);
    assert_eq!(
        launch.model(),
        Some("forced-model-9"),
        "`model.force` is the operator's own stronger statement. argv: {:?}",
        launch.argv
    );
}

#[test]
fn an_explicit_concrete_id_is_still_forwarded_as_typed() {
    let f = Fixture::new();
    let mut command = f.launch("claude", &["--model", "claude-haiku-5-5", "-p"]);
    command.env("AMPLIHACK_DEFAULT_MODEL", HOUSE_MODEL);
    let launch = f.finish(&mut command);
    assert_eq!(
        launch.model(),
        Some("claude-haiku-5-5"),
        "issue #1527: an id the caller typed is left alone, and the operator default \
         replaces a tier word only. argv: {:?}",
        launch.argv
    );
}

#[test]
fn an_operator_default_adds_no_second_model_pair() {
    let f = Fixture::new();
    let mut command = f.launch("claude", &["--model", "strong", "-p"]);
    command.env("AMPLIHACK_DEFAULT_MODEL", HOUSE_MODEL);
    let launch = f.finish(&mut command);
    let pairs = launch
        .argv
        .iter()
        .filter(|a| *a == "--model" || a.starts_with("--model="))
        .count();
    assert_eq!(
        pairs, 1,
        "the tier pair is replaced in place, never doubled. argv: {:?}",
        launch.argv
    );
}

// -------------------------------------------------------------------------
// V2 — the untiered path validates the same value the tiered path does
// -------------------------------------------------------------------------

/// Without a tier word there is nothing for the resolver to replace, and
/// `model_args` writes `AMPLIHACK_DEFAULT_MODEL` onto the command line
/// itself. That path did no validation at all, so the same variable was
/// refused on a tiered step and written verbatim on an untiered one — and
/// both Codex and Claude carry single-token sandbox-bypass flags.
#[test]
fn an_argument_shaped_operator_default_never_reaches_argv() {
    let f = Fixture::new();
    // What this launch looks like with the variable unset. Comparing against
    // it is stricter than looking for the candidate by name, and it stays
    // sound for a token amplihack adds on its own account:
    // `--dangerously-skip-permissions` is already the first argument of every
    // Claude launch amplihack makes.
    let baseline = f.run("claude", &["-p", "go"]).argv;
    for candidate in [
        "--dangerously-skip-permissions",
        "-p",
        "--model",
        "--allow-all-tools",
    ] {
        let mut command = f.launch("claude", &["-p", "go"]);
        command.env("AMPLIHACK_DEFAULT_MODEL", candidate);
        let launch = f.finish(&mut command);
        assert_eq!(
            launch.argv, baseline,
            "`{candidate}` moved the command line: a value that could act as another \
             argument is refused on the untiered path too, by the same code that \
             refuses it on a tiered step"
        );
        assert_ne!(launch.model(), Some(candidate), "argv: {:?}", launch.argv);
    }
}

#[test]
fn a_refused_operator_default_still_launches_the_step() {
    let f = Fixture::new();
    let mut command = f.launch("claude", &["--model", "fast", "-p"]);
    command.env("AMPLIHACK_DEFAULT_MODEL", "--dangerously-skip-permissions");
    let launch = f.finish(&mut command);
    assert_eq!(
        launch.model(),
        Some("claude-haiku-5-5"),
        "the step's own tier applies and the launch proceeds: a refusal must never \
         stop a run of a hundred steps. argv: {:?}",
        launch.argv
    );
    assert!(
        !launch.stderr().contains("--dangerously-skip-permissions"),
        "a refusal names the rule broken, never the refused value. stderr:\n{}",
        launch.stderr()
    );
}

#[test]
fn an_unset_operator_default_leaves_the_table_in_force() {
    let f = Fixture::new();
    let launch = f.run("claude", &["--model", "fast", "-p"]);
    assert_eq!(
        launch.model(),
        Some("claude-haiku-5-5"),
        "with the variable unset the shipped mapping decides, which is what every \
         saving this issue measures depends on. argv: {:?}",
        launch.argv
    );
}

#[test]
fn a_blank_operator_default_leaves_the_table_in_force() {
    let f = Fixture::new();
    let mut command = f.launch("claude", &["--model", "standard", "-p"]);
    command.env("AMPLIHACK_DEFAULT_MODEL", "   ");
    let launch = f.finish(&mut command);
    assert_eq!(
        launch.model(),
        Some("claude-sonnet-5-5"),
        "a blank value means `pass nothing and let the CLI decide`; it is not a \
         model the table should lose to. argv: {:?}",
        launch.argv
    );
}
