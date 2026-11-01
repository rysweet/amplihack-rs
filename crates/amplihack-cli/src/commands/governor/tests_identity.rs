//! Issue #1811 (TDD): workstream identity and class (D7, D9).
//!
//! Wire with `#[cfg(test)] mod tests_identity;` in `governor/mod.rs`.
//!
//! Expected API in `identity` (the environment is passed as a closure, so
//! no test touches the process environment):
//! - `parse_workstream(&str) -> Result<String, String>`: the clap value
//!   parser for `--workstream`; `[A-Za-z0-9._-]{1,64}`, not `.` or `..`.
//! - `workstream(flag: Option<&str>, env: &dyn Fn(&str) -> Option<String>)
//!   -> String`: flag, `AMPLIHACK_WORKSTREAM`, `AMPLIHACK_WORKSTREAM_ISSUE`
//!   (decimal only, as `ws-<n>`), cleaned `AMPLIHACK_RECIPE_RUN_ID`,
//!   `default`.
//! - `class(flag: Option<Class>, env: &dyn Fn(&str) -> Option<String>,
//!   recipe: Option<&str>, task: Option<&str>) -> Class`.

use super::identity::{class, parse_workstream, workstream};
use super::model::Class;
use std::collections::BTreeMap;

fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
    let map: BTreeMap<String, String> = pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    move |k: &str| map.get(k).cloned()
}

#[test]
fn parse_workstream_accepts_the_documented_alphabet() {
    for ok in ["a", "ws-1811", "A.b_c-9", &"x".repeat(64)] {
        assert_eq!(parse_workstream(ok).as_deref(), Ok(ok), "{ok}");
    }
}

#[test]
fn parse_workstream_rejects_everything_else() {
    let long = "x".repeat(65);
    for bad in [
        "",
        ".",
        "..",
        "bad/id",
        "a b",
        "ä",
        "a\nb",
        "a\u{1b}[31m",
        &long,
    ] {
        assert!(parse_workstream(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn precedence_is_flag_then_env_then_issue_then_run_id() {
    let all = env(&[
        ("AMPLIHACK_WORKSTREAM", "from-env"),
        ("AMPLIHACK_WORKSTREAM_ISSUE", "1811"),
        ("AMPLIHACK_RECIPE_RUN_ID", "run-7"),
    ]);
    assert_eq!(workstream(Some("flag"), &all), "flag");
    assert_eq!(workstream(None, &all), "from-env");
    let no_ws = env(&[
        ("AMPLIHACK_WORKSTREAM_ISSUE", "1811"),
        ("AMPLIHACK_RECIPE_RUN_ID", "run-7"),
    ]);
    assert_eq!(workstream(None, &no_ws), "ws-1811");
    assert_eq!(
        workstream(None, &env(&[("AMPLIHACK_RECIPE_RUN_ID", "run-7")])),
        "run-7"
    );
    assert_eq!(workstream(None, &env(&[])), "default");
}

#[test]
fn invalid_env_workstream_becomes_default() {
    let e = env(&[("AMPLIHACK_WORKSTREAM", "bad/id SECRET")]);
    assert_eq!(workstream(None, &e), "default");
}

#[test]
fn issue_number_must_be_decimal() {
    for bad in ["18a1", "-3", "", "1 2", "0x10"] {
        let e = env(&[("AMPLIHACK_WORKSTREAM_ISSUE", bad)]);
        assert_eq!(workstream(None, &e), "default", "{bad:?}");
    }
}

#[test]
fn run_id_is_cleaned_and_truncated() {
    let e = env(&[("AMPLIHACK_RECIPE_RUN_ID", "run id/2026:11")]);
    assert_eq!(workstream(None, &e), "run-id-2026-11");
    let long = "r".repeat(100);
    let e = env(&[("AMPLIHACK_RECIPE_RUN_ID", long.as_str())]);
    assert_eq!(workstream(None, &e), "r".repeat(64));
}

#[test]
fn run_id_still_invalid_after_cleaning_is_skipped() {
    for bad in [".", "..", ""] {
        let e = env(&[("AMPLIHACK_RECIPE_RUN_ID", bad)]);
        assert_eq!(workstream(None, &e), "default", "{bad:?}");
    }
}

#[test]
fn class_precedence_is_flag_then_env_then_inference() {
    let e = env(&[("AMPLIHACK_GOVERNOR_CLASS", "review")]);
    let task = Some("[coord] ping");
    assert_eq!(
        class(Some(Class::Recovery), &e, None, task),
        Class::Recovery
    );
    assert_eq!(class(None, &e, None, task), Class::Review);
    assert_eq!(class(None, &env(&[]), None, task), Class::Coordination);
    assert_eq!(class(None, &env(&[]), None, None), Class::Ordinary);
}

#[test]
fn invalid_class_env_falls_through_to_inference() {
    let e = env(&[("AMPLIHACK_GOVERNOR_CLASS", "root")]);
    assert_eq!(
        class(None, &e, None, Some("[coord] x")),
        Class::Coordination
    );
    assert_eq!(class(None, &e, None, None), Class::Ordinary);
}

#[test]
fn coord_marker_counts_only_as_the_leading_token() {
    let e = env(&[]);
    assert_eq!(
        class(None, &e, None, Some("  [coord] hello")),
        Class::Coordination
    );
    for task in [
        "hello [coord]",
        "[coordination] x",
        "x\n[coord] y",
        "[coord]x",
    ] {
        assert_eq!(
            class(None, &e, None, Some(task)),
            Class::Ordinary,
            "{task:?}"
        );
    }
}

#[test]
fn recipe_names_map_from_fixed_lists() {
    let e = env(&[]);
    for name in [
        "consensus-pr-review",
        "workflow-pr-review",
        "workflow-refactor-review",
    ] {
        assert_eq!(class(None, &e, Some(name), None), Class::Review, "{name}");
    }
    for name in ["recovery", "recovery-stuck-run"] {
        assert_eq!(class(None, &e, Some(name), None), Class::Recovery, "{name}");
    }
    for name in [
        "default-workflow",
        "my-review-hack",
        "norecovery",
        "quick-change",
    ] {
        assert_eq!(class(None, &e, Some(name), None), Class::Ordinary, "{name}");
    }
}

#[test]
fn task_text_cannot_claim_a_recipe_class() {
    let e = env(&[]);
    let task = Some("workflow-pr-review recovery");
    assert_eq!(
        class(None, &e, Some("default-workflow"), task),
        Class::Ordinary
    );
}

#[test]
fn class_parses_as_a_clap_value_enum() {
    use clap::ValueEnum;
    for (raw, want) in [
        ("coordination", Class::Coordination),
        ("recovery", Class::Recovery),
        ("review", Class::Review),
        ("ordinary", Class::Ordinary),
    ] {
        assert_eq!(Class::from_str(raw, false), Ok(want));
        assert_eq!(want.as_str(), raw);
    }
    assert!(Class::from_str("admin", false).is_err());
}

/// S1 (PR 1831): a recipe's task description can come from a GitHub
/// issue, so `[coord]` in it never reaches class inference. `claude -p`
/// still does, and a recipe run opts in through the flag or the variable.
#[test]
fn recipe_task_text_cannot_claim_coordination() {
    use super::{AdmitRequest, GovernorArgs};
    let args = GovernorArgs::default();
    let recipe = AdmitRequest::recipe(Some(&args), "default-workflow");
    assert_eq!(recipe.task, None);
    assert_eq!(
        class(
            None,
            &env(&[]),
            recipe.recipe.as_deref(),
            recipe.task.as_deref()
        ),
        Class::Ordinary
    );
    let opted_in = env(&[("AMPLIHACK_GOVERNOR_CLASS", "coordination")]);
    assert_eq!(
        class(
            None,
            &opted_in,
            recipe.recipe.as_deref(),
            recipe.task.as_deref()
        ),
        Class::Coordination
    );
    assert_eq!(
        class(
            Some(Class::Coordination),
            &env(&[]),
            recipe.recipe.as_deref(),
            recipe.task.as_deref()
        ),
        Class::Coordination
    );

    let claude_args = ["-p".to_string(), "[coord] ping".to_string()];
    let launch = AdmitRequest::launch(&args, &claude_args);
    assert_eq!(
        class(
            None,
            &env(&[]),
            launch.recipe.as_deref(),
            launch.task.as_deref()
        ),
        Class::Coordination
    );
}
