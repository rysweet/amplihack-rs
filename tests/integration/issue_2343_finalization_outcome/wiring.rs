//! The two steps' wiring, by running them (#2343, #2809, #2819).
//!
//! These cases used to read recipe text and shell text: they grepped the
//! finalize recipe for `--guard` and `condition:`, and the validator for
//! forbidden words. `docs/howto/keep-tests-passing-under-host-load.md` forbids
//! that, and for a reason the repo had already hit -- a test that pins how
//! code is written fails when the wording moves and passes when the behaviour
//! moves. Each case below runs the real thing instead:
//!
//! - the shipped `assess-finalization` step, through the real pinned recipe
//!   runner, with the shipped sub-recipe beside it, so what is asserted is
//!   that the step is attempted on every outcome and that its failure -- the
//!   agent's, or the sub-recipe's own -- never ends the run;
//! - the shipped `validate-agentic-finalization` step's own `command:`, with a
//!   stub `amplihack` in its path, so what is asserted is the call that
//!   actually reaches `step-recovery run` and the exit the body actually gave
//!   it.
//!
//! Nothing here asserts the presence or absence of a word in a file.

use std::fs;
use std::path::PathBuf;

use super::fixture_ext::*;
use crate::issue_1641_finalization_open_pr::builders::as_refs;
use crate::issue_1641_finalization_open_pr::fixture::*;
use serde_json::{Value, json};

fn bundle_recipe(name: &str) -> PathBuf {
    workspace_root()
        .join("amplifier-bundle/recipes")
        .join(format!("{name}.yaml"))
}

/// The `<id>` step of a shipped recipe, parsed as the document it is, so the
/// step that runs below is the shipped one and not a copy of it.
fn shipped_step(recipe: &str, id: &str) -> serde_yaml::Value {
    let text = fs::read_to_string(bundle_recipe(recipe))
        .unwrap_or_else(|e| panic!("read {recipe}.yaml: {e}"));
    let document: serde_yaml::Value =
        serde_yaml::from_str(&text).unwrap_or_else(|e| panic!("parse {recipe}.yaml: {e}"));
    document["steps"]
        .as_sequence()
        .unwrap_or_else(|| panic!("{recipe}.yaml has no steps"))
        .iter()
        .find(|step| step["id"].as_str() == Some(id))
        .unwrap_or_else(|| panic!("{recipe}.yaml has no step {id}"))
        .clone()
}

/// `recipe-runner-rs` at the revision in `claude-plugin/recipe-runner.rev`
/// (#2658): never the host's installed runner.
fn pinned_runner() -> PathBuf {
    crate::pinned_runner::pinned_runner(&workspace_root())
}

struct Run {
    dir: tempfile::TempDir,
    result: Value,
}

impl Run {
    fn status(&self, step_id: &str) -> String {
        self.result["step_results"]
            .as_array()
            .expect("step results")
            .iter()
            .find(|step| step["step_id"] == step_id)
            .unwrap_or_else(|| panic!("no result for {step_id}: {}", self.result))
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    }
}

/// Run the shipped `assess-finalization` step followed by a marker step that
/// stands for everything after it, with `evidence` as the run's outcome.
/// `with_sub_recipe` decides whether the shipped sub-recipe is reachable.
fn run_assessment_chain(evidence: &str, with_sub_recipe: bool) -> Run {
    let dir = tempfile::TempDir::new().expect("tempdir");
    if with_sub_recipe {
        let name = "workflow-finalization-assessment";
        fs::copy(bundle_recipe(name), dir.path().join(format!("{name}.yaml")))
            .expect("stage the shipped sub-recipe beside the run");
    }
    let marker = dir.path().join("reached-the-validator");
    let recipe = json!({
        "name": "issue-2819-assessment-chain",
        "version": "1.0.0",
        "description": "the shipped assess-finalization step, then what follows it",
        "context": {
            "finalization_evidence": evidence,
            "finalizer_step_status": r#"{"status":"ok","reporting_failure":"false"}"#,
            "finalization_assessment": "",
        },
        "steps": [
            serde_yaml::from_value::<Value>(shipped_step("workflow-finalize", "assess-finalization"))
                .expect("the shipped step is representable as JSON"),
            {
                "id": "after-the-assessment",
                "type": "bash",
                "command": format!("touch {}", marker.display()),
            },
        ],
    });
    let path = dir.path().join("chain.yaml");
    fs::write(&path, serde_yaml::to_string(&recipe).expect("serialize")).expect("write recipe");

    let output = crate::isolated_amplihack::isolated(pinned_runner().to_str().expect("runner"))
        .arg(&path)
        .arg("--working-dir")
        .arg(dir.path())
        .args(["--no-auto-stage", "--output-format", "json"])
        .env("HOME", dir.path())
        .env("TMPDIR", dir.path())
        .output()
        .expect("run the pinned runner");
    let result: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "the runner must serialize its result: {e}\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    assert!(
        marker.exists(),
        "the steps after the assessment must still run: {result}"
    );
    Run { dir, result }
}

// The assessment runs on every outcome and never ends the run by itself. It
// once carried a `condition:` that skipped it for the verdicts the bash
// classifier exited on; both are gone. Asserted by running the shipped step
// over two opposite outcomes with no agent on the host, so the agent step
// inside it fails every time: the step is still attempted, and the run still
// finishes.
#[test]
fn the_assessment_is_attempted_on_every_outcome_and_never_ends_the_run() {
    for (what, evidence) in [
        (
            "a finished run",
            json!({"schema_version": 1, "outcome": {"pr_number": "7", "pr_state": "OPEN"}}),
        ),
        (
            "a run that published nothing",
            json!({"schema_version": 1, "outcome": {"pr_number": "", "remote_head": ""}}),
        ),
    ] {
        let run = run_assessment_chain(&evidence.to_string(), true);

        assert_eq!(
            run.result["success"], true,
            "{what}: a declined assessment must not end the run: {}",
            run.result
        );
        assert_ne!(
            run.status("assess-finalization"),
            "skipped",
            "{what}: the assessment decides every run's terminal state, so no outcome skips it: {}",
            run.result
        );
        assert_eq!(
            run.status("after-the-assessment"),
            "completed",
            "{what}: {}",
            run.result
        );
        drop(run.dir);
    }
}

// The sub-recipe's own failure -- resolution, the recursion limit -- is the
// other half of the same guarantee, and it is a different `continue_on_error`
// from the one above: the outer step's.
#[test]
fn an_assessment_the_run_cannot_even_resolve_still_lets_the_run_continue() {
    let run = run_assessment_chain(r#"{"schema_version":1}"#, false);

    assert_eq!(
        run.result["success"], true,
        "an unresolvable assessment must not end the run: {}",
        run.result
    );
    assert_eq!(
        run.status("assess-finalization"),
        "failed",
        "the step really did fail, rather than being skipped: {}",
        run.result
    );
    assert_eq!(run.status("after-the-assessment"), "completed");
}

/// A stand-in for the agent CLI the runner starts for an agent step. It
/// records the prompt it was handed and answers with the decision the case
/// recorded, so the shipped assessment recipe can be driven end to end
/// without an LLM (#2819 R4). The runner passes the prompt by path -- `-p
/// "Read and follow the complete instructions in the file <path>. ..."` -- so
/// the path is taken out of that sentence and the file copied where the case
/// can read it.
const AGENT_STUB: &str = r#"#!/usr/bin/env bash
prompt_arg=""; out=""
for ((i = 1; i <= $#; i++)); do
  case "${!i}" in
    -p) j=$((i + 1)); prompt_arg="${!j}" ;;
    --output-last-message) j=$((i + 1)); out="${!j}" ;;
  esac
done
file="$(printf '%s' "$prompt_arg" | sed -n 's/^.*instructions in the file \(.*\)\. Its contents.*$/\1/p')"
if [ -n "$file" ] && [ -f "$file" ]; then cat "$file" > "$AGENT_STUB_PROMPT"
else printf '%s' "$prompt_arg" > "$AGENT_STUB_PROMPT"; fi
n=$(( $(cat "$AGENT_STUB_ANSWER.count" 2>/dev/null || echo 0) + 1 )); echo "$n" > "$AGENT_STUB_ANSWER.count"
cp "$AGENT_STUB_PROMPT" "$AGENT_STUB_PROMPT.$n"
answer="$(cat "$AGENT_STUB_ANSWER.$n" 2>/dev/null || cat "$AGENT_STUB_ANSWER")"
[ -z "$out" ] || printf '%s' "$answer" > "$out"
printf '%s\n' "$answer"
"#;

/// The runner starts an agent step through the `amplihack` launcher, which it
/// invokes as `amplihack <agent binary> -- <args>`. This stands in for that
/// launcher and runs what it was told to run.
const LAUNCHER_STUB: &str = r#"#!/usr/bin/env bash
exec "$@"
"#;

/// The nested `amplihack recipe run` that `continue-ci-repair` starts (#2966).
/// It records the call and the `ci_repair_reentry` note it was handed, then
/// runs the recipe it was given -- the bundle's own assessment recipe --
/// through the pinned runner, with the same context, so the nested pass is the
/// shipped recipe with the shipped conditions and the same recorded agent.
/// First on PATH, so no test reaches the host's own `amplihack` or agent.
const NESTED_RUN_STUB: &str = r#"#!/usr/bin/env bash
printf '%s\n' "$*" >> "$NESTED_RUN_CALLS"
[ "$1 $2" = "recipe run" ] || { echo "unexpected amplihack call: $*" >&2; exit 96; }
recipe="$3"; shift 3
wd="."; ctx='{}'
while [ $# -gt 0 ]; do
  case "$1" in
    --working-dir) wd="$2"; shift 2 ;;
    -f) shift 2 ;;
    --context-from-file)
      k="${2%%=*}"; f="${2#*=}"
      [ "$k" != ci_repair_reentry ] || cp "$f" "$NESTED_RUN_NOTE"
      ctx="$(jq -c --arg k "$k" --rawfile v "$f" '. + {($k): $v}' <<<"$ctx")"; shift 2 ;;
    -c) ctx="$(jq -c --arg k "${2%%=*}" --arg v "${2#*=}" '. + {($k): $v}' <<<"$ctx")"; shift 2 ;;
    *) echo "unexpected argument: $1" >&2; exit 96 ;;
  esac
done
cf="$(mktemp "${TMPDIR:-/tmp}/nested-ctx.XXXXXX")"
printf '%s' "$ctx" > "$cf"
exec "$PINNED_RUNNER" "$recipe" --working-dir "$wd" --no-auto-stage --output-format json --context-file "$cf"
"#;

/// What a run of the shipped assessment recipe produced: the prompt the agent
/// was given, and the outer run it happened in.
struct Assessed {
    prompt: String,
    run: Run,
}

impl Assessed {
    /// `finalization_assessment` as the outer run carries it after the
    /// sub-recipe returned, serialized the way a step's `RECIPE_VAR_` is.
    fn assessment(&self) -> String {
        match &self.run.result["context"]["finalization_assessment"] {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        }
    }

    /// The nested `recipe run` calls `continue-ci-repair` made (#2966).
    fn nested_runs(&self) -> Vec<String> {
        fs::read_to_string(self.run.dir.path().join("nested-runs.log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// The prompt the agent was given on its `n`th call (1 is the first pass).
    fn prompt_of(&self, n: usize) -> String {
        fs::read_to_string(self.run.dir.path().join(format!("prompt.md.{n}")))
            .unwrap_or_else(|e| panic!("the agent was not called {n} times: {e}"))
    }

    /// The `ci_repair_reentry` note the nested run was handed.
    fn nested_note(&self) -> Value {
        let text = fs::read_to_string(self.run.dir.path().join("nested-note.json"))
            .expect("the nested run was handed a note");
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("the note is JSON ({e}): {text}"))
    }
}

/// Run the shipped `assess-finalization` step -- the real sub-recipe, with its
/// real prompt -- over `evidence`, with an agent that answers `answer`.
fn assess_with_recorded_agent(evidence: &str, answer: &str) -> Assessed {
    assess_with_recorded_agent_passes(evidence, answer, &[])
}

/// The same, with the agent answering `later[i]` on call `i + 2` (a pass
/// `continue-ci-repair` started) and `first` on every other call.
fn assess_with_recorded_agent_passes(evidence: &str, first: &str, later: &[&str]) -> Assessed {
    let answer = first;
    let dir = tempfile::TempDir::new().expect("tempdir");
    let name = "workflow-finalization-assessment";
    fs::copy(bundle_recipe(name), dir.path().join(format!("{name}.yaml")))
        .expect("stage the shipped sub-recipe beside the run");
    let agent = dir.path().join("bin/agent");
    let launcher = dir.path().join("bin/launcher");
    fs::create_dir_all(dir.path().join("bin")).expect("bin dir");
    write_executable(&agent, AGENT_STUB);
    write_executable(&launcher, LAUNCHER_STUB);
    write_executable(&dir.path().join("bin/amplihack"), NESTED_RUN_STUB);
    let path_var = format!(
        "{}:{}",
        dir.path().join("bin").display(),
        std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into())
    );
    let prompt_file = dir.path().join("prompt.md");
    let answer_file = dir.path().join("answer.txt");
    fs::write(&answer_file, answer).expect("record the answer");
    for (i, text) in later.iter().enumerate() {
        fs::write(dir.path().join(format!("answer.txt.{}", i + 2)), text)
            .expect("record a later answer");
    }

    let marker = dir.path().join("reached-the-validator");
    let recipe = json!({
        "name": "issue-2819-recorded-assessment",
        "version": "1.0.0",
        "description": "the shipped assess-finalization step, driven by a recorded agent",
        "context": {
            "finalization_evidence": evidence,
            "finalizer_step_status": r#"{"status":"ok","reporting_failure":"false"}"#,
            "finalization_assessment": "",
        },
        "steps": [
            serde_yaml::from_value::<Value>(shipped_step("workflow-finalize", "assess-finalization"))
                .expect("the shipped step is representable as JSON"),
            {
                "id": "after-the-assessment",
                "type": "bash",
                "command": format!("touch {}", marker.display()),
            },
        ],
    });
    let path = dir.path().join("chain.yaml");
    fs::write(&path, serde_yaml::to_string(&recipe).expect("serialize")).expect("write recipe");

    let output = crate::isolated_amplihack::isolated(pinned_runner().to_str().expect("runner"))
        .arg(&path)
        .arg("--working-dir")
        .arg(dir.path())
        .args(["--no-auto-stage", "--output-format", "json"])
        .env("HOME", dir.path())
        .env("TMPDIR", dir.path())
        .env("AMPLIHACK_AGENT_BINARY", &agent)
        .env("AMPLIHACK_LAUNCHER_BINARY", &launcher)
        .env("AGENT_STUB_PROMPT", &prompt_file)
        .env("AGENT_STUB_ANSWER", &answer_file)
        // #2966: continue-ci-repair finds its helper in the bundle, as in a
        // real run, and its nested run is the stub above.
        .env("AMPLIHACK_HOME", workspace_root())
        .env("PATH", path_var)
        .env("NESTED_RUN_CALLS", dir.path().join("nested-runs.log"))
        .env("NESTED_RUN_NOTE", dir.path().join("nested-note.json"))
        .env("PINNED_RUNNER", pinned_runner())
        .output()
        .expect("run the pinned runner");
    let result: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "the runner must serialize its result: {e}\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    assert!(
        marker.exists(),
        "the steps after the assessment must still run: {result}"
    );
    let prompt = fs::read_to_string(dir.path().join("prompt.md.1")).unwrap_or_default();
    Assessed {
        prompt,
        run: Run { dir, result },
    }
}

/// The evidence of a finished run: pushed at `head`, pull request #7 open at
/// it, checks green, worktree clean.
fn finished_evidence(head: &str) -> String {
    json!({
        "schema_version": 1,
        "git": {"dirty_worktree": "false", "meaningful_diff": "true"},
        "tooling": {"missing": "", "gh_required": "false"},
        "task": {"description": "Fix the finalization outcome reader", "description_read": "true"},
        "agent_outputs": {"implementation_output": "present", "hollow_success_signals": "unknown"},
        "outcome": {
            "schema_version": 3, "branch": BRANCH, "local_head": head, "remote": REPO,
            "github_origin": "true", "remote_head": head,
            "remote_head_descends_local_head": "true", "open_pr_count": "1",
            "pr_number": "7", "pr_url": PR_URL, "pr_state": "OPEN", "pr_draft": "false",
            "pr_head": head, "pr_cross_repository": "false",
            "pr_head_descends_local_head": "true", "checks": "green",
            "check_states": "ci=SUCCESS", "unread": "",
        },
    })
    .to_string()
}

// #2819 R4: the decision, tested where it is made. Every other case in this
// directory hands the validator a decision and asserts the reporting; this one
// runs the shipped `workflow-finalization-assessment.yaml`, with its shipped
// prompt, against an agent whose answer is recorded, and follows that answer
// out of the sub-recipe and into the state finalization publishes.
//
// What it can show is the wiring, which is what broke in #2343: that the facts
// `collect-finalization-evidence` recorded reach the prompt the deciding agent
// reads, that the object the agent returns is what the outer run carries as
// `finalization_assessment`, and that the validator publishes that decision
// and no other. What no test can show is the judgement itself: choosing the
// state is an LLM call, so the answer here is a control.
#[test]
fn the_facts_reach_the_deciding_agent_and_its_answer_is_what_gets_published() {
    let fx = Fixture::new();
    fx.with_remote();
    fx.push();
    fx.gh_pr_at(&fx.head(), green_rollup_for_tests());
    let head = fx.head();

    let assessed = assess_with_recorded_agent(
        &finished_evidence(&head),
        &json!({
            "terminal_state": "EXISTING_OPEN_PR",
            "action_taken": "none",
            "pr_number": 7,
            "reason": "gh pr view 7 reports the request open at the run's head",
            "next_action": "Wait for CI on PR #7.",
        })
        .to_string(),
    );

    assert_eq!(
        assessed.run.status("assess-finalization"),
        "completed",
        "the shipped assessment must run: {}",
        assessed.run.result
    );

    // 1. The facts reached the prompt the deciding agent read. Each of these
    //    is a fact an assessment of this run has to have: without them the
    //    agent is deciding on the prompt's prose alone.
    for fact in [
        head.as_str(),
        BRANCH,
        REPO,
        r#""pr_number":"7""#,
        r#""checks":"green""#,
        r#""dirty_worktree":"false""#,
        "Fix the finalization outcome reader",
    ] {
        assert!(
            assessed.prompt.contains(fact),
            "the deciding agent must be given `{fact}`; prompt was:\n{}",
            assessed.prompt
        );
    }
    // And the instruction that makes it the decider, from the shipped recipe.
    assert!(
        assessed.prompt.contains("Choose one terminal state"),
        "the prompt must be the shipped assessment prompt:\n{}",
        assessed.prompt
    );

    // 2. Its answer is what the outer run carries.
    let carried = assessed.assessment();
    let parsed: Value = serde_json::from_str(&carried)
        .unwrap_or_else(|e| panic!("the assessment must arrive as JSON ({e}): {carried}"));
    assert_eq!(parsed["terminal_state"], "EXISTING_OPEN_PR", "{carried}");
    assert_eq!(parsed["pr_number"], 7, "{carried}");
    // A decision passes continue-ci-repair unchanged and starts no second pass.
    assert!(
        assessed.nested_runs().is_empty(),
        "a decision must not start the assessment again: {:?}",
        assessed.nested_runs()
    );

    // 3. And the validator publishes that decision, from the value the real
    //    sub-recipe produced rather than one written here.
    let mut envs = finished_env(&fx.repo_path());
    envs.push(("RECIPE_VAR_finalization_assessment", carried));
    let published = fx.validate(&as_refs(&envs));

    assert!(published.success(), "{}", published.describe());
    assert_eq!(published.field("terminal_state"), "EXISTING_OPEN_PR");
    assert_eq!(published.field("terminal_success"), "true");
    assert_eq!(published.field("pr_number"), "7");
    assert_eq!(
        published.field("terminal_reason"),
        "gh pr view 7 reports the request open at the run's head",
        "the published reason is the agent's own sentence"
    );
}

// The other end of the same wire, and the mechanism #2847 item 1 is about: an
// agent that answers with prose rather than the one JSON object leaves the run
// with facts and no decision. The run does not end at the assessment -- the
// step is `continue_on_error: true` -- and nothing repairs the prose into
// something that reads like a decision. Since #2966 the prose is first started
// again: the runner's condition on continue-ci-repair finds no terminal_state,
// so it runs one fresh pass of the shipped recipe, which here answers the same
// prose. The nested run's reentered-pass-undecided step fails it, so the step
// fails for its recovery agent (none here) and the sub-recipe fails. The runner drops a failed
// sub-recipe's output, so what reaches `validate-agentic-finalization` is no
// answer at all, and the validator reports that it has no decision rather than
// inventing one about a run whose pull request is open and green. Driven with a
// recorded agent so what fails is the real answer, not a missing CLI.
#[test]
fn an_agent_that_answers_with_prose_leaves_the_run_with_no_decision() {
    let fx = Fixture::new();
    fx.with_remote();
    fx.push();
    fx.gh_pr_at(&fx.head(), green_rollup_for_tests());
    let prose = "The run looks finished to me; PR #7 is open and green.";

    let assessed = assess_with_recorded_agent(&finished_evidence(&fx.head()), prose);

    assert_eq!(
        assessed.run.result["success"], true,
        "a declined assessment must not end the run: {}",
        assessed.run.result
    );
    assert_eq!(
        assessed.run.status("after-the-assessment"),
        "completed",
        "{}",
        assessed.run.result
    );
    // The prose was started again once. Prose is no answer the context reader
    // can hand on, so the pass is told the last answer was null.
    assert_eq!(
        assessed.nested_runs().len(),
        1,
        "one second pass: {:?}",
        assessed.nested_runs()
    );
    assert_eq!(assessed.nested_note(), json!({"previous_answer": null}));
    assert!(
        assessed
            .run
            .result
            .to_string()
            .contains("did not decide either"),
        "the fresh pass failed on the nested run's own condition: {}",
        assessed.run.result
    );
    // The second pass decided nothing either, so the step failed and nothing
    // that reads like a decision was carried out of the sub-recipe.
    assert_eq!(
        assessed.run.status("assess-finalization"),
        "failed",
        "{}",
        assessed.run.result
    );
    let carried = assessed.assessment();
    assert!(
        carried.is_empty() || carried == prose,
        "no decision may be carried out of the sub-recipe: {carried}"
    );

    // What finalization does with it: no decision, and no verdict of its own.
    let mut envs = finished_env(&fx.repo_path());
    envs.push(("RECIPE_VAR_finalization_assessment", carried));
    let published = fx.validate(&as_refs(&envs));

    assert!(!published.success(), "{}", published.describe());
    assert_eq!(
        published.field("terminal_state"),
        "FAILED_MISSING_TERMINAL_EVIDENCE",
        "{}",
        published.describe()
    );
    assert_ne!(
        published.field("terminal_state"),
        "FAILED_IMPLEMENTATION",
        "the #2343 symptom: a finished run must not be classed as unimplemented"
    );
    assert!(
        published
            .field("terminal_reason")
            .contains("no terminal state"),
        "the reason must say the assessment decided nothing: {}",
        published.field("terminal_reason")
    );
    // The facts are still reported, so the operator sees the run that was
    // finished rather than only the gap in finalization.
    assert!(
        published.field("evidence_used").contains("outcome.pr=7"),
        "the facts must survive the missing decision: {}",
        published.field("evidence_used")
    );
}

/// A `CI_REPAIR_UNFINISHED` answer from a session that stopped with the head red.
fn unfinished_answer() -> String {
    json!({
        "terminal_state": "CI_REPAIR_UNFINISHED",
        "action_taken": "none",
        "pr_number": "7",
        "reason": "stopped with smoke failing; head abc, 2 green, 1 failing, 0 pending, 0 unknown",
        "next_action": "reproduce the smoke check locally and fix it",
    })
    .to_string()
}

// #2966, crusty round 3: the runner, not a script, decides that an unfinished
// repair needs another pass, and that the pass decided. CI_REPAIR_UNFINISHED is
// the session's own statement; continue-ci-repair's condition starts one fresh
// pass with that answer as data, and when the pass decides, its answer is what
// the outer run carries. When the pass is unfinished too, the nested run's
// reentered-pass-undecided fails it and the step fails, after one pass, for its
// recovery agent.
#[test]
fn an_unfinished_repair_gets_one_fresh_pass_and_its_decision_is_carried() {
    let fx = Fixture::new();
    fx.with_remote();
    fx.push();
    fx.gh_pr_at(&fx.head(), green_rollup_for_tests());
    let decision = json!({
        "terminal_state": "EXISTING_OPEN_PR",
        "action_taken": "repaired_ci",
        "pr_number": "7",
        "reason": "head def, 3 green, 0 failing, 0 pending, 0 unknown",
        "next_action": "See pull request #7",
    })
    .to_string();

    let assessed = assess_with_recorded_agent_passes(
        &finished_evidence(&fx.head()),
        &unfinished_answer(),
        &[&decision],
    );
    assert_eq!(
        assessed.nested_runs().len(),
        1,
        "{:?}",
        assessed.nested_runs()
    );
    let last: Value = serde_json::from_str(&unfinished_answer()).unwrap();
    assert_eq!(assessed.nested_note(), json!({"previous_answer": last}));
    assert!(
        assessed
            .prompt_of(2)
            .contains("reproduce the smoke check locally"),
        "the fresh session reads the last answer in its reentry block"
    );
    assert_eq!(
        assessed.run.status("assess-finalization"),
        "completed",
        "{}",
        assessed.run.result
    );
    let carried: Value = serde_json::from_str(&assessed.assessment())
        .unwrap_or_else(|e| panic!("a decision is carried ({e}): {}", assessed.assessment()));
    assert_eq!(carried["terminal_state"], "EXISTING_OPEN_PR");
    assert_eq!(
        carried["reason"],
        "head def, 3 green, 0 failing, 0 pending, 0 unknown"
    );

    let still = assess_with_recorded_agent(&finished_evidence(&fx.head()), &unfinished_answer());
    assert_eq!(
        still.nested_runs().len(),
        1,
        "one pass, no loop: {:?}",
        still.nested_runs()
    );
    assert_eq!(
        still.run.status("assess-finalization"),
        "failed",
        "an unfinished fresh pass fails the step for its recovery agent: {}",
        still.run.result
    );
    assert_eq!(still.run.status("after-the-assessment"), "completed");
}

// #2966, crusty round 3, concern 3: a BLOCKED_CI that leaves out its owner's
// step is not started again by a check and is not refused. It goes to the
// validator as the assessment answered it.
#[test]
fn an_unowned_blocked_ci_is_not_started_again() {
    let fx = Fixture::new();
    fx.with_remote();
    fx.push();
    fx.gh_pr_at(&fx.head(), green_rollup_for_tests());
    let unowned = json!({
        "terminal_state": "BLOCKED_CI",
        "action_taken": "none",
        "pr_number": "7",
        "reason": "deploy: the workflow reads secrets.DEPLOY_TOKEN, which the repository does not define",
        "next_action": "",
    })
    .to_string();

    let assessed = assess_with_recorded_agent(&finished_evidence(&fx.head()), &unowned);

    assert!(
        assessed.nested_runs().is_empty(),
        "{:?}",
        assessed.nested_runs()
    );
    let carried: Value = serde_json::from_str(&assessed.assessment()).unwrap();
    assert_eq!(carried, serde_json::from_str::<Value>(&unowned).unwrap());
}

/// The shipped `validate-agentic-finalization` step's own `command:`.
fn validator_step_command() -> String {
    shipped_step("workflow-finalize", "validate-agentic-finalization")["command"]
        .as_str()
        .expect("the validator step has a bash command")
        .to_owned()
}

// Recovery on the validator step offers a stop, not a repair (#2819 R2).
// Asserted by running the step with a stub in `amplihack`'s place and reading
// the call it made. `--no-repair` withholds `retry_after_repair`
// (docs/reference/step-failure-recovery.md#the-four-choices), which is right
// here because a retry re-runs the same body against the same recorded
// decision and the same recorded evidence: the case below repairs the world
// between two runs and gets the same answer twice. `--guard` would also
// withhold the declared alternatives and make this a check rather than a
// report, so the step is not one.
#[test]
fn a_no_decision_exit_reaches_a_recovery_agent_that_can_only_stop_with_the_reason() {
    let fx = Fixture::new();
    fx.with_remote();
    fx.push();
    fx.gh_pr_at(&fx.head(), green_rollup_for_tests());
    fx.stub_step_recovery_driver();
    let mut envs = finished_env(&fx.repo_path());
    envs.push(("RECIPE_VAR_step_recovery_dir", fx.repo_path()));

    let outcome = fx.run_step(&validator_step_command(), &as_refs(&envs));

    let calls = fx.driver_calls();
    let call = calls
        .iter()
        .find(|call| call.contains("step-recovery run"))
        .unwrap_or_else(|| panic!("the step must run under step recovery, got {calls:?}"));
    assert!(
        call.contains("--step-id validate-agentic-finalization"),
        "recovery must be recorded against this step: {call}"
    );
    for required in ["--essential", "--stop-on-handoff", "--no-repair"] {
        assert!(call.contains(required), "{required} is missing: {call}");
    }
    assert!(
        !call.contains("--guard"),
        "the step reports a decision rather than checking one, so it is not a guard: {call}"
    );
    // And what reached it is the gap, not a verdict: the body the driver ran
    // reported that nothing decided, and failed.
    assert_eq!(
        outcome.output.status.code(),
        Some(1),
        "the body must fail for recovery to see it\n{}",
        outcome.describe()
    );
    assert_eq!(
        outcome.field("terminal_state"),
        "FAILED_MISSING_TERMINAL_EVIDENCE",
        "{}",
        outcome.describe()
    );
    assert_ne!(
        outcome.field("terminal_state"),
        "FAILED_IMPLEMENTATION",
        "the validator must not reach a verdict of its own"
    );
}

/// The evidence `collect-finalization-evidence` records, carrying the facts of
/// the run's outcome under `outcome` the way a real run does.
fn recorded_evidence(dirty: &str, outcome: Value) -> (&'static str, String) {
    (
        "RECIPE_VAR_finalization_evidence",
        json!({
            "schema_version": 1,
            "git": {"dirty_worktree": dirty},
            "tooling": {"missing": "", "gh_required": "false"},
            "prior_terminal_state": {"terminal_state": ""},
            "agent_outputs": {"hollow_success_signals": "unknown"},
            "outcome": outcome,
        })
        .to_string(),
    )
}

// Why `--no-repair` is the honest flag, shown rather than argued (#2819 R2).
// `retry_after_repair` re-runs the same body with the same environment. Every
// input this body reads was recorded before the first attempt, so repairing
// the world between the two runs changes nothing it reads.
//
// Each case starts from a real failure a recovery agent would be tempted to
// repair, applies the repair that would fix the run, and runs the shipped
// step's own `command:` again. The two results must be identical, field for
// field. (The agent is in fact forbidden both repairs below -- it may make no
// GitHub write and may not rewrite history -- which is the other half of the
// argument; this half holds even for an agent that could.)
#[test]
fn repairing_the_run_between_two_runs_of_the_step_changes_nothing_it_reports() {
    // Nothing on the remote and no pull request: the assessment decided
    // FAILED_IMPLEMENTATION on exactly that, and the facts were recorded.
    // The repair is the one the goal text used to invite -- push the branch
    // and open the request.
    assert_a_repair_is_invisible(
        "nothing was pushed",
        "FAILED_IMPLEMENTATION",
        |fx| {
            fx.with_remote();
            fx.gh_no_pr();
            let mut envs = finished_env(&fx.repo_path());
            envs.push(recorded_evidence(
                "false",
                json!({"branch": BRANCH, "local_head": fx.head(), "remote": REPO,
                       "remote_head": "", "pr_number": "", "unread": ""}),
            ));
            envs.push(assessed("FAILED_IMPLEMENTATION", ""));
            envs
        },
        |fx| {
            fx.push();
            fx.gh_pr_at(&fx.head(), green_rollup_for_tests());
        },
    );

    // A dirty worktree, recorded as such, under a decision that would
    // otherwise be a success. The repair is to commit what was uncommitted.
    assert_a_repair_is_invisible(
        "the worktree was dirty",
        "FAILED_DIRTY_WORKTREE",
        |fx| {
            fx.with_remote();
            fx.push();
            fx.gh_pr_at(&fx.head(), green_rollup_for_tests());
            fs::write(fx.repo.join("scratch.txt"), "uncommitted\n").expect("dirty the worktree");
            let mut envs = finished_env(&fx.repo_path());
            envs.push(recorded_evidence(
                "true",
                json!({"branch": BRANCH, "local_head": fx.head(), "remote": REPO,
                       "remote_head": fx.head(), "pr_number": "7", "pr_state": "OPEN",
                       "pr_head": fx.head(), "checks": "green", "unread": ""}),
            ));
            envs.push(assessed("EXISTING_OPEN_PR", "7"));
            envs
        },
        |fx| {
            fx.git(&["add", "scratch.txt"]);
            fx.git(&["commit", "-q", "-m", "commit the scratch file"]);
        },
    );
}

fn assessed(terminal_state: &str, pr: &str) -> (&'static str, String) {
    crate::issue_1641_finalization_open_pr::builders::assessment(terminal_state, "none", pr)
}

/// Set a run up, run the shipped step, repair the world, run it again, and
/// require the two results to be the same object.
fn assert_a_repair_is_invisible(
    what: &str,
    state: &str,
    setup: impl Fn(&Fixture) -> Vec<(&'static str, String)>,
    repair: impl Fn(&Fixture),
) {
    let fx = Fixture::new();
    fx.stub_step_recovery_driver();
    let envs = setup(&fx);

    let before = fx.run_step(&validator_step_command(), &as_refs(&envs));
    assert_eq!(
        before.field("terminal_state"),
        state,
        "{what}: the first run must report {state}\n{}",
        before.describe()
    );

    repair(&fx);
    let after = fx.run_step(&validator_step_command(), &as_refs(&envs));

    assert_eq!(
        after.json(),
        before.json(),
        "{what}: the repair is invisible to a retry, so the result is the same object. \
         That is why the step is wired --no-repair.\nbefore: {}\nafter: {}",
        before.describe(),
        after.describe()
    );
    assert_eq!(
        after.output.status.code(),
        before.output.status.code(),
        "{what}: and it exits the same way"
    );
}

// With recovery switched off the step still runs its body, and nothing is
// handed to a driver that is not there.
#[test]
fn without_a_recovery_directory_the_validator_runs_its_body_directly() {
    let fx = Fixture::new();
    fx.with_remote();
    fx.push();
    fx.gh_pr_at(&fx.head(), green_rollup_for_tests());
    fx.stub_step_recovery_driver();
    let envs = decided(&fx, "EXISTING_OPEN_PR", "7");

    let outcome = fx.run_step(&validator_step_command(), &as_refs(&envs));

    assert!(outcome.success(), "{}", outcome.describe());
    assert_eq!(outcome.field("terminal_state"), "EXISTING_OPEN_PR");
    assert_eq!(outcome.field("pr_number"), "7");
    assert!(
        fx.driver_calls().is_empty(),
        "with no recovery directory nothing is handed to the driver: {:?}",
        fx.driver_calls()
    );
}

/// Every terminal state finalization itself decides, as `assess-finalization`
/// may return it. The deterministic blockers (`FAILED_DIRTY_WORKTREE`,
/// `FAILED_MISSING_TOOLING`) and the states other producers own are not here:
/// no assessment reaches them.
const DECIDABLE: [(&str, bool); 11] = [
    ("EXISTING_OPEN_PR", true),
    ("IMPLEMENTED_VERIFIED", true),
    ("ALLOW_NO_OP", true),
    ("NO_DIFF_SUCCESS", true),
    ("BLOCKED_CI", false),
    ("FAILED_IMPLEMENTATION", false),
    ("FAILED_REPORTING", false),
    ("HOLLOW_SUCCESS", false),
    ("FAILED_MEANINGFUL_DIFF", false),
    ("FAILED_CLOSED_UNMERGED", false),
    ("FAILED_PR_METADATA_UNAVAILABLE", false),
];

// The validator and `workflow-complete` must agree on the vocabulary, and
// this used to be asserted by parsing the predicate bodies out of the shell
// source. Asserted here by running both: every state the validator publishes
// is reported by the completion step and appears in the vocabulary it
// publishes, and the states it calls a success are exactly the four.
#[test]
fn every_state_the_validator_publishes_is_one_workflow_complete_reports() {
    for (state, is_success) in DECIDABLE {
        let fx = Fixture::new();
        fx.with_remote();
        fx.push();
        fx.gh_pr_at(&fx.head(), green_rollup_for_tests());

        let validated = fx.validate(&as_refs(&decided(&fx, state, "7")));

        assert_eq!(
            validated.field("terminal_state"),
            state,
            "the validator must report the state the assessment decided\n{}",
            validated.describe()
        );
        assert_eq!(
            validated.field("terminal_success"),
            is_success.to_string(),
            "{state}: the published vocabulary decides whether it is a success\n{}",
            validated.describe()
        );
        assert_eq!(validated.success(), is_success, "{}", validated.describe());

        let completed = fx.run_dispatcher(
            "complete",
            &[("RECIPE_VAR_workflow_result", validated.stdout.trim())],
        );

        assert!(completed.success(), "{state}: {}", completed.describe());
        let envelope = completed.json();
        assert_eq!(
            envelope["terminal_outcome"], state,
            "{state}: completion must carry the state through: {envelope}"
        );
        let vocabulary = envelope["terminal_vocabulary"]
            .as_array()
            .expect("terminal_vocabulary is a list");
        assert!(
            vocabulary.iter().any(|known| known == state),
            "`{state}` is reported but is not in the vocabulary workflow-complete publishes: \
             {vocabulary:?}"
        );
    }
}

// A state outside that vocabulary is no decision at all, whatever it looks
// like, and the run goes to recovery rather than publishing it.
#[test]
fn a_state_the_vocabulary_does_not_carry_is_never_published() {
    for invented in ["MERGED_BY_HAND", "EXISTING_OPEN_PR_", "ok", ""] {
        let fx = Fixture::new();
        fx.with_remote();
        fx.push();
        fx.gh_pr_at(&fx.head(), green_rollup_for_tests());

        let outcome = fx.validate(&as_refs(&decided(&fx, invented, "7")));

        assert!(!outcome.success(), "`{invented}`: {}", outcome.describe());
        assert_eq!(
            outcome.field("terminal_state"),
            "FAILED_MISSING_TERMINAL_EVIDENCE",
            "`{invented}` must not be published\n{}",
            outcome.describe()
        );
    }
}
