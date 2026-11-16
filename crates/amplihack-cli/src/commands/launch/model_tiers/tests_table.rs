//! Issue #1619 (TDD): the tier vocabulary and the per-harness table.
//!
//! This module reads no file and no environment variable: the shipped table
//! is `include_str!`ed and the override is parsed from text.

use super::table::{Effort, Table, Tier, column_for, validate_model_id};
use crate::commands::launch::DEFAULT_MODEL;

// -------------------------------------------------------------------------
// The vocabulary is closed
// -------------------------------------------------------------------------

#[test]
fn the_vocabulary_is_exactly_three_words() {
    assert_eq!(Tier::ALL.map(Tier::as_str), ["fast", "standard", "strong"]);
    for word in Tier::ALL.map(Tier::as_str) {
        assert_eq!(
            Tier::parse(word).map(Tier::as_str),
            Some(word),
            "`{word}` must round-trip"
        );
    }
}

#[test]
fn no_other_word_is_a_tier() {
    // `light`, `medium` and `heavy` were the other candidate vocabulary and
    // are deliberately not aliases: one set, used everywhere.
    for word in [
        "light",
        "medium",
        "heavy",
        "xhigh",
        "auto",
        "",
        " ",
        "fastest",
        "strongly",
        "claude-opus-5[1m]",
        "gpt-6.1-sol",
    ] {
        assert_eq!(Tier::parse(word), None, "`{word}` must not parse as a tier");
    }
}

#[test]
fn a_tier_is_matched_trimmed_and_case_insensitively() {
    for raw in ["strong", "Strong", "STRONG", "  strong  ", "\tStRoNg\n"] {
        assert_eq!(
            Tier::parse(raw).map(Tier::as_str),
            Some("strong"),
            "`{raw:?}` must parse as strong"
        );
    }
}

#[test]
fn the_effort_allowlist_is_exactly_five_words() {
    assert_eq!(
        Effort::ALL.map(Effort::as_str),
        ["minimal", "low", "medium", "high", "xhigh"]
    );
    for word in Effort::ALL.map(Effort::as_str) {
        assert_eq!(Effort::parse(word).map(Effort::as_str), Some(word));
    }
    for word in ["turbo", "max", "none", "", "HIGH ", "high;id"] {
        // The allowlist is argv safety: the value becomes the right-hand side
        // of `-c model_reasoning_effort=…` on a command line.
        assert!(
            Effort::parse(word).is_none() || word.trim().eq_ignore_ascii_case("high"),
            "`{word}` must not be accepted as an effort"
        );
    }
}

// -------------------------------------------------------------------------
// Harness columns
// -------------------------------------------------------------------------

#[test]
fn every_claude_compatible_tool_shares_one_column() {
    for name in ["claude", "rusty", "rustyclawd", "amplifier"] {
        assert_eq!(
            column_for(name),
            "claude",
            "{name} is Claude-compatible and reads the claude column"
        );
    }
    assert_eq!(column_for("codex"), "codex");
    assert_eq!(column_for("copilot"), "copilot");
}

#[test]
fn an_unknown_tool_gets_its_own_column_name_and_no_shipped_entries() {
    assert_eq!(column_for("gemini"), "gemini");
    assert!(
        !Table::shipped().has_column("gemini"),
        "a tool amplihack learns to launch later has no shipped column, which is what \
         makes its tiers fall back rather than guess"
    );
}

// -------------------------------------------------------------------------
// The shipped table
// -------------------------------------------------------------------------

#[test]
fn the_shipped_claude_column_names_concrete_ids() {
    let table = Table::shipped();
    for (tier, expected) in [
        (Tier::Strong, "claude-opus-5[1m]"),
        (Tier::Standard, "claude-sonnet-5-5"),
        (Tier::Fast, "claude-haiku-5-5"),
    ] {
        let entry = table
            .entry("claude", tier)
            .unwrap_or_else(|| panic!("no claude entry for {}", tier.as_str()));
        assert_eq!(entry.model.as_deref(), Some(expected));
    }
}

#[test]
fn the_shipped_strong_claude_model_is_the_built_in_default() {
    // Issue #1421: amplihack asks for a concrete id, never an alias, and the
    // strong tier reuses the id amplihack already defaults to rather than
    // introducing a second one that can drift away from it.
    assert_eq!(
        Table::shipped()
            .entry("claude", Tier::Strong)
            .and_then(|e| e.model.as_deref()),
        Some(DEFAULT_MODEL)
    );
}

#[test]
fn no_shipped_claude_model_is_an_alias() {
    for tier in [Tier::Fast, Tier::Standard, Tier::Strong] {
        let model = Table::shipped()
            .entry("claude", tier)
            .and_then(|e| e.model.as_deref())
            .unwrap_or_else(|| panic!("no model for {}", tier.as_str()));
        assert!(
            model.starts_with("claude-") && model.contains('-'),
            "`{model}` must be a concrete id; an alias is resolved by the CLI, so its \
             meaning depends on the CLI version (issue #1421)"
        );
        assert!(
            !["opus", "sonnet", "haiku", "opus[1m]"].contains(&model),
            "`{model}` is an alias"
        );
    }
}

#[test]
fn the_shipped_codex_column_names_no_model_and_differs_by_effort() {
    let table = Table::shipped();
    for (tier, effort) in [
        (Tier::Strong, "xhigh"),
        (Tier::Standard, "high"),
        (Tier::Fast, "low"),
    ] {
        let entry = table
            .entry("codex", tier)
            .unwrap_or_else(|| panic!("no codex entry for {}", tier.as_str()));
        assert_eq!(
            entry.model, None,
            "Opus and Sonnet do not exist on Codex, and naming any Codex model in a \
             shipped default would guess at the account"
        );
        assert_eq!(entry.effort.map(Effort::as_str), Some(effort));
    }
}

#[test]
fn the_shipped_copilot_column_has_no_entries() {
    // An empty cell is an absent cell, so shipping three empty ones would be
    // dead config. The column exists to be filled in by an instance.
    let table = Table::shipped();
    for tier in [Tier::Fast, Tier::Standard, Tier::Strong] {
        assert!(
            table
                .entry("copilot", tier)
                .and_then(|e| e.model.as_deref())
                .is_none(),
            "amplihack passes no --model to Copilot today and a model name there would \
             depend on the account"
        );
    }
}

#[test]
fn the_shipped_table_names_no_host_path_or_account() {
    let text = include_str!("../model-tiers.toml");
    for forbidden in ["/home/", "/Users/", "~/", "candide", "@", "http"] {
        assert!(
            !text.contains(forbidden),
            "the shipped mapping is configuration that ships to every install; it must \
             contain no host name, user path or account name. Found {forbidden:?}"
        );
    }
}

#[test]
fn the_shipped_table_parses_and_declares_its_version() {
    let text = include_str!("../model-tiers.toml");
    let (parsed, notices) =
        Table::parse_override(text).expect("the shipped table must parse as an override file");
    assert!(
        notices.is_empty(),
        "the shipped table must produce no diagnostics: {notices:?}"
    );
    assert_eq!(
        parsed
            .entry("claude", Tier::Strong)
            .and_then(|e| e.model.as_deref()),
        Some(DEFAULT_MODEL),
        "the shipped file and Table::shipped() must be the same table"
    );
}

// -------------------------------------------------------------------------
// Override parsing and merging
// -------------------------------------------------------------------------

#[test]
fn an_override_replaces_one_cell_and_leaves_the_others() {
    let (overrides, _) =
        Table::parse_override("version = 1\n[claude.standard]\nmodel = \"claude-opus-5[1m]\"\n")
            .expect("valid override");
    let merged = overrides.merged_over(Table::shipped());
    assert_eq!(
        merged
            .entry("claude", Tier::Standard)
            .and_then(|e| e.model.as_deref()),
        Some("claude-opus-5[1m]")
    );
    assert_eq!(
        merged
            .entry("claude", Tier::Fast)
            .and_then(|e| e.model.as_deref()),
        Some("claude-haiku-5-5"),
        "merging is cell by cell"
    );
    assert_eq!(
        merged
            .entry("codex", Tier::Strong)
            .and_then(|e| e.effort.map(Effort::as_str)),
        Some("xhigh"),
        "an untouched column is untouched"
    );
}

#[test]
fn an_empty_cell_clears_the_shipped_model() {
    // This is how an instance says "that model is not available to me".
    let (overrides, _) =
        Table::parse_override("version = 1\n[claude.strong]\n").expect("valid override");
    let merged = overrides.merged_over(Table::shipped());
    assert_eq!(
        merged
            .entry("claude", Tier::Strong)
            .and_then(|e| e.model.as_deref()),
        None
    );
}

#[test]
fn an_unknown_tier_key_is_reported_by_name() {
    let (table, notices) =
        Table::parse_override("version = 1\n[claude.heavy]\nmodel = \"claude-opus-5[1m]\"\n")
            .expect("an unknown tier is a diagnostic, not a load failure");
    assert!(
        notices.iter().any(|n| format!("{n:?}").contains("heavy")),
        "the diagnostic must name the word that is not a tier: {notices:?}"
    );
    assert!(
        !table.has_column("claude") || table.entry("claude", Tier::Strong).is_none(),
        "the rejected entry must not be smuggled into another tier"
    );
}

#[test]
fn an_unknown_column_is_kept_so_a_newer_file_still_loads() {
    let (table, notices) =
        Table::parse_override("version = 1\n[gemini.strong]\nmodel = \"gemini-3-pro\"\n")
            .expect("valid override");
    assert!(
        table.has_column("gemini"),
        "a file written for a newer amplihack must still load on an older one"
    );
    assert!(
        notices.is_empty(),
        "an unknown column is kept, not reported: {notices:?}"
    );
}

#[test]
fn step_tiers_is_reserved_and_is_not_a_harness_column() {
    let (table, _) =
        Table::parse_override("version = 1\n[step_tiers]\nstep-08-implement = \"standard\"\n")
            .expect("valid override");
    assert!(
        !table.has_column("step_tiers"),
        "`step_tiers` is matched before the keep-unknown-columns rule"
    );
    assert_eq!(
        table
            .step_tiers()
            .get("step-08-implement")
            .map(|t| t.as_str()),
        Some("standard")
    );
}

#[test]
fn a_step_tier_value_that_is_not_a_tier_drops_that_entry_only() {
    let (table, notices) = Table::parse_override(
        "version = 1\n[step_tiers]\nstep-08-implement = \"heavy\"\n\
         step-20-final-cleanup = \"fast\"\n",
    )
    .expect("valid override");
    assert!(
        table.step_tiers().get("step-08-implement").is_none(),
        "the bad entry is dropped and the step keeps the tier its recipe declares"
    );
    assert_eq!(
        table
            .step_tiers()
            .get("step-20-final-cleanup")
            .map(|t| t.as_str()),
        Some("fast"),
        "the rest of the table still applies"
    );
    assert!(
        notices
            .iter()
            .any(|n| format!("{n:?}").contains("step-08-implement")),
        "the dropped entry is reported by name: {notices:?}"
    );
}

#[test]
fn an_unknown_field_inside_a_cell_is_reported() {
    // A typo such as `modle = …` must be visible rather than silently ignored.
    let result =
        Table::parse_override("version = 1\n[claude.fast]\nmodle = \"claude-haiku-5-5\"\n");
    match result {
        Err(reason) => assert!(
            reason.contains("modle"),
            "the reason must name the unknown field: {reason}"
        ),
        Ok((_, notices)) => assert!(
            notices.iter().any(|n| format!("{n:?}").contains("modle")),
            "the unknown field must be reported: {notices:?}"
        ),
    }
}

#[test]
fn a_version_amplihack_does_not_know_is_refused() {
    let reason = Table::parse_override("version = 2\n[claude.fast]\nmodel = \"x\"\n")
        .expect_err("an unknown version is refused so the shipped mapping applies");
    assert!(
        reason.contains('2') || reason.to_lowercase().contains("version"),
        "{reason}"
    );
}

#[test]
fn malformed_toml_is_refused_with_a_reason_that_quotes_no_contents() {
    let reason = Table::parse_override("version = 1\n[claude.fast\nmodel = ")
        .expect_err("malformed TOML is refused");
    assert!(
        !reason.contains("claude-"),
        "the reason names the problem, never the file's contents: {reason}"
    );
}

#[test]
fn an_override_effort_outside_the_allowlist_is_dropped_and_the_model_kept() {
    let (table, notices) = Table::parse_override(
        "version = 1\n[codex.fast]\nmodel = \"gpt-6.1-sol\"\neffort = \"turbo\"\n",
    )
    .expect("a bad effort is a diagnostic, not a load failure");
    let entry = table
        .entry("codex", Tier::Fast)
        .expect("the cell still exists");
    assert_eq!(entry.model.as_deref(), Some("gpt-6.1-sol"));
    assert_eq!(entry.effort, None, "the effort is dropped");
    assert!(
        notices.iter().any(|n| format!("{n:?}").contains("turbo")),
        "the dropped value is named: {notices:?}"
    );
}

// -------------------------------------------------------------------------
// Model-id validation — the one gate every model string crosses
// -------------------------------------------------------------------------

#[test]
fn model_ids_amplihack_accepts() {
    for good in [
        "claude-opus-5[1m]",
        "claude-sonnet-5-5",
        "claude-sonnet-5.5",
        "claude-haiku-5-5",
        "gpt-6.1-sol",
        "gpt-6.1-sol:high",
        "a",
        &"x".repeat(128),
    ] {
        assert!(
            validate_model_id(good).is_ok(),
            "`{good}` must be accepted as a model id"
        );
    }
}

#[test]
fn model_ids_amplihack_rejects() {
    // The value lands as the argv token right after `--model`, and both codex
    // and claude carry single-token sandbox-bypass flags.
    for bad in [
        "",
        " ",
        "-s",
        "--dangerously-skip-permissions",
        "--yolo",
        "a b",
        "a\tb",
        "a\nb",
        "a\0b",
        "a/b",
        "a\\b",
        "a;b",
        "a$b",
        "a`b",
        "a'b",
        "a\"b",
        "a|b",
        "a&b",
        &"x".repeat(129),
    ] {
        assert!(
            validate_model_id(bad).is_err(),
            "`{bad:?}` must be rejected as a model id"
        );
    }
}

#[test]
fn a_rejected_override_model_leaves_the_shipped_value_in_place() {
    let (overrides, notices) = Table::parse_override(
        "version = 1\n[claude.fast]\nmodel = \"--dangerously-skip-permissions\"\n",
    )
    .expect("a rejected id is a diagnostic, not a load failure");
    assert!(!notices.is_empty(), "the rejection is reported");
    let merged = overrides.merged_over(Table::shipped());
    assert_eq!(
        merged
            .entry("claude", Tier::Fast)
            .and_then(|e| e.model.as_deref()),
        Some("claude-haiku-5-5"),
        "fail secure means fail to the audited chain, never fail to the bad value"
    );
}
