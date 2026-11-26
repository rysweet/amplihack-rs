//! Issue #1619 (TDD): the pure resolver — the single decision point.
//!
//! Purity is a security property here, not only a testing one: every
//! rejection path is reachable from a table-driven unit test rather than from
//! a process spawn, so the rejection paths are actually exercised.
//!
//! The guarantees, asserted below:
//!
//! - **G1** no tier word ever leaves as a model value, on any harness;
//! - **G2** argv is byte-identical when there is no tier and no force;
//! - **G3** `--model` occurrences are replaced or deleted, never inserted;
//! - **G4** a concrete id is never read or rewritten (issue #1527).

use super::notice::Notice;
use super::record::Fallback;
use super::resolve::{Request, Resolution, resolve};
use super::table::{Effort, Table, Tier};

fn argv(words: &[&str]) -> Vec<String> {
    words.iter().map(|w| (*w).to_string()).collect()
}

fn request<'a>(harness: &'a str, args: &'a [String], table: &'a Table) -> Request<'a> {
    Request {
        harness,
        argv: args,
        table,
        force: None,
        default_model: None,
        through_gateway: false,
    }
}

/// The value of the last `--model` in a resolved argv, or `None`.
fn model_of(resolution: &Resolution) -> Option<&str> {
    let mut found = None;
    let mut i = 0;
    while i < resolution.argv.len() {
        let arg = resolution.argv[i].as_str();
        if arg == "--model" {
            found = resolution.argv.get(i + 1).map(String::as_str);
            i += 2;
            continue;
        }
        if let Some(value) = arg.strip_prefix("--model=") {
            found = Some(value);
        }
        i += 1;
    }
    found
}

// -------------------------------------------------------------------------
// G2 — nothing to do means nothing changes
// -------------------------------------------------------------------------

#[test]
fn argv_is_byte_identical_without_a_tier_or_a_force() {
    let table = Table::shipped();
    for args in [
        argv(&["exec", "--output-last-message", "/tmp/x", "-"]),
        argv(&["-p", "do the thing"]),
        argv(&[]),
        argv(&["--", "--model", "strong"]),
    ] {
        for harness in ["claude", "codex", "copilot"] {
            let out = resolve(&request(harness, &args, table));
            assert_eq!(
                out.argv, args,
                "{harness}: a launch with no tier must be untouched"
            );
            assert!(out.notices.is_empty(), "{harness}: {:?}", out.notices);
        }
    }
}

#[test]
fn a_tier_word_after_the_end_of_options_marker_is_not_a_tier() {
    let table = Table::shipped();
    let args = argv(&["exec", "--", "--model", "strong"]);
    let out = resolve(&request("codex", &args, table));
    assert_eq!(
        out.argv, args,
        "everything after `--` is the child's own argument, not amplihack's"
    );
}

// -------------------------------------------------------------------------
// G1/G3 — resolution per harness
// -------------------------------------------------------------------------

#[test]
fn a_tier_resolves_to_the_column_model_on_a_claude_compatible_harness() {
    let table = Table::shipped();
    for (tier, expected) in [
        ("strong", "claude-opus-5[1m]"),
        ("standard", "claude-sonnet-5-5"),
        ("fast", "claude-haiku-5-5"),
    ] {
        for harness in ["claude", "rusty", "rustyclawd", "amplifier"] {
            let args = argv(&["--model", tier, "-p"]);
            let out = resolve(&request(harness, &args, table));
            assert_eq!(model_of(&out), Some(expected), "{harness}/{tier}");
        }
    }
}

#[test]
fn a_codex_tier_carries_an_effort_and_deletes_the_model_pair() {
    // `strong` ships at `high`, not `xhigh`: a Codex model that does not
    // accept `xhigh` would fail every strong step of an instance with no tier
    // file of its own. An instance raises it in its own `[codex.strong]` cell.
    let table = Table::shipped();
    for (tier, effort) in [("strong", "high"), ("standard", "high"), ("fast", "low")] {
        let args = argv(&["exec", "--model", tier, "-"]);
        let out = resolve(&request("codex", &args, table));
        assert_eq!(model_of(&out), None, "codex/{tier}: {:?}", out.argv);
        assert_eq!(out.effort.map(Effort::as_str), Some(effort), "codex/{tier}");
        assert_eq!(
            out.argv,
            argv(&["exec", "-"]),
            "the pair is removed whole, and only the pair"
        );
    }
}

#[test]
fn no_tier_word_survives_on_any_harness() {
    // G1, stated absolutely. An unresolved tier word asks the CLI for a model
    // called "strong" and breaks the step.
    let table = Table::shipped();
    for tier in ["fast", "standard", "strong", "Strong", " fast "] {
        for harness in ["claude", "rusty", "codex", "copilot", "gemini"] {
            let args = argv(&["--model", tier, "-p"]);
            let out = resolve(&request(harness, &args, table));
            for word in ["fast", "standard", "strong"] {
                assert_ne!(
                    model_of(&out),
                    Some(word),
                    "{harness}: tier word left as a model value: {:?}",
                    out.argv
                );
            }
        }
    }
}

#[test]
fn every_occurrence_is_rewritten_and_none_is_added() {
    let table = Table::shipped();
    let args = argv(&[
        "--model",
        "fast",
        "--model=standard",
        "--model",
        "strong",
        "-p",
    ]);
    let out = resolve(&request("claude", &args, table));
    let occurrences = out
        .argv
        .iter()
        .filter(|a| *a == "--model" || a.starts_with("--model="))
        .count();
    assert_eq!(
        occurrences, 3,
        "three in, three out: G3 says replaced or deleted, never inserted. argv: {:?}",
        out.argv
    );
    assert_eq!(out.argv.len(), args.len(), "argv: {:?}", out.argv);
}

#[test]
fn the_attached_spelling_resolves_to_the_attached_spelling() {
    let table = Table::shipped();
    let args = argv(&["--model=standard", "-p"]);
    let out = resolve(&request("claude", &args, table));
    assert_eq!(
        out.argv,
        argv(&["--model=claude-sonnet-5-5", "-p"]),
        "a rewrite keeps the operator's spelling shape, so nothing downstream that \
         counts arguments sees a different count"
    );
}

#[test]
fn a_model_word_in_a_value_position_is_not_a_flag() {
    // `--output-last-message` takes a value. Without arity-aware walking this
    // would delete the following argument and change where codex writes.
    let table = Table::shipped();
    let args = argv(&["exec", "--output-last-message", "--model", "-"]);
    let out = resolve(&request("codex", &args, table));
    assert_eq!(out.argv, args, "argv: {:?}", out.argv);
}

#[test]
fn a_dangling_model_with_no_value_changes_nothing() {
    let table = Table::shipped();
    let args = argv(&["exec", "-", "--model"]);
    let out = resolve(&request("codex", &args, table));
    assert_eq!(out.argv, args, "argv: {:?}", out.argv);
}

#[test]
fn a_codex_short_option_with_an_attached_value_is_walked_correctly() {
    let table = Table::shipped();
    let args = argv(&["exec", "-sread-only", "--model", "fast", "-"]);
    let out = resolve(&request("codex", &args, table));
    assert_eq!(
        out.argv,
        argv(&["exec", "-sread-only", "-"]),
        "the caller's sandbox choice survives the rewrite: {:?}",
        out.argv
    );
}

// -------------------------------------------------------------------------
// G4 — a concrete id is never read or rewritten
// -------------------------------------------------------------------------

#[test]
fn a_concrete_id_is_forwarded_exactly_as_typed() {
    let table = Table::shipped();
    for id in [
        "claude-opus-5[1m]",
        "claude-sonnet-5.5",
        "gpt-6.1-sol",
        "auto",
        "some-model-nobody-has",
    ] {
        for harness in ["claude", "codex", "copilot"] {
            let args = argv(&["--model", id, "-p"]);
            let out = resolve(&request(harness, &args, table));
            assert_eq!(out.argv, args, "{harness}/{id}: issue #1527");
            assert!(out.notices.is_empty(), "{harness}/{id}: {:?}", out.notices);
            assert!(
                out.record.is_none() || out.effort.is_none(),
                "a concrete id is not a tier and carries no tier's effort"
            );
        }
    }
}

#[test]
fn the_rate_limit_auto_model_is_treated_as_a_concrete_id() {
    // The pinned runner substitutes the literal `auto` for the step's model on
    // its final attempt, before amplihack runs. `auto` is not one of the three
    // words, so it is forwarded, and the tier's effort is lost with it — which
    // is stated in the reference rather than silently worked around.
    let table = Table::shipped();
    let args = argv(&["exec", "--model", "auto", "-"]);
    let out = resolve(&request("codex", &args, table));
    assert_eq!(out.argv, args);
    assert_eq!(out.effort, None);
}

// -------------------------------------------------------------------------
// The fall-back: remove, never substitute
// -------------------------------------------------------------------------

#[test]
fn a_harness_with_no_column_deletes_the_pair_and_reports_it_once() {
    let table = Table::shipped();
    let args = argv(&["--model", "strong", "-p"]);
    let out = resolve(&request("gemini", &args, table));
    assert_eq!(out.argv, argv(&["-p"]));
    assert_eq!(out.notices.len(), 1, "{:?}", out.notices);
    assert!(
        matches!(out.notices[0], Notice::NoColumn { .. }),
        "{:?}",
        out.notices
    );
    assert_eq!(
        out.record.as_ref().and_then(|r| r.fallback),
        Some(Fallback::NoColumn)
    );
}

#[test]
fn a_tier_with_no_entry_in_its_column_falls_back() {
    let (overrides, _) =
        Table::parse_override("version = 1\n[claude.strong]\n").expect("valid override");
    let table = overrides.merged_over(Table::shipped());
    let args = argv(&["--model", "strong", "-p"]);
    let out = resolve(&request("claude", &args, &table));
    assert_eq!(
        out.argv,
        argv(&["-p"]),
        "falling back removes the tier word and substitutes nothing of its own; the \
         existing audited chain then decides"
    );
    assert_eq!(
        out.record.as_ref().and_then(|r| r.fallback),
        Some(Fallback::NoModel)
    );
}

#[test]
fn the_shipped_copilot_column_falls_back_for_every_tier() {
    let table = Table::shipped();
    for tier in ["fast", "standard", "strong"] {
        let args = argv(&["--model", tier, "-p", "go"]);
        let out = resolve(&request("copilot", &args, table));
        assert_eq!(out.argv, argv(&["-p", "go"]), "copilot/{tier}");
        assert!(out.record.as_ref().and_then(|r| r.fallback).is_some());
    }
}

#[test]
fn a_fall_back_never_invents_a_model_name() {
    // Issue #1421's lesson: no path here may write a model string that is not
    // in the table or in model.force, or a stale name reappears in a 404 that
    // nobody chose.
    let (overrides, _) =
        Table::parse_override("version = 1\n[claude.strong]\n[claude.standard]\n[claude.fast]\n")
            .expect("valid override");
    let table = overrides.merged_over(Table::shipped());
    for tier in ["fast", "standard", "strong"] {
        for harness in ["claude", "codex", "copilot", "gemini"] {
            let args = argv(&["--model", tier]);
            let out = resolve(&request(harness, &args, &table));
            assert!(
                !out.argv
                    .iter()
                    .any(|a| a == "--model" || a.starts_with("--model=")),
                "{harness}/{tier}: the fall-back deletes, it does not substitute: {:?}",
                out.argv
            );
        }
    }
}

#[test]
fn a_gateway_launch_deletes_the_tier_and_says_why() {
    let table = Table::shipped();
    let args = argv(&["--model", "strong", "-p"]);
    let out = resolve(&Request {
        harness: "claude",
        argv: &args,
        table,
        force: None,
        default_model: None,
        through_gateway: true,
    });
    assert_eq!(
        out.argv,
        argv(&["-p"]),
        "deleting the pair is what lets amplihack's own chain write \
         --model $AMPLIHACK_LITELLM_MODEL in its place"
    );
    assert!(
        matches!(out.notices.as_slice(), [Notice::Gateway { .. }]),
        "{:?}",
        out.notices
    );
    assert_eq!(
        out.record.as_ref().and_then(|r| r.fallback),
        Some(Fallback::Gateway)
    );
}

// -------------------------------------------------------------------------
// model.force
// -------------------------------------------------------------------------

#[test]
fn a_forced_model_replaces_a_tier_and_keeps_the_tiers_effort() {
    let table = Table::shipped();
    for (tier, effort) in [("strong", "high"), ("fast", "low")] {
        let args = argv(&["exec", "--model", tier, "-"]);
        let out = resolve(&Request {
            harness: "codex",
            argv: &args,
            table,
            force: Some("gpt-6.1-mini"),
            default_model: None,
            through_gateway: false,
        });
        assert_eq!(model_of(&out), Some("gpt-6.1-mini"), "codex/{tier}");
        assert_eq!(
            out.effort.map(Effort::as_str),
            Some(effort),
            "forcing one model across a mixed-tier recipe must not flatten effort"
        );
    }
}

#[test]
fn a_forced_model_supplies_one_where_the_command_has_none() {
    let table = Table::shipped();
    let args = argv(&["-p"]);
    let out = resolve(&Request {
        harness: "claude",
        argv: &args,
        table,
        force: Some("claude-haiku-5-5"),
        default_model: None,
        through_gateway: false,
    });
    assert_eq!(model_of(&out), Some("claude-haiku-5-5"), "{:?}", out.argv);
}

#[test]
fn a_forced_model_lands_before_the_end_of_options_marker() {
    // Everything past `--` is the caller's, forwarded verbatim (D9). A pair
    // appended there is not an option amplihack set: for `-p` it makes a
    // second prompt, which the Claude passthrough rewriter refuses outright,
    // and outside print mode it reaches the harness as positional text while
    // `Notice::Forced` says the model was applied.
    let table = Table::shipped();
    for args in [
        argv(&["-p", "--", "the user prompt"]),
        argv(&["--", "--settings", "/x"]),
    ] {
        let out = resolve(&Request {
            harness: "claude",
            argv: &args,
            table,
            force: Some("claude-haiku-5-5"),
            default_model: None,
            through_gateway: false,
        });
        let marker = out
            .argv
            .iter()
            .position(|arg| arg == "--")
            .expect("the marker survives");
        let model = out
            .argv
            .iter()
            .position(|arg| arg == "--model")
            .expect("the forced model is on the command line");
        assert!(
            model < marker,
            "the pair belongs in amplihack's own region: {:?}",
            out.argv
        );
        assert_eq!(model_of(&out), Some("claude-haiku-5-5"), "{:?}", out.argv);
    }
}

#[test]
fn a_forced_model_does_not_make_a_second_prompt_in_print_mode() {
    // The reproduction: `-p -- <prompt>` plus `model.force` used to come back
    // with the pair after the marker, and `rewrite_claude_passthrough` then
    // failed the launch with "the prompt was given twice".
    use crate::commands::launch::passthrough_rewrite::rewrite_claude_passthrough;
    let table = Table::shipped();
    let args = argv(&["-p", "--", "the user prompt"]);
    let out = resolve(&Request {
        harness: "claude",
        argv: &args,
        table,
        force: Some("claude-haiku-5-5"),
        default_model: None,
        through_gateway: false,
    });
    assert!(
        rewrite_claude_passthrough(&out.argv).is_ok(),
        "a forced model must not stop a launch the same tail runs without it: {:?}",
        out.argv
    );
}

#[test]
fn a_forced_model_with_no_marker_is_appended_as_before() {
    let table = Table::shipped();
    let args = argv(&["-p"]);
    let out = resolve(&Request {
        harness: "claude",
        argv: &args,
        table,
        force: Some("claude-haiku-5-5"),
        default_model: None,
        through_gateway: false,
    });
    assert_eq!(
        out.argv,
        argv(&["-p", "--model", "claude-haiku-5-5"]),
        "with no end-of-options marker the whole tail is amplihack's region"
    );
}

#[test]
fn a_forced_model_outranks_an_explicit_concrete_id() {
    // The one narrow, documented exception to issue #1527. It fires only from
    // the operator's own user config or environment.
    let table = Table::shipped();
    let args = argv(&["--model", "claude-opus-5[1m]", "-p"]);
    let out = resolve(&Request {
        harness: "claude",
        argv: &args,
        table,
        force: Some("claude-haiku-5-5"),
        default_model: None,
        through_gateway: false,
    });
    assert_eq!(model_of(&out), Some("claude-haiku-5-5"));
    assert!(
        out.notices
            .iter()
            .any(|n| matches!(n, Notice::Forced { .. })),
        "the exception is announced, never silent: {:?}",
        out.notices
    );
}

#[test]
fn a_forced_model_does_not_route_past_the_gateway() {
    let table = Table::shipped();
    let args = argv(&["--model", "strong", "-p"]);
    let out = resolve(&Request {
        harness: "claude",
        argv: &args,
        table,
        force: Some("claude-haiku-5-5"),
        default_model: None,
        through_gateway: true,
    });
    assert_eq!(
        model_of(&out),
        None,
        "the gateway routes on the model name and has no default of its own; a forced \
         model cannot route past it either: {:?}",
        out.argv
    );
}

#[test]
fn a_forced_model_that_does_not_validate_is_refused_and_the_tier_applies() {
    let table = Table::shipped();
    let args = argv(&["--model", "fast", "-p"]);
    let out = resolve(&Request {
        harness: "claude",
        argv: &args,
        table,
        force: Some("--dangerously-skip-permissions"),
        default_model: None,
        through_gateway: false,
    });
    assert_eq!(
        model_of(&out),
        Some("claude-haiku-5-5"),
        "a forced value that could act as another argument is refused, and the step's \
         own tier still applies: {:?}",
        out.argv
    );
    assert!(!out.notices.is_empty(), "the refusal is reported");
}

// -------------------------------------------------------------------------
// The record, and the invariant that holds it to argv
// -------------------------------------------------------------------------

#[test]
fn the_record_distinguishes_requested_from_effective() {
    let (overrides, _) =
        Table::parse_override("version = 1\n[claude.strong]\n").expect("valid override");
    let table = overrides.merged_over(Table::shipped());
    let args = argv(&["--model", "strong", "-p"]);
    let out = resolve(&request("claude", &args, &table));
    let record = out.record.expect("a tiered launch records one line");
    assert_eq!(record.tier.map(Tier::as_str), Some("strong"));
    assert_eq!(record.effective_model, None);
    assert_eq!(record.fallback, Some(Fallback::NoModel));
}

#[test]
fn the_notice_and_the_record_cannot_disagree() {
    // Both derive from one value, so a change to one cannot leave the other
    // naming something else.
    let table = Table::shipped();
    let args = argv(&["--model", "strong", "-p"]);
    let out = resolve(&request("copilot", &args, table));
    let fallback = out.record.as_ref().and_then(|r| r.fallback);
    assert!(fallback.is_some());
    assert_eq!(
        out.notices.len(),
        1,
        "one launch, one record, no attempts to count: {:?}",
        out.notices
    );
}

#[test]
fn a_resolved_tier_records_no_fall_back() {
    let table = Table::shipped();
    let args = argv(&["--model", "fast", "-p"]);
    let out = resolve(&request("claude", &args, table));
    assert_eq!(
        out.record.as_ref().and_then(|r| r.fallback),
        None,
        "`fallback` is null when the launch did not fall back"
    );
}

#[test]
fn no_emitted_token_after_model_can_act_as_another_argument() {
    let table = Table::shipped();
    for tier in ["fast", "standard", "strong"] {
        for harness in ["claude", "codex", "copilot", "rusty"] {
            let args = argv(&["--model", tier, "-p"]);
            let out = resolve(&request(harness, &args, table));
            for (i, arg) in out.argv.iter().enumerate() {
                if arg == "--model" {
                    let value = out.argv.get(i + 1).map(String::as_str).unwrap_or("");
                    assert!(
                        !value.starts_with('-') && !value.is_empty(),
                        "{harness}/{tier}: {:?}",
                        out.argv
                    );
                }
            }
        }
    }
}

// -------------------------------------------------------------------------
// Issue #2916: the Claude column's reasoning effort
// -------------------------------------------------------------------------

#[test]
fn claude_strong_on_the_shipped_table_resolves_to_xhigh() {
    let table = Table::shipped();
    let args = argv(&["--model", "strong", "-p"]);
    let out = resolve(&request("claude", &args, table));
    assert_eq!(out.effort, Some(Effort::Xhigh));
    let record = out.record.as_ref().expect("recorded");
    assert_eq!(record.requested_effort, Some(Effort::Xhigh));
    assert_eq!(record.effective_effort, Some(Effort::Xhigh));
}

#[test]
fn claude_standard_and_fast_ask_for_no_effort() {
    let table = Table::shipped();
    for tier in ["standard", "fast"] {
        let args = argv(&["--model", tier, "-p"]);
        let out = resolve(&request("claude", &args, table));
        assert_eq!(out.effort, None, "{tier}");
    }
}

#[test]
fn a_concrete_claude_model_gets_no_tier_and_no_effort() {
    let table = Table::shipped();
    let args = argv(&["--model", "claude-opus-5[1m]", "-p"]);
    let out = resolve(&request("claude", &args, table));
    assert_eq!(out.effort, None);
    assert!(out.record.is_none());
}

#[test]
fn a_gateway_claude_launch_keeps_the_effort_and_still_says_why() {
    let table = Table::shipped();
    let args = argv(&["--model", "strong", "-p"]);
    let out = resolve(&Request {
        harness: "claude",
        argv: &args,
        table,
        force: None,
        default_model: None,
        through_gateway: true,
    });
    assert_eq!(out.argv, argv(&["-p"]), "the model pair is still deleted");
    assert_eq!(
        out.effort,
        Some(Effort::Xhigh),
        "an effort is a session setting, not a routing key"
    );
    assert!(
        out.notices
            .iter()
            .any(|n| matches!(n, Notice::Gateway { .. })),
        "the gateway line must keep printing: {:?}",
        out.notices
    );
}

#[test]
fn a_column_with_no_effort_control_still_gets_no_effort() {
    let table = Table::shipped();
    let args = argv(&["--model", "strong"]);
    let out = resolve(&request("copilot", &args, table));
    assert_eq!(out.effort, None);
}
