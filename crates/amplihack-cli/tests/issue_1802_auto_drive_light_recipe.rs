//! Issue #1802: the structure of `amplifier-bundle/recipes/auto-drive-light.yaml`
//! and the invariants it shares with `auto-drive-to-merge`.
//!
//! These tests parse the live YAML, the two new tools and the manifest; no
//! recipe runs. They cover acceptance test (f) (no design, consensus or
//! architect steps and no `default-workflow`), the escalation wiring, the
//! "merge phase reached" condition of test (a), and the prohibited strings.

use serde_yaml::Value;
use sha2::{Digest, Sha256};

use crate::support::auto_drive_light::{read, recipe, repo_root};

const NAME: &str = "auto-drive-light";

const STEPS: &[&str] = &[
    "light-prereqs",
    "light-pr-input",
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
    "light-mode-marker",
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
    "light-mode-marker",
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
        "light-mode",
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
fn marker_step_writes_light_mode_atomically() {
    let cmd = step(&r(), "light-mode-marker")["command"]
        .as_str()
        .unwrap()
        .to_string();
    for needle in ["light-mode", "umask 077", "mktemp", "mv -f"] {
        assert!(cmd.contains(needle), "light-mode-marker must use {needle}");
    }
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
    for path in ["light-escalated", "escalate", "light", "follow-up-issue"] {
        assert!(cmd.contains(path), "light-report must handle {path}");
    }
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
        "amplifier-bundle/tools/autodrive_light_round.sh",
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
    let round = read("amplifier-bundle/tools/autodrive_light_round.sh");
    assert!(
        round.contains("79"),
        "the round tool passes exit 79 through"
    );
    assert!(!text.contains("continue_on_error: true"));
}

#[test]
fn tools_are_loaded_from_the_tool_root_not_the_worktree() {
    for (id, cmd) in bash_commands(&r()) {
        for tool in [
            "autodrive_light_eligibility.sh",
            "autodrive_light_round.sh",
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

// --- the shared round ------------------------------------------------------

#[test]
fn crusty_round_changes_only_step_03_and_step_06() {
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
    assert!(s3.contains("light-mode"));
    assert!(s3.contains("autodrive_light_round.sh"));
    assert!(s3.contains("light-minor-rule"));
    let s6 = step("step-06-write-round-record")["command"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(s6.contains("light-minor-rule"));
    assert!(s6.contains("light-side-file-invalid"));
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
