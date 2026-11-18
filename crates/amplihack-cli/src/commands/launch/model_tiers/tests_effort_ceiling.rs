//! Issue #1619, B3(c) (TDD): a declared per-column effort ceiling, so a tier
//! an instance raised steps down rather than failing on an account whose
//! model does not accept that effort.
//!
//! Amplihack cannot learn that a model refuses an effort without making a
//! live call, and the brief excludes probing, retries and timers. So the
//! ceiling is a declaration about the operator's own account rather than
//! detection: an optional `max_effort` inside a column, applied to every
//! tier of that column before the effort is written.
//!
//! The shipped `codex.strong` is `high`, so with no tier file nothing asks
//! for `xhigh`. An instance that wants `xhigh` for orchestration and formal
//! steps, as the issue's 3 November comment does, writes it in its own
//! `[codex.strong]` cell; the cases below that need an `xhigh` request write
//! that cell, as `RAISED_STRONG` does.
//!
//! Three properties hold the key in place:
//!
//! - it is reserved, and matched before [`Tier::parse`], so it is never read
//!   as a tier word and never reported as an unknown one;
//! - it only ever lowers an effort, in either direction of comparison;
//! - the value parses through [`Effort`], so no string reaches the
//!   right-hand side of `-c model_reasoning_effort=`.

use super::notice::Notice;
use super::resolve::{Request, resolve};
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

/// The instance cell that raises Codex `strong` to `xhigh`. The shipped cell
/// is `high`, so a ceiling below `xhigh` has nothing to lower without it.
const RAISED_STRONG: &str = "[codex.strong]\neffort = \"xhigh\"\n";

/// An instance file, merged over the shipped mapping the way a real one is.
fn with_override(text: &str) -> (Table, Vec<Notice>) {
    let (overrides, notices) = Table::parse_override(text).expect("valid override");
    (overrides.merged_over(Table::shipped()), notices)
}

// -------------------------------------------------------------------------
// The key parses, and is not a tier word
// -------------------------------------------------------------------------

#[test]
fn a_column_may_declare_a_max_effort() {
    let (table, notices) = with_override("version = 1\n[codex]\nmax_effort = \"high\"\n");
    assert_eq!(table.max_effort("codex"), Some(Effort::High));
    assert!(
        notices.is_empty(),
        "`max_effort` is a reserved key, not an unknown tier: {notices:?}"
    );
}

#[test]
fn a_column_with_no_max_effort_has_none() {
    assert_eq!(
        Table::shipped().max_effort("codex"),
        None,
        "the shipped mapping declares no ceiling: an account that needs one says so \
         in its own file"
    );
    assert_eq!(Table::shipped().max_effort("claude"), None);
    assert_eq!(Table::shipped().max_effort("gemini"), None);
}

#[test]
fn max_effort_is_never_read_as_a_tier_word() {
    assert_eq!(
        Tier::parse("max_effort"),
        None,
        "the vocabulary is closed at three words"
    );
    let (table, _) = with_override("version = 1\n[codex]\nmax_effort = \"low\"\n");
    for tier in Tier::ALL {
        assert_eq!(
            table.entry("codex", tier).and_then(|e| e.effort),
            Table::shipped().entry("codex", tier).and_then(|e| e.effort),
            "declaring a ceiling must not disturb a cell"
        );
    }
}

#[test]
fn max_effort_under_step_tiers_is_an_ordinary_step_id() {
    // The reservation is per-position and is only two `if` statements apart
    // from the tier loop; a refactor that reordered them would have no other
    // symptom.
    let (overrides, notices) =
        Table::parse_override("version = 1\n[step_tiers]\nmax_effort = \"fast\"\n")
            .expect("valid override");
    assert_eq!(
        overrides.step_tiers().get("max_effort"),
        Some(&Tier::Fast),
        "inside `[step_tiers]` the key is a step id like any other: {notices:?}"
    );
}

#[test]
fn a_max_effort_that_is_not_an_effort_is_reported_and_no_ceiling_applies() {
    let (table, notices) = with_override("version = 1\n[codex]\nmax_effort = \"enormous\"\n");
    assert_eq!(
        table.max_effort("codex"),
        None,
        "a typo must fail open: applying some default cap instead would silently \
         change what every Codex step runs at"
    );
    assert!(
        !notices.is_empty(),
        "and it is reported where the operator wrote it"
    );
}

#[test]
fn a_max_effort_merges_per_column_over_the_shipped_mapping() {
    let (table, _) = with_override(
        "version = 1\n[codex]\nmax_effort = \"medium\"\n[claude]\nmax_effort = \"high\"\n",
    );
    assert_eq!(table.max_effort("codex"), Some(Effort::Medium));
    assert_eq!(table.max_effort("claude"), Some(Effort::High));
}

// -------------------------------------------------------------------------
// It lowers an effort, and only ever lowers one
// -------------------------------------------------------------------------

#[test]
fn a_ceiling_steps_the_strong_tier_down_on_codex() {
    let (table, _) = with_override(&format!(
        "version = 1\n[codex]\nmax_effort = \"high\"\n{RAISED_STRONG}"
    ));
    let args = argv(&["exec", "--model", "strong", "-"]);
    let out = resolve(&request("codex", &args, &table));
    assert_eq!(
        out.effort,
        Some(Effort::High),
        "the instance's `strong` asks for xhigh; this account declares high as its \
         ceiling, so high is what reaches Codex"
    );
}

#[test]
fn a_ceiling_never_raises_an_effort() {
    let (table, _) = with_override(&format!(
        "version = 1\n[codex]\nmax_effort = \"xhigh\"\n{RAISED_STRONG}"
    ));
    for (tier, expected) in [
        ("fast", Effort::Low),
        ("standard", Effort::High),
        ("strong", Effort::Xhigh),
    ] {
        let args = argv(&["exec", "--model", tier, "-"]);
        let out = resolve(&request("codex", &args, &table));
        assert_eq!(
            out.effort,
            Some(expected),
            "`{tier}` sits at or below a ceiling of xhigh and must be untouched"
        );
    }
}

#[test]
fn a_ceiling_above_the_shipped_efforts_leaves_them_as_shipped() {
    let (table, _) = with_override("version = 1\n[codex]\nmax_effort = \"xhigh\"\n");
    for (tier, expected) in [
        ("fast", Effort::Low),
        ("standard", Effort::High),
        ("strong", Effort::High),
    ] {
        let args = argv(&["exec", "--model", tier, "-"]);
        let out = resolve(&request("codex", &args, &table));
        assert_eq!(
            out.effort,
            Some(expected),
            "`{tier}`: a ceiling of xhigh must not lift the shipped effort to itself"
        );
    }
}

#[test]
fn a_ceiling_applies_to_every_tier_of_its_column() {
    let (table, _) = with_override("version = 1\n[codex]\nmax_effort = \"minimal\"\n");
    for tier in Tier::ALL.map(Tier::as_str) {
        let args = argv(&["exec", "--model", tier, "-"]);
        let out = resolve(&request("codex", &args, &table));
        assert_eq!(
            out.effort,
            Some(Effort::Minimal),
            "`{tier}` is capped like every other tier of the column"
        );
    }
}

#[test]
fn a_ceiling_on_one_column_does_not_reach_another() {
    let (table, _) = with_override(&format!(
        "version = 1\n[claude]\nmax_effort = \"minimal\"\n{RAISED_STRONG}"
    ));
    let args = argv(&["exec", "--model", "strong", "-"]);
    let out = resolve(&request("codex", &args, &table));
    assert_eq!(
        out.effort,
        Some(Effort::Xhigh),
        "a ceiling declared on `claude` says nothing about Codex"
    );
}

#[test]
fn a_capped_step_still_resolves_and_never_fails() {
    let (table, _) = with_override(
        "version = 1\n[codex]\nmax_effort = \"low\"\n[codex.strong]\nmodel = \"gpt-6.1-sol\"\n\
         effort = \"xhigh\"\n",
    );
    let args = argv(&["exec", "--model", "strong", "-"]);
    let out = resolve(&request("codex", &args, &table));
    assert_eq!(
        out.argv,
        argv(&["exec", "--model", "gpt-6.1-sol", "-"]),
        "the model is untouched by the ceiling: a ceiling is about effort"
    );
    assert_eq!(out.effort, Some(Effort::Low));
    assert_eq!(
        out.record.as_ref().and_then(|r| r.fallback),
        None,
        "stepping an effort down is not a fall-back and the step does not fail"
    );
}

// -------------------------------------------------------------------------
// It is recorded, once, and only when it happened
// -------------------------------------------------------------------------

#[test]
fn the_record_keeps_both_the_requested_and_the_capped_effort() {
    let (table, _) = with_override(&format!(
        "version = 1\n[codex]\nmax_effort = \"medium\"\n{RAISED_STRONG}"
    ));
    let args = argv(&["exec", "--model", "strong", "-"]);
    let out = resolve(&request("codex", &args, &table));
    let record = out.record.expect("a tiered launch records one line");
    assert_eq!(
        record.requested_effort,
        Some(Effort::Xhigh),
        "what the tier asked for"
    );
    assert_eq!(
        record.effective_effort,
        Some(Effort::Medium),
        "and what actually reached the harness"
    );
}

/// One `Record` with both effort fields set as a launch would leave them.
fn record(requested: Option<Effort>, effective: Option<Effort>) -> super::record::Record {
    super::record::Record {
        v: 1,
        seq: 0,
        run_id: None,
        harness: "codex".to_string(),
        column: "codex".to_string(),
        tier: Some(Tier::Strong),
        requested_model: None,
        effective_model: None,
        requested_effort: requested,
        effective_effort: effective,
        fallback: None,
    }
}

#[test]
fn the_applied_line_qualifies_the_effort_it_already_reports() {
    let notice = super::applied_notice(
        "codex",
        &record(Some(Effort::Xhigh), Some(Effort::Medium)),
        super::EffortOutcome::Added(Effort::Medium),
    )
    .expect("a launch that got an effort says so");
    assert!(
        matches!(
            notice,
            Notice::Applied {
                effort: Some(Effort::Medium),
                capped_from: Some(Effort::Xhigh),
                ..
            }
        ),
        "the cap qualifies the one line the launch already prints, rather than \
         adding a second: {notice:?}"
    );
}

#[test]
fn an_uncapped_launch_claims_no_cap() {
    let notice = super::applied_notice(
        "codex",
        &record(Some(Effort::High), Some(Effort::High)),
        super::EffortOutcome::Added(Effort::High),
    )
    .expect("a launch that got an effort says so");
    assert!(
        matches!(
            notice,
            Notice::Applied {
                capped_from: None,
                ..
            }
        ),
        "equal efforts were not capped: {notice:?}"
    );
}

#[test]
fn a_cap_is_claimed_only_when_both_efforts_are_known() {
    // `Option<Effort>` orders `None` below every `Some`, and a non-Codex
    // column records an effort it never writes. A naive comparison would
    // print a cap line about an effort that never reached a harness, and the
    // run log is a file people paste into issues.
    for (requested, effective) in [
        (None, Some(Effort::Low)),
        (Some(Effort::Xhigh), None),
        (None, None),
    ] {
        let outcome = match effective {
            Some(effort) => super::EffortOutcome::Added(effort),
            None => super::EffortOutcome::NotApplicable,
        };
        let notice = super::applied_notice("claude", &record(requested, effective), outcome);
        assert!(
            !matches!(
                notice,
                Some(Notice::Applied {
                    capped_from: Some(_),
                    ..
                })
            ),
            "requested={requested:?} effective={effective:?} is not evidence of a \
             cap: {notice:?}"
        );
    }
}

#[test]
fn a_column_with_no_effort_control_reports_no_cap() {
    // Claude Code has no launch control for reasoning effort, so a declared
    // effort there is recorded and documented as not a launch control. A
    // ceiling must not invent one, nor claim to have lowered one.
    let (table, _) = with_override(
        "version = 1\n[claude]\nmax_effort = \"low\"\n[claude.strong]\n\
         model = \"claude-opus-5[1m]\"\neffort = \"xhigh\"\n",
    );
    let args = argv(&["--model", "strong", "-p"]);
    let out = resolve(&request("claude", &args, &table));
    assert_eq!(
        out.effort, None,
        "nothing is written, so nothing is capped: {:?}",
        out.argv
    );
}
