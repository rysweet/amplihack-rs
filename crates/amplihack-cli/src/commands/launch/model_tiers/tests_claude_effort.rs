//! Issue #2916 (TDD): injecting Claude Code's `--effort`.
//!
//! `inject` takes an `Effort`, not a string, so only a compiled-in word can
//! become the argv token. The pair is prepended, so a positional prompt can
//! never be read as the flag's value. A caller's own `--effort`, in either
//! spelling and before any bare `--`, is left alone. Text after `--` is
//! prompt text and neither suppresses nor triggers the injection.
//!
//! Pure: it reads only the argv tail, like `codex_effort`.

use super::argv_scan::Injection;
use super::claude_effort::{FLAG, inject};
use super::table::Effort;

fn argv(words: &[&str]) -> Vec<String> {
    words.iter().map(|w| (*w).to_string()).collect()
}

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
fn the_flag_is_claude_codes_own_option() {
    assert_eq!(FLAG, "--effort");
}

#[test]
fn the_pair_is_prepended() {
    let out = added(inject(&argv(&["--model", "m", "-p"]), effort("xhigh")));
    assert_eq!(out, argv(&["--effort", "xhigh", "--model", "m", "-p"]));
}

#[test]
fn every_accepted_claude_effort_is_injected_as_exactly_two_tokens() {
    for word in ["low", "medium", "high", "xhigh", "max"] {
        let args = argv(&["-p"]);
        let out = added(inject(&args, effort(word)));
        assert_eq!(out.len(), args.len() + 2, "{word}: {out:?}");
        assert_eq!(out[0], FLAG);
        assert_eq!(out[1], word);
        assert_eq!(&out[2..], &args[..]);
    }
}

#[test]
fn a_positional_prompt_is_never_the_flags_value() {
    let out = added(inject(&argv(&["do the thing"]), effort("high")));
    assert_eq!(out, argv(&["--effort", "high", "do the thing"]));
}

#[test]
fn an_empty_tail_still_gets_the_pair() {
    let out = added(inject(&argv(&[]), effort("max")));
    assert_eq!(out, argv(&["--effort", "max"]));
}

#[test]
fn a_callers_own_effort_is_never_overridden() {
    for args in [
        argv(&["--effort", "medium", "-p"]),
        argv(&["--effort=medium", "-p"]),
        argv(&["-p", "--model", "m", "--effort", "low"]),
        argv(&["-p", "--effort"]),
    ] {
        assert!(
            matches!(inject(&args, effort("xhigh")), Injection::CallerChose),
            "{args:?}"
        );
    }
}

#[test]
fn a_different_option_is_not_mistaken_for_effort() {
    for args in [
        argv(&["--effort-level", "low", "-p"]),
        argv(&["--model", "efforts", "-p"]),
        argv(&["-p", "effort"]),
    ] {
        assert!(
            matches!(inject(&args, effort("high")), Injection::Added(_)),
            "{args:?}"
        );
    }
}

#[test]
fn an_effort_after_the_end_of_options_marker_is_prompt_text() {
    let args = argv(&["-p", "--", "--effort", "medium"]);
    let out = added(inject(&args, effort("xhigh")));
    assert_eq!(out[..2], argv(&["--effort", "xhigh"])[..]);
    assert_eq!(&out[2..], &args[..], "the caller's tail is untouched");
}

#[test]
fn exactly_one_effort_flag_is_ever_present() {
    let out = added(inject(&argv(&["--model", "m", "-p"]), effort("low")));
    assert_eq!(out.iter().filter(|a| *a == FLAG).count(), 1, "{out:?}");
}

#[test]
fn nothing_else_in_the_argv_moves() {
    let args = argv(&["--model", "m", "--append-system-prompt-file", "/x", "-p"]);
    let out = added(inject(&args, effort("high")));
    assert_eq!(&out[2..], &args[..]);
}

#[test]
fn an_unaccepted_word_never_becomes_an_effort() {
    // Argv safety lives in the type: these cannot be handed to `inject`.
    for bad in ["turbo", "", "high high", "high;--yolo", "$(id)", "--effort"] {
        assert!(Effort::parse(bad).is_none(), "`{bad:?}`");
    }
}

// -------------------------------------------------------------------------
// The dispatch: which binaries get which flag
// -------------------------------------------------------------------------

use super::EffortOutcome;
use super::reasoning_effort;

#[test]
fn every_claude_compatible_binary_gets_claudes_flag() {
    // `rusty` and `amplifier` have no amplihack subcommand of their own, so
    // the integration suite cannot start them; the dispatch is the thing
    // that decides, and it reads the column.
    for tool in ["claude", "rusty", "rustyclawd", "amplifier"] {
        let (out, outcome) = reasoning_effort(tool, argv(&["-p"]), Some(effort("xhigh")));
        assert_eq!(outcome, EffortOutcome::Added(effort("xhigh")), "{tool}");
        assert_eq!(out, argv(&["--effort", "xhigh", "-p"]), "{tool}");
    }
}

#[test]
fn codex_still_gets_its_own_setting_and_never_claudes_flag() {
    let (out, outcome) = reasoning_effort("codex", argv(&["exec", "-"]), Some(effort("high")));
    assert_eq!(outcome, EffortOutcome::Added(effort("high")));
    assert_eq!(
        out,
        argv(&["-c", "model_reasoning_effort=high", "exec", "-"])
    );
    assert!(!out.iter().any(|a| a == FLAG), "{out:?}");
}

#[test]
fn a_column_with_no_launch_control_is_written_nothing() {
    for tool in ["copilot", "gemini"] {
        let args = argv(&["--model", "m"]);
        let (out, outcome) = reasoning_effort(tool, args.clone(), Some(effort("high")));
        assert_eq!(outcome, EffortOutcome::NotApplicable, "{tool}");
        assert_eq!(out, args, "{tool}");
    }
}

#[test]
fn a_tier_that_names_no_effort_writes_nothing_anywhere() {
    for tool in ["claude", "codex", "copilot"] {
        let args = argv(&["--model", "m"]);
        let (out, outcome) = reasoning_effort(tool, args.clone(), None);
        assert_eq!(outcome, EffortOutcome::NotApplicable, "{tool}");
        assert_eq!(out, args, "{tool}");
    }
}

#[test]
fn a_callers_own_claude_effort_is_reported_rather_than_doubled() {
    let args = argv(&["--effort", "medium", "--model", "m"]);
    let (out, outcome) = reasoning_effort("claude", args.clone(), Some(effort("xhigh")));
    assert_eq!(outcome, EffortOutcome::CallerChose);
    assert_eq!(out, args);
}

// The fall-back for a Claude Code too old to know the option: decided from
// the version amplihack already holds, with no spawn.

use super::claude_effort::{MIN_EFFORT_VERSION, version_accepts_effort};

#[test]
fn the_minimum_is_the_oldest_version_seen_to_accept_the_flag() {
    assert_eq!(MIN_EFFORT_VERSION, (2, 1, 329));
}

#[test]
fn version_rule_table() {
    let rows: &[(Option<&str>, bool)] = &[
        (Some("2.1.328"), false),
        (Some("2.1.328 (Claude Code)"), false),
        (Some("2.1.329"), true),
        (Some("2.1.329 (Claude Code)"), true),
        (Some("2.2.0"), true),
        (Some("3.0.0"), true),
        (Some("1.99.999"), false),
        (Some("garbage"), false),
        (Some(""), false),
        (Some("99999999999.0.0"), false),
        (None, false),
    ];
    for (version, want) in rows {
        assert_eq!(version_accepts_effort(*version), *want, "{version:?}");
    }
}

/// A Claude launch as `resolve_passthrough` leaves it, built from the pure
/// dispatch so no tier file, home directory or environment is read. With no
/// record, `finish` writes nothing to any run directory.
fn launch_for(args: &[&str]) -> (Vec<String>, super::TierLaunch) {
    let (out, outcome) = reasoning_effort("claude", argv(args), Some(effort("xhigh")));
    let pending = super::Pending {
        tool: "claude".to_string(),
        claude_effort: matches!(outcome, EffortOutcome::Added(_)),
        record: None,
        effort: outcome,
    };
    (out, super::TierLaunch(Some(pending)))
}

#[test]
fn finish_keeps_the_flag_for_a_version_that_accepts_it() {
    for version in ["2.1.329", "2.1.329 (Claude Code)", "2.2.0"] {
        let (out, tier) = launch_for(&["--model", "m", "-p"]);
        assert!(
            out.windows(2).any(|w| w == ["--effort", "xhigh"]),
            "{out:?}"
        );
        let out = tier.finish(out, Some(version));
        assert_eq!(
            out,
            argv(&["--effort", "xhigh", "--model", "m", "-p"]),
            "{version}"
        );
    }
}

#[test]
fn finish_withdraws_the_flag_for_an_older_or_unreadable_version() {
    for version in [Some("2.1.320"), None, Some("garbage")] {
        let (out, tier) = launch_for(&["--model", "m", "-p"]);
        let out = tier.finish(out, version);
        assert_eq!(out, argv(&["--model", "m", "-p"]), "{version:?}");
    }
}

#[test]
fn finish_never_touches_a_callers_own_effort() {
    for tail in [
        &["--effort", "low", "--model", "m", "-p"][..],
        &["--effort=low", "--model", "m", "-p"][..],
    ] {
        for version in [Some("2.1.100"), None] {
            let (out, tier) = launch_for(tail);
            assert_eq!(out, argv(tail), "{tail:?}");
            assert_eq!(
                tier.finish(out, version),
                argv(tail),
                "{tail:?} {version:?}"
            );
        }
    }
}

// Withdrawal removes the pair `inject` added, by position, and nothing else.

#[test]
fn withdraw_leaves_a_caller_value_that_equals_the_flag() {
    // The caller's `--append-system-prompt` value is the text `--effort`.
    // `inject` reads that as the caller's own choice (its scan is not
    // arity-aware, on purpose), so the pair is built here as `inject` would
    // have prepended it, to pin that withdrawal goes by position.
    let caller = argv(&["--append-system-prompt", "--effort", "-p"]);
    assert!(matches!(
        inject(&caller, effort("xhigh")),
        Injection::CallerChose
    ));
    let mut out = argv(&["--effort", "xhigh"]);
    out.extend(caller.iter().cloned());
    assert_eq!(super::claude_effort::withdraw(out), caller);
}

#[test]
fn withdraw_leaves_a_tail_that_does_not_start_with_the_added_pair() {
    for tail in [
        argv(&["--model", "m", "--effort", "xhigh"]),
        argv(&["--effort", "--model", "m"]),
        argv(&[]),
    ] {
        assert_eq!(super::claude_effort::withdraw(tail.clone()), tail);
    }
}

#[test]
fn withdraw_leaves_positional_text_after_a_terminator() {
    let caller = argv(&["-p", "--", "--effort", "text"]);
    let out = added(inject(&caller, effort("xhigh")));
    assert_eq!(&out[..2], argv(&["--effort", "xhigh"]).as_slice());
    assert_eq!(super::claude_effort::withdraw(out), caller);
}

#[test]
fn inject_does_not_read_text_after_a_terminator_as_the_callers_choice() {
    let out = added(inject(
        &argv(&["-p", "--", "--effort", "x"]),
        effort("high"),
    ));
    assert_eq!(out[0], FLAG);
}

#[test]
fn a_model_word_is_not_an_effort() {
    assert!(Effort::parse("--model").is_none());
}

#[test]
fn withdraw_removes_only_the_added_pair() {
    let added = added(inject(
        &argv(&["--model", "m", "-p", "xhigh"]),
        effort("xhigh"),
    ));
    assert_eq!(
        super::claude_effort::withdraw(added),
        argv(&["--model", "m", "-p", "xhigh"])
    );
}
