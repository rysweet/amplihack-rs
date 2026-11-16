//! Issue #1619 (TDD): the run summary names the model and effort each step
//! ran on.
//!
//! Wire with `#[cfg(test)] mod tests_model_tiers_summary;` in
//! `commands/recipe/run/mod.rs` when the summary section lands.
//!
//! Expected API (`recipe::run::format`):
//!
//! ```ignore
//! fn model_tier_lines(
//!     records: &[Record],
//!     step_results: &[RecipeRunStepResult],
//! ) -> Vec<String>;
//! ```
//!
//! `Record` derives `Clone` so a test can build a one-element slice from one.
//!
//! Pure: it takes the records and the runner's ordered step results and
//! returns the lines of the new section. The caller reads the record file.
//!
//! The section is additive. The existing step status line and its
//! `    Output: ` line are a machine-read contract —
//! `amplifier-bundle/tools/autodrive_loop.sh` parses them with awk — and
//! nothing here may reshape them.
//!
//! Attribution is a correlation, not a lookup: no step id ever reaches a
//! child process, because the recipe runner sets no step-id environment
//! variable. Step ids therefore appear only when the counts agree, and
//! otherwise the section prints by launch sequence and says why. A retry, a
//! parallel group or a resumed run all make the counts disagree.

use super::super::*;
use crate::commands::launch::model_tiers::record::{Fallback, Record};
use crate::commands::launch::model_tiers::table::{Effort, Tier};
use crate::commands::recipe::run::format::model_tier_lines;

fn record(seq: u64, tier: Tier, model: Option<&str>) -> Record {
    Record {
        v: 1,
        seq,
        run_id: Some("20261116-000000-test".to_string()),
        harness: "claude".to_string(),
        column: "claude".to_string(),
        tier: Some(tier),
        requested_model: model.map(str::to_string),
        effective_model: model.map(str::to_string),
        requested_effort: None,
        effective_effort: None,
        fallback: None,
    }
}

fn step(id: &str) -> RecipeRunStepResult {
    RecipeRunStepResult {
        step_id: id.to_string(),
        status: "completed".to_string(),
        ..Default::default()
    }
}

fn joined(lines: &[String]) -> String {
    lines.join("\n")
}

// -------------------------------------------------------------------------
// The section itself
// -------------------------------------------------------------------------

#[test]
fn the_section_is_empty_when_no_launch_recorded_a_tier() {
    assert!(
        model_tier_lines(&[], &[step("step-01")]).is_empty(),
        "a run with no tiered launch prints no section at all"
    );
}

#[test]
fn a_matched_run_names_the_step_the_tier_and_the_model() {
    let records = [
        record(1, Tier::Fast, Some("claude-haiku-5-5")),
        record(2, Tier::Strong, Some("claude-opus-5[1m]")),
    ];
    let steps = [step("step-01-prepare"), step("step-05-architecture")];
    let text = joined(&model_tier_lines(&records, &steps));
    assert!(text.starts_with("Model tiers:"), "{text}");
    assert!(
        text.contains("step-01-prepare")
            && text.contains("fast")
            && text.contains("claude-haiku-5-5"),
        "{text}"
    );
    assert!(
        text.contains("step-05-architecture")
            && text.contains("strong")
            && text.contains("claude-opus-5[1m]"),
        "{text}"
    );
}

#[test]
fn a_codex_line_names_the_effort_because_that_is_what_the_tier_changed() {
    let mut r = record(1, Tier::Strong, None);
    r.harness = "codex".to_string();
    r.column = "codex".to_string();
    r.requested_effort = Some(Effort::Xhigh);
    r.effective_effort = Some(Effort::Xhigh);
    let text = joined(&model_tier_lines(&[r], &[step("step-08-implement")]));
    assert!(text.contains("xhigh"), "{text}");
    assert!(
        text.contains("codex"),
        "the line must say the model is codex's own default rather than leave it blank: \
         {text}"
    );
}

#[test]
fn a_fall_back_line_names_what_was_asked_for_and_what_ran() {
    let mut r = record(1, Tier::Standard, None);
    r.harness = "copilot".to_string();
    r.column = "copilot".to_string();
    r.requested_model = None;
    r.effective_model = None;
    r.fallback = Some(Fallback::NoMapping);
    let text = joined(&model_tier_lines(&[r], &[step("step-06-documentation")]));
    assert!(text.contains("standard"), "the tier asked for: {text}");
    assert!(
        text.contains("copilot"),
        "and the harness whose default ran instead: {text}"
    );
}

#[test]
fn a_forced_model_names_its_origin() {
    let mut r = record(1, Tier::Standard, Some("claude-opus-5[1m]"));
    r.requested_model = Some("claude-sonnet-5-5".to_string());
    let text = joined(&model_tier_lines(
        &[r],
        &[step("step-10-pre-commit-review")],
    ));
    assert!(
        text.contains("claude-opus-5[1m]"),
        "the effective model is the one that ran: {text}"
    );
    assert!(
        text.contains("claude-sonnet-5-5") || text.contains("requested"),
        "requested and effective are distinguished, so a substitution is visible \
         rather than silent: {text}"
    );
}

// -------------------------------------------------------------------------
// Attribution: step ids only when the counts agree
// -------------------------------------------------------------------------

#[test]
fn more_launches_than_agent_steps_prints_by_sequence_and_says_why() {
    // A rate-limit retry relaunches one step: the runner runs a step up to
    // 1 + AMPLIHACK_RATELIMIT_MAX_RETRIES times, so one step can produce
    // several launches with no parallel group anywhere.
    let records = [
        record(1, Tier::Strong, Some("claude-opus-5[1m]")),
        record(2, Tier::Standard, Some("claude-sonnet-5-5")),
        record(3, Tier::Standard, Some("claude-sonnet-5-5")),
    ];
    let steps = [step("step-05-architecture"), step("step-06-documentation")];
    let text = joined(&model_tier_lines(&records, &steps));
    for id in ["step-05-architecture", "step-06-documentation"] {
        assert!(
            !text.contains(id),
            "amplihack must not guess a pairing it cannot make: {text}"
        );
    }
    assert!(
        text.contains("launch 1") && text.contains("launch 3"),
        "lines print by launch sequence instead: {text}"
    );
    assert!(
        text.contains('3') && text.contains('2'),
        "one line explains the counts, so a reader knows why the ids are missing: \
         {text}"
    );
}

#[test]
fn fewer_launches_than_agent_steps_also_prints_by_sequence() {
    let records = [record(1, Tier::Fast, Some("claude-haiku-5-5"))];
    let steps = [step("a"), step("b"), step("c")];
    let text = joined(&model_tier_lines(&records, &steps));
    assert!(!text.contains("step"), "{text}");
    assert!(text.contains("launch 1"), "{text}");
}

#[test]
fn a_skipped_step_does_not_break_a_matched_run() {
    // A skipped step launches nothing, so it is not an agent launch and must
    // not be counted as one.
    let records = [record(1, Tier::Fast, Some("claude-haiku-5-5"))];
    let mut skipped = step("step-02-skipped");
    skipped.status = "skipped".to_string();
    let steps = [step("step-01-ran"), skipped];
    let text = joined(&model_tier_lines(&records, &steps));
    assert!(
        text.contains("step-01-ran"),
        "the one launch pairs with the one step that ran: {text}"
    );
}

// -------------------------------------------------------------------------
// The existing machine-read lines are untouched
// -------------------------------------------------------------------------

#[test]
fn the_section_adds_lines_and_reshapes_none() {
    let text = joined(&model_tier_lines(
        &[record(1, Tier::Fast, Some("claude-haiku-5-5"))],
        &[step("step-01")],
    ));
    for line in text.lines().skip(1) {
        assert!(
            !line.starts_with("    Output: "),
            "nothing in this section may look like the step output line that \
             autodrive_loop.sh parses with awk: {line}"
        );
        assert!(
            !line.starts_with("  ✓ ") && !line.starts_with("  ✗ "),
            "nor like a step status line: {line}"
        );
    }
}

#[test]
fn the_section_quotes_no_path_argv_or_prompt() {
    // The summary is pasted into issues and PRs just as the record file is.
    let mut r = record(1, Tier::Fast, Some("claude-haiku-5-5"));
    r.run_id = Some("20261116-000000-test".to_string());
    let text = joined(&model_tier_lines(&r_slice(&r), &[step("step-01")]));
    for forbidden in ["/home/", "/Users/", "--dangerously", "--model"] {
        assert!(
            !text.contains(forbidden),
            "the section names a tier, a model and an effort, nothing else. \
             Found {forbidden:?} in:\n{text}"
        );
    }
}

fn r_slice(record: &Record) -> Vec<Record> {
    vec![record.clone()]
}
