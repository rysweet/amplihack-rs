//! Pinned-runner coverage for the QA inputs, judgement, and round outcome.
//! The agent performs fixture actions and records the evidence it received.

use super::{assert_staging_rule, divergent_default_world, field, read_json};
use crate::support::criterion1_world::{
    World, bundle_guidance, bundle_tools, private_dir, repo_root, show, write_exec,
};
use serde_json::{Value, json};
use std::{fs, path::Path};

const QA_TOML: &str = "command = \"npm test\"\nscenarios_ci_job = \"Checkout QA\"\n";
const OWNER: &str = "repository maintainer";
const NEEDS: &str = "restore the payment sandbox";

const AGENT: &str = r#"#!/usr/bin/env bash
set -euo pipefail
if [[ "${1:-}" == --version ]]; then echo 'fixture agent'; exit; fi
prompt=""; out=""
while (( $# )); do
  case "$1" in
    -p) prompt="$2"; shift ;;
    --output-last-message) out="$2"; shift ;;
  esac
  shift
done
file="$(sed -n 's/^.*instructions in the file \(.*\)\. Its contents.*$/\1/p' <<< "$prompt")"
if [[ -n "$file" && -f "$file" ]]; then prompt="$(cat -- "$file")"; fi
capture() { printf '%s\n' "$prompt" > "$FIXTURE/$1.prompt"; }
# Extract a runtime JSON object embedded in the prompt. Only the named field
# identifies it, so fixture behavior does not depend on JSON key order.
evidence() {
  local line suffix prefix candidate
  while IFS= read -r line; do
    [[ "$line" == *\"$1\"* && "$line" == *\{*\}* ]] || continue
    suffix="$line"
    while [[ "$suffix" == *\{*\}* ]]; do
      suffix="${suffix#*\{}"
      prefix="$suffix"
      while [[ "$prefix" == *\}* ]]; do
        prefix="${prefix%\}*}"
        candidate="{$prefix}"
        if jq -e --arg key "$1" 'type == "object" and has($key)' <<< "$candidate" >/dev/null 2>&1; then
          printf '%s\n' "$candidate"
          return
        fi
      done
    done
  done <<< "$prompt"
  return 1
}
head_sha() { git rev-parse HEAD; }
answer() {
  case "$prompt" in
    *'# Small change'*)
      head_sha > "$FIXTURE/implementation-start"
      printf '\nInstall with npm.\n' >> docs/install.md
      echo 'Updated docs/install.md.'
      ;;
    *'Judge merge-ready criterion 1'*'before the PR exists'*)
      capture light-qa
      evidence qa_status > "$FIXTURE/light-measurement.json"
      echo '{"qa_judgement":"not_user_facing","qa_judgement_notes":"docs/install.md is prose only.","qa_handoff":null}'
      ;;
    *'Complete responsibility light-open-pr'*)
      # Act on the publication command the shipped agent actually receives.
      # Model the agent looking up the runner's current worktree context.
      source "$FIXTURE/light-context.env"
      sed -n '/^```bash$/,/^```$/{ /^```/d; p; }' <<< "$prompt" > "$FIXTURE/open-pr.sh"
      bash -euo pipefail "$FIXTURE/open-pr.sh"
      ;;
    *'Observe PR #'*) jq -cn --arg head "$(head_sha)" --arg base "$TARGET" '{base:$base,head_sha:$head,synced:"true"}' ;;
    *'Judge merge-ready criterion 1 for PR'*)
      capture qa
      evidence qa_status > "$FIXTURE/rendered-measurement.json" || :
      AUTODRIVE_QA_EVIDENCE="$FIXTURE/rendered-measurement.json" bash "$QA_AGENT" |
        jq '{qa_judgement, qa_judgement_notes, qa_handoff, qa_cited_files, qa_blockers, qa_scenario_runs, qa_fallback_replaced}' > "$FIXTURE/judgement.json"
      cat "$FIXTURE/judgement.json"
      ;;
    *'Observe every CI check'*)
      capture ci
      evidence qa_status > "$FIXTURE/ci-measurement.json" || :
      jq -cn --arg head "$(head_sha)" '{ci_status:"GREEN",head_sha:$head}'
      ;;
    *'Collect complete platform observations'*) echo '{"approval_status":"MET"}' ;;
    *'Assess what the reviewer changed'*) echo 'The review made no edits.' ;;
    *'did the assessment conclude ready'*)
      capture outcome
      writeup="$(sed -n '/^<<<UNTRUSTED-MERGE-READY-WRITEUP$/,/^UNTRUSTED-MERGE-READY-WRITEUP>>>$/{ /^<<<\|^UNTRUSTED-/d; p; }' <<< "$prompt")"
      printf 'ci: pass\n'
      case "$writeup" in
        *'Conclude hand-off for '*) echo hand-off ;;
        *'Conclude ready at '*) echo ready ;;
        *) echo not-ready ;;
      esac
      ;;
    *'Clear the merge-ready blockers'*) capture repair; echo 'Continue with a fresh measurement.' ;;
    *'# merge-ready assessment'*)
      capture readiness
      evidence qa_judgement > "$FIXTURE/readiness-judgement.json" || :
      evidence qa_status > "$FIXTURE/readiness-measurement.json" || :
      handoff="$(jq -r '.qa_handoff | select(type == "object" and .choice == "handoff") | "Conclude hand-off for \(.owner): \(.needs)"' "$FIXTURE/readiness-judgement.json")"
      if [[ -n "$handoff" ]]; then
        printf '%s\n' "$handoff"
      elif [[ "$(jq -r .head_sha "$FIXTURE/readiness-measurement.json")" == "$(head_sha)" ]] &&
           [[ "$(jq -r .qa_judgement "$FIXTURE/readiness-judgement.json")" == not_user_facing ]]; then
        printf 'Conclude ready at %s.\n' "$(head_sha)"
      else
        echo 'Conclude not ready. Assess the current head with applicable QA evidence.'
      fi
      ;;
    *'crusty_status'*'Crusty'*) echo '{"crusty_status":"DONE_CLEAN"}' ;;
    *) echo 'Unexpected agent invocation' >&2; return 90 ;;
  esac
}
if [[ -z "$out" ]]; then
  answer
else
  answer > "$out"
  cat "$out"
fi
"#;

fn run(
    w: &World,
    recipe: &Path,
    target: &str,
    plan: &str,
    context: &Value,
    env: &[(&str, &str)],
) -> Value {
    let agent = w.bin.join("fixture-agent");
    let launcher = w.bin.join("fixture-launcher");
    write_exec(&agent, AGENT);
    write_exec(&launcher, "#!/bin/sh\nexec \"$@\"\n");
    let inputs = w.base.join("context.json");
    fs::write(&inputs, context.to_string()).expect("write recipe context");
    let runner = crate::pinned_runner::pinned_runner(&repo_root());
    let mut command = w.cmd(runner);
    command
        .current_dir(&w.repo)
        .arg(recipe)
        .arg("--working-dir")
        .arg(&w.repo)
        .args([
            "--no-update-check",
            "--no-auto-stage",
            "--output-format",
            "json",
        ])
        .arg("--context-file")
        .arg(inputs)
        .arg("--recipe-dir")
        .arg(repo_root().join("amplifier-bundle/recipes"))
        .env("AMPLIHACK_AGENT_BINARY", agent)
        .env("AMPLIHACK_LAUNCHER_BINARY", launcher)
        .env("REPO_PATH", &w.repo)
        .env("FIXTURE", &w.base)
        .env("TARGET", target)
        .env("QA_AGENT", &w.agent)
        .env("AUTODRIVE_QA_GUIDANCE", bundle_guidance())
        .env("AUTODRIVE_LAUNCH_TOOLS", bundle_tools())
        .env("GADUGI_PLAN", plan)
        .env("AMPLIHACK_WORK_ROOT", &w.base)
        .env("AMPLIHACK_WORK_ROOT_EXPLICIT", "1")
        .env("AMPLIHACK_RUNTIME_ROOT", w.base.join("runtime"))
        .envs(env.iter().copied());
    let output = command.output().expect("run the pinned recipe runner");
    let result: Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("runner result: {error}\n{}", show(&output)));
    assert_eq!(result["success"], true, "{}", show(&output));
    result
}

fn run_round(w: &World, target: &str, plan: &str, extra: &[(&str, &str)]) -> Value {
    let context = json!({
        "repo_path": w.repo,
        "pr_number": "7",
        "autodrive_base_branch": target,
        "autodrive_round_label": "qa-round",
        "autodrive_qa_evidence": w.evidence_path(),
        "autodrive_state_dir": w.state,
        "autodrive_launch_tools": bundle_tools(),
        "autodrive_round_record": w.state.join("merge-ready-qa-round.json")
    });
    fs::remove_file(w.bin.join("jq")).expect("use real jq for the round's storage");
    run(
        w,
        &repo_root().join("amplifier-bundle/recipes/autodrive-merge-round.yaml"),
        target,
        plan,
        &context,
        extra,
    )
}

fn assert_round(w: &World, verdict: &str) -> Value {
    let read = |name| read_json(&w.base.join(name));
    let measurement = read("rendered-measurement.json");
    let saved = read_json(&w.evidence_path());
    for field in ["head_sha", "qa_round", "qa_base", "qa_status"] {
        assert_eq!(measurement[field], saved[field], "QA agent's {field}");
    }
    assert_eq!(
        read("readiness-measurement.json"),
        measurement,
        "readiness measurement"
    );
    assert_eq!(read("ci-measurement.json"), measurement, "CI measurement");
    let judgement = read("judgement.json");
    assert!(judgement.get("qa_status").is_none());
    assert_eq!(
        read("readiness-judgement.json"),
        judgement,
        "readiness judgement"
    );
    assert_eq!(
        read_json(&w.state.join("merge-ready-qa-round.json"))["merge_ready_verdict"],
        verdict,
        "persisted round verdict"
    );
    judgement
}

fn assert_no_finalization_after_handoff(w: &World) {
    let recipes = repo_root().join("amplifier-bundle/recipes");
    let mut recipe: serde_yaml::Value = serde_yaml::from_str(
        &fs::read_to_string(recipes.join("autodrive-merge-loop.yaml")).unwrap(),
    )
    .unwrap();
    // The driver also has its own HANDOFF coverage. Feed the round's actual
    // persisted verdict through the shipped finalization selector and step.
    recipe["steps"].as_sequence_mut().unwrap().retain(|step| {
        matches!(
            step["id"].as_str(),
            Some("step-02a-loop-outcome" | "step-03-finalize")
        )
    });
    let path = w.base.join("loop-outcome.yaml");
    fs::write(&path, serde_yaml::to_string(&recipe).unwrap()).unwrap();
    let round = read_json(&w.state.join("merge-ready-qa-round.json"));
    let result = run(
        w,
        &path,
        "main",
        "unreachable",
        &json!({
            "repo_path": w.repo,
            "merge_loop_preflight": {"should_run":"true","repo":w.repo,"pr":"7","state_dir":w.state},
            "merge_loop_result": json!({"loop_result":round["merge_ready_verdict"]}).to_string()
        }),
        &[],
    );
    assert_eq!(
        result["context"]["merge_loop_outcome"]["outcome"],
        "handoff"
    );
    assert_eq!(result["context"]["merge_loop_outcome"]["finalise"], "false");
    let steps = result["step_results"].as_array().unwrap();
    assert!(
        steps
            .iter()
            .any(|step| step["step_id"] == "step-03-finalize" && step["status"] == "skipped")
    );
}

#[test]
fn a_silent_invocation_does_not_supply_an_earlier_rounds_declaration_to_ci() {
    let w = World::new();
    w.node_repo(&[(".amplihack/qa.toml", QA_TOML)]);
    let old_head = w.feature_pr();
    let prior = w.record(&[]);
    assert_eq!(prior["qa_criterion1_declaration"], "ci-job:Checkout QA");
    assert_eq!(prior["head_sha"], old_head);
    let prior_bytes = fs::read(w.evidence_path()).unwrap();
    w.write("src/app.js", "console.log('different head');\n");
    let new_head = w.commit("advance reviewed head");
    let tools = w.private_bundle("silent", "#!/bin/sh\nexit 0\n");
    fs::remove_file(w.bin.join("jq")).unwrap();

    let recipes = repo_root().join("amplifier-bundle/recipes");
    run(
        &w,
        &recipes.join("autodrive-merge-evidence.yaml"),
        "main",
        "pass",
        &json!({
            "repo_path": w.repo,
            "pr_number": "7",
            "autodrive_base_branch": "main",
            "autodrive_round_label": "new-round",
            "autodrive_qa_evidence": w.evidence_path(),
            "autodrive_tools_dir": tools,
        }),
        &[],
    );
    assert_ne!(old_head, new_head);
    assert_eq!(fs::read(w.evidence_path()).unwrap(), prior_bytes);
    let measurement = read_json(&w.base.join("ci-measurement.json"));
    assert_eq!(measurement["qa_reason"], "qa-measurement-unavailable");
    let prompt = fs::read_to_string(w.base.join("ci.prompt")).unwrap();
    assert!(!prompt.contains("ci-job:Checkout QA"));
}

#[test]
fn the_runner_carries_opaque_measurement_text_to_both_qa_and_ci() {
    let w = World::new();
    w.node_repo(&[]);
    w.docs_pr();
    let raw = "{qa fixture missing a quote}\n  second line of measured text";
    let tools = w.private_bundle("opaque", &format!("#!/bin/sh\nprintf '%s\\n' '{raw}'\n"));
    fs::remove_file(w.bin.join("jq")).unwrap();
    let result = run(
        &w,
        &repo_root().join("amplifier-bundle/recipes/autodrive-merge-evidence.yaml"),
        "main",
        "pass",
        &json!({
            "repo_path": w.repo,
            "pr_number": "7",
            "autodrive_base_branch": "main",
            "autodrive_qa_evidence": w.evidence_path(),
            "autodrive_tools_dir": tools,
        }),
        &[],
    );
    assert_eq!(
        result["context"]["qa_inputs"].as_str().map(str::trim_end),
        Some(raw)
    );
    for name in ["qa.prompt", "ci.prompt"] {
        let rendered = fs::read_to_string(w.base.join(name)).expect("agent received prompt");
        assert!(rendered.contains(raw), "{name} lost the measurement text");
    }
}

#[test]
fn a_node_docs_change_reaches_readiness_with_its_file_backed_reason() {
    let w = divergent_default_world();
    run_round(&w, "main", "pass", &[]);
    let judgement = assert_round(&w, "MERGE_READY");
    assert_eq!(judgement["qa_judgement"], "not_user_facing");
    assert_eq!(judgement["qa_cited_files"], json!(["docs/install.md"]));
    assert!(field(&judgement, "qa_judgement_notes").contains("docs/install.md"));
    assert!(w.gadugi_calls().is_empty());
}

#[test]
fn a_behaviour_neutral_configuration_change_can_earn_a_file_backed_exemption() {
    let w = World::new();
    w.node_repo(&[]);
    w.write(".editorconfig", "root = true\n\n[*]\nindent_size = 2\n");
    w.commit("set editor whitespace preferences");
    run_round(
        &w,
        "main",
        "pass",
        &[("AGENT_JUDGEMENT", "not_user_facing")],
    );
    let judgement = assert_round(&w, "MERGE_READY");
    assert_eq!(read_json(&w.evidence_path())["docs_only"], "false");
    assert_eq!(judgement["qa_judgement"], "not_user_facing");
    assert_eq!(judgement["qa_cited_files"], json!([".editorconfig"]));
    assert!(field(&judgement, "qa_judgement_notes").contains("editor formatting preferences"));
    assert!(w.gadugi_calls().is_empty());
}

#[test]
fn a_feature_with_no_scenario_writes_and_exercises_one_in_the_round() {
    let w = World::new();
    w.node_repo(&[]);
    let measured_head = w.feature_pr();
    run_round(&w, "main", "pass", &[]);
    let judgement = assert_round(&w, "NOT_MERGE_READY");
    let scenario = "tests/agentic/checkout.yaml";
    assert_eq!(judgement["qa_scenario_runs"][0]["written_in_pr"], true);
    assert_eq!(judgement["qa_scenario_runs"][0]["result"], "pass");
    assert_eq!(judgement["qa_scenario_runs"][0]["path"], scenario);
    assert!(w.diff_files().contains(scenario));
    assert_staging_rule(&w.gadugi_calls());
    assert!(
        w.base.join("repair.prompt").is_file(),
        "the changed head needs reassessment"
    );
    assert_eq!(read_json(&w.evidence_path())["head_sha"], measured_head);
    assert_ne!(w.git(&["rev-parse", "HEAD"]), measured_head);
}

#[test]
fn a_failed_declared_command_reaches_qa_and_a_scenario_failure_gets_a_passing_rerun() {
    let w = World::new();
    w.node_repo(&[(".amplihack/qa.toml", QA_TOML)]);
    let measured_head = w.feature_pr();
    w.declared_command_fails();
    run_round(&w, "main", "fail-then-pass", &[]);
    let judgement = assert_round(&w, "NOT_MERGE_READY");
    let measurement = read_json(&w.base.join("rendered-measurement.json"));
    assert_eq!(measurement["qa_reason"], "qa-command-failed");
    assert_eq!(measurement["qa_exit_code"], "1");
    assert_eq!(judgement["qa_scenario_runs"][0]["attempts"], 2);
    assert_eq!(judgement["qa_scenario_runs"][0]["result"], "pass");
    assert!(field(&judgement, "qa_judgement_notes").contains("cause product"));
    assert_eq!(
        w.git(&["show", "HEAD:src/app.js"]),
        "const total = 3;\nif (process.argv[2] === \"checkout\") console.log(\"Order placed: \" + total);"
    );
    assert_staging_rule(&w.gadugi_calls());
    let ci_prompt = fs::read_to_string(w.base.join("ci.prompt")).expect("CI agent ran");
    assert!(
        ci_prompt.contains(&measured_head),
        "CI needs the older measurement's observed head to assess applicability"
    );
}

#[test]
fn an_unreachable_service_persists_the_qa_handoff_and_skips_round_repair() {
    let w = World::new();
    w.node_repo(&[]);
    w.feature_pr();
    run_round(&w, "main", "unreachable", &[]);
    let judgement = assert_round(&w, "HANDOFF");
    assert_eq!(judgement["qa_handoff"]["owner"], OWNER);
    assert!(field(&judgement["qa_handoff"], "needs").contains(NEEDS));
    let writeup = fs::read_to_string(w.state.join("merge-ready-qa-round.writeup")).unwrap();
    assert!(
        writeup.contains(OWNER) && writeup.contains(NEEDS),
        "{writeup}"
    );
    assert!(!w.base.join("repair.prompt").exists());
    assert_staging_rule(&w.gadugi_calls());
    assert_no_finalization_after_handoff(&w);
}

#[test]
fn a_missing_operator_directory_reaches_qa_and_uses_the_tracked_fallback() {
    let w = World::new();
    w.node_repo(&[("scenarios/other.yaml", "name: unrelated\n")]);
    w.feature_pr();
    run_round(
        &w,
        "main",
        "pass",
        &[("AUTODRIVE_QA_SCENARIO_DIR", "operator-scenarios")],
    );
    let judgement = assert_round(&w, "NOT_MERGE_READY");
    assert_eq!(judgement["qa_fallback_replaced"], "operator-scenarios");
    assert_eq!(
        judgement["qa_scenario_runs"][0]["path"],
        "scenarios/checkout.yaml"
    );
    assert_eq!(judgement["qa_scenario_runs"][0]["result"], "pass");
    assert_staging_rule(&w.gadugi_calls());
}

#[test]
fn a_new_light_pr_starts_measures_and_opens_against_the_requested_target() {
    let w = World::new();
    w.node_repo(&[]);
    w.git(&["config", "--local", "user.name", "QA Fixture"]);
    w.git(&["config", "--local", "user.email", "qa@example.test"]);
    w.git(&["checkout", "-q", "-b", "release"]);
    w.write(
        "release.txt",
        "Only the release branch starts with this file.\n",
    );
    let release = w.commit("release starting point");
    w.git(&["checkout", "-q", "main"]);
    let origin = w.base.join("origin.git");
    w.git(&[
        "init",
        "--bare",
        "-q",
        "-b",
        "main",
        origin.to_str().unwrap(),
    ]);
    w.git(&["remote", "add", "origin", origin.to_str().unwrap()]);
    w.git(&["push", "-q", "origin", "main", "release"]);
    w.git(&["remote", "set-head", "origin", "main"]);
    fs::remove_file(w.bin.join("jq")).expect("use real jq for worktree setup");
    write_exec(
        &w.bin.join("gh"),
        r#"#!/usr/bin/env bash
set -eu
[[ "$1 $2" == 'pr create' ]]
printf '%s\n' "$@" > "$FIXTURE/pr-create.args"
printf '%s\n' 'https://github.com/example/shop/pull/7'
"#,
    );

    // Stop after publication. The next light steps would re-enter the
    // existing-PR review loops, which the round tests drive separately.
    let recipes = repo_root().join("amplifier-bundle/recipes");
    let mut recipe: serde_yaml::Value =
        serde_yaml::from_str(&fs::read_to_string(recipes.join("auto-drive-light.yaml")).unwrap())
            .unwrap();
    let steps = recipe["steps"].as_sequence_mut().unwrap();
    let end = steps
        .iter()
        .position(|step| step["id"] == "light-open-pr")
        .unwrap();
    steps.truncate(end + 1);
    // Bash exposes runner context as environment variables. A recorded
    // observation lets the fake agent adapt the shell example using the
    // actual worktree setup output and pinned commit.
    steps.insert(end, serde_yaml::to_value(json!({
        "id": "observe-publication-context",
        "type": "bash",
        "command": r#"declare -p RECIPE_VAR_worktree_setup RECIPE_VAR_light_base RECIPE_VAR_base_branch RECIPE_VAR_task_description > "$FIXTURE/light-context.env""#
    })).unwrap());
    let fixture_recipes = w.base.join("recipes");
    private_dir(&fixture_recipes);
    let path = fixture_recipes.join("auto-drive-light.yaml");
    fs::write(&path, serde_yaml::to_string(&recipe).unwrap()).unwrap();

    run(
        &w,
        &path,
        "release",
        "pass",
        &json!({
            "repo_path": w.repo,
            "base_branch": "release",
            "task_description": "Explain the install command",
        }),
        &[],
    );

    let call = fs::read_to_string(w.base.join("pr-create.args")).expect("gh pr create was called");
    let args: Vec<_> = call.lines().collect();
    let publication_base = args
        .windows(2)
        .find(|pair| pair[0] == "--base")
        .map(|pair| pair[1]);
    let branch_start = fs::read_to_string(w.base.join("implementation-start")).unwrap();
    let measurement = read_json(&w.base.join("light-measurement.json"));
    assert_eq!(
        (
            publication_base,
            branch_start.trim(),
            measurement["qa_base"].as_str(),
            measurement["head_sha"].as_str()
        ),
        (
            Some("release"),
            release.as_str(),
            Some("release"),
            Some(release.as_str())
        ),
        "the published target, starting commit, and measured target and head must agree"
    );
}
