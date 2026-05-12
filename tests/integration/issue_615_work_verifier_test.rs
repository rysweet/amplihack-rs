//! Issue #615: agentic work-verifier contract tests.
//!
//! These tests lock the behavioral contract for the step-08c pair that
//! replaced the legacy six-escape-hatch bash hollow-success guard. They
//! complement `default_workflow_decomposition_test.rs`, which only checks
//! that the new step IDs *exist* in the inventory.
//!
//! Two layers of assertion:
//!
//! 1. **Structural** — assert the YAML shape of both new steps (types,
//!    wiring, prompt content, opt-out preservation, condition clauses,
//!    output keys, removal of the legacy step ID).
//! 2. **Runtime** — execute the actual bash command of
//!    `step-08c-enforce-verdict` against synthetic `VERDICT_JSON`,
//!    `IMPLEMENTATION`, and `ALLOW_NO_OP` inputs and assert the exit
//!    code mapping required by issue #615:
//!      WORK_VERIFIED          -> 0 silent
//!      INSUFFICIENT_EVIDENCE  -> 0 with WARN on stderr
//!      HOLLOW_SUCCESS         -> 1 with rationale on stderr
//!      empty / unparseable    -> 0 with WARN (fail-safe per issue #615)
//!      ALLOW_NO_OP=true       -> 0 (issue #425 fast-path)
//!      orchestration sentinel -> 0 (issue #425 fast-path)
//!
//! Why both layers: prompt-content checks would silently pass even if the
//! bash gate's case-arm logic regressed. Runtime checks would silently pass
//! even if the agent step were rewired to type=bash with a stale prompt.
//! Together they pin the contract issue #615 specified.

use std::collections::HashSet;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Recipe {
    #[serde(default)]
    steps: Vec<Step>,
}

#[derive(Debug, Deserialize)]
struct Step {
    id: String,
    #[serde(rename = "type", default)]
    step_type: Option<String>,
    #[serde(default)]
    agent: Option<String>,
    #[serde(default)]
    working_dir: Option<String>,
    #[serde(default)]
    condition: Option<String>,
    #[serde(default)]
    prompt: Option<String>,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    output: Option<String>,
}

const RECIPE_NAME: &str = "workflow-tdd";
const VERIFIER_STEP_ID: &str = "step-08c-work-verifier";
const ENFORCE_STEP_ID: &str = "step-08c-enforce-verdict";
const LEGACY_STEP_ID: &str = "step-08c-implementation-no-op-guard";

const EXPECTED_CONDITION: &str = "resume_checkpoint != 'checkpoint-after-implementation' and resume_checkpoint != 'checkpoint-after-review-feedback'";

const ORCHESTRATION_SENTINEL: &str = "No files modified \u{2014} orchestration task";

fn workflow_tdd_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("amplifier-bundle")
        .join("recipes")
        .join(format!("{RECIPE_NAME}.yaml"))
}

fn load_workflow_tdd() -> Recipe {
    let path = workflow_tdd_path();
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_yaml::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

fn step_by_id(recipe: &Recipe, id: &str) -> Step {
    let raw = std::fs::read_to_string(workflow_tdd_path()).unwrap();
    let value: serde_yaml::Value = serde_yaml::from_str(&raw).unwrap();
    let steps = value
        .get("steps")
        .and_then(|s| s.as_sequence())
        .unwrap_or_else(|| panic!("workflow-tdd.yaml: top-level `steps:` missing"));
    for step in steps {
        let step_id = step.get("id").and_then(|v| v.as_str()).unwrap_or_default();
        if step_id == id {
            return serde_yaml::from_value(step.clone())
                .unwrap_or_else(|e| panic!("decode step {id}: {e}"));
        }
    }
    let _ = recipe; // silence unused
    panic!("workflow-tdd.yaml: step `{id}` not found")
}

// ---------------------------------------------------------------------------
// Structural: agent step (step-08c-work-verifier)
// ---------------------------------------------------------------------------

#[test]
fn step_08c_work_verifier_is_agent_type_with_correct_wiring() {
    let recipe = load_workflow_tdd();
    let step = step_by_id(&recipe, VERIFIER_STEP_ID);

    // Issue #615: this step MUST be an agent invocation, never bash. The
    // legacy guard was a 150-line bash heuristic; the whole point of #615 is
    // to delegate the judgement to an agent that can read the codebase.
    assert!(
        step.command.is_none(),
        "{VERIFIER_STEP_ID} must NOT be a bash step (no `command:` field) — \
         issue #615 replaced the legacy bash heuristic with an agent step"
    );
    assert!(
        step.agent.is_some(),
        "{VERIFIER_STEP_ID} must declare an `agent:` field"
    );
    let agent = step.agent.as_deref().unwrap();
    assert!(
        agent.starts_with("amplihack:"),
        "{VERIFIER_STEP_ID} must invoke an `amplihack:*` agent (got {agent:?})"
    );

    // Must run inside the worktree set up by the prep phase, so the agent's
    // git/gh/grep tools see the actual implementation diff.
    assert_eq!(
        step.working_dir.as_deref(),
        Some("{{worktree_setup.worktree_path}}"),
        "{VERIFIER_STEP_ID} must set working_dir to {{{{worktree_setup.worktree_path}}}}"
    );

    // Output key drives the bash gate.
    assert_eq!(
        step.output.as_deref(),
        Some("verdict_json"),
        "{VERIFIER_STEP_ID} must emit `verdict_json` so the gate can read it"
    );

    // Prompt is mandatory.
    let prompt = step
        .prompt
        .as_deref()
        .unwrap_or_else(|| panic!("{VERIFIER_STEP_ID} must declare a prompt"));
    assert!(
        !prompt.trim().is_empty(),
        "{VERIFIER_STEP_ID} prompt must not be empty"
    );
}

#[test]
fn step_08c_work_verifier_prompt_documents_investigation_surfaces() {
    let recipe = load_workflow_tdd();
    let step = step_by_id(&recipe, VERIFIER_STEP_ID);
    let prompt = step.prompt.unwrap();

    // Per task brief: "Be liberal about WHERE work landed (worktrees, sibling
    // branches, in-step merged PRs, closed issues, etc.) — the verifier
    // prompt must explicitly tell the agent to look across these surfaces."
    let required_surfaces = [
        ("worktree", "git worktree list / sibling worktrees"),
        ("branch", "sibling branches via git branch -a"),
        ("merge-base", "in-step merged PR detection"),
        ("gh pr", "GitHub PR cross-reference"),
        ("gh issue", "linked issue closure cross-reference"),
        ("origin/main", "branch-point comparison"),
    ];
    for (needle, why) in required_surfaces {
        assert!(
            prompt.contains(needle),
            "{VERIFIER_STEP_ID} prompt must mention `{needle}` ({why}) so the agent investigates that surface"
        );
    }

    // Per task brief: "Be strict about WHETHER work landed (empty commits
    // don't count; unrelated edits don't count)."
    let strict_phrases = ["empty commit", "do NOT count", "intent"];
    for needle in strict_phrases {
        assert!(
            prompt.to_lowercase().contains(&needle.to_lowercase()),
            "{VERIFIER_STEP_ID} prompt must mention `{needle}` to enforce strict intent matching"
        );
    }
}

#[test]
fn step_08c_work_verifier_prompt_documents_fast_path_optouts() {
    let recipe = load_workflow_tdd();
    let step = step_by_id(&recipe, VERIFIER_STEP_ID);
    let prompt = step.prompt.unwrap();

    // Issue #425 parity (per task requirement #5): both ALLOW_NO_OP and the
    // orchestration sentinel must continue to short-circuit to WORK_VERIFIED.
    assert!(
        prompt.contains("allow_no_op"),
        "{VERIFIER_STEP_ID} prompt must reference the `allow_no_op` opt-out (issue #425)"
    );
    assert!(
        prompt.contains(ORCHESTRATION_SENTINEL),
        "{VERIFIER_STEP_ID} prompt must contain the literal orchestration sentinel \
         (with em-dash U+2014) — `{ORCHESTRATION_SENTINEL}`"
    );
}

#[test]
fn step_08c_work_verifier_prompt_documents_three_verdicts_and_json_contract() {
    let recipe = load_workflow_tdd();
    let step = step_by_id(&recipe, VERIFIER_STEP_ID);
    let prompt = step.prompt.unwrap();

    for verdict in ["WORK_VERIFIED", "HOLLOW_SUCCESS", "INSUFFICIENT_EVIDENCE"] {
        assert!(
            prompt.contains(verdict),
            "{VERIFIER_STEP_ID} prompt must define the `{verdict}` verdict"
        );
    }
    // JSON output contract: the gate reads the LAST line.
    assert!(
        prompt.contains("\"verdict\""),
        "{VERIFIER_STEP_ID} prompt must show the JSON schema with a `verdict` key"
    );
    assert!(
        prompt.to_lowercase().contains("last line")
            || prompt.to_lowercase().contains("very last line"),
        "{VERIFIER_STEP_ID} prompt must specify the JSON goes on the LAST LINE \
         of stdout (the bash gate uses tail-grep extraction)"
    );
}

// ---------------------------------------------------------------------------
// Structural: bash gate (step-08c-enforce-verdict)
// ---------------------------------------------------------------------------

#[test]
fn step_08c_enforce_verdict_is_bash_type_with_correct_wiring() {
    let recipe = load_workflow_tdd();
    let step = step_by_id(&recipe, ENFORCE_STEP_ID);

    assert_eq!(
        step.step_type.as_deref(),
        Some("bash"),
        "{ENFORCE_STEP_ID} must be type=bash"
    );
    assert!(
        step.agent.is_none(),
        "{ENFORCE_STEP_ID} must NOT be an agent step — it is a deterministic gate"
    );
    let command = step
        .command
        .as_deref()
        .unwrap_or_else(|| panic!("{ENFORCE_STEP_ID} must declare a `command:` field"));
    assert!(
        command.contains("set -euo pipefail"),
        "{ENFORCE_STEP_ID} must `set -euo pipefail` to fail loud on shell errors"
    );
    // The gate must consume the verifier's verdict via the env var name
    // produced from `output: verdict_json`.
    assert!(
        command.contains("VERDICT_JSON"),
        "{ENFORCE_STEP_ID} command must read $VERDICT_JSON (the verifier's output)"
    );
}

#[test]
fn step_08c_pair_preserves_condition_clause() {
    let recipe = load_workflow_tdd();

    // Per task requirement #3: the existing condition clause must be
    // preserved verbatim on BOTH new steps. If the gate runs without the
    // verifier (or vice versa), checkpoint-resume runs would either re-run
    // judgement they already passed or skip judgement entirely.
    for id in [VERIFIER_STEP_ID, ENFORCE_STEP_ID] {
        let step = step_by_id(&recipe, id);
        assert_eq!(
            step.condition.as_deref(),
            Some(EXPECTED_CONDITION),
            "{id} must preserve the original step-08c condition clause verbatim"
        );
    }
}

#[test]
fn step_08c_pair_appears_consecutively_with_verifier_before_gate() {
    let recipe = load_workflow_tdd();
    let positions: Vec<(usize, &str)> = recipe
        .steps
        .iter()
        .enumerate()
        .filter(|(_, s)| s.id == VERIFIER_STEP_ID || s.id == ENFORCE_STEP_ID)
        .map(|(i, s)| (i, s.id.as_str()))
        .collect();
    assert_eq!(
        positions.len(),
        2,
        "workflow-tdd.yaml must contain exactly one of each step in the pair; got {positions:?}"
    );
    assert_eq!(
        positions[0].1, VERIFIER_STEP_ID,
        "verifier (agent) must precede the enforce gate (bash)"
    );
    assert_eq!(positions[1].1, ENFORCE_STEP_ID);
    assert_eq!(
        positions[1].0,
        positions[0].0 + 1,
        "the gate must immediately follow the verifier (no foreign step in between)"
    );
}

#[test]
fn legacy_step_08c_implementation_no_op_guard_is_removed() {
    // Regression guard: nobody may re-introduce the brittle bash guard
    // alongside the new agentic verifier (would cause double-failure).
    let recipe = load_workflow_tdd();
    let ids: HashSet<&str> = recipe.steps.iter().map(|s| s.id.as_str()).collect();
    assert!(
        !ids.contains(LEGACY_STEP_ID),
        "{LEGACY_STEP_ID} must remain removed — it was replaced by \
         {VERIFIER_STEP_ID} + {ENFORCE_STEP_ID} per issue #615"
    );
}

// ---------------------------------------------------------------------------
// Runtime: execute the actual bash gate against synthetic verdicts
// ---------------------------------------------------------------------------

struct GateRun {
    code: i32,
    stderr: String,
}

/// Execute `step-08c-enforce-verdict`'s bash command body with the given
/// environment. We extract the actual command from the YAML so the test
/// breaks the moment the gate logic regresses.
fn run_gate(verdict_json: &str, implementation: &str, allow_no_op: &str) -> GateRun {
    let recipe = load_workflow_tdd();
    let step = step_by_id(&recipe, ENFORCE_STEP_ID);
    let command = step.command.expect("enforce-verdict has command");

    let mut script_file = tempfile::NamedTempFile::new().expect("tempfile");
    script_file
        .as_file_mut()
        .write_all(command.as_bytes())
        .expect("write script");
    let script_path = script_file.path().to_path_buf();

    let output = Command::new("bash")
        .arg(&script_path)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("VERDICT_JSON", verdict_json)
        .env("IMPLEMENTATION", implementation)
        .env("ALLOW_NO_OP", allow_no_op)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn bash");

    GateRun {
        code: output.status.code().unwrap_or(-1),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    }
}

#[test]
fn enforce_verdict_returns_zero_for_work_verified() {
    let verdict = r#"{"verdict": "WORK_VERIFIED", "evidence": ["abc1234"], "rationale": "ok"}"#;
    let run = run_gate(verdict, "Files modified: src/foo.rs", "false");
    assert_eq!(
        run.code, 0,
        "WORK_VERIFIED must exit 0 (stderr was: {})",
        run.stderr
    );
    assert!(
        run.stderr.contains("APPROVED") || run.stderr.contains("WORK_VERIFIED"),
        "WORK_VERIFIED branch should announce success on stderr; got: {}",
        run.stderr
    );
}

#[test]
fn enforce_verdict_returns_zero_with_warning_for_insufficient_evidence() {
    let verdict =
        r#"{"verdict": "INSUFFICIENT_EVIDENCE", "evidence": [], "rationale": "no network"}"#;
    let run = run_gate(verdict, "anything", "false");
    assert_eq!(
        run.code, 0,
        "INSUFFICIENT_EVIDENCE must exit 0 (recipe continues) — got code={}, stderr={}",
        run.code, run.stderr
    );
    assert!(
        run.stderr.contains("WARN") || run.stderr.contains("INSUFFICIENT_EVIDENCE"),
        "INSUFFICIENT_EVIDENCE must produce a loud stderr warning; got: {}",
        run.stderr
    );
}

#[test]
fn enforce_verdict_returns_one_for_hollow_success() {
    let verdict =
        r#"{"verdict": "HOLLOW_SUCCESS", "evidence": [], "rationale": "no concrete artifact"}"#;
    let run = run_gate(verdict, "anything", "false");
    assert_eq!(
        run.code, 1,
        "HOLLOW_SUCCESS must fail the recipe with exit 1 — got code={}, stderr={}",
        run.code, run.stderr
    );
    assert!(
        run.stderr.contains("HOLLOW_SUCCESS") || run.stderr.contains("ERROR"),
        "HOLLOW_SUCCESS must surface the verdict on stderr; got: {}",
        run.stderr
    );
}

#[test]
fn enforce_verdict_returns_one_for_unknown_verdict() {
    let verdict = r#"{"verdict": "WHATEVER", "evidence": [], "rationale": "typo"}"#;
    let run = run_gate(verdict, "anything", "false");
    assert_eq!(
        run.code, 1,
        "Unknown verdict literal must fail closed (exit 1) — got code={}, stderr={}",
        run.code, run.stderr
    );
}

#[test]
fn enforce_verdict_orchestration_sentinel_short_circuits_to_zero() {
    // Issue #425 parity: even if the verifier emitted HOLLOW_SUCCESS, the
    // orchestration sentinel in implement output must short-circuit the
    // gate to exit 0 BEFORE parsing the verdict.
    let verdict = r#"{"verdict": "HOLLOW_SUCCESS", "evidence": [], "rationale": "x"}"#;
    let impl_output = format!("Step 8 complete.\n\n{ORCHESTRATION_SENTINEL}\n");
    let run = run_gate(verdict, &impl_output, "false");
    assert_eq!(
        run.code, 0,
        "Orchestration sentinel must short-circuit to exit 0 (issue #425 fast-path); \
         got code={}, stderr={}",
        run.code, run.stderr
    );
    assert!(
        run.stderr.contains("orchestration") || run.stderr.contains("sentinel"),
        "Sentinel fast-path should announce why it short-circuited; got: {}",
        run.stderr
    );
}

#[test]
fn enforce_verdict_allow_no_op_short_circuits_to_zero() {
    // Issue #425 parity: ALLOW_NO_OP=true (orchestration / docs-only / audit
    // / meta classification) must short-circuit BEFORE parsing the verdict.
    let verdict = r#"{"verdict": "HOLLOW_SUCCESS", "evidence": [], "rationale": "x"}"#;
    let run = run_gate(verdict, "no diff", "true");
    assert_eq!(
        run.code, 0,
        "ALLOW_NO_OP=true must short-circuit to exit 0 (issue #425 fast-path); \
         got code={}, stderr={}",
        run.code, run.stderr
    );
    assert!(
        run.stderr.contains("ALLOW_NO_OP") || run.stderr.contains("opt-out"),
        "ALLOW_NO_OP fast-path should announce why it short-circuited; got: {}",
        run.stderr
    );
}

#[test]
fn enforce_verdict_empty_input_fail_safes_to_warn_and_continue() {
    // Per the verifier prompt and gate comments: "if you emit malformed JSON
    // the gate treats the verdict as INSUFFICIENT_EVIDENCE so a verifier
    // formatting bug never masks a real failure". Empty input must NOT
    // silently pass with no warning, and must NOT exit 1 (which would block
    // every recipe whose verifier crashed).
    let run = run_gate("", "anything", "false");
    assert_eq!(
        run.code, 0,
        "Empty VERDICT_JSON must fail-safe to exit 0 with WARN, never block the recipe; \
         got code={}, stderr={}",
        run.code, run.stderr
    );
    assert!(
        run.stderr.contains("WARN"),
        "Empty VERDICT_JSON must produce a loud WARN on stderr; got: {}",
        run.stderr
    );
}

#[test]
fn enforce_verdict_unparseable_input_fail_safes_to_warn_and_continue() {
    // Free-form prose with no JSON object on the last line: the agent's
    // formatter regressed. Must fail-safe identically to the empty case.
    let run = run_gate(
        "I investigated and the work landed but I forgot the JSON line.",
        "anything",
        "false",
    );
    assert_eq!(
        run.code, 0,
        "Unparseable VERDICT_JSON must fail-safe to exit 0 with WARN; \
         got code={}, stderr={}",
        run.code, run.stderr
    );
    assert!(
        run.stderr.contains("WARN"),
        "Unparseable VERDICT_JSON must produce a loud WARN on stderr; got: {}",
        run.stderr
    );
}

#[test]
fn enforce_verdict_extracts_json_from_last_line_of_mixed_prose() {
    // The agent prompt is explicit: prose followed by JSON on the very last
    // line. The gate must tolerate prose preceding the JSON.
    let mixed = "I investigated.\nFound commit abc123.\n\
                 {\"verdict\": \"WORK_VERIFIED\", \"evidence\": [\"abc123\"], \"rationale\": \"ok\"}";
    let run = run_gate(mixed, "anything", "false");
    assert_eq!(
        run.code, 0,
        "Gate must extract JSON from last line even when prose precedes it; \
         got code={}, stderr={}",
        run.code, run.stderr
    );
}
