//! Issue #1802: the structure of `amplifier-bundle/recipes/auto-drive-light.yaml`
//! and the invariants it shares with `auto-drive-to-merge`.
//!
//! These tests parse the live YAML, the two new tools and the manifest; no
//! recipe runs. They cover acceptance test (f) (no design, consensus or
//! architect steps and no `default-workflow`), the escalation wiring, the
//! "merge phase reached" condition of test (a), and the prohibited strings.
//!
//! Issue #1982 moved light mode to the per-finding follow-up model of #1979:
//! the effective task id (`plan_task_id`, else `#<issue_number>`) is worked
//! out once by `light-task-id`, through `autodrive_followups.sh resolve-task`
//! (no copy of the rule lives in the recipe), and forwarded to all three child
//! recipes; light mode has no marker and no hook inside the loops, the change
//! is measured only by `light-eligibility`; and `light-report` lists the
//! issues from `followups.tsv`. The bash bodies of `light-prereqs`,
//! `light-task-id` and `light-report` are also run, with fakes on `PATH`.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Output;

use serde_yaml::Value;
use sha2::{Digest, Sha256};

use crate::support::auto_drive_light::{
    Fakes, LightRepo, fake_bin, jq_available, private_dir, private_file, read, recipe, repo_root,
    run_bash, show, stderr, stdout, step_command,
};

const NAME: &str = "auto-drive-light";

const STEPS: &[&str] = &[
    "light-prereqs",
    "light-pr-input",
    "light-task-id",
    "light-worktree",
    "light-base-pin",
    "light-implement",
    "light-qa",
    "light-precommit",
    "light-commit-push",
    "light-open-pr",
    "light-resolve-pr",
    "light-eligibility",
    "light-escalate",
    "light-crusty-loop",
    "light-check-unmerged",
    "light-merge-loop",
    "light-report",
];

const BUILD_STEPS: &[&str] = &[
    "light-worktree",
    "light-base-pin",
    "light-implement",
    "light-qa",
    "light-precommit",
    "light-commit-push",
    "light-open-pr",
];

const LIGHT_PATH: &[&str] = &[
    "light-crusty-loop",
    "light-check-unmerged",
    "light-merge-loop",
];

const LIGHT_COND: &str = "light_eligibility.decision == 'light'";
const ESCALATE_COND: &str = "light_eligibility.decision == 'escalate'";

/// Never in a bash command of the recipe, nor anywhere in either tool.
const PROHIBITED: &[&str] = &["--admin", "--no-verify", "SKIP=", "gh pr merge", "eval "];

fn r() -> Value {
    recipe(NAME)
}

fn steps(r: &Value) -> Vec<Value> {
    r["steps"].as_sequence().expect("steps").clone()
}

fn step(r: &Value, id: &str) -> Value {
    steps(r)
        .into_iter()
        .find(|s| s["id"].as_str() == Some(id))
        .unwrap_or_else(|| panic!("step {id} missing"))
}

fn condition(r: &Value, id: &str) -> String {
    step(r, id)["condition"].as_str().unwrap_or("").to_string()
}

fn is_agent(s: &Value) -> bool {
    s["type"].as_str() == Some("agent") || s.get("agent").is_some()
}

fn is_bash(s: &Value) -> bool {
    s["type"].as_str() == Some("bash")
}

fn yaml_text(v: &Value) -> String {
    serde_yaml::to_string(v).unwrap()
}

fn bash_commands(r: &Value) -> Vec<(String, String)> {
    steps(r)
        .into_iter()
        .filter(is_bash)
        .map(|s| {
            (
                s["id"].as_str().unwrap().to_string(),
                s["command"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect()
}

fn manifest_hash(rel: &str) -> String {
    let body = std::fs::read(repo_root().join(rel)).unwrap();
    let hex: String = Sha256::digest(&body)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    hex[..16].to_string()
}

// --- identity, order and limits --------------------------------------------

#[test]
fn steps_are_present_in_order() {
    let r = r();
    assert_eq!(r["name"].as_str(), Some(NAME));
    let ids: Vec<String> = steps(&r)
        .iter()
        .map(|s| s["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(ids, STEPS);
}

#[test]
fn recursion_limits_and_no_timeouts() {
    let r = r();
    assert_eq!(r["recursion"]["max_depth"].as_u64(), Some(6));
    assert_eq!(r["recursion"]["max_total_steps"].as_u64(), Some(200));
    let text = read("amplifier-bundle/recipes/auto-drive-light.yaml");
    for banned in ["timeout:", "timeout_seconds", "default_step_timeout"] {
        assert!(
            !text.contains(banned),
            "auto-drive-light.yaml contains {banned}"
        );
    }
}

#[test]
fn context_declares_every_documented_key_with_defaults() {
    let r = r();
    let ctx = &r["context"];
    for (k, v) in [
        ("task_description", ""),
        ("repo_path", "."),
        ("issue_number", ""),
        ("plan_task_id", ""),
        ("light_task_id", ""),
        ("existing_branch", ""),
        ("pr_number", ""),
        ("qa_command", ""),
        ("light_max_lines", "300"),
        ("light_max_new_modules", "0"),
        ("light_max_public_api", "0"),
        ("light_max_schema", "0"),
        ("light_max_recipe_contract", "0"),
        ("autodrive_state_dir", ""),
        ("autodrive_dry_run", "false"),
        ("no_merge", "true"),
    ] {
        assert_eq!(ctx[k].as_str(), Some(v), "context.{k}");
    }
}

// --- test (f): no design phases, no default-workflow ------------------------

#[test]
fn no_design_consensus_architect_or_default_workflow_step() {
    let r = r();
    for s in steps(&r) {
        let id = s["id"].as_str().unwrap();
        for banned in ["design", "consensus", "architect"] {
            assert!(!id.contains(banned), "step {id} looks like a {banned} step");
        }
        if let Some(rec) = s["recipe"].as_str() {
            for banned in [
                "default-workflow",
                "consensus-workflow",
                "autodrive-build",
                "smart-orchestrator",
                "investigation-workflow",
            ] {
                assert_ne!(rec, banned, "step {id} calls {banned}");
            }
        }
        if let Some(agent) = s["agent"].as_str() {
            assert!(!agent.contains("architect"), "step {id} uses {agent}");
        }
    }
    let text = yaml_text(&r["steps"]);
    assert!(
        !text.contains("default-workflow"),
        "no step may name default-workflow"
    );
}

#[test]
fn exactly_one_agent_step_and_it_carries_the_no_merge_line() {
    let r = r();
    let agents: Vec<Value> = steps(&r).into_iter().filter(is_agent).collect();
    assert_eq!(agents.len(), 1, "one implementation session only");
    let a = &agents[0];
    assert_eq!(a["id"].as_str(), Some("light-implement"));
    let prompt = a["prompt"].as_str().expect("prompt");
    assert!(prompt.contains("[amplihack:no-merge]"));
    let lower = prompt.to_lowercase();
    for banned in [
        "dev-orchestrator",
        "smart-orchestrator",
        "ultrathink",
        "recipe run",
        "/dev ",
        "default-workflow",
    ] {
        assert!(
            !lower.contains(banned),
            "the prompt must not mention {banned}"
        );
    }
}

// --- escalation wiring -----------------------------------------------------

#[test]
fn auto_drive_to_merge_is_called_only_from_light_escalate() {
    let r = r();
    for s in steps(&r) {
        let id = s["id"].as_str().unwrap();
        let mentions = yaml_text(&s).contains("auto-drive-to-merge");
        if id == "light-escalate" {
            assert_eq!(s["type"].as_str(), Some("recipe"));
            assert_eq!(s["recipe"].as_str(), Some("auto-drive-to-merge"));
        } else {
            assert!(!mentions, "step {id} must not name auto-drive-to-merge");
        }
    }
}

#[test]
fn light_escalate_runs_only_on_escalate_and_passes_the_same_pr_and_state() {
    let r = r();
    assert_eq!(condition(&r, "light-escalate"), ESCALATE_COND);
    let ctx = &step(&r, "light-escalate")["context"];
    assert_eq!(ctx["pr_number"].as_str(), Some("{{build_result.pr}}"));
    assert_eq!(ctx["existing_branch"].as_str(), Some(""));
    assert_eq!(
        ctx["autodrive_state_dir"].as_str(),
        Some("{{build_result.state_dir}}")
    );
    for k in [
        "task_description",
        "repo_path",
        "autodrive_dry_run",
        "no_merge",
    ] {
        assert_eq!(
            ctx[k].as_str(),
            Some(format!("{{{{{k}}}}}").as_str()),
            "light-escalate passes {k} unchanged"
        );
    }
}

#[test]
fn light_path_steps_run_only_on_light() {
    let r = r();
    for id in LIGHT_PATH {
        assert_eq!(condition(&r, id), LIGHT_COND, "{id}");
    }
}

#[test]
fn merge_phase_has_no_condition_other_than_light() {
    // Acceptance test (a): with the fake evaluator answering DONE, nothing
    // else stands between a light CLEAN round and the merge loop.
    let r = r();
    let s = step(&r, "light-merge-loop");
    assert_eq!(s["type"].as_str(), Some("recipe"));
    assert_eq!(s["recipe"].as_str(), Some("autodrive-merge-loop"));
    assert_eq!(s["condition"].as_str(), Some(LIGHT_COND));
}

#[test]
fn loops_are_the_unchanged_shared_sub_recipes_on_the_build_result() {
    let r = r();
    let crusty = step(&r, "light-crusty-loop");
    assert_eq!(crusty["recipe"].as_str(), Some("autodrive-crusty-loop"));
    assert_eq!(
        crusty["context"]["pr_number"].as_str(),
        Some("{{build_result.pr}}")
    );
    assert_eq!(
        crusty["context"]["autodrive_state_dir"].as_str(),
        Some("{{build_result.state_dir}}")
    );
    let merge = step(&r, "light-merge-loop");
    assert_eq!(
        merge["context"]["autodrive_state_dir"].as_str(),
        Some("{{build_result.state_dir}}")
    );
    assert_eq!(
        merge["context"]["autodrive_dry_run"].as_str(),
        Some("{{autodrive_dry_run}}")
    );
    assert_eq!(
        step(&r, "light-worktree")["recipe"].as_str(),
        Some("workflow-worktree")
    );
}

#[test]
fn build_steps_run_only_for_a_new_change() {
    let r = r();
    for id in BUILD_STEPS {
        let c = condition(&r, id);
        assert!(
            c.contains("pr_number") && c.contains("existing_branch"),
            "{id}: build steps are skipped when pr_number or existing_branch is set, got {c:?}"
        );
    }
}

#[test]
fn always_steps_have_no_condition() {
    let r = r();
    for id in [
        "light-prereqs",
        "light-pr-input",
        "light-task-id",
        "light-resolve-pr",
        "light-eligibility",
        "light-report",
    ] {
        assert_eq!(condition(&r, id), "", "{id} always runs");
    }
}

#[test]
fn eligibility_step_measures_records_and_handles_escalation() {
    let r = r();
    let s = step(&r, "light-eligibility");
    assert_eq!(s["output"].as_str(), Some("light_eligibility"));
    assert_eq!(s["parse_json"].as_bool(), Some(true));
    let cmd = s["command"].as_str().unwrap();
    for needle in [
        "autodrive_light_eligibility.sh",
        "light-eligibility.json",
        "escalation.json",
        "previously-escalated",
        "RECIPE_VAR_light_max_lines",
        "RECIPE_VAR_light_max_new_modules",
        "RECIPE_VAR_light_max_public_api",
        "RECIPE_VAR_light_max_schema",
        "RECIPE_VAR_light_max_recipe_contract",
    ] {
        assert!(cmd.contains(needle), "light-eligibility must use {needle}");
    }
    assert!(!cmd.contains("--force"), "no force-light override");
}

#[test]
fn light_mode_has_no_marker() {
    // #1982: nothing inside the loops reads a light-mode marker any more, so
    // the recipe neither writes nor removes one.
    for (id, cmd) in bash_commands(&r()) {
        assert!(!cmd.contains("light-mode"), "{id} still touches light-mode");
    }
    let r = r();
    assert!(
        r["context"].get("light_marker").is_none(),
        "light_marker is gone"
    );
    assert!(
        r["context"].get("crusty_round_context").is_none(),
        "crusty_round_context was declared only for the removed light helper"
    );
}

#[test]
fn resolve_pr_refuses_cross_repository_prs_and_existing_branch_without_pr() {
    let cmd = step(&r(), "light-resolve-pr")["command"]
        .as_str()
        .unwrap()
        .to_string();
    for needle in [
        "isCrossRepository",
        "light-cross-repo",
        "light-existing-branch-no-pr",
        "state_dir",
        "head_sha",
    ] {
        assert!(cmd.contains(needle), "light-resolve-pr must use {needle}");
    }
    assert_eq!(
        step(&r(), "light-resolve-pr")["output"].as_str(),
        Some("build_result")
    );
}

#[test]
fn check_unmerged_uses_the_shared_state_helper() {
    let cmd = step(&r(), "light-check-unmerged")["command"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(cmd.contains("autodrive_require_unmerged"));
    assert!(cmd.contains("AMPLIHACK_HOME"));
}

#[test]
fn qa_runs_the_caller_command_or_the_quick_change_checks() {
    let cmd = step(&r(), "light-qa")["command"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(cmd.contains("bash -c \"$RECIPE_VAR_qa_command\""));
    assert!(cmd.contains("quick_change_checks.sh"));
}

#[test]
fn report_names_every_path() {
    let cmd = step(&r(), "light-report")["command"]
        .as_str()
        .unwrap()
        .to_string();
    for path in [
        "escalate",
        "light",
        "followups.tsv",
        "follow_up_issues",
        "follow-up issues:",
    ] {
        assert!(cmd.contains(path), "light-report must handle {path}");
    }
    // #1982: the path is decided once, by light-eligibility; no in-loop
    // escalation exists to report.
    assert!(!cmd.contains("light-escalated"), "{cmd}");
    assert!(!cmd.contains("escalation.json"), "{cmd}");
    // #1982: the single-issue file and key are retired.
    assert!(
        !cmd.contains("follow-up-issue"),
        "light-report must not read follow-up-issue"
    );
    assert!(
        !cmd.match_indices("follow_up_issue")
            .any(|(i, m)| !cmd[i + m.len()..].starts_with('s')),
        "the output key is follow_up_issues, never follow_up_issue"
    );
    let text = read("amplifier-bundle/recipes/auto-drive-light.yaml");
    assert!(
        text.contains("| Follow-up issues | {{light_summary.follow_up_issues}} |"),
        "the report template has a Follow-up issues row"
    );
    assert!(!text.contains("{{light_summary.follow_up_issue}}"));
}

// --- invariants ------------------------------------------------------------

#[test]
fn bash_steps_have_no_templates_and_read_context_only_as_recipe_var() {
    let r = r();
    let keys: Vec<String> = r["context"]
        .as_mapping()
        .unwrap()
        .keys()
        .map(|k| k.as_str().unwrap().to_string())
        .collect();
    for (id, cmd) in bash_commands(&r) {
        assert!(
            !cmd.contains("{{"),
            "{id}: no {{{{...}}}} templates in bash"
        );
        for k in &keys {
            let up = k.to_uppercase();
            for form in [format!("${{{up}"), format!("${up}")] {
                // `$X` must not be followed by another identifier character.
                let hit = cmd.match_indices(&form).any(|(i, m)| {
                    cmd[i + m.len()..]
                        .chars()
                        .next()
                        .is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_'))
                });
                assert!(!hit, "{id}: reads {k} as {form}, not RECIPE_VAR_{k}");
            }
        }
    }
}

#[test]
fn prohibited_strings_are_absent_from_bash_steps_and_both_tools() {
    for (id, cmd) in bash_commands(&r()) {
        for p in PROHIBITED {
            assert!(!cmd.contains(p), "{id} contains {p}");
        }
    }
    for tool in [
        "amplifier-bundle/tools/autodrive_light_eligibility.sh",
        "amplifier-bundle/tools/autodrive_followups.sh",
    ] {
        let text = read(tool);
        for p in PROHIBITED {
            assert!(!text.contains(p), "{tool} contains {p}");
        }
        assert!(text.contains("set -"), "{tool} must set shell options");
    }
}

#[test]
fn exit_79_is_never_retried() {
    let text = read("amplifier-bundle/recipes/auto-drive-light.yaml");
    for (id, cmd) in bash_commands(&r()) {
        assert!(
            !cmd.contains("retry") || cmd.contains("79"),
            "{id}: any retry must exclude exit 79"
        );
    }
    assert!(!text.contains("continue_on_error: true"));
}

#[test]
fn tools_are_loaded_from_the_tool_root_not_the_worktree() {
    for (id, cmd) in bash_commands(&r()) {
        for tool in [
            "autodrive_light_eligibility.sh",
            "autodrive_followups.sh",
            "autodrive_state.sh",
            "quick_change_common.sh",
        ] {
            if cmd.contains(tool) {
                assert!(
                    !cmd.contains(&format!(
                        "$(git rev-parse --show-toplevel)/amplifier-bundle/tools/{tool}"
                    )),
                    "{id}: {tool} must come from the tool root"
                );
                assert!(
                    cmd.contains("AMPLIHACK_HOME"),
                    "{id}: {tool} is resolved from AMPLIHACK_HOME"
                );
            }
        }
    }
}

#[test]
fn quick_change_common_is_loaded_from_amplihack_home_only() {
    let mut n = 0;
    for (id, cmd) in bash_commands(&r()) {
        for line in cmd.lines().filter(|l| l.contains("quick_change_common.sh")) {
            if line.trim_start().starts_with('#') {
                continue;
            }
            for banned in ["RECIPE_VAR_repo_path", ".copilot", ".amplihack", "for c in"] {
                assert!(
                    !line.contains(banned),
                    "{id}: quick_change_common.sh has no fallback ({banned}): {line}"
                );
            }
            if line.contains(". \"$QC\"") {
                assert!(
                    line.contains(
                        "QC=\"${AMPLIHACK_HOME:-}/amplifier-bundle/tools/quick_change_common.sh\""
                    ),
                    "{id}: {line}"
                );
                assert!(
                    line.contains("exit 1"),
                    "{id}: a missing file stops the step"
                );
                n += 1;
            }
        }
    }
    assert_eq!(n, 5, "five steps source quick_change_common.sh");
    let pre = yaml_text(&step(&r(), "light-prereqs"));
    assert!(
        pre.contains("quick_change_common.sh"),
        "light-prereqs checks quick_change_common.sh"
    );
}

#[test]
fn crusty_round_has_no_light_hook() {
    // #1982: the in-loop re-measurement is gone; neither step-03 nor step-06
    // loads a light helper, reads a marker or sets a light verdict.
    for id in [
        "step-03-extract-crusty-verdict",
        "step-06-write-round-record",
    ] {
        let cmd = step_command("autodrive-crusty-round", id);
        for gone in [
            "autodrive_light_steps.sh",
            "autodrive_light_round.sh",
            "autodrive_light_step",
            "light-mode",
            "light-minor-rule",
            "light-side-file-invalid",
            "SUFFIX",
        ] {
            assert!(!cmd.contains(gone), "{id} must not contain {gone}");
        }
    }
    assert!(
        read("amplifier-bundle/recipes/autodrive-crusty-round.yaml")
            .lines()
            .count()
            <= 400,
        "autodrive-crusty-round.yaml stays within 400 lines"
    );
}

// --- the shared round ------------------------------------------------------

#[test]
fn crusty_round_has_no_light_branch_in_any_step() {
    let round = recipe("autodrive-crusty-round");
    let ids: Vec<&str> = round["steps"]
        .as_sequence()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        [
            "step-00-review-dir-guard",
            "step-01-round-context",
            "step-02-crusty-review",
            "step-02b-review-dir-unchanged",
            "step-03-extract-crusty-verdict",
            "step-04-address-concerns",
            "step-05-verify-concerns-addressed",
            "step-06-write-round-record",
        ]
    );
    let step = |id: &str| {
        round["steps"]
            .as_sequence()
            .unwrap()
            .iter()
            .find(|s| s["id"].as_str() == Some(id))
            .unwrap()
            .clone()
    };
    for id in [
        "step-04-address-concerns",
        "step-05-verify-concerns-addressed",
    ] {
        assert_eq!(
            step(id)["condition"].as_str(),
            Some("crusty_verdict.crusty_verdict == 'CONCERNS'"),
            "{id} keeps its condition"
        );
        assert!(
            !yaml_text(&step(id)).contains("light"),
            "{id} is untouched by the light branch"
        );
    }
    let s3 = step("step-03-extract-crusty-verdict")["command"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(!s3.contains("light"), "#1982: step-03 has no light branch");
    let s6 = step("step-06-write-round-record")["command"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(!s6.contains("light"), "#1982: step-06 has no light branch");
    assert!(
        !s6.contains("SUFFIX"),
        "#1982: the record has no light suffix"
    );
    assert_eq!(
        round["context"].as_mapping().unwrap().len(),
        10,
        "no new context key on autodrive-crusty-round"
    );
}

#[test]
fn crusty_loop_gets_no_new_context_key() {
    let lp = recipe("autodrive-crusty-loop");
    let keys: Vec<&str> = lp["context"]
        .as_mapping()
        .unwrap()
        .keys()
        .map(|k| k.as_str().unwrap())
        .collect();
    assert!(
        !keys.iter().any(|k| k.contains("light")),
        "autodrive-crusty-loop must not gain a light key: {keys:?}"
    );
}

// --- manifest and docs -----------------------------------------------------

#[test]
fn manifest_entries_match_the_recipe_hashes() {
    let manifest: serde_json::Value =
        serde_json::from_str(&read("amplifier-bundle/recipes/_recipe_manifest.json")).unwrap();
    for name in [NAME, "autodrive-crusty-round"] {
        assert_eq!(
            manifest[name].as_str(),
            Some(manifest_hash(&format!("amplifier-bundle/recipes/{name}.yaml")).as_str()),
            "manifest entry for {name}"
        );
    }
}

#[test]
fn docs_are_linked_and_in_the_nav() {
    assert!(
        repo_root()
            .join("docs/reference/auto-drive-light.md")
            .is_file()
    );
    let mk = read("mkdocs.yml");
    let light = mk
        .find("reference/auto-drive-light.md")
        .expect("mkdocs nav entry for auto-drive-light");
    let quick = mk
        .find("reference/quick-change-recipe.md")
        .expect("mkdocs nav entry for quick-change");
    assert!(
        light > quick,
        "auto-drive-light comes after Quick-Change in the nav"
    );
    assert!(read("docs/index.md").contains("auto-drive-light.md"));
    assert!(read("docs/reference/auto-drive-to-merge.md").contains("auto-drive-light.md"));
    let readme = read("docs/recipes/README.md");
    for name in ["quick-change", "auto-drive-light", "auto-drive-to-merge"] {
        assert!(readme.contains(name), "recipes README compares {name}");
    }
}

// ===========================================================================
// Issue #1982: the effective task id, the prerequisites and the report
// ===========================================================================

const CHILDREN: &[&str] = &["light-escalate", "light-crusty-loop", "light-merge-loop"];

/// The exact line `light-task-id` prints before exit 2.
const TASK_ID_ERROR: &str =
    "ERROR: auto-drive-light: plan_task_id must be #N or 1 to 64 characters from A-Za-z0-9._:/-";

fn skip() -> bool {
    if !jq_available() {
        eprintln!("skip: `jq` not available on PATH");
        return true;
    }
    false
}

#[test]
fn plan_task_id_defaults_empty_and_reaches_every_child() {
    let r = r();
    assert_eq!(r["context"]["plan_task_id"].as_str(), Some(""));
    assert_eq!(r["context"]["light_task_id"].as_str(), Some(""));
    for id in CHILDREN {
        assert_eq!(
            step(&r, id)["context"]["plan_task_id"].as_str(),
            Some("{{light_task_id}}"),
            "{id} receives the effective task id as plan_task_id"
        );
    }
    let ids: Vec<String> = steps(&r)
        .iter()
        .map(|s| s["id"].as_str().unwrap().to_string())
        .collect();
    let at = |id: &str| ids.iter().position(|x| x == id).unwrap();
    assert_eq!(
        at("light-task-id"),
        at("light-pr-input") + 1,
        "light-task-id runs straight after light-pr-input"
    );
    for id in CHILDREN {
        assert!(
            at("light-task-id") < at(id),
            "light-task-id runs before {id}"
        );
    }
}

#[test]
fn light_task_id_is_a_plain_string_output_of_a_bash_step() {
    let s = step(&r(), "light-task-id");
    assert_eq!(s["type"].as_str(), Some("bash"));
    assert_eq!(s["output"].as_str(), Some("light_task_id"));
    assert_ne!(
        s["parse_json"].as_bool(),
        Some(true),
        "light_task_id is a plain string, not JSON"
    );
    let cmd = s["command"].as_str().unwrap();
    for needle in ["RECIPE_VAR_plan_task_id", "RECIPE_VAR_issue_number"] {
        assert!(cmd.contains(needle), "light-task-id reads {needle}");
    }
}

#[test]
fn prereqs_check_the_follow_up_tools_and_hold_no_task_id_rule() {
    let cmd = step(&r(), "light-prereqs")["command"]
        .as_str()
        .unwrap()
        .to_string();
    for f in [
        "autodrive_light_eligibility.sh",
        "autodrive_finding_rules.sh",
        "autodrive_followups.sh",
        "autodrive_followups_check.sh",
        "workflow_issue_tracking.sh",
        "autodrive_state.sh",
    ] {
        assert!(cmd.contains(f), "light-prereqs checks {f}");
    }
    for gone in ["autodrive_light_round.sh", "autodrive_light_steps.sh"] {
        assert!(!cmd.contains(gone), "light-prereqs must not need {gone}");
    }
    // #1982 review: the task id is checked once, by light-task-id through
    // autodrive_followups.sh resolve-task; no copy of the rule is kept.
    for (id, cmd) in bash_commands(&r()) {
        for copy in ["task_ok", "[A-Za-z0-9._:/-]{1,64}", "0*[1-9][0-9]{0,9}"] {
            assert!(
                !cmd.contains(copy),
                "{id} holds a copy of the task-id rule ({copy})"
            );
        }
    }
    let task = step(&r(), "light-task-id")["command"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        task.contains("resolve-task --task-id"),
        "light-task-id calls autodrive_followups.sh resolve-task"
    );
    assert!(task.contains("AMPLIHACK_HOME"), "from the tool root only");
}

/// A scratch repository and the fakes, with a stub `gadugi-test` on `PATH`
/// so `light-prereqs` can pass its last check.
struct StepEnv {
    repo: LightRepo,
    fakes: Fakes,
    tmpdir: PathBuf,
}

impl StepEnv {
    fn new() -> Self {
        let repo = LightRepo::new();
        let fakes = fake_bin(repo.tmp.path());
        crate::support::exec_stub::write_executable(
            &fakes.bin.join("gadugi-test"),
            "#!/usr/bin/env bash\nexit 0\n",
            0o755,
        );
        let tmpdir = repo.tmp.path().join("private-tmp");
        private_dir(&tmpdir);
        StepEnv {
            repo,
            fakes,
            tmpdir,
        }
    }

    /// Runs the bash body of `id`, in the worktree, with the documented
    /// defaults as RECIPE_VAR_* values and `vars` on top.
    fn run(&self, id: &str, vars: &[(&str, &str)]) -> Output {
        let script = format!(
            "cd {:?} || exit 99\n{}",
            self.repo.wt.display().to_string(),
            step_command(NAME, id)
        );
        let root = repo_root();
        let mut owned: Vec<(String, std::ffi::OsString)> = vec![
            ("AMPLIHACK_HOME".into(), root.into_os_string()),
            ("TMPDIR".into(), self.tmpdir.clone().into_os_string()),
            (
                "RECIPE_VAR_repo_path".into(),
                self.repo.wt.clone().into_os_string(),
            ),
            ("RECIPE_VAR_task_description".into(), "Fix a typo".into()),
            ("RECIPE_VAR_light_max_lines".into(), "300".into()),
            ("RECIPE_VAR_light_max_new_modules".into(), "0".into()),
            ("RECIPE_VAR_light_max_public_api".into(), "0".into()),
            ("RECIPE_VAR_light_max_schema".into(), "0".into()),
            ("RECIPE_VAR_light_max_recipe_contract".into(), "0".into()),
        ];
        owned.extend(vars.iter().map(|(k, v)| (k.to_string(), (*v).into())));
        let env: Vec<(&str, &OsStr)> = owned
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_os_str()))
            .collect();
        run_bash(&self.repo.home, &self.fakes, Some(&script), &[], &env, "")
    }
}

/// `autodrive_followups.sh resolve-task --task-id <value>`, run from the
/// live tool: the one task-id rule.
fn resolve_task(value: &str) -> Output {
    crate::isolated_amplihack::isolated("bash")
        .arg(repo_root().join("amplifier-bundle/tools/autodrive_followups.sh"))
        .arg("resolve-task")
        .arg("--task-id")
        .arg(value)
        .output()
        .expect("run resolve-task")
}

fn valid_task_ids() -> Vec<String> {
    vec![
        "1846".into(),
        "#1846".into(),
        "007".into(),
        "#007".into(),
        "1234567890".into(),
        "plan/17.104".into(),
        "PLAN-1".into(),
        "a:b.c_d-e".into(),
        "https://example.org/t/1".into(),
        "x".repeat(64),
    ]
}

/// What `resolve-task` prints for each of [`valid_task_ids`], in order.
fn normalised_task_ids() -> Vec<String> {
    vec![
        "#1846".into(),
        "#1846".into(),
        "#7".into(),
        "#7".into(),
        "#1234567890".into(),
        "plan/17.104".into(),
        "PLAN-1".into(),
        "a:b.c_d-e".into(),
        "https://example.org/t/1".into(),
        "x".repeat(64),
    ]
}

fn invalid_task_ids(canary: &Path) -> Vec<String> {
    vec![
        "#0".into(),
        "0".into(),
        "000".into(),
        "#".into(),
        "##1".into(),
        "task 17".into(),
        "12345678901".into(),
        "#12345678901".into(),
        "x".repeat(65),
        "{{light_task_id}}".into(),
        format!("$(touch {})", canary.display()),
        "a\"b".into(),
        "a'b".into(),
        "a\nb".into(),
        "<x>".into(),
        "a>b".into(),
        "a$b".into(),
    ]
}

#[test]
fn resolve_task_accepts_and_normalises_the_task_id_table() {
    let canary = std::env::temp_dir().join("autodrive-resolve-task-never-created");
    for (v, want) in valid_task_ids().into_iter().zip(normalised_task_ids()) {
        let o = resolve_task(&v);
        assert_eq!(o.status.code(), Some(0), "{v:?}: {}", show(&o));
        assert_eq!(stdout(&o), want, "{v:?}: {}", show(&o));
        assert!(stderr(&o).is_empty(), "{v:?}: {}", show(&o));
    }
    for v in invalid_task_ids(&canary).into_iter().chain([String::new()]) {
        let o = resolve_task(&v);
        assert_eq!(o.status.code(), Some(2), "{v:?}: {}", show(&o));
        assert!(stdout(&o).is_empty(), "{v:?}: {}", show(&o));
        if v.len() >= 5 {
            assert!(
                !stderr(&o).contains(v.as_str()),
                "{v:?}: the value itself is never logged: {}",
                show(&o)
            );
        }
    }
    assert!(!canary.exists(), "a task id is never evaluated");
    // Extra options are refused: resolve-task reads no state.
    let o = crate::isolated_amplihack::isolated("bash")
        .arg(repo_root().join("amplifier-bundle/tools/autodrive_followups.sh"))
        .args(["resolve-task", "--task-id", "#1", "--state-dir", "/tmp"])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(2), "{}", show(&o));
}

#[test]
fn light_task_id_accepts_exactly_the_task_ids_resolve_task_accepts() {
    if skip() {
        return;
    }
    let e = StepEnv::new();
    let canary = e.repo.tmp.path().join("canary");
    for (v, want) in valid_task_ids().into_iter().zip(normalised_task_ids()) {
        let o = e.run(
            "light-task-id",
            &[
                ("RECIPE_VAR_plan_task_id", &v),
                ("RECIPE_VAR_issue_number", "1846"),
            ],
        );
        assert_eq!(o.status.code(), Some(0), "{v:?}: {}", show(&o));
        assert_eq!(stdout(&o), want, "{v:?}: {}", show(&o));
    }
    for v in invalid_task_ids(&canary) {
        let o = e.run(
            "light-task-id",
            &[
                ("RECIPE_VAR_plan_task_id", &v),
                ("RECIPE_VAR_issue_number", "1846"),
            ],
        );
        assert_eq!(o.status.code(), Some(2), "{v:?}: {}", show(&o));
        assert!(
            stdout(&o).is_empty(),
            "{v:?}: no task id is printed: {}",
            show(&o)
        );
        assert!(
            stderr(&o).lines().any(|l| l == TASK_ID_ERROR),
            "{v:?}: the documented ERROR line: {}",
            show(&o)
        );
        if v.len() >= 5 {
            assert!(
                !stderr(&o).contains(v.as_str()),
                "{v:?}: the value itself is never logged: {}",
                show(&o)
            );
        }
    }
    assert!(!canary.exists(), "a task id is never evaluated");
    assert!(
        e.fakes.gh_calls().is_empty(),
        "light-task-id makes no gh call: {:?}",
        e.fakes.gh_calls()
    );
}

#[test]
fn light_prereqs_does_not_check_the_task_id() {
    if skip() {
        return;
    }
    // light-task-id is the one check; light-prereqs passes any value through.
    let e = StepEnv::new();
    let o = e.run("light-prereqs", &[("RECIPE_VAR_plan_task_id", "task 17")]);
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    assert!(!stderr(&o).contains(TASK_ID_ERROR), "{}", show(&o));
}

#[test]
fn light_task_id_derives_the_effective_task_id() {
    if skip() {
        return;
    }
    let e = StepEnv::new();
    for (plan, issue, want) in [
        ("plan/17.104", "1846", "plan/17.104"),
        ("1846", "", "#1846"),
        ("#5", "1846", "#5"),
        ("", "1846", "#1846"),
        ("", "007", "#7"),
        ("", "0", ""),
        ("", "00", ""),
        ("", "", ""),
        ("", "12345678901", ""),
    ] {
        let o = e.run(
            "light-task-id",
            &[
                ("RECIPE_VAR_plan_task_id", plan),
                ("RECIPE_VAR_issue_number", issue),
            ],
        );
        assert_eq!(o.status.code(), Some(0), "{plan:?}/{issue:?}: {}", show(&o));
        let out = stdout(&o);
        assert!(
            out.lines().count() <= 1,
            "{plan:?}/{issue:?}: one plain line at most: {}",
            show(&o)
        );
        assert_eq!(
            out.trim_end_matches('\n'),
            want,
            "{plan:?}/{issue:?}: {}",
            show(&o)
        );
    }
}

/// Runs `light-report` on a light run whose state directory is `dir`.
fn report(e: &StepEnv, dir: &Path) -> (Output, serde_json::Value) {
    let br = serde_json::json!({"pr": "1850", "state_dir": dir}).to_string();
    let o = e.run(
        "light-report",
        &[
            ("RECIPE_VAR_build_result", &br),
            (
                "RECIPE_VAR_light_eligibility",
                r#"{"decision":"light","reason":"within-limits"}"#,
            ),
            (
                "RECIPE_VAR_merge_gate_result",
                r#"{"merge_result":"MERGED"}"#,
            ),
        ],
    );
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    let line = stdout(&o)
        .lines()
        .rev()
        .find(|l| l.trim_start().starts_with('{'))
        .unwrap_or_else(|| panic!("no JSON line: {}", show(&o)))
        .to_string();
    let j: serde_json::Value = serde_json::from_str(&line).unwrap();
    (o, j)
}

#[test]
fn light_report_lists_each_filed_issue_once_from_followups_tsv() {
    if skip() {
        return;
    }
    let e = StepEnv::new();
    let dir = e.repo.tmp.path().join("state");
    private_dir(&dir);
    let canary = e.repo.tmp.path().join("canary");
    let rows = format!(
        "naming-1\t1851\t#1982\t1850\n\
         doc-2\t1852\t#1982\t1850\n\
         dup-3\t1851\t#1982\t1850\n\
         word-4\tabc\t#1982\t1850\n\
         inj-5\t1;touch {}\t#1982\t1850\n\
         hash-6\t#5\t#1982\t1850\n\
         long-7\t12345678901\t#1982\t1850\n\
         doc-8\t1853\t#1982\t1850\n",
        canary.display()
    );
    private_file(&dir.join("followups.tsv"), &rows);
    // A legacy single-issue file is not read.
    private_file(&dir.join("follow-up-issue"), "1999\n");
    let (o, j) = report(&e, &dir);
    assert_eq!(j["follow_up_issues"], "#1851,#1852,#1853", "{}", show(&o));
    let keys: Vec<&str> = j.as_object().unwrap().keys().map(String::as_str).collect();
    let mut keys = keys;
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "follow_up_issues",
            "merge_result",
            "outcome",
            "path",
            "pr",
            "reason"
        ]
    );
    assert!(
        stderr(&o).contains("follow-up issues: #1851,#1852,#1853"),
        "{}",
        show(&o)
    );
    assert!(!stderr(&o).contains("1999"), "{}", show(&o));
    assert!(!canary.exists(), "a cell is never run");
}

#[test]
fn light_report_says_none_without_a_safe_followups_tsv() {
    if skip() {
        return;
    }
    type Setup = fn(&Path, &Path);
    let cases: Vec<(&str, Setup)> = vec![
        ("absent", |_, _| {}),
        ("legacy follow-up-issue only", |d, _| {
            private_file(&d.join("follow-up-issue"), "1851\n")
        }),
        ("symlink", |d, tmp| {
            let real = tmp.join("elsewhere.tsv");
            private_file(&real, "naming-1\t1851\t#1982\t1850\n");
            std::os::unix::fs::symlink(&real, d.join("followups.tsv")).unwrap();
        }),
        ("directory", |d, _| {
            std::fs::create_dir(d.join("followups.tsv")).unwrap()
        }),
        ("no valid number", |d, _| {
            private_file(&d.join("followups.tsv"), "naming-1\tabc\t#1982\t1850\n")
        }),
    ];
    for (name, setup) in cases {
        let e = StepEnv::new();
        let dir = e.repo.tmp.path().join("state");
        private_dir(&dir);
        setup(&dir, e.repo.tmp.path());
        let (o, j) = report(&e, &dir);
        assert_eq!(j["follow_up_issues"], "none", "{name}: {}", show(&o));
        assert!(
            stderr(&o).contains("follow-up issues: none"),
            "{name}: {}",
            show(&o)
        );
    }
}

// --- retired names ---------------------------------------------------------

fn files_under(rel: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![repo_root().join(rel)];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push(p);
            }
        }
    }
    out
}

#[test]
fn retired_light_follow_up_names_are_gone_from_tools_and_recipes() {
    for dir in ["amplifier-bundle/tools", "amplifier-bundle/recipes"] {
        for p in files_under(dir) {
            let Ok(text) = std::fs::read_to_string(&p) else {
                continue;
            };
            for gone in [
                "light-minor-rule",
                "follow-up-posted-ids",
                "auto-drive-light:pr-",
                "follow-up-issue",
                "light-mode",
                "light-escalated",
                "autodrive_light_round",
                "autodrive_light_steps",
            ] {
                assert!(
                    !text.contains(gone),
                    "{} still contains {gone} (#1982)",
                    p.display()
                );
            }
        }
    }
}

#[test]
fn the_only_light_tool_makes_no_issue_or_comment_call() {
    // #1982: autodrive_light_round.sh and autodrive_light_steps.sh are gone;
    // the eligibility tool is the only light tool left, and it is offline.
    for gone in ["autodrive_light_round.sh", "autodrive_light_steps.sh"] {
        assert!(
            !repo_root()
                .join("amplifier-bundle/tools")
                .join(gone)
                .exists(),
            "{gone} must not come back"
        );
    }
    let tool = "amplifier-bundle/tools/autodrive_light_eligibility.sh";
    let text = read(tool);
    for line in text.lines().filter(|l| !l.trim_start().starts_with('#')) {
        for gone in [
            "gh ",
            "issue create",
            "issue comment",
            "pr comment",
            "git fetch",
        ] {
            assert!(!line.contains(gone), "{tool}: {gone}: {line}");
        }
    }
}
