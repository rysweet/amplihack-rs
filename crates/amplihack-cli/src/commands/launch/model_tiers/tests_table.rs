//! Issue #1619 (TDD): the tier vocabulary and the per-harness table.
//!
//! This module reads no file and no environment variable: the shipped table
//! is the one `Table::shipped()` returns, and an override is parsed from text
//! the test writes.

use super::notice::Notice;
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
fn the_effort_union_is_exactly_six_words_ranked_weakest_to_strongest() {
    assert_eq!(
        Effort::ALL.map(Effort::as_str),
        ["minimal", "low", "medium", "high", "xhigh", "max"]
    );
    for word in Effort::ALL.map(Effort::as_str) {
        assert_eq!(Effort::parse(word).map(Effort::as_str), Some(word));
    }
    // The derived `Ord` is what makes `min(cap)` correct, so `Max` must be
    // the strongest and the declaration order must match the word order.
    assert!(Effort::Max > Effort::Xhigh);
    assert!(Effort::Xhigh > Effort::High);
    assert!(Effort::Minimal < Effort::Low);
    let mut sorted = Effort::ALL;
    sorted.sort();
    assert_eq!(sorted, Effort::ALL);
}

#[test]
fn each_column_accepts_its_own_vocabulary() {
    let words = |column: &str| {
        Effort::accepted_in(column)
            .iter()
            .map(|e| e.as_str())
            .collect::<Vec<_>>()
    };
    assert_eq!(words("claude"), ["low", "medium", "high", "xhigh", "max"]);
    assert_eq!(
        words("codex"),
        ["minimal", "low", "medium", "high", "xhigh"]
    );
}

#[test]
fn a_column_without_a_launch_control_accepts_the_union() {
    for column in ["copilot", "gemini", "an-invented-column", ""] {
        assert_eq!(
            Effort::accepted_in(column),
            &Effort::ALL[..],
            "`{column}`: amplihack writes nothing there, so no word can be wrong"
        );
    }
}

#[test]
fn only_claude_and_codex_have_an_effort_control() {
    assert!(Effort::has_effort_control("claude"));
    assert!(Effort::has_effort_control("codex"));
    for column in ["copilot", "gemini", "", "Claude"] {
        assert!(!Effort::has_effort_control(column), "`{column}`");
    }
}

#[test]
fn hostile_words_are_never_an_effort() {
    for word in [
        "turbo",
        "none",
        "",
        "high;id",
        "xhigh --model x",
        "max\n",
        "--effort",
        "\u{1b}[31mmax",
        &"x".repeat(10 * 1024),
    ] {
        assert!(Effort::parse(word).is_none(), "`{word:?}` must not parse");
    }
}

/// The notices an override file produces, and the table it merges into.
fn read(text: &str) -> (Table, Vec<Notice>) {
    let (overrides, notices) = Table::parse_override(text).expect("valid override");
    (overrides.merged_over(Table::shipped()), notices)
}

fn bad_efforts(notices: &[Notice]) -> Vec<(&str, &str, &str)> {
    notices
        .iter()
        .filter_map(|n| match n {
            Notice::BadEffort {
                column,
                tier,
                value,
            } => Some((column.as_str(), tier.as_str(), value.as_str())),
            _ => None,
        })
        .collect()
}

#[test]
fn claude_takes_max_in_a_cell_and_codex_refuses_it() {
    let (table, notices) = read("version = 1\n[claude.strong]\nmodel = \"m\"\neffort = \"max\"\n");
    assert!(bad_efforts(&notices).is_empty(), "{notices:?}");
    assert_eq!(
        table.entry("claude", Tier::Strong).and_then(|e| e.effort),
        Some(Effort::Max)
    );

    let (table, notices) = read("version = 1\n[codex.strong]\neffort = \"max\"\n");
    assert_eq!(
        bad_efforts(&notices),
        [("codex", "strong", "max")],
        "{notices:?}"
    );
    assert_eq!(
        table.entry("codex", Tier::Strong).and_then(|e| e.effort),
        None
    );
}

#[test]
fn codex_takes_minimal_and_claude_refuses_it_but_keeps_the_model() {
    let (table, notices) = read("version = 1\n[codex.fast]\neffort = \"minimal\"\n");
    assert!(bad_efforts(&notices).is_empty(), "{notices:?}");
    assert_eq!(
        table.entry("codex", Tier::Fast).and_then(|e| e.effort),
        Some(Effort::Minimal)
    );

    let (table, notices) =
        read("version = 1\n[claude.strong]\nmodel = \"claude-x\"\neffort = \"minimal\"\n");
    assert_eq!(
        bad_efforts(&notices),
        [("claude", "strong", "minimal")],
        "{notices:?}"
    );
    let entry = table.entry("claude", Tier::Strong).expect("cell");
    assert_eq!(
        entry.model.as_deref(),
        Some("claude-x"),
        "the model still applies"
    );
    assert_eq!(entry.effort, None);
}

#[test]
fn a_ceiling_is_validated_against_its_columns_vocabulary() {
    let (table, notices) = read("version = 1\n[claude]\nmax_effort = \"max\"\n");
    assert!(bad_efforts(&notices).is_empty(), "{notices:?}");
    assert_eq!(table.max_effort("claude"), Some(Effort::Max));

    let (table, notices) = read("version = 1\n[claude]\nmax_effort = \"minimal\"\n");
    assert!(!notices.is_empty(), "minimal is not a Claude word");
    assert_eq!(table.max_effort("claude"), None);

    let (table, notices) = read("version = 1\n[codex]\nmax_effort = \"max\"\n");
    assert!(!notices.is_empty(), "max is not a Codex word");
    assert_eq!(table.max_effort("codex"), None);
}

#[test]
fn hostile_cell_values_are_reported_and_the_model_still_applies() {
    for hostile in [
        "xhigh --model x",
        "max\\n",
        "MAX ",
        "--effort",
        "",
        "\\u001b[31mmax",
    ] {
        let text =
            format!("version = 1\n[claude.strong]\nmodel = \"claude-x\"\neffort = \"{hostile}\"\n");
        let (table, notices) = read(&text);
        let entry = table.entry("claude", Tier::Strong).expect("cell");
        assert_eq!(entry.model.as_deref(), Some("claude-x"), "{hostile:?}");
        assert_eq!(entry.effort, None, "{hostile:?}");
        assert_eq!(bad_efforts(&notices).len(), 1, "{hostile:?}: {notices:?}");
    }
}

/// `Effort::parse` does not trim. A padded Codex word was accepted before
/// issue #2916 and is now refused, with the padding visible in the notice so
/// the operator can see what to fix.
#[test]
fn a_padded_codex_effort_is_refused_not_trimmed() {
    let (table, notices) = read("version = 1\n[codex.strong]\nmodel = \"m\"\neffort = \" high\"\n");
    assert_eq!(
        bad_efforts(&notices),
        [("codex", "strong", " high")],
        "{notices:?}"
    );
    let entry = table.entry("codex", Tier::Strong).expect("cell");
    assert_eq!(entry.model.as_deref(), Some("m"), "the model still applies");
    assert_eq!(entry.effort, None);

    let rendered = notices
        .iter()
        .find(|n| matches!(n, Notice::BadEffort { .. }))
        .expect("a BadEffort notice")
        .render();
    assert!(
        rendered.contains(" high"),
        "padding must stay visible: {rendered}"
    );
}

#[test]
fn the_bad_effort_notice_lists_the_columns_own_words() {
    let claude = Notice::BadEffort {
        column: "claude".into(),
        tier: "strong".into(),
        value: "minimal".into(),
    }
    .render();
    assert!(claude.contains("low, medium, high, xhigh, max"), "{claude}");
    assert!(!claude.contains("minimal,"), "{claude}");

    let codex = Notice::BadEffort {
        column: "codex".into(),
        tier: "strong".into(),
        value: "max".into(),
    }
    .render();
    assert!(
        codex.contains("minimal, low, medium, high, xhigh"),
        "{codex}"
    );
    assert!(!codex.contains("xhigh, max"), "{codex}");
}

#[test]
fn the_shipped_claude_strong_tier_asks_for_xhigh_and_the_others_ask_for_nothing() {
    let table = Table::shipped();
    assert_eq!(
        table.entry("claude", Tier::Strong).and_then(|e| e.effort),
        Some(Effort::Xhigh)
    );
    for tier in [Tier::Standard, Tier::Fast] {
        assert_eq!(
            table.entry("claude", tier).and_then(|e| e.effort),
            None,
            "{tier:?}"
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
fn the_shipped_codex_column_names_no_model_and_sets_an_effort() {
    // `strong` ships at `high`: a Codex model that does not accept `xhigh`
    // would otherwise fail every strong step of an instance with no tier file
    // of its own. An instance that has `xhigh` raises it in its own cell.
    let table = Table::shipped();
    for (tier, effort) in [
        (Tier::Strong, "high"),
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
fn the_shipped_table_parses_and_declares_its_version() {
    // `Table::shipped()` panics on a file that does not parse, carries an
    // unknown version or produces any diagnostic, so reaching the assertions
    // is the parse check. Every cell is then asserted, so a change to any of
    // them is a deliberate change to this test too.
    let table = Table::shipped();

    for (tier, model, effort) in [
        (Tier::Strong, "claude-opus-5[1m]", Some(Effort::Xhigh)),
        (Tier::Standard, "claude-sonnet-5-5", None),
        (Tier::Fast, "claude-haiku-5-5", None),
    ] {
        let entry = table
            .entry("claude", tier)
            .unwrap_or_else(|| panic!("no claude cell for {}", tier.as_str()));
        assert_eq!(
            entry.model.as_deref(),
            Some(model),
            "claude/{}",
            tier.as_str()
        );
        // Issue #2916: `strong` is the planning and design tier and ships at
        // `xhigh`, which Claude Code takes as `--effort xhigh`.
        assert_eq!(entry.effort, effort, "claude/{}", tier.as_str());
    }
    assert_eq!(
        table
            .entry("claude", Tier::Strong)
            .and_then(|e| e.model.as_deref()),
        Some(DEFAULT_MODEL),
        "the strong Claude cell is the built-in default"
    );

    for (tier, effort) in [
        (Tier::Strong, Effort::High),
        (Tier::Standard, Effort::High),
        (Tier::Fast, Effort::Low),
    ] {
        let entry = table
            .entry("codex", tier)
            .unwrap_or_else(|| panic!("no codex cell for {}", tier.as_str()));
        assert_eq!(entry.model, None, "codex/{}", tier.as_str());
        assert_eq!(entry.effort, Some(effort), "codex/{}", tier.as_str());
    }

    assert!(
        table.has_column("copilot"),
        "the copilot column ships, empty, so an instance can fill it in"
    );
    for tier in Tier::ALL {
        assert_eq!(
            table.entry("copilot", tier),
            None,
            "copilot/{}",
            tier.as_str()
        );
    }

    for column in ["claude", "codex", "copilot"] {
        assert_eq!(
            table.max_effort(column),
            None,
            "{column}: the shipped mapping declares no ceiling"
        );
    }
    assert!(
        table.step_tiers().is_empty(),
        "no shipped per-step override"
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
        Some("high"),
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
// Model-id validation — the one check every model string crosses
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
