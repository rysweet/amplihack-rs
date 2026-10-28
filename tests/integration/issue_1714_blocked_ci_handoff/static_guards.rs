//! Static guards for #1714: who can switch the hand-off on, where the
//! allow-list lives, how the loops receive the PR, and that the merge gate
//! never reads the hand-off.

use std::fs;
use std::path::PathBuf;

use super::fixture::*;

fn recipe_files() -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut dirs = vec![workspace_root().join("amplifier-bundle/recipes")];
    while let Some(dir) = dirs.pop() {
        for entry in fs::read_dir(&dir).expect("read recipes dir").flatten() {
            let path = entry.path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|e| e == "yaml" || e == "yml") {
                files.push(path);
            }
        }
    }
    files
}

fn tool_files() -> Vec<PathBuf> {
    fs::read_dir(workspace_root().join("amplifier-bundle/tools"))
        .expect("read tools dir")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "sh"))
        .collect()
}

fn rel(path: &std::path::Path) -> String {
    path.strip_prefix(workspace_root())
        .unwrap_or(path)
        .display()
        .to_string()
}

// T6: only autodrive-build step-02 sets the flag; default-workflow and
// autodrive-build's own context declare "".
#[test]
fn only_autodrive_build_step_02_switches_the_handoff_on() {
    let mut setters = Vec::new();
    for path in recipe_files() {
        for (n, line) in read(&path).lines().enumerate() {
            let line = line.trim();
            if line.starts_with('#') {
                continue;
            }
            if let Some(value) = line.strip_prefix("blocked_ci_handoff:") {
                setters.push(format!("{}:{}={}", rel(&path), n + 1, value.trim()));
            }
        }
    }
    let unexpected: Vec<_> = setters
        .iter()
        .filter(|s| {
            !(s.starts_with("amplifier-bundle/recipes/autodrive-build.yaml:")
                && (s.ends_with("=\"true\"") || s.ends_with("=\"\""))
                || s.starts_with("amplifier-bundle/recipes/default-workflow.yaml:")
                    && s.ends_with("=\"\""))
        })
        .collect();
    assert!(
        unexpected.is_empty(),
        "unexpected blocked_ci_handoff settings: {unexpected:?}"
    );
    assert_eq!(setters.len(), 3, "{setters:?}");

    let build = recipe_yaml("autodrive-build");
    let step02 = step(&build, "step-02-default-workflow");
    assert_eq!(
        step02["context"]["blocked_ci_handoff"].as_str(),
        Some("true")
    );
    assert_eq!(build["context"]["workflow_result"].as_str(), Some(""));
    assert_eq!(
        build["context"]["blocked_ci_handoff"].as_str(),
        Some(""),
        "autodrive-build's own context must declare the flag off"
    );
    let default = recipe_yaml("default-workflow");
    assert_eq!(default["context"]["blocked_ci_handoff"].as_str(), Some(""));
}

// T12: no step can produce the flag at run time, and workflow_result has one
// producer within the default-workflow call tree.
#[test]
fn no_step_outputs_the_flag_and_workflow_result_has_one_producer() {
    for path in recipe_files() {
        assert!(
            !read(&path).contains("output: \"blocked_ci_handoff\""),
            "{} must not output blocked_ci_handoff",
            rel(&path)
        );
    }
    let mut seen = vec!["default-workflow".to_owned()];
    let mut queue = seen.clone();
    while let Some(name) = queue.pop() {
        for line in read(&recipe_path(&name)).lines() {
            if let Some(sub) = line
                .trim()
                .strip_prefix("recipe: \"")
                .and_then(|s| s.strip_suffix('"'))
                && !seen.iter().any(|s| s == sub)
                && recipe_path(sub).is_file()
            {
                seen.push(sub.to_owned());
                queue.push(sub.to_owned());
            }
        }
    }
    let producers: Vec<_> = seen
        .iter()
        .filter(|name| read(&recipe_path(name)).contains("output: \"workflow_result\""))
        .collect();
    assert_eq!(producers, ["workflow-finalize"], "call tree: {seen:?}");
}

// T6: the allow-list and both hand-off functions are defined only in the brick.
#[test]
fn the_allow_list_is_defined_once_in_the_brick() {
    let brick_text = read(&tool_path(BRICK));
    for function in [
        "blocked_ci_handoff_allowed()",
        "blocked_ci_handoff_flag()",
        "finalization_fail_or_handoff()",
        "autodrive_build_handoff()",
    ] {
        let mut definers: Vec<String> = tool_files()
            .into_iter()
            .filter(|p| read(p).contains(function))
            .map(|p| rel(&p))
            .collect();
        for path in recipe_files() {
            if read(&path).contains(function) {
                definers.push(rel(&path));
            }
        }
        assert_eq!(
            definers,
            [format!("amplifier-bundle/tools/{BRICK}")],
            "`{function}` must be defined only in the brick"
        );
    }
    for state in [
        "FAILED_CLOSED_UNMERGED",
        "FAILED_PR_METADATA_UNAVAILABLE",
        "FAILED_MEANINGFUL_DIFF",
        "FAILED_IMPLEMENTATION",
        "HOLLOW_SUCCESS",
        "INCOMPLETE",
    ] {
        assert!(
            brick_text.contains(state),
            "the brick's header must record why {state} is rejected"
        );
    }
    let build = read(&recipe_path("autodrive-build"));
    assert!(
        !code_lines(&build).any(|l| l.contains("BLOCKED_CI)") || l.contains("BLOCKED_CI|")),
        "autodrive-build must not hold its own copy of the allow-list"
    );
    assert!(
        brick_text.lines().count() <= 300,
        "the brick must fit the 300-line budget"
    );
}

// T6: the hand-off is decided from typed fields, never from logs or stderr.
#[test]
fn the_handoff_never_reads_log_text_or_stderr() {
    let brick_text = read(&tool_path(BRICK));
    for line in code_lines(&brick_text) {
        for forbidden in ["grep", "2>&1 |", ".log", "narrative"] {
            assert!(
                !line.contains(forbidden),
                "the hand-off must not read `{forbidden}`: {line}"
            );
        }
        assert!(
            !line.contains("RECIPE_VAR_workflow_result__") && !line.contains("WORKFLOW_RESULT_"),
            "workflow_result is read through rv_field only: {line}"
        );
    }
}

// T6: the dispatcher sources the brick before the validator, and the
// validator routes both BLOCKED_CI exits through the brick.
#[test]
fn the_validator_routes_both_blocked_ci_exits_through_the_brick() {
    let dispatcher = read(&tool_path("workflow_agentic_finalization.sh"));
    let brick_at = dispatcher
        .find(&format!(". \"$here/{BRICK}\""))
        .expect("the dispatcher must source the brick through $here");
    let validate_at = dispatcher
        .find(". \"$here/workflow_finalization_validate.sh\"")
        .expect("the dispatcher sources the validator");
    assert!(brick_at < validate_at, "brick before validator");

    let validator = read(&tool_path("workflow_finalization_validate.sh"));
    assert!(validator.lines().count() <= 300, "validator line budget");
    let calls = code_lines(&validator)
        .filter(|l| l.contains("finalization_fail_or_handoff"))
        .count();
    assert_eq!(calls, 2, "exactly the two BLOCKED_CI exits hand off");
    let open_pr_exit = code_lines(&validator)
        .find(|l| l.contains("\"$open_pr_checks\" != \"failing\""))
        .expect("the open-PR failing-check exit");
    assert!(open_pr_exit.contains("finalization_fail_or_handoff \"BLOCKED_CI\""));
}

// T1d: the loops take the build's PR and have no condition that skips a
// handed-off build; the report shows the two new fields.
#[test]
fn both_loops_run_on_a_handed_off_build() {
    let composer = recipe_yaml("auto-drive-to-merge");
    for id in ["autodrive-crusty-loop", "autodrive-merge-loop"] {
        let loop_step = step(&composer, id);
        assert_eq!(
            loop_step["context"]["pr_number"].as_str(),
            Some("{{build_result.pr}}"),
            "{id}"
        );
        let condition = loop_step["condition"].as_str().unwrap_or("");
        for needle in ["handed_off", "terminal_state", "BLOCKED_CI"] {
            assert!(
                !condition.contains(needle),
                "{id} must not skip on `{needle}`: {condition}"
            );
        }
    }
    let text = read(&recipe_path("auto-drive-to-merge"));
    assert!(
        text.contains(
            "Phase 1 terminal state: {{build_result.terminal_state}} (handed off: {{build_result.handed_off}})"
        ),
        "autodrive-report must show how phase 1 ended"
    );
}

// T6: the merge gate stays the only merge control and never reads the hand-off.
#[test]
fn the_merge_gate_never_reads_the_handoff() {
    let mut files = vec![tool_path("autodrive_merge_gate.sh")];
    for name in [
        "autodrive-merge-loop",
        "autodrive-merge-round",
        "autodrive-merge-evidence",
        "autodrive-crusty-loop",
    ] {
        files.push(recipe_path(name));
    }
    for path in files {
        let text = read(&path);
        for needle in [
            "handed_off",
            "build_result.terminal_state",
            "blocked_ci_handoff",
            "merge_ready_loop",
        ] {
            assert!(
                !text.contains(needle),
                "{} must not read `{needle}`",
                rel(&path)
            );
        }
    }
}

// The step-03 call: direct, after PR resolution, before the no-PR error.
#[test]
fn step_03_calls_the_handoff_before_the_no_pr_error() {
    let command = step03_command();
    let call = command
        .find("autodrive_build_handoff \"$PR\" || exit 1")
        .expect("step-03 calls autodrive_build_handoff directly");
    let no_pr = command
        .find("phase 1 finished without a pull request")
        .expect("the existing no-PR error");
    assert!(call < no_pr, "hand-off decision precedes the no-PR error");
    assert!(
        !command.contains("$(autodrive_build_handoff"),
        "a subshell would lose HANDOFF_* variables"
    );
    assert!(
        !command.contains("REPO_PATH:-.}/amplifier-bundle"),
        "the brick is never sourced from REPO_PATH"
    );
    assert!(
        command.contains("jq -n"),
        "build_result is built with jq -n --arg"
    );
}
