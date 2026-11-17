//! Issue #1619, B2 (TDD): an operator's own model choice outranks the shipped
//! tier column.
//!
//! Before this feature, `AMPLIHACK_DEFAULT_MODEL` chose the model for every
//! launch amplihack made. The tier table must not take that lever away, or a
//! backend with no Opus and no Sonnet — Ryan's condition on the issue — loses
//! the only setting that made amplihack usable there: every tiered step would
//! ask for a Claude id the account does not have.
//!
//! The rank is 3, below `model.force` and below an explicit concrete
//! `--model`, and above the per-step override and the table. It replaces a
//! tier word and only a tier word, on every column, so:
//!
//! - a concrete id is still forwarded as typed (issue #1527);
//! - no `--model` pair is ever inserted where the command line had none
//!   (G3), which is what keeps the untiered command line byte-identical;
//! - the tier's Codex reasoning effort still reaches the harness, because
//!   the substitution is a resolved model, not a fall-back.
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

// -------------------------------------------------------------------------
// It replaces a tier word, on every column
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

/// Ryan's condition, stated on the issue on 16 November: amplihack runs on
/// harnesses where Opus and Sonnet do not exist. The operator's own id has to
/// reach those steps, and `codex` is the shipped column that names no model.
#[test]
fn an_operator_default_reaches_a_column_with_no_opus_and_no_sonnet() {
    let table = Table::shipped();
    let args = argv(&["exec", "--model", "strong", "-"]);
    let out = resolve(&with_default("codex", &args, table, Some("gpt-6.1-sol")));
    assert_eq!(
        model_of(&out.argv),
        Some("gpt-6.1-sol"),
        "the shipped codex cell names no model, and the operator's does: {:?}",
        out.argv
    );
    assert_eq!(
        out.effort,
        Some(Effort::Xhigh),
        "the substitution is a resolved model, not a fall-back, so the tier's own \
         Codex reasoning effort still reaches the harness"
    );
    assert_eq!(
        out.record.as_ref().and_then(|r| r.fallback),
        None,
        "nothing fell back: a model was chosen"
    );
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
