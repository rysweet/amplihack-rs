//! Workflow selection for `amplihack orch helper select-workflow` (#1740).
//!
//! Decides between `quick-change`, `default-workflow` and `none` from the
//! `-c workflow=` override, the normalised task type, the classifier JSON and
//! the workstream count. No I/O: the CLI arm in `orch.rs` reads stdin and
//! writes the warnings and the log line. The precedence table, the warning
//! text and the `reason=` values are fixed by
//! `docs/reference/quick-change-recipe.md`.

pub const QUICK_CHANGE: &str = "quick-change";
pub const DEFAULT_WORKFLOW: &str = "default-workflow";
pub const NO_WORKFLOW: &str = "none";

const REASON_MAX: usize = 200;
const QUOTE_MAX: usize = 64;
const UNSPECIFIED: &str = "unspecified";

/// The outcome of [`select`]: the bare workflow value, where it came from,
/// the sanitised one-line reason and any warnings to print before the log
/// line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    pub workflow: &'static str,
    pub source: &'static str,
    pub reason: String,
    pub warnings: Vec<String>,
}

/// Match a workflow value against the exact allowlist, after trimming and
/// lower-casing. Returns the canonical constant, or `None`.
pub fn valid_override(raw: &str) -> Option<&'static str> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "quick-change" => Some(QUICK_CHANGE),
        "default-workflow" => Some(DEFAULT_WORKFLOW),
        _ => None,
    }
}

/// W1: the override is set but is not on the allowlist.
pub fn unknown_override_warning(raw: &str) -> String {
    format!(
        "[dev-orchestrator] WARNING: unknown workflow override '{}' ignored",
        sanitise(raw.trim(), QUOTE_MAX)
    )
}

fn non_development_warning(raw: &str, task_type: &str) -> String {
    format!(
        "[dev-orchestrator] WARNING: workflow override '{}' ignored for task_type {}",
        sanitise(raw.trim(), QUOTE_MAX),
        sanitise(task_type, QUOTE_MAX)
    )
}

/// Replace control characters and bidirectional or invisible formatting
/// characters with spaces, trim, and cut to `max` chars on a char boundary.
pub fn sanitise(raw: &str, max: usize) -> String {
    let replaced: String = raw
        .chars()
        .map(|c| if is_unsafe(c) { ' ' } else { c })
        .collect();
    replaced.trim().chars().take(max).collect()
}

fn is_unsafe(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}'
        )
}

fn reason_or_unspecified(raw: &str) -> String {
    let reason = sanitise(raw, REASON_MAX);
    if reason.is_empty() {
        UNSPECIFIED.to_string()
    } else {
        reason
    }
}

/// Apply the eight-row precedence table.
///
/// `classifier_json` is the classifier output; empty or invalid JSON gives
/// row 8. A `workstream_count` other than 1 (including 0 for an unreadable
/// count) demotes a classifier `quick-change` to `default-workflow`.
pub fn select(
    override_value: &str,
    task_type: &str,
    classifier_json: &str,
    workstream_count: usize,
) -> Selection {
    let mut warnings = Vec::new();
    let has_override = !override_value.trim().is_empty();
    let is_development = super::normalise_type(task_type.trim()) == "Development";

    if !is_development {
        let reason = format!("task_type {}", reason_or_unspecified(task_type));
        let source = if has_override {
            warnings.push(non_development_warning(override_value, task_type));
            "override"
        } else {
            "default"
        };
        return Selection {
            workflow: NO_WORKFLOW,
            source,
            reason: sanitise(&reason, REASON_MAX),
            warnings,
        };
    }

    if has_override {
        match valid_override(override_value) {
            Some(workflow) => {
                return Selection {
                    workflow,
                    source: "override",
                    reason: format!("-c workflow={workflow}"),
                    warnings,
                };
            }
            None => warnings.push(unknown_override_warning(override_value)),
        }
    }

    let parsed = super::extract_json(classifier_json).unwrap_or(serde_json::Value::Null);
    let field = |name: &str| {
        parsed
            .get(name)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    let recommended = valid_override(&field("recommended_workflow"));
    let reason = reason_or_unspecified(&field("workflow_reason"));

    let (workflow, source, reason) = match recommended {
        Some(QUICK_CHANGE) if workstream_count == 1 => (QUICK_CHANGE, "classifier", reason),
        Some(QUICK_CHANGE) => (
            DEFAULT_WORKFLOW,
            "classifier",
            format!("quick-change needs one workstream; got {workstream_count}"),
        ),
        Some(_) => (DEFAULT_WORKFLOW, "classifier", reason),
        None => (DEFAULT_WORKFLOW, "default", reason),
    };
    Selection {
        workflow,
        source,
        reason,
        warnings,
    }
}

/// Why a run cannot use `quick-change`, which always works on a new
/// `quick/` branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExistingTarget {
    /// `existing_branch` or `pr_number` is set.
    BranchOrPr,
    /// The task text names its branch (`Branch: <ref>`), which
    /// `workflow-worktree` uses ahead of the `quick/` prefix (#1426).
    NamedInTask,
}

/// The target a run already has, checked in the order `workflow-worktree`
/// uses: `existing_branch` and `pr_number` first, and the task text only
/// when both are blank.
pub fn existing_target(
    existing_branch: &str,
    pr_number: &str,
    task_description: &str,
) -> Option<ExistingTarget> {
    if !existing_branch.trim().is_empty() || !pr_number.trim().is_empty() {
        Some(ExistingTarget::BranchOrPr)
    } else if super::named_branch::task_names_branch(task_description) {
        Some(ExistingTarget::NamedInTask)
    } else {
        None
    }
}

/// A run with an [`ExistingTarget`] cannot use `quick-change`: its branch
/// check stops the run once `workflow-worktree` has given it a branch that
/// does not start with `quick/`. Such a run gets `default-workflow`
/// instead, with W3 or W4 when the user asked for `quick-change` explicitly.
pub fn demote_for_existing_target(
    selection: Selection,
    target: Option<ExistingTarget>,
) -> Selection {
    let Some(target) = target else {
        return selection;
    };
    if selection.workflow != QUICK_CHANGE {
        return selection;
    }
    let (warning, reason) = match target {
        ExistingTarget::BranchOrPr => (EXISTING_TARGET_WARNING, EXISTING_TARGET_REASON),
        ExistingTarget::NamedInTask => (NAMED_BRANCH_WARNING, NAMED_BRANCH_REASON),
    };
    let mut warnings = selection.warnings;
    if selection.source == "override" {
        warnings.push(warning.to_string());
    }
    Selection {
        workflow: DEFAULT_WORKFLOW,
        source: selection.source,
        reason: reason.to_string(),
        warnings,
    }
}

/// The `-c workflow=` value that `build-workstreams-config` applies to the
/// Development workstreams of a parallel decomposition, under the rules
/// `select` and [`demote_for_existing_target`] apply and log: no override
/// for a task_type that is not Development (W2), and no `quick-change` when
/// the run has an [`ExistingTarget`] (W3, W4). `None` leaves every
/// Development workstream on `default-workflow`.
pub fn parallel_override(
    override_value: &str,
    task_type: &str,
    targets_existing: bool,
) -> Option<&'static str> {
    if super::normalise_type(task_type.trim()) != "Development" {
        return None;
    }
    match valid_override(override_value) {
        Some(QUICK_CHANGE) if targets_existing => None,
        other => other,
    }
}

/// W3: `-c workflow=quick-change` on a run that targets an existing branch.
pub const EXISTING_TARGET_WARNING: &str = "[dev-orchestrator] WARNING: workflow override 'quick-change' ignored: existing_branch or pr_number is set";
const EXISTING_TARGET_REASON: &str = "existing_branch or pr_number set";
/// W4: `-c workflow=quick-change` on a run whose task names its branch.
pub const NAMED_BRANCH_WARNING: &str =
    "[dev-orchestrator] WARNING: workflow override 'quick-change' ignored: the task names a branch";
const NAMED_BRANCH_REASON: &str = "task names a branch";

/// The fixed log line written to stderr after any warnings.
pub fn log_line(selection: &Selection) -> String {
    format!(
        "[dev-orchestrator] Workflow: {} source={} reason={}",
        selection.workflow, selection.source, selection.reason
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const W1_QUICK: &str = "[dev-orchestrator] WARNING: unknown workflow override 'quick' ignored";
    const W2_QA: &str =
        "[dev-orchestrator] WARNING: workflow override 'quick-change' ignored for task_type Q&A";

    fn classifier(workflow: &str, reason: &str) -> String {
        serde_json::json!({"recommended_workflow": workflow, "workflow_reason": reason}).to_string()
    }

    #[test]
    fn constants_are_the_three_bare_values() {
        assert_eq!(QUICK_CHANGE, "quick-change");
        assert_eq!(DEFAULT_WORKFLOW, "default-workflow");
        assert_eq!(NO_WORKFLOW, "none");
    }

    #[test]
    fn valid_override_is_an_exact_allowlist_after_trim_and_lowercase() {
        assert_eq!(valid_override("quick-change"), Some(QUICK_CHANGE));
        assert_eq!(valid_override("  Quick-Change\n"), Some(QUICK_CHANGE));
        assert_eq!(valid_override("DEFAULT-WORKFLOW"), Some(DEFAULT_WORKFLOW));
        for bad in [
            "",
            "quick",
            "quick_change",
            "quick-change-x",
            "investigation-workflow",
        ] {
            assert_eq!(valid_override(bad), None, "{bad:?} must not be accepted");
        }
    }

    #[test]
    fn row_1_override_on_non_development_gives_none_with_w2() {
        let s = select("quick-change", "Q&A", &classifier("quick-change", "x"), 1);
        assert_eq!((s.workflow, s.source), (NO_WORKFLOW, "override"));
        assert_eq!(s.warnings, vec![W2_QA]);
    }

    #[test]
    fn row_2_non_development_without_override_gives_none() {
        for task_type in ["Q&A", "Operations", "Investigation"] {
            let s = select("", task_type, &classifier("quick-change", "x"), 1);
            assert_eq!(
                (s.workflow, s.source),
                (NO_WORKFLOW, "default"),
                "{task_type}"
            );
            assert!(s.warnings.is_empty());
        }
    }

    #[test]
    fn row_3_valid_override_wins_in_both_directions_and_ignores_workstreams() {
        let s = select(
            "quick-change",
            "Development",
            &classifier("default-workflow", "big"),
            4,
        );
        assert_eq!((s.workflow, s.source), (QUICK_CHANGE, "override"));
        assert_eq!(s.reason, "-c workflow=quick-change");
        let s = select(
            "default-workflow",
            "Development",
            &classifier("quick-change", "doc"),
            1,
        );
        assert_eq!((s.workflow, s.source), (DEFAULT_WORKFLOW, "override"));
        assert_eq!(s.reason, "-c workflow=default-workflow");
        assert!(s.warnings.is_empty());
    }

    #[test]
    fn row_4_unknown_override_warns_w1_and_falls_through_to_the_classifier() {
        let s = select(
            "quick",
            "Development",
            &classifier("quick-change", "typo"),
            1,
        );
        assert_eq!(s.warnings, vec![W1_QUICK]);
        assert_eq!((s.workflow, s.source), (QUICK_CHANGE, "classifier"));
        let s = select("quick", "Development", "", 1);
        assert_eq!(s.warnings, vec![W1_QUICK]);
        assert_eq!((s.workflow, s.source), (DEFAULT_WORKFLOW, "default"));
    }

    #[test]
    fn row_5_classifier_quick_change_with_one_workstream() {
        let s = select(
            "",
            "Development",
            &classifier("quick-change", "comment typo"),
            1,
        );
        assert_eq!((s.workflow, s.source), (QUICK_CHANGE, "classifier"));
        assert_eq!(s.reason, "comment typo");
    }

    #[test]
    fn row_6_classifier_quick_change_with_several_workstreams_is_demoted() {
        let s = select("", "Development", &classifier("quick-change", "doc"), 2);
        assert_eq!((s.workflow, s.source), (DEFAULT_WORKFLOW, "classifier"));
        assert_eq!(s.reason, "quick-change needs one workstream; got 2");
    }

    #[test]
    fn row_7_classifier_default_workflow() {
        let s = select(
            "",
            "Development",
            &classifier("default-workflow", "new feature"),
            1,
        );
        assert_eq!((s.workflow, s.source), (DEFAULT_WORKFLOW, "classifier"));
        assert_eq!(s.reason, "new feature");
    }

    #[test]
    fn row_8_missing_unknown_or_unparseable_recommendation_gives_default() {
        for input in [
            "",
            "not json",
            "{}",
            &classifier("investigation-workflow", "x"),
        ] {
            let s = select("", "Development", input, 1);
            assert_eq!(
                (s.workflow, s.source),
                (DEFAULT_WORKFLOW, "default"),
                "{input:?}"
            );
        }
        assert_eq!(select("", "Development", "{}", 1).reason, "unspecified");
    }

    #[test]
    fn empty_classifier_reason_is_written_as_unspecified() {
        let s = select("", "Development", &classifier("quick-change", "   "), 1);
        assert_eq!(s.reason, "unspecified");
    }

    #[test]
    fn log_line_has_the_fixed_format() {
        let s = select(
            "",
            "Development",
            &classifier("quick-change", "comment typo"),
            1,
        );
        assert_eq!(
            log_line(&s),
            "[dev-orchestrator] Workflow: quick-change source=classifier reason=comment typo"
        );
    }

    #[test]
    fn sanitise_replaces_control_bidi_and_invisible_characters_with_spaces() {
        let raw = "a\nb\x1b[31mc\u{202e}d\u{200b}e\u{2066}f\u{feff}g\th";
        let out = sanitise(raw, 200);
        assert!(!out.chars().any(|c| c.is_control()), "{out:?}");
        for bad in ['\u{202e}', '\u{200b}', '\u{2066}', '\u{feff}'] {
            assert!(!out.contains(bad), "{out:?} still holds {bad:?}");
        }
        assert!(out.starts_with("a b") && out.ends_with("g h"), "{out:?}");
    }

    #[test]
    fn sanitise_cuts_on_char_boundaries() {
        assert_eq!(sanitise(&"é".repeat(300), 200).chars().count(), 200);
        assert_eq!(sanitise(&"x".repeat(100), 64).chars().count(), 64);
    }

    #[test]
    fn long_multi_line_reason_is_one_line_of_at_most_200_chars() {
        let reason = format!("{}\n{}", "r".repeat(300), "second line");
        let s = select("", "Development", &classifier("quick-change", &reason), 1);
        assert!(!s.reason.contains('\n'));
        assert_eq!(s.reason.chars().count(), 200);
    }

    #[test]
    fn warning_values_are_sanitised_and_cut_to_64_chars() {
        let bogus = format!("q\x1b{}", "z".repeat(100));
        let s = select(&bogus, "Development", "", 1);
        let quoted = s.warnings[0]
            .strip_prefix("[dev-orchestrator] WARNING: unknown workflow override '")
            .and_then(|rest| rest.strip_suffix("' ignored"))
            .expect("W1 shape");
        assert_eq!(quoted.chars().count(), 64);
        assert!(!quoted.contains('\x1b'));
    }

    #[test]
    fn existing_target_demotes_quick_change_only() {
        let bp = Some(ExistingTarget::BranchOrPr);
        let quick = select("", "Development", &classifier("quick-change", "typo"), 1);
        let kept = demote_for_existing_target(quick.clone(), None);
        assert_eq!(kept, quick);
        let demoted = demote_for_existing_target(quick, bp);
        assert_eq!(demoted.workflow, DEFAULT_WORKFLOW);
        assert_eq!(demoted.source, "classifier");
        assert_eq!(demoted.reason, "existing_branch or pr_number set");
        assert!(demoted.warnings.is_empty());

        let forced = select("quick-change", "Development", "", 1);
        let demoted = demote_for_existing_target(forced, bp);
        assert_eq!(demoted.workflow, DEFAULT_WORKFLOW);
        assert_eq!(demoted.warnings, vec![EXISTING_TARGET_WARNING.to_string()]);

        let full = select("", "Development", &classifier("default-workflow", "big"), 1);
        assert_eq!(demote_for_existing_target(full.clone(), bp), full);
        let none = select("", "Q&A", "", 1);
        assert_eq!(demote_for_existing_target(none.clone(), bp), none);
    }

    #[test]
    fn a_task_that_names_its_branch_demotes_quick_change_with_w4() {
        let named = Some(ExistingTarget::NamedInTask);
        let quick = select("", "Development", &classifier("quick-change", "typo"), 1);
        let demoted = demote_for_existing_target(quick, named);
        assert_eq!(
            (demoted.workflow, demoted.source),
            (DEFAULT_WORKFLOW, "classifier")
        );
        assert_eq!(demoted.reason, "task names a branch");
        assert!(demoted.warnings.is_empty());

        let forced = select("quick-change", "Development", "", 1);
        let demoted = demote_for_existing_target(forced, named);
        assert_eq!(
            (demoted.workflow, demoted.source),
            (DEFAULT_WORKFLOW, "override")
        );
        assert_eq!(demoted.warnings, vec![NAMED_BRANCH_WARNING.to_string()]);
    }

    #[test]
    fn existing_target_checks_the_task_text_only_when_no_target_is_set() {
        let task = "Fix the typo.\nBranch: feat/issue-9-typo\n";
        assert_eq!(existing_target("", "", "Fix the typo"), None);
        assert_eq!(
            existing_target("", " ", task),
            Some(ExistingTarget::NamedInTask)
        );
        assert_eq!(
            existing_target("feat/x", "", task),
            Some(ExistingTarget::BranchOrPr)
        );
        assert_eq!(
            existing_target("", "42", "Fix the typo"),
            Some(ExistingTarget::BranchOrPr)
        );
    }

    #[test]
    fn parallel_override_follows_the_logged_selection() {
        // Each case: the override the parallel builder applies agrees with
        // the workflow select-workflow logs for a Development override.
        let cases = [
            ("quick-change", "Development", false, Some(QUICK_CHANGE)),
            (
                " Default-Workflow ",
                "Development",
                false,
                Some(DEFAULT_WORKFLOW),
            ),
            ("quick-change", "Development", true, None),
            (
                "default-workflow",
                "Development",
                true,
                Some(DEFAULT_WORKFLOW),
            ),
            ("quick-change", "Investigation", false, None),
            ("quick-change", "Q&A", false, None),
            ("quick-change", "Operations", false, None),
            ("quick", "Development", false, None),
            ("", "Development", false, None),
        ];
        for (ov, task_type, existing, want) in cases {
            assert_eq!(
                parallel_override(ov, task_type, existing),
                want,
                "{ov:?} {task_type:?} existing={existing}"
            );
            let target = existing.then_some(ExistingTarget::BranchOrPr);
            let logged = demote_for_existing_target(select(ov, task_type, "", 2), target);
            if let Some(w) = want {
                assert_eq!(logged.workflow, w, "{ov:?} {task_type:?}");
                assert_eq!(logged.source, "override");
            } else {
                assert_ne!(logged.workflow, QUICK_CHANGE, "{ov:?} {task_type:?}");
            }
        }
    }
}
