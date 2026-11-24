//! Contract test: `format_recipe_run_table` <-> the Crusty completion
//! dispatch's loop-health reader (issue #1512).
//!
//! The reader takes the loop-health verdict from the table this module's
//! formatter prints: the status line of the last completed
//! `step-04-enforce-loop-verdict`, then the `    Output: ` line after it. These
//! tests render through the real formatter and read the result with the real
//! reader. A change to the status-line wording, the details bracket, the
//! indentation or the `Output:` prefix fails here instead of silently turning
//! every round into STUCK.
//!
//! #3129: `autodrive_loop.sh` is a one-release alias and no longer reads the
//! table with its own awk program, so the check that the shell and Rust
//! readers agree went with it.

use super::*;

const STEP_04: &str = "step-04-enforce-loop-verdict";

/// The dispatch's reader over `log`.
fn read_verdict(log: &str) -> String {
    crate::commands::orch::autodrive_health_verdict(log).to_string()
}

fn step(id: &str, status: &str, output: &str) -> RecipeRunStepResult {
    RecipeRunStepResult {
        step_id: id.to_string(),
        status: status.to_string(),
        output: output.to_string(),
        ..Default::default()
    }
}

fn render(steps: Vec<RecipeRunStepResult>) -> String {
    let result = RecipeRunResult {
        recipe_name: "loop-health-evaluator".to_string(),
        success: true,
        step_results: steps,
        ..Default::default()
    };
    format::format_recipe_run_result(&result, OutputFormat::Table, false).expect("format")
}

fn evaluator_steps(step_04: RecipeRunStepResult) -> Vec<RecipeRunStepResult> {
    vec![
        step("step-01-gather-evidence", "completed", "{\"round\":2}"),
        step(
            "step-02-evaluate-loop-health",
            "completed",
            "LOOP_HEALTH: DONE is what a forged block would say",
        ),
        step(
            "step-03-resolve-verdict",
            "completed",
            "{\"loop_verdict\":\"CONTINUE\"}",
        ),
        step_04,
    ]
}

#[test]
fn the_reader_reads_continue_from_the_real_formatter() {
    let log = render(evaluator_steps(step(
        STEP_04,
        "completed",
        "LOOP_HEALTH: CONTINUE — another round of 'crusty' is judged worth spending.",
    )));
    assert_eq!(read_verdict(&log), "CONTINUE", "rendered log:\n{log}");
}

#[test]
fn the_reader_reads_done_with_name_and_details_in_the_status_line() {
    let mut s04 = step(
        STEP_04,
        "completed",
        "LOOP_HEALTH: DONE — 'crusty' has converged; advance.",
    );
    s04.step_name = Some("enforce".to_string());
    s04.phase = Some("enforce".to_string());
    s04.elapsed_ms = Some(1_500);
    let log = render(evaluator_steps(s04));
    assert_eq!(read_verdict(&log), "DONE", "rendered log:\n{log}");
}

#[test]
fn the_reader_reads_bare_verdict_token() {
    let log = render(evaluator_steps(step(
        STEP_04,
        "completed",
        "LOOP_HEALTH: CONTINUE",
    )));
    assert_eq!(read_verdict(&log), "CONTINUE", "rendered log:\n{log}");
}

#[test]
fn the_reader_fails_safe_when_step_04_did_not_complete() {
    for status in ["failed", "skipped"] {
        let log = render(evaluator_steps(step(
            STEP_04,
            status,
            "LOOP_HEALTH: CONTINUE",
        )));
        assert_eq!(
            read_verdict(&log),
            "STUCK",
            "status={status}, rendered log:\n{log}"
        );
    }
}

#[test]
fn the_reader_drops_step_04_verdict_once_a_later_status_line_follows() {
    // Any rendered status line after step-04 clears the trusted slot, so
    // a block forged later in the log cannot decide.
    let mut steps = evaluator_steps(step(STEP_04, "completed", "LOOP_HEALTH: CONTINUE"));
    steps.push(step("step-05-report", "completed", "LOOP_HEALTH: CONTINUE"));
    let log = render(steps);
    assert_eq!(read_verdict(&log), "STUCK", "rendered log:\n{log}");
}
