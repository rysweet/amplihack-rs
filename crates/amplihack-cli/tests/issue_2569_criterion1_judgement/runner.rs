//! The four behaviour cases #2569 names, each run as one merge round
//! (`autodrive-merge-round.yaml`) through the pinned recipe runner. The
//! measurement and the bash steps are real; every agent step is the fixture
//! agent below, and the criterion-1 judgement is the world's fake agent.

use super::assert_staging_rule;
use crate::support::criterion1_world::{
    World, bundle_tools, field, read_json, repo_root, show, write_exec,
};
use serde_json::{Value, json};
use std::fs;

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
# The first JSON object in the prompt that carries the named field.
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
    *'Observe PR #'*) jq -cn --arg head "$(head_sha)" '{base:"main",head_sha:$head,synced:"true"}' ;;
    *'Judge merge-ready criterion 1 for PR'*)
      evidence qa_status > "$FIXTURE/rendered-measurement.json" || :
      AUTODRIVE_QA_EVIDENCE="$FIXTURE/rendered-measurement.json" bash "$QA_AGENT" > "$FIXTURE/judgement.json"
      cat "$FIXTURE/judgement.json"
      ;;
    *'Observe every CI check'*) jq -cn --arg head "$(head_sha)" '{ci_status:"GREEN",head_sha:$head}' ;;
    *'Collect complete platform observations'*) echo '{"approval_status":"MET"}' ;;
    *'Assess what the reviewer changed'*) echo 'The review made no edits.' ;;
    *'did the assessment conclude ready'*)
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
      evidence qa_judgement > "$FIXTURE/readiness-judgement.json" || :
      handoff="$(jq -r '.qa_handoff | select(type == "object" and .choice == "handoff") | "Conclude hand-off for \(.owner): \(.needs)"' "$FIXTURE/readiness-judgement.json")"
      if [[ -n "$handoff" ]]; then
        printf '%s\n' "$handoff"
      elif [[ "$(jq -r .qa_judgement "$FIXTURE/readiness-judgement.json")" == not_user_facing ]]; then
        printf 'Conclude ready at %s.\n' "$(head_sha)"
      else
        echo 'Conclude not ready. The judgement changed the head; assess it again.'
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

/// One merge round of PR #7 against `main`, through the pinned runner.
fn run_round(w: &World, plan: &str) -> Value {
    let agent = w.bin.join("fixture-agent");
    let launcher = w.bin.join("fixture-launcher");
    write_exec(&agent, AGENT);
    write_exec(&launcher, "#!/bin/sh\nexec \"$@\"\n");
    let context = w.base.join("context.json");
    let inputs = json!({
        "repo_path": w.repo,
        "pr_number": "7",
        "autodrive_base_branch": "main",
        "autodrive_round_label": "qa-round",
        "autodrive_qa_evidence": w.evidence_path(),
        "autodrive_state_dir": w.state,
        "autodrive_launch_tools": bundle_tools(),
        "autodrive_round_record": w.state.join("merge-ready-qa-round.json")
    });
    fs::write(&context, inputs.to_string()).expect("write recipe context");
    let recipes = repo_root().join("amplifier-bundle/recipes");
    let output = w
        .cmd(crate::pinned_runner::pinned_runner(&repo_root()))
        .current_dir(&w.repo)
        .arg(recipes.join("autodrive-merge-round.yaml"))
        .arg("--working-dir")
        .arg(&w.repo)
        .args([
            "--no-update-check",
            "--no-auto-stage",
            "--output-format",
            "json",
        ])
        .arg("--context-file")
        .arg(&context)
        .arg("--recipe-dir")
        .arg(&recipes)
        .env("AMPLIHACK_AGENT_BINARY", &agent)
        .env("AMPLIHACK_LAUNCHER_BINARY", &launcher)
        .env("REPO_PATH", &w.repo)
        .env("FIXTURE", &w.base)
        .env("QA_AGENT", &w.agent)
        .env("AUTODRIVE_LAUNCH_TOOLS", bundle_tools())
        .env("GADUGI_PLAN", plan)
        .env("AMPLIHACK_WORK_ROOT", &w.base)
        .env("AMPLIHACK_WORK_ROOT_EXPLICIT", "1")
        .env("AMPLIHACK_RUNTIME_ROOT", w.base.join("runtime"))
        .output()
        .expect("run the pinned recipe runner");
    let result: Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("runner result: {error}\n{}", show(&output)));
    assert_eq!(result["success"], true, "{}", show(&output));
    result
}

/// The judgement the QA step recorded, after checking that it reached the
/// readiness assessment and that the round persisted `verdict`.
fn judgement_after_round(w: &World, verdict: &str) -> Value {
    let measurement = read_json(&w.base.join("rendered-measurement.json"));
    assert!(
        measurement.get("qa_status").is_some(),
        "the QA step received the measurement: {measurement}"
    );
    let judgement = read_json(&w.base.join("judgement.json"));
    assert_eq!(
        read_json(&w.base.join("readiness-judgement.json")),
        judgement,
        "the readiness assessment read the recorded judgement"
    );
    assert_eq!(
        read_json(&w.state.join("merge-ready-qa-round.json"))["merge_ready_verdict"],
        verdict,
        "persisted round verdict"
    );
    judgement
}

/// A documentation-only PR in a Node repository, the case that stalled
/// drives on 12 and 13 November, is judged not user-facing with a reason
/// citing files in its diff, runs no scenario and reaches readiness.
#[test]
fn a_docs_only_pr_in_a_node_repository_passes_with_a_recorded_reason() {
    let w = World::new();
    w.node_repo(&[]);
    w.docs_pr();
    run_round(&w, "pass");
    let judgement = judgement_after_round(&w, "MERGE_READY");
    assert_eq!(judgement["qa_judgement"], "not_user_facing");
    let cited: Vec<String> =
        serde_json::from_value(judgement["qa_cited_files"].clone()).expect("cited files");
    assert!(!cited.is_empty(), "the reason cites its files: {judgement}");
    let diff = w.diff_files();
    for f in &cited {
        assert!(
            diff.contains(f),
            "{f} is cited but not in the diff {diff:?}"
        );
    }
    assert!(field(&judgement, "qa_judgement_notes").contains("docs/install.md"));
    assert!(w.gadugi_calls().is_empty(), "{:?}", w.gadugi_calls());
}

/// A feature PR with no scenario is not held for having none: the judgement
/// writes one, commits it to the PR branch and runs it, and the round returns
/// to the loop to assess the new head.
#[test]
fn a_feature_pr_without_scenarios_gets_one_written_and_run() {
    let w = World::new();
    w.node_repo(&[]);
    let measured_head = w.feature_pr();
    run_round(&w, "pass");
    let judgement = judgement_after_round(&w, "NOT_MERGE_READY");
    let run = &judgement["qa_scenario_runs"][0];
    assert_eq!(judgement["qa_judgement"], "user_facing");
    assert_eq!(run["written_in_pr"], true);
    assert_eq!(run["result"], "pass");
    let scenario = field(run, "path");
    assert!(
        w.diff_files().contains(&scenario),
        "{scenario} is not committed"
    );
    assert_ne!(w.git(&["rev-parse", "HEAD"]), measured_head);
    assert_eq!(
        w.git(&["status", "--porcelain", "--untracked-files=all"]),
        ""
    );
    assert_staging_rule(&w.gadugi_calls());
    assert!(
        w.base.join("repair.prompt").is_file(),
        "the round went on to repair rather than stopping"
    );
}

/// A failing declared command is evidence the judgement reads, not the
/// verdict, and a failing scenario loops to a committed product fix and a
/// passing run without the scenario being weakened.
#[test]
fn a_failing_scenario_loops_to_a_fix() {
    let w = World::new();
    w.node_repo(&[(".amplihack/qa.toml", "command = \"npm test\"\n")]);
    w.feature_pr();
    w.declared_command_fails();
    run_round(&w, "fail-then-pass");
    let judgement = judgement_after_round(&w, "NOT_MERGE_READY");
    let measurement = read_json(&w.base.join("rendered-measurement.json"));
    assert_eq!(measurement["qa_reason"], "qa-command-failed");
    assert_eq!(judgement["qa_judgement"], "user_facing");
    let run = &judgement["qa_scenario_runs"][0];
    assert_eq!(run["attempts"], 2);
    assert_eq!(run["result"], "pass");
    assert!(field(&judgement, "qa_judgement_notes").contains("cause product"));
    let log = w.git(&["log", "--format=%s", "main..HEAD"]);
    assert!(log.lines().any(|l| l.starts_with("fix:")), "{log}");
    let scenario = w.git(&["show", &format!("HEAD:{}", field(run, "path"))]);
    assert!(
        scenario.contains("expect: Order placed"),
        "the scenario was weakened: {scenario}"
    );
    assert_staging_rule(&w.gadugi_calls());
}

/// A scenario that needs a service the drive cannot reach ends in a recorded
/// hand-off with the #2539 recovery fields. The round persists `HANDOFF`,
/// skips repair, and the loop does not finalise.
#[test]
fn an_unreachable_service_ends_in_a_recorded_handoff() {
    let w = World::new();
    w.node_repo(&[]);
    w.feature_pr();
    run_round(&w, "unreachable");
    let judgement = judgement_after_round(&w, "HANDOFF");
    let h = &judgement["qa_handoff"];
    assert_eq!(h["choice"], "handoff");
    for key in ["step_id", "failure", "reason", "owner", "needs"] {
        assert!(
            !field(h, key).trim().is_empty(),
            "hand-off field {key} is empty: {h}"
        );
    }
    assert_eq!(h["owner"], OWNER);
    assert!(field(&judgement, "qa_judgement_notes").contains("cause environment"));
    let writeup = fs::read_to_string(w.state.join("merge-ready-qa-round.writeup")).unwrap();
    assert!(
        writeup.contains(OWNER) && writeup.contains(NEEDS),
        "{writeup}"
    );
    assert!(!w.base.join("repair.prompt").exists());
    assert_no_finalization_after_handoff(&w);
}

/// Feed the round's persisted verdict through the shipped loop-outcome
/// selector and finalisation step: a hand-off is never finalised.
fn assert_no_finalization_after_handoff(w: &World) {
    let recipes = repo_root().join("amplifier-bundle/recipes");
    let mut recipe: serde_yaml::Value = serde_yaml::from_str(
        &fs::read_to_string(recipes.join("autodrive-merge-loop.yaml")).unwrap(),
    )
    .unwrap();
    recipe["steps"].as_sequence_mut().unwrap().retain(|step| {
        matches!(
            step["id"].as_str(),
            Some("step-02a-loop-outcome" | "step-03-finalize")
        )
    });
    let path = w.base.join("loop-outcome.yaml");
    fs::write(&path, serde_yaml::to_string(&recipe).unwrap()).unwrap();
    let round = read_json(&w.state.join("merge-ready-qa-round.json"));
    let context = w.base.join("loop-context.json");
    fs::write(
        &context,
        json!({
            "repo_path": w.repo,
            "merge_loop_preflight": {"should_run":"true","repo":w.repo,"pr":"7","state_dir":w.state},
            "merge_loop_result": json!({"loop_result":round["merge_ready_verdict"]}).to_string()
        })
        .to_string(),
    )
    .unwrap();
    let output = w
        .cmd(crate::pinned_runner::pinned_runner(&repo_root()))
        .current_dir(&w.repo)
        .arg(&path)
        .arg("--working-dir")
        .arg(&w.repo)
        .args([
            "--no-update-check",
            "--no-auto-stage",
            "--output-format",
            "json",
        ])
        .arg("--context-file")
        .arg(&context)
        .arg("--recipe-dir")
        .arg(&recipes)
        .env("AMPLIHACK_AGENT_BINARY", w.bin.join("fixture-agent"))
        .env("AMPLIHACK_LAUNCHER_BINARY", w.bin.join("fixture-launcher"))
        .env("FIXTURE", &w.base)
        .env("AMPLIHACK_WORK_ROOT", &w.base)
        .env("AMPLIHACK_WORK_ROOT_EXPLICIT", "1")
        .env("AMPLIHACK_RUNTIME_ROOT", w.base.join("runtime"))
        .output()
        .expect("run the loop outcome");
    let result: Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("runner result: {error}\n{}", show(&output)));
    assert_eq!(result["success"], true, "{}", show(&output));
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
