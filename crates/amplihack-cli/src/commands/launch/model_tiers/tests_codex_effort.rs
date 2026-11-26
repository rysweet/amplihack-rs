//! Issue #1619 (TDD): injecting Codex's reasoning effort.
//!
//! `inject` takes an `Effort`, not a string: only one of Codex's five
//! accepted words can become the right-hand side of a `-c` token, and the type says
//! so. A word outside the list is refused at the table, where the operator
//! wrote it, with a `BadEffort` notice naming the file — see
//! `tests_table.rs`. There is therefore no rejection branch here to pin.
//!
//! This is the fix for the defect reported in the issue's 3 November comment:
//! an outer Codex session's `-c model_reasoning_effort=…` is a setting of
//! that session and is not inherited, and a recipe's `-c reasoning_effort=…`
//! is a context variable the runner never passes on. The setting has to be
//! put on the child's own command line or it does not exist.
//!
//! Pure: it reads only the argv tail, like `codex_agent_step`.

use super::argv_scan::Injection;
use super::codex_effort::{KEY, inject};
use super::table::Effort;

fn argv(words: &[&str]) -> Vec<String> {
    words.iter().map(|w| (*w).to_string()).collect()
}

/// The effort an accepted word parses to. A word that does not parse is not
/// a value this module can be handed.
fn effort(word: &str) -> Effort {
    Effort::parse(word).unwrap_or_else(|| panic!("`{word}` is an accepted effort"))
}

fn added(injection: Injection) -> Vec<String> {
    match injection {
        Injection::Added(args) => args,
        other => panic!("expected an injection, got {other:?}"),
    }
}

#[test]
fn the_key_is_codexs_own_setting_name() {
    assert_eq!(KEY, "model_reasoning_effort");
}

#[test]
fn an_effort_is_added_as_a_native_config_override() {
    let out = added(inject(&argv(&["exec", "-"]), effort("xhigh")));
    assert!(
        out.windows(2)
            .any(|w| w[0] == "-c" && w[1] == "model_reasoning_effort=xhigh"),
        "the effort must reach codex as its own `-c key=value` argument: {out:?}"
    );
}

#[test]
fn the_exec_subcommand_is_still_the_first_positional_afterwards() {
    // `codex_agent_step::exec_subcommand_index` finds `exec` by walking argv
    // with arity, so a `-c key=value` pair before it is skipped as an option
    // and its value. The sandbox splice must still land after `exec`.
    use crate::commands::launch::codex_agent_step::exec_subcommand_index;
    let out = added(inject(
        &argv(&["exec", "--output-last-message", "/tmp/x", "-"]),
        effort("high"),
    ));
    assert_eq!(
        exec_subcommand_index(&out).map(|i| out[i].as_str()),
        Some("exec"),
        "argv: {out:?}"
    );
}

#[test]
fn every_accepted_effort_is_injected() {
    for word in ["minimal", "low", "medium", "high", "xhigh"] {
        let out = added(inject(&argv(&["exec", "-"]), effort(word)));
        assert!(
            out.iter()
                .any(|a| a == &format!("model_reasoning_effort={word}")),
            "{word}: {out:?}"
        );
    }
}

#[test]
fn an_unaccepted_word_is_not_an_effort_at_all() {
    // Argv safety: the value becomes the right-hand side of a `-c` token, so
    // only Codex's five accepted words are ever written. The allowlist is the
    // `Effort` type, enforced where the word is read — so these never become
    // an `Effort` and cannot be handed to `inject`. Whether the installed
    // Codex accepts a word it is handed is Codex's decision.
    for bad in [
        "turbo",
        "",
        " ",
        "high high",
        "high;--yolo",
        "high\nmodel=x",
        "$(id)",
        "high`id`",
    ] {
        assert!(Effort::parse(bad).is_none(), "`{bad:?}` must be rejected");
    }
}

#[test]
fn a_callers_own_setting_is_never_overridden() {
    for args in [
        argv(&["exec", "-c", "model_reasoning_effort=medium", "-"]),
        argv(&["exec", "--config", "model_reasoning_effort=medium", "-"]),
        argv(&["exec", "-c", "model_reasoning_effort = medium", "-"]),
        argv(&["-c", "model_reasoning_effort=medium", "exec", "-"]),
    ] {
        assert!(
            matches!(inject(&args, effort("xhigh")), Injection::CallerChose),
            "amplihack adds nothing when the command line already sets the effort: \
             {args:?}"
        );
    }
}

#[test]
fn a_different_config_key_is_not_mistaken_for_this_one() {
    let out = added(inject(
        &argv(&["exec", "-c", "model_reasoning_summary=detailed", "-"]),
        effort("high"),
    ));
    assert!(
        out.iter().any(|a| a == "model_reasoning_effort=high"),
        "another `-c` key is not this one: {out:?}"
    );
}

#[test]
fn a_setting_after_the_end_of_options_marker_is_not_the_callers_choice() {
    // Everything after `--` is the child's own argument, not an option of the
    // command amplihack is building.
    let args = argv(&["exec", "--", "-c", "model_reasoning_effort=medium"]);
    assert!(
        matches!(inject(&args, effort("xhigh")), Injection::Added(_)),
        "a token past `--` does not count as the caller choosing: {args:?}"
    );
}

#[test]
fn exactly_one_setting_is_ever_present() {
    let out = added(inject(&argv(&["exec", "-"]), effort("low")));
    assert_eq!(
        out.iter()
            .filter(|a| a.starts_with("model_reasoning_effort="))
            .count(),
        1,
        "argv: {out:?}"
    );
}

#[test]
fn nothing_else_in_the_argv_moves() {
    let args = argv(&[
        "exec",
        "--output-last-message",
        "/tmp/x",
        "--sandbox",
        "read-only",
        "-",
    ]);
    let out = added(inject(&args, effort("high")));
    let rest: Vec<&String> = out
        .iter()
        .filter(|a| *a != "-c" && !a.starts_with("model_reasoning_effort="))
        .collect();
    let original: Vec<&String> = args.iter().collect();
    assert_eq!(rest, original, "only the pair is added: {out:?}");
}

#[test]
fn argv_without_an_exec_subcommand_is_still_handled() {
    // An interactive `amplihack codex` carries no `exec`. Whatever the
    // injection decides, it must not corrupt the argv.
    let args = argv(&["resume", "--last"]);
    match inject(&args, effort("high")) {
        Injection::Added(out) => assert!(
            out.ends_with(&args),
            "the caller's arguments stay last, as with every other injection: {out:?}"
        ),
        Injection::CallerChose => {}
    }
}
