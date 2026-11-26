//! Issue #1619, B2 and B-new-1 (TDD): an operator's own model choice
//! outranks the shipped Claude tier column, and reaches no other column.
//!
//! Before this feature, `AMPLIHACK_DEFAULT_MODEL` chose the model for every
//! Claude-compatible launch amplihack made (`claude`, `rusty`, `rustyclawd`
//! and `amplifier`) and was never read for Codex or Copilot. The tier table
//! must not take that lever away from the Claude column, and must not hand it
//! to the others: the #1619 issue comment of 16 November says amplihack runs
//! on harnesses where Opus and Sonnet do not exist, and a Claude id written
//! onto a Codex command line fails the step.
//!
//! The rank is 3, below `model.force` and below an explicit concrete
//! `--model`, and above the per-step override and the table. On the Claude
//! column it replaces a tier word and only a tier word, so:
//!
//! - a concrete id is still forwarded as typed (issue #1527);
//! - no `--model` pair is ever inserted where the command line had none
//!   (G3), which is what keeps the untiered command line byte-identical.
//!
//! On every other column the variable is not read at all: the column's own
//! cell decides, a Codex user names a model in a `[codex.<tier>]` cell, and
//! no line about the variable is printed.
//!
//! The value is held to [`validate_model_id`] by the same code and in the
//! same position as `model.force`: it is an environment variable, so a
//! parent recipe step, a CI configuration or a `direnv` file in a cloned tree
//! can set it, and the string lands as the argv token straight after
//! `--model`.

use super::notice::Notice;
use super::record::Fallback;
use super::resolve::{Request, resolve};
use super::table::{Effort, Table, Tier};

/// A Claude model id an operator might export for their Claude launches. It
/// is the shipped `standard` cell, so it passes validation everywhere.
const CLAUDE_ID: &str = "claude-sonnet-5-5";

fn argv(words: &[&str]) -> Vec<String> {
    words.iter().map(|w| (*w).to_string()).collect()
}

/// A request with an operator default and nothing else set.
fn with_default<'a>(
    harness: &'a str,
    args: &'a [String],
    table: &'a Table,
    default_model: Option<&'a str>,
) -> Request<'a> {
    Request {
        harness,
        argv: args,
        table,
        force: None,
        default_model,
        through_gateway: false,
        local_model: false,
    }
}

/// The value of the last `--model` in a resolved argv, or `None`.
fn model_of(resolved: &[String]) -> Option<&str> {
    let mut found = None;
    let mut i = 0;
    while i < resolved.len() {
        let arg = resolved[i].as_str();
        if arg == "--model" {
            found = resolved.get(i + 1).map(String::as_str);
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

/// Neither line about `AMPLIHACK_DEFAULT_MODEL` is in `notices`.
fn assert_no_default_notice(harness: &str, notices: &[Notice]) {
    assert!(
        !notices.iter().any(|n| matches!(
            n,
            Notice::DefaultModel { .. } | Notice::DefaultModelRefused { .. }
        )),
        "{harness}: the variable is not read on this column, so nothing is said \
         about it: {notices:?}"
    );
}

/// Off the Claude column the variable leaves no trace: resolving with
/// `default_model` set gives the same argv, effort, notices and record as
/// resolving with it unset.
fn assert_same_as_unset(harness: &str, args: &[String], table: &Table, default_model: &str) {
    let set = resolve(&with_default(harness, args, table, Some(default_model)));
    let unset = resolve(&with_default(harness, args, table, None));
    assert_eq!(set.argv, unset.argv, "{harness} {args:?}: argv");
    assert_eq!(set.effort, unset.effort, "{harness} {args:?}: effort");
    assert_eq!(set.notices, unset.notices, "{harness} {args:?}: notices");
    assert_eq!(set.record, unset.record, "{harness} {args:?}: record");
}

// -------------------------------------------------------------------------
// It replaces a tier word on the Claude column
// -------------------------------------------------------------------------

#[test]
fn an_operator_default_replaces_the_shipped_claude_tier_model() {
    let table = Table::shipped();
    let args = argv(&["--model", "strong", "-p"]);
    let out = resolve(&with_default(
        "claude",
        &args,
        table,
        Some("my-house-model"),
    ));
    assert_eq!(
        model_of(&out.argv),
        Some("my-house-model"),
        "a set AMPLIHACK_DEFAULT_MODEL outranks the shipped tier column: {:?}",
        out.argv
    );
}

#[test]
fn an_operator_default_applies_to_every_tier_word() {
    let table = Table::shipped();
    for tier in Tier::ALL.map(Tier::as_str) {
        let args = argv(&["--model", tier, "-p"]);
        let out = resolve(&with_default("claude", &args, table, Some("house-model")));
        assert_eq!(
            model_of(&out.argv),
            Some("house-model"),
            "`{tier}` must take the operator's default like every other tier: {:?}",
            out.argv
        );
    }
}

#[test]
fn every_claude_compatible_tool_takes_the_operator_default() {
    let table = Table::shipped();
    for harness in ["claude", "rusty", "rustyclawd", "amplifier"] {
        let args = argv(&["--model", "strong", "-p"]);
        let out = resolve(&with_default(harness, &args, table, Some("house-model")));
        assert_eq!(
            model_of(&out.argv),
            Some("house-model"),
            "{harness} shares the `claude` column, which is where the variable \
             applied before tiers: {:?}",
            out.argv
        );
    }
}

// -------------------------------------------------------------------------
// B-new-1: it never reaches a column other than Claude's
// -------------------------------------------------------------------------

/// The #1619 issue comment of 16 November: amplihack runs on harnesses where
/// Opus and Sonnet do not exist. A Claude id an operator exported for their
/// Claude launches must not become the model every tiered Codex step asks
/// for; the step would fail.
#[test]
fn a_claude_operator_default_never_reaches_codex() {
    let table = Table::shipped();
    for tier in Tier::ALL.map(Tier::as_str) {
        let args = argv(&["exec", "--model", tier, "-"]);
        let out = resolve(&with_default("codex", &args, table, Some(CLAUDE_ID)));
        assert_eq!(
            model_of(&out.argv),
            None,
            "codex/{tier}: the shipped codex cell names no model, and the Claude \
             column's variable is not read here: {:?}",
            out.argv
        );
        assert!(
            !out.argv.iter().any(|a| a.contains("claude")),
            "codex/{tier}: no Claude id on a Codex command line: {:?}",
            out.argv
        );
        assert_eq!(out.argv, argv(&["exec", "-"]), "codex/{tier}");
        assert_eq!(
            out.record.as_ref().and_then(|r| r.fallback),
            Some(Fallback::NoModel),
            "codex/{tier}: the model falls back to Codex's own, as with the variable unset"
        );
        assert_no_default_notice("codex", &out.notices);
        assert_same_as_unset("codex", &args, table, CLAUDE_ID);
    }
}

#[test]
fn a_claude_operator_default_leaves_the_codex_tier_effort_in_place() {
    let table = Table::shipped();
    let args = argv(&["exec", "--model", "strong", "-"]);
    let out = resolve(&with_default("codex", &args, table, Some(CLAUDE_ID)));
    assert_eq!(
        out.effort,
        Some(Effort::High),
        "the shipped codex `strong` effort reaches Codex whatever the Claude column's \
         variable says"
    );
}

#[test]
fn a_claude_operator_default_never_reaches_copilot() {
    let table = Table::shipped();
    for tier in Tier::ALL.map(Tier::as_str) {
        let args = argv(&["--model", tier, "-p"]);
        let out = resolve(&with_default("copilot", &args, table, Some(CLAUDE_ID)));
        assert_eq!(
            model_of(&out.argv),
            None,
            "copilot/{tier}: the shipped copilot column is empty and the variable is \
             not read here: {:?}",
            out.argv
        );
        assert!(
            !out.argv.iter().any(|a| a.contains("claude")),
            "copilot/{tier}: {:?}",
            out.argv
        );
        assert_eq!(
            out.record.as_ref().and_then(|r| r.fallback),
            Some(Fallback::NoMapping),
            "copilot/{tier}"
        );
        assert_no_default_notice("copilot", &out.notices);
        assert_same_as_unset("copilot", &args, table, CLAUDE_ID);
    }
}

#[test]
fn a_harness_with_no_column_ignores_the_operator_default() {
    let table = Table::shipped();
    let args = argv(&["--model", "strong", "-p"]);
    let out = resolve(&with_default("gemini", &args, table, Some(CLAUDE_ID)));
    assert_eq!(
        model_of(&out.argv),
        None,
        "only the Claude-compatible tools read the variable: {:?}",
        out.argv
    );
    assert_eq!(
        out.record.as_ref().and_then(|r| r.fallback),
        Some(Fallback::NoColumn)
    );
    assert_no_default_notice("gemini", &out.notices);
    assert_same_as_unset("gemini", &args, table, CLAUDE_ID);
}

#[test]
fn an_instance_codex_cell_names_the_codex_model_whatever_the_operator_default() {
    // What a Codex user who knows their account's model does instead: name it
    // in a `[codex.<tier>]` cell of their own tier file.
    let (overrides, notices) = Table::parse_override(
        "version = 1\n[codex.strong]\nmodel = \"gpt-6.1-sol\"\neffort = \"high\"\n",
    )
    .expect("valid override");
    assert!(notices.is_empty(), "{notices:?}");
    let table = overrides.merged_over(Table::shipped());
    let args = argv(&["exec", "--model", "strong", "-"]);
    let out = resolve(&with_default("codex", &args, &table, Some(CLAUDE_ID)));
    assert_eq!(
        out.argv,
        argv(&["exec", "--model", "gpt-6.1-sol", "-"]),
        "the instance cell decides the Codex model"
    );
    assert_eq!(out.effort, Some(Effort::High));
    assert_eq!(out.record.as_ref().and_then(|r| r.fallback), None);
    assert_no_default_notice("codex", &out.notices);
}

#[test]
fn an_argument_shaped_operator_default_is_not_reported_on_codex_or_copilot() {
    // The variable is not read on these columns, so a value it would reject
    // is not reported there either: the Claude-column launch that reads it
    // says so.
    let table = Table::shipped();
    for (harness, args) in [
        ("codex", argv(&["exec", "--model", "fast", "-"])),
        ("copilot", argv(&["--model", "fast", "-p"])),
    ] {
        let out = resolve(&with_default(
            harness,
            &args,
            table,
            Some("--dangerously-skip-permissions"),
        ));
        assert!(
            !out.argv
                .iter()
                .any(|a| a == "--dangerously-skip-permissions"),
            "{harness}: {:?}",
            out.argv
        );
        assert_no_default_notice(harness, &out.notices);
        assert_same_as_unset(harness, &args, table, "--dangerously-skip-permissions");
    }
}

#[test]
fn a_forced_model_still_reaches_codex_with_an_operator_default_set() {
    // `model.force` keeps its rank and its reach: the user chose it for every
    // step, on every harness.
    let table = Table::shipped();
    let args = argv(&["exec", "--model", "strong", "-"]);
    let out = resolve(&Request {
        harness: "codex",
        argv: &args,
        table,
        force: Some("gpt-6.1-mini"),
        default_model: Some(CLAUDE_ID),
        through_gateway: false,
        local_model: false,
    });
    assert_eq!(model_of(&out.argv), Some("gpt-6.1-mini"), "{:?}", out.argv);
    assert_eq!(out.effort, Some(Effort::High));
}

#[test]
fn an_operator_default_is_announced_once() {
    let table = Table::shipped();
    let args = argv(&["--model", "fast", "-p"]);
    let out = resolve(&with_default("claude", &args, table, Some("house-model")));
    let announced = out
        .notices
        .iter()
        .filter(|n| matches!(n, Notice::DefaultModel { .. }))
        .count();
    assert_eq!(
        announced, 1,
        "an operator default that displaced a tier says so, once: {:?}",
        out.notices
    );
}

#[test]
fn the_record_names_the_operator_default_as_the_effective_model() {
    let table = Table::shipped();
    let args = argv(&["--model", "standard", "-p"]);
    let out = resolve(&with_default("claude", &args, table, Some("house-model")));
    let record = out.record.expect("a tiered launch records one line");
    assert_eq!(
        record.requested_model.as_deref(),
        Some("claude-sonnet-5-5"),
        "the record still says what the table asked for"
    );
    assert_eq!(
        record.effective_model.as_deref(),
        Some("house-model"),
        "and what actually went on the command line"
    );
    assert_eq!(record.fallback, None);
}

// -------------------------------------------------------------------------
// Where it sits: below force, below an explicit id, above the table
// -------------------------------------------------------------------------

#[test]
fn a_forced_model_outranks_an_operator_default() {
    let table = Table::shipped();
    let args = argv(&["--model", "strong", "-p"]);
    let out = resolve(&Request {
        harness: "claude",
        argv: &args,
        table,
        force: Some("forced-model"),
        default_model: Some("house-model"),
        through_gateway: false,
        local_model: false,
    });
    assert_eq!(
        model_of(&out.argv),
        Some("forced-model"),
        "`model.force` is rank 1 and the operator default is rank 3: {:?}",
        out.argv
    );
}

#[test]
fn an_explicit_concrete_id_outranks_an_operator_default() {
    let table = Table::shipped();
    let args = argv(&["--model", "claude-haiku-5-5", "-p"]);
    let out = resolve(&with_default("claude", &args, table, Some("house-model")));
    assert_eq!(
        out.argv, args,
        "issue #1527: a concrete id the caller typed is forwarded untouched, and the \
         operator default replaces a tier word only: {:?}",
        out.argv
    );
}

#[test]
fn an_operator_default_outranks_the_table_but_not_a_gateway_launch() {
    let table = Table::shipped();
    let args = argv(&["--model", "strong", "-p"]);
    let out = resolve(&Request {
        harness: "claude",
        argv: &args,
        table,
        force: None,
        default_model: Some("house-model"),
        through_gateway: true,
        local_model: false,
    });
    assert_eq!(
        model_of(&out.argv),
        None,
        "the gateway routes on the model name and has no default of its own; an \
         environment variable must not route past it either, exactly as a forced \
         model cannot: {:?}",
        out.argv
    );
    assert_eq!(
        out.record.as_ref().and_then(|r| r.fallback),
        Some(Fallback::Gateway)
    );
}

// -------------------------------------------------------------------------
// G3 — it inserts nothing, and an unset value decides nothing
// -------------------------------------------------------------------------

#[test]
fn an_operator_default_never_inserts_a_model_pair() {
    let table = Table::shipped();
    for args in [
        argv(&["exec", "--output-last-message", "/tmp/x", "-"]),
        argv(&["-p", "do the thing"]),
        argv(&[]),
        argv(&["--", "--model", "strong"]),
    ] {
        for harness in ["claude", "codex", "copilot"] {
            let out = resolve(&with_default(harness, &args, table, Some("house-model")));
            assert_eq!(
                out.argv, args,
                "{harness}: with no tier word to replace there is nothing to do, and \
                 inserting a pair here would duplicate the one `model_args` adds or \
                 land past `--`"
            );
            assert!(out.notices.is_empty(), "{harness}: {:?}", out.notices);
        }
    }
}

#[test]
fn no_operator_default_leaves_the_table_in_force() {
    // The unset case must stay the shipped behaviour: `configured_default_model`
    // answers `BuiltInDefault` when the variable is unset, and treating that as
    // an operator decision would flatten every tier onto one model and hide
    // every fall-back the feature exists to report.
    let table = Table::shipped();
    let args = argv(&["--model", "fast", "-p"]);
    let out = resolve(&with_default("claude", &args, table, None));
    assert_eq!(
        model_of(&out.argv),
        Some("claude-haiku-5-5"),
        "{:?}",
        out.argv
    );

    let args = argv(&["--model", "strong", "-p"]);
    let out = resolve(&with_default("gemini", &args, table, None));
    assert_eq!(
        out.record.as_ref().and_then(|r| r.fallback),
        Some(Fallback::NoColumn),
        "the fall-backs stay reachable"
    );
}

#[test]
fn a_blank_operator_default_is_not_a_decision() {
    let table = Table::shipped();
    let args = argv(&["--model", "fast", "-p"]);
    for blank in ["", "   "] {
        let out = resolve(&with_default("claude", &args, table, Some(blank)));
        assert_eq!(
            model_of(&out.argv),
            Some("claude-haiku-5-5"),
            "a blank value means `pass nothing and let the CLI decide`, which is not \
             a model the table should lose to: {:?}",
            out.argv
        );
    }
}

// -------------------------------------------------------------------------
// V1 — the exact string that reaches argv is validated
// -------------------------------------------------------------------------

#[test]
fn an_operator_default_that_could_act_as_another_argument_is_refused() {
    let table = Table::shipped();
    let args = argv(&["--model", "fast", "-p"]);
    let out = resolve(&with_default(
        "claude",
        &args,
        table,
        Some("--dangerously-skip-permissions"),
    ));
    assert_eq!(
        model_of(&out.argv),
        Some("claude-haiku-5-5"),
        "a value that could act as another argument is refused and the step's own \
         tier still applies: {:?}",
        out.argv
    );
    assert!(
        out.notices
            .iter()
            .any(|n| matches!(n, Notice::DefaultModelRefused { .. })),
        "the refusal is reported: {:?}",
        out.notices
    );
}

#[test]
fn a_refusal_line_never_carries_the_refused_value() {
    let table = Table::shipped();
    let args = argv(&["--model", "fast", "-p"]);
    let secret = "--dangerously-skip-permissions";
    let out = resolve(&with_default("claude", &args, table, Some(secret)));
    for notice in &out.notices {
        assert!(
            !notice.render().contains(secret),
            "a refusal names the rule broken, never the refused value: {}",
            notice.render()
        );
    }
}

#[test]
fn no_token_an_operator_default_emits_can_act_as_another_argument() {
    let table = Table::shipped();
    let args = argv(&["--model", "strong", "-p"]);
    for candidate in [
        "-p",
        "--model",
        "model with spaces",
        "model\nnewline",
        "model;rm -rf /",
        "$(id)",
        "`id`",
        "a/../../etc/passwd",
    ] {
        let out = resolve(&with_default("claude", &args, table, Some(candidate)));
        for (i, arg) in out.argv.iter().enumerate() {
            if arg != "--model" {
                continue;
            }
            let value = out.argv.get(i + 1).map(String::as_str).unwrap_or_default();
            assert!(
                !value.starts_with('-'),
                "`{candidate}` reached argv as `{value}`"
            );
            assert!(
                value.chars().all(|c| {
                    c.is_ascii_alphanumeric()
                        || matches!(c, '.' | '_' | '-' | '[' | ']' | '@' | ':')
                }),
                "`{candidate}` reached argv as `{value}`"
            );
        }
    }
}

#[test]
fn an_over_long_operator_default_is_refused() {
    let table = Table::shipped();
    let args = argv(&["--model", "standard", "-p"]);
    let long = "a".repeat(super::table::MAX_MODEL_ID_BYTES + 1);
    let out = resolve(&with_default("claude", &args, table, Some(&long)));
    assert_eq!(
        model_of(&out.argv),
        Some("claude-sonnet-5-5"),
        "over the length bound, so the table applies: {:?}",
        out.argv
    );
}
