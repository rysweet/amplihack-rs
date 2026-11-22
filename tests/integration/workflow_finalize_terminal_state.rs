//! tests/integration/workflow_finalize_terminal_state.rs
//!
//! TDD-red contracts for finalization as the loud terminal-state arbiter.
//! Terminal success may complete the workflow, but dirty work, CI failures,
//! closed-unmerged PRs, and meaningful unmerged diffs must remain blockers.

use std::fs;
use std::path::PathBuf;

use serde_yaml::Value;

fn workspace_root() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.pop();
    path
}

fn load_finalize_recipe() -> Value {
    let path = workspace_root()
        .join("amplifier-bundle")
        .join("recipes")
        .join("workflow-finalize.yaml");
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_yaml::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

fn steps(recipe: &Value) -> &[Value] {
    recipe
        .get("steps")
        .and_then(Value::as_sequence)
        .expect("workflow-finalize must contain top-level steps")
}

fn step_index(recipe: &Value, id: &str) -> usize {
    steps(recipe)
        .iter()
        .position(|step| step.get("id").and_then(Value::as_str) == Some(id))
        .unwrap_or_else(|| panic!("workflow-finalize missing step `{id}`"))
}

fn find_terminal_probe_index(recipe: &Value) -> usize {
    steps(recipe)
        .iter()
        .position(|step| step.get("recipe").and_then(Value::as_str) == Some("workflow-terminal-state"))
        .expect("workflow-finalize must re-run workflow-terminal-state before mutation, CI, or merge steps")
}

fn step_condition<'a>(recipe: &'a Value, id: &str) -> &'a str {
    steps(recipe)
        .iter()
        .find(|step| step.get("id").and_then(Value::as_str) == Some(id))
        .and_then(|step| step.get("condition").and_then(Value::as_str))
        .unwrap_or_else(|| panic!("workflow-finalize step `{id}` must declare a condition"))
}

/// The shell a step runs, following an `amplifier-bundle/tools/` call-out
/// left by the brick-budget extraction of issue #1619.
fn step_command(recipe: &Value, id: &str) -> String {
    crate::recipe_step_body::resolved(
        steps(recipe)
            .iter()
            .find(|step| step.get("id").and_then(Value::as_str) == Some(id))
            .and_then(|step| step.get("command").and_then(Value::as_str))
            .unwrap_or_else(|| panic!("workflow-finalize step `{id}` must be a bash command step")),
    )
}

fn step_by_id<'a>(recipe: &'a Value, id: &str) -> &'a Value {
    steps(recipe)
        .iter()
        .find(|step| step.get("id").and_then(Value::as_str) == Some(id))
        .unwrap_or_else(|| panic!("workflow-finalize missing step `{id}`"))
}

fn step_type<'a>(recipe: &'a Value, id: &str) -> &'a str {
    step_by_id(recipe, id)
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("")
}

fn step_output<'a>(recipe: &'a Value, id: &str) -> &'a str {
    step_by_id(recipe, id)
        .get("output")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("workflow-finalize step `{id}` must declare output"))
}

fn step_prompt(recipe: &Value, id: &str) -> String {
    step_by_id(recipe, id)
        .get("prompt")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("workflow-finalize step `{id}` must declare a prompt"))
        .to_owned()
}

fn recipe_text(recipe: &Value) -> String {
    serde_yaml::to_string(recipe).expect("serialize workflow-finalize")
}

#[test]
fn finalize_rechecks_terminal_state_before_mutation_ci_or_merge_steps() {
    let recipe = load_finalize_recipe();
    let terminal_probe_index = find_terminal_probe_index(&recipe);

    for guarded_step in [
        "step-20-sync-pr-head",
        "step-20b-push-cleanup",
        "step-21-pr-ready",
        "step-22-ensure-mergeable",
    ] {
        let guarded_index = step_index(&recipe, guarded_step);
        assert!(
            terminal_probe_index < guarded_index,
            "workflow-finalize must re-check terminal state before `{guarded_step}`"
        );
    }
}

#[test]
fn finalize_mutation_and_ci_steps_are_suppressed_for_terminal_success() {
    let recipe = load_finalize_recipe();

    for guarded_step in [
        "step-20-sync-pr-head",
        "step-20b-push-cleanup",
        "step-21-pr-ready",
        "step-22-ensure-mergeable",
    ] {
        let condition = step_condition(&recipe, guarded_step);
        assert!(
            condition.contains("terminal_success != 'true'")
                || condition.contains("terminal_state.terminal_success != 'true'")
                || condition.contains("should_finalize == 'true'")
                || condition.contains("should_run_ci_wait == 'true'"),
            "`{guarded_step}` must be skipped when terminal_success=true; condition was `{condition}`"
        );
    }
}

#[test]
fn finalize_preserves_loud_blockers_instead_of_converting_them_to_success() {
    let recipe = load_finalize_recipe();
    let text = recipe_text(&recipe);

    // #2966: a red check is repaired, not a blocker the probe ends the run on,
    // so `BLOCKED_CI` and the probe's `exit 1` are no longer pinned here. What
    // a red check does is shown by behaviour: the probe tests in
    // `default_workflow_terminal_state` and the assessed-route cases in
    // `issue_1714_finalization_handoff::assessed_ci`.
    for blocker in ["FAILED_CLOSED_UNMERGED", "FAILED_MEANINGFUL_DIFF"] {
        assert!(
            text.contains(blocker),
            "workflow-finalize must preserve loud blocker `{blocker}`"
        );
    }
}

#[test]
fn final_status_reports_terminal_success_and_blocked_semantics_structurally() {
    let recipe = load_finalize_recipe();
    let command = step_command(&recipe, "workflow-complete");

    for required in [
        "terminal_state",
        "terminal_success",
        "MERGED",
        "CLOSED_OBSOLETE",
        "NO_DIFF_SUCCESS",
        "FOLLOWUP_CREATED",
        "BLOCKED_CI",
    ] {
        assert!(
            command.contains(required),
            "workflow-complete JSON must expose `{required}`"
        );
    }

    assert!(
        !command.contains("status: \"complete\"") || command.contains("terminal_success"),
        "workflow-complete must not unconditionally report complete without terminal_success evidence"
    );
}

#[test]
fn finalize_pipeline_boxes_judgment_in_agentic_finalizer_and_keeps_validation_deterministic() {
    let recipe = load_finalize_recipe();

    assert_eq!(
        step_type(&recipe, "collect-finalization-evidence"),
        "bash",
        "workflow-finalize must collect structured evidence deterministically before agent judgment"
    );
    assert_eq!(
        step_output(&recipe, "collect-finalization-evidence"),
        "finalization_evidence",
        "evidence collection must persist a structured finalization_evidence object"
    );
    assert!(
        step_by_id(&recipe, "agentic-finalizer")
            .get("agent")
            .and_then(Value::as_str)
            .is_some(),
        "workflow-finalize must use an agentic finalizer for the human-readable narrative"
    );
    assert_eq!(
        step_output(&recipe, "agentic-finalizer"),
        "agentic_finalizer_narrative",
        "the agentic finalizer output is a human-readable narrative artifact, never machine-control state"
    );
    assert_eq!(
        step_type(&recipe, "finalizer-step-status"),
        "bash",
        "a deterministic step must record the reporting step's typed status"
    );
    assert_eq!(
        step_output(&recipe, "finalizer-step-status"),
        "finalizer_step_status",
        "finalizer-step-status must persist typed {{status, reporting_failure}} state"
    );
    assert_eq!(
        step_by_id(&recipe, "finalizer-step-status")
            .get("parse_json")
            .and_then(Value::as_bool),
        Some(true),
        "finalizer-step-status must emit typed JSON parsed into recipe state"
    );
    assert_eq!(
        step_type(&recipe, "validate-agentic-finalization"),
        "bash",
        "terminal-state classification must remain deterministic"
    );
    assert_eq!(
        step_output(&recipe, "validate-agentic-finalization"),
        "workflow_result",
        "deterministic validation must be the only writer of canonical workflow_result"
    );

    assert!(
        step_index(&recipe, "collect-finalization-evidence")
            < step_index(&recipe, "agentic-finalizer")
            && step_index(&recipe, "agentic-finalizer")
                < step_index(&recipe, "finalizer-step-status")
            && step_index(&recipe, "finalizer-step-status")
                < step_index(&recipe, "validate-agentic-finalization")
            && step_index(&recipe, "validate-agentic-finalization")
                < step_index(&recipe, "workflow-complete"),
        "finalize pipeline order must be evidence collection -> agentic narrative -> typed step status -> deterministic validation -> workflow-complete"
    );
}

#[test]
fn agentic_finalizer_prompt_requests_human_narrative_not_machine_json() {
    let recipe = load_finalize_recipe();
    let prompt = step_prompt(&recipe, "agentic-finalizer");

    // Issue #969: the finalizer emits a free-form narrative for humans. The
    // brittle "one JSON object / no prose" machine contract must be gone.
    for forbidden in [
        "single JSON object",
        "no prose",
        "Return only the single JSON object",
        "Output schema",
    ] {
        assert!(
            !prompt.contains(forbidden),
            "agentic finalizer prompt must not demand a machine-parseable JSON blob; found `{forbidden}`"
        );
    }

    for required in ["narrative", "structured evidence"] {
        assert!(
            prompt.contains(required),
            "agentic finalizer prompt must request a human-readable narrative from structured evidence; missing `{required}`"
        );
    }

    // Observed failure-mode context is still valuable operator background.
    for failure_mode in [
        "dirty worktree",
        "closed-unmerged",
        "missing tooling",
        "failed CI",
        "hollow success",
    ] {
        assert!(
            prompt.contains(failure_mode),
            "agentic finalizer prompt should preserve observed failure-mode context `{failure_mode}`"
        );
    }
}

/// The validation step reports the terminal state `assess-finalization`
/// decided (#2343, #2809). It runs after that step, consumes no agent prose,
/// and does not mask a failure as a completion. That the typed completion
/// booleans decide nothing, and that every published state is reported as the
/// assessment named it, is run in workflow_finalize_resilience.rs.
#[test]
fn validation_step_reports_the_assessment_and_reads_no_agent_prose() {
    let recipe = load_finalize_recipe();
    let command = step_command(&recipe, "validate-agentic-finalization");
    let validator = crate::issue_1641_finalization_open_pr::fixture::tool_text(
        "workflow_finalization_validate.sh",
    );

    assert!(
        step_index(&recipe, "assess-finalization")
            < step_index(&recipe, "validate-agentic-finalization"),
        "the decision must be made before the step that reports it"
    );
    assert!(
        command.contains("workflow_agentic_finalization.sh") && command.contains("validate"),
        "the step must run the finalization helper in validate mode"
    );

    // Neither the step nor the validator may touch agent narrative or the
    // retired brittle state.
    for forbidden in [
        "AGENTIC_FINALIZER_OUTPUT",
        "agentic_finalizer_output",
        "FAILED_FINALIZER_OUTPUT",
        "single JSON object",
    ] {
        assert!(
            !command.contains(forbidden) && !validator.contains(forbidden),
            "validation must not consume agent prose via `{forbidden}`"
        );
    }

    // The state comes from the assessment's structured terminal_state. The
    // validator itself ends a run only on a blocker in the local checkout or
    // on there being no decision to report.
    for required in [
        "finalization_assessment terminal_state",
        "FAILED_MISSING_TERMINAL_EVIDENCE",
        "FAILED_DIRTY_WORKTREE",
        "FAILED_MISSING_TOOLING",
    ] {
        assert!(
            validator.contains(required),
            "the validator must report the assessment's state; missing `{required}`"
        );
    }

    assert!(
        !command.contains("|| true") && !command.contains("status: \"complete\""),
        "validation step must not mask a non-success terminal state as successful completion"
    );
}

#[test]
fn workflow_complete_reports_canonical_agentic_workflow_result_fields_and_failure_vocabulary() {
    let recipe = load_finalize_recipe();
    let command = step_command(&recipe, "workflow-complete");

    for required in [
        "workflow_result",
        "required_next_action",
        "hollow_success_detected",
        "evidence_used",
        "reporting_failure",
        "finalizer_schema_version",
        "finalizer_confidence",
        "finalizer_output_valid",
        "terminal_failure",
        "FAILED_INVALID_EVIDENCE",
        "FAILED_REPORTING",
        "FAILED_IMPLEMENTATION",
        "FAILED_MISSING_TOOLING",
        "FAILED_PR_METADATA_UNAVAILABLE",
        "FAILED_DIRTY_WORKTREE",
        "FAILED_MEANINGFUL_DIFF",
        "HOLLOW_SUCCESS",
        "INCOMPLETE",
        "SUPERSEDED",
    ] {
        assert!(
            command.contains(required),
            "workflow-complete must report canonical agentic workflow_result field/state `{required}`"
        );
    }

    assert!(
        !command.contains("FAILED_FINALIZER_OUTPUT"),
        "workflow-complete must retire the brittle FAILED_FINALIZER_OUTPUT state"
    );
    assert!(
        !command.contains("status: \"complete\"") && !command.contains("steps_completed: 23"),
        "workflow-complete must not report legacy unconditional completion once finalizer validation owns terminal status"
    );
}

// #2966: a red CI sends the run back to repair it, and the repair is the CI
// repair step's. Only the shape that makes that repair possible is checked here,
// as fields of the recipe and not as prose: what the agent is told is the
// agent's guidance, and what it does is shown by the QA scenario
// (`tests/gadugi/run-issue-2966-finalize-ci-repair-scenario.sh`).
//
// - It runs in the run's own worktree, where the fix is committed and pushed.
//   Started anywhere else, a repair would be made in the wrong tree, or in the
//   caller's checkout.
// - It still runs on a failing check: the probe lets the run continue, and its
//   gate is the same two flags every finalization step shares.
// - It is not wrapped. A failure of the repair agent itself ends the run with
//   its own failure, which is an agent failure and not a verdict on CI, and no
//   JSON contract or error swallowing sits between its work and its result.
#[test]
fn the_ci_repair_step_works_in_the_runs_worktree_and_is_not_wrapped() {
    let recipe = load_finalize_recipe();
    let step = step_by_id(&recipe, "step-22-ensure-mergeable");

    assert_eq!(
        step.get("agent").and_then(Value::as_str),
        Some("amplihack:ci-diagnostic-workflow"),
        "the CI repair step is the ci-diagnostic-workflow agent"
    );
    assert_eq!(
        step.get("working_dir").and_then(Value::as_str),
        Some("{{worktree_setup.worktree_path}}"),
        "the repair must be started in the run's linked worktree"
    );
    for key in ["parse_json", "continue_on_error"] {
        assert!(
            step.get(key).is_none(),
            "the CI repair step must not declare `{key}`: its failure is its own"
        );
    }
    let condition = step_condition(&recipe, "step-22-ensure-mergeable");
    assert!(
        condition.contains("terminal_state.terminal_success != 'true'")
            && condition.contains("terminal_state.should_run_ci_wait == 'true'"),
        "a failing check reaches the repair through the probe's continue gates; condition was `{condition}`"
    );
}

// #2966: the assessment that decides the terminal state is still the one
// agent step of its recipe. The CI repair is done inside it, not by a step
// added beside it, so the recipe's step budget (`max_total_steps`) is unchanged.
#[test]
fn the_assessment_recipe_gains_no_step_for_the_repair() {
    let path = workspace_root()
        .join("amplifier-bundle")
        .join("recipes")
        .join("workflow-finalization-assessment.yaml");
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let recipe: Value =
        serde_yaml::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()));

    let ids: Vec<&str> = recipe
        .get("steps")
        .and_then(Value::as_sequence)
        .expect("the assessment recipe has steps")
        .iter()
        .filter_map(|step| step.get("id").and_then(Value::as_str))
        .collect();
    assert_eq!(ids, ["assess-finalization"], "one step, as before");
    assert_eq!(
        recipe["recursion"]["max_total_steps"].as_u64(),
        Some(4),
        "the step budget is unchanged"
    );
}

// #2966 (SEC-02). The CI repair never edits `.github/**`, whether or not the
// pull request changes it: an unreviewed workflow change would run on the
// branch with the job's secrets, and a token may hold the `workflow` scope, so
// GitHub refusing the push is not the control. The other protected paths keep
// the "already in the pull request's diff" exception. The rule is stated in the
// assessment prompt and in the agent file, which is shipped twice; this keeps
// them naming the same paths, keeps `.github/**` out of the exception, and keeps
// any wording that makes the rule depend on the token's scope out of the
// prompts.
const PROTECTED_PATHS: [&str; 12] = [
    ".github/**",
    ".pre-commit-config.yaml",
    ".githooks/**",
    ".husky/**",
    "lefthook.yml",
    "core.hooksPath",
    ".cargo/config*",
    "rust-toolchain*",
    "deny.toml",
    "amplifier-bundle/recipes/**",
    "amplifier-bundle/agents/**",
    "amplifier-bundle/tools/workflow_finalization_*.sh",
];

const GITHUB_RULE: &str =
    "`.github/**` is never edited, whether or not the pull request changes it";

fn flat(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The text between `start` and the first `end` after it, both required.
fn between<'a>(text: &'a str, start: &str, end: &str, what: &str) -> &'a str {
    let from = text
        .find(start)
        .unwrap_or_else(|| panic!("{what}: `{start}` not found"));
    let rest = &text[from..];
    let to = rest
        .find(end)
        .unwrap_or_else(|| panic!("{what}: `{end}` not found after `{start}`"));
    &rest[..to]
}

/// Backticked tokens that name a path: they hold a `/` or a `*`, or end in a
/// configuration file suffix. `secrets.*` is an expression, not a path.
fn named_paths(text: &str) -> std::collections::BTreeSet<String> {
    text.split('`')
        .skip(1)
        .step_by(2)
        .filter(|t| {
            *t != "secrets.*"
                && (t.contains('/')
                    || t.contains('*')
                    || t.ends_with(".yml")
                    || t.ends_with(".yaml")
                    || t.ends_with(".toml"))
        })
        .map(str::to_owned)
        .collect()
}

#[test]
fn the_ci_repair_never_edits_github_and_names_the_same_protected_paths_everywhere() {
    let root = workspace_root();
    let assessment_path =
        root.join("amplifier-bundle/recipes/workflow-finalization-assessment.yaml");
    let assessment: Value = serde_yaml::from_str(
        &fs::read_to_string(&assessment_path).expect("read the assessment recipe"),
    )
    .expect("parse the assessment recipe");
    let prompt = assessment["steps"][0]["prompt"]
        .as_str()
        .expect("assess-finalization has a prompt")
        .to_owned();
    let agent_paths = [
        "amplifier-bundle/agents/specialized/ci-diagnostic-workflow.md",
        "docs/claude/agents/amplihack/specialized/ci-diagnostic-workflow.md",
    ];
    let agents: Vec<String> = agent_paths
        .iter()
        .map(|p| fs::read_to_string(root.join(p)).unwrap_or_else(|e| panic!("read {p}: {e}")))
        .collect();

    let prompt_rules = between(
        &prompt,
        "- editing `.github/**`",
        "- changing a dependency",
        "assessment prompt",
    );
    let agent_sections: Vec<&str> = agents
        .iter()
        .zip(agent_paths)
        .map(|(text, p)| between(text, "## Protected Paths", "\n## Dependencies", p))
        .collect();
    assert_eq!(
        agent_sections[0], agent_sections[1],
        "both copies of the agent carry the same Protected Paths section"
    );

    for (what, rules, exception) in [
        (
            "assessment prompt",
            prompt_rules,
            between(
                prompt_rules,
                "- editing hook configuration",
                "outside the diff is",
                "assessment prompt exception",
            ),
        ),
        (
            "agent Protected Paths",
            agent_sections[0],
            between(
                agent_sections[0],
                "These paths are not edited unless",
                "\n\nA failure caused",
                "agent exception",
            ),
        ),
    ] {
        assert!(
            flat(rules).contains(GITHUB_RULE),
            "{what} must say {GITHUB_RULE}"
        );
        assert!(
            !exception.contains(".github"),
            "{what}: the diff exception must not reach `.github/**`:\n{exception}"
        );
        for path in PROTECTED_PATHS {
            assert!(
                rules.contains(&format!("`{path}`")),
                "{what} must name the protected path `{path}`"
            );
        }
    }
    assert_eq!(
        named_paths(prompt_rules),
        named_paths(agent_sections[0]),
        "the assessment prompt and the agent's Protected Paths name the same paths"
    );

    let step22 = step_prompt(&load_finalize_recipe(), "step-22-ensure-mergeable");
    assert!(
        step22.contains("never edit `.github/**`"),
        "step-22 must say never edit `.github/**`:\n{step22}"
    );
    for (what, text) in [
        ("assessment prompt", prompt.as_str()),
        ("agent", agents[0].as_str()),
        ("agent copy", agents[1].as_str()),
        ("step-22", step22.as_str()),
    ] {
        let lower = flat(text).to_lowercase();
        for stale in [
            "`workflow` scope",
            "report the rejection",
            "rejects the push",
        ] {
            assert!(
                !lower.contains(stale),
                "{what} must not make the rule depend on the token: found `{stale}`"
            );
        }
    }
}

// #2966, from the step-17b review. The prompt tells the agent to return a
// terminal state when it must stop before the head is green and before it can
// name a cause the run cannot fix. It has to say which state: with none named,
// the nearest reading is `EXISTING_OPEN_PR`, which finalization publishes as a
// success over a red head. It is `FAILED_IMPLEMENTATION` -- the verification
// the run needs is not complete -- and never a success.
#[test]
fn the_assessment_names_the_state_for_a_stop_before_green() {
    let path =
        workspace_root().join("amplifier-bundle/recipes/workflow-finalization-assessment.yaml");
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let paragraph = flat(between(
        &text,
        "If you must stop before the head is green",
        "An empty or unparseable answer",
        "assessment prompt",
    ));

    assert!(
        paragraph.contains("`FAILED_IMPLEMENTATION`"),
        "the stop-before-green paragraph must name the state to return: {paragraph}"
    );
    for success in [
        "EXISTING_OPEN_PR",
        "IMPLEMENTED_VERIFIED",
        "ALLOW_NO_OP",
        "NO_DIFF_SUCCESS",
    ] {
        assert!(
            !paragraph.contains(success),
            "a stop before green is never {success}: {paragraph}"
        );
    }
}
