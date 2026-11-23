#!/usr/bin/env bash
# workflow_finalization_reentry.sh - one more pass of the CI repair (#2966).
#
# Sourced, never executed. Defines one function and one constant and changes
# nothing else, so it can be sourced twice. The dispatcher calls
# continue_ci_repair for its `reassess` mode, which the continue-ci-repair step
# of workflow-finalization-assessment.yaml runs.
#
# Why it exists. assess-finalization is one agent session, and a session can
# end before its work does: it crashes, it is cut off, it answers in prose, it
# says the CI repair is unfinished (CI_REPAIR_UNFINISHED), or it leaves the
# run's worktree dirty after its own commits and builds. continue_ci_repair
# starts the assessment once more, in a fresh session, and prints that
# session's answer.
#
# What it does not do. It reads nothing in the answer and decides nothing. It
# has no loop and counts nothing. The decisions are made elsewhere:
#   - whether a pass is needed: the runner, by continue-ci-repair's condition
#     (the answer names no terminal_state, or names CI_REPAIR_UNFINISHED, the
#     session's own statement that it has not decided, or
#     read-worktree-after-assessment did not find the recorded worktree clean);
#   - whether the fresh pass decided: the runner again, by the same condition on
#     reentered-pass-undecided, the step that fails the nested run when it holds;
#   - whether to start another pass after that: continue-ci-repair's recovery
#     agent. A retry runs this function again; a handoff ends the run with the
#     agent's reason on record (docs/reference/step-failure-recovery.md).
# What a session should do instead of returning something that is not a
# decision, including a BLOCKED_CI it cannot yet give an owner and a step, is
# guidance in the assessment prompt, not a check here.
#
# Exit status. 0 with the pass's answer on stdout, unread, when the nested run
# succeeded. Otherwise the nested run's own code: 75 (orphan stop), 79 (policy
# refusal) and 128+n (signal) are passed on unchanged; anything else is
# returned as 1, with one ERROR line for the recovery agent. The line says
# whether the nested run's reentered-pass-undecided step failed or the run
# failed some other way, as the runner recorded it, and carries the pass's
# answer as a JSON string, so its control characters are escaped, and the
# nested run's own worktree reading as JSON.
#
# The note the pass gets is {previous_answer, worktree}: the last answer, and
# what read-worktree-after-assessment found after it (null when it did not
# run). Both are data for the session, which reads `git status` itself.
#
# Requires workflow_context.sh and workflow_finalization_metadata.sh to be
# sourced first, as the dispatcher does. The recipe it runs is the bundle's own,
# beside this file, never one from the worktree under repair (#1543). Values
# reach the nested run through files in a private scratch directory, never argv,
# and the nested run inherits no RECIPE_VAR_* and no context file: the recipe
# declares every key it reads (#1561).

FINALIZATION_REENTRY_TOOLS_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" 2>/dev/null && pwd)"

continue_ci_repair() {
  local scratch recipe previous worktree repo_path rc=0 answer why after
  local -a ctx
  command -v jq >/dev/null 2>&1 || { echo "ERROR: #2966: continue-ci-repair requires jq" >&2; return 1; }
  recipe="${FINALIZATION_REENTRY_TOOLS_DIR%/*}/recipes/workflow-finalization-assessment.yaml"
  scratch="$(mktemp -d "${TMPDIR:-/tmp}/ci-repair-reentry.XXXXXX")" \
    || { echo "ERROR: #2966: cannot create a private scratch directory" >&2; return 1; }
  chmod 700 "$scratch"
  # The note is never empty, even when the last session left no readable
  # answer: the nested run skips continue-ci-repair because ci_repair_reentry
  # is set, so an empty note would let it start a pass of its own.
  # Each part is one JSON value or null, so the note is always one object.
  previous="$(workflow_context_read finalization_assessment 2>/dev/null | jq -cs 'if length == 1 then .[0] else null end' 2>/dev/null || :)"
  worktree="$(workflow_context_read worktree_after_assessment 2>/dev/null | jq -cs 'if length == 1 and (.[0] | type) == "object" then .[0] else null end' 2>/dev/null || :)"
  jq -nc --argjson p "${previous:-null}" --argjson w "${worktree:-null}" '{previous_answer: $p, worktree: $w}' >"$scratch/note" \
    || printf '{"previous_answer":null,"worktree":null}' >"$scratch/note"
  ctx=(--context-from-file "ci_repair_reentry=$scratch/note")
  workflow_context_read finalization_evidence >"$scratch/evidence" 2>/dev/null || : >"$scratch/evidence"
  workflow_context_read finalizer_step_status >"$scratch/step-status" 2>/dev/null || : >"$scratch/step-status"
  [ ! -s "$scratch/evidence" ] || ctx+=(--context-from-file "finalization_evidence=$scratch/evidence")
  [ ! -s "$scratch/step-status" ] || ctx+=(--context-from-file "finalizer_step_status=$scratch/step-status")
  repo_path="$(workflow_finalization_metadata repo_path || :)"
  [ -z "$repo_path" ] || ctx+=(-c "repo_path=$repo_path")
  # The nested pass reads the same recorded worktree after its session.
  workflow_context_read worktree_setup >"$scratch/worktree-setup" 2>/dev/null || : >"$scratch/worktree-setup"
  [ ! -s "$scratch/worktree-setup" ] || ctx+=(--context-from-file "worktree_setup=$scratch/worktree-setup")
  [ -z "${RECIPE_VAR_base_branch:-}" ] || ctx+=(-c "base_branch=$RECIPE_VAR_base_branch")
  echo "INFO: #2966: assess-finalization did not decide (no terminal_state, or CI_REPAIR_UNFINISHED), or left the run's worktree not clean; starting it again in a fresh session" >&2
  (
    while IFS= read -r v; do
      case "$v" in RECIPE_VAR_*|AMPLIHACK_CONTEXT_FILE) unset "$v" ;; esac
    done < <(compgen -e)
    exec amplihack recipe run "$recipe" --working-dir "$PWD" -f json "${ctx[@]}"
  ) >"$scratch/pass.json" || rc=$?
  answer="$(jq -r '[.step_results[]? | select(.step_id == "assess-finalization") | .output
                   | select(. != null) | if type == "string" then . else tojson end] | last // ""' \
    "$scratch/pass.json" 2>/dev/null || :)"
  # Which nested step failed is the runner's record, not the answer's content.
  why="the fresh assessment run failed before it reached a decision"
  if jq -e 'any(.step_results[]?; .step_id == "reentered-pass-undecided" and .status == "failed")' \
    "$scratch/pass.json" >/dev/null 2>&1; then
    why="the fresh assess-finalization session did not decide either, or left the worktree not clean"
  fi
  after="$(jq -c '[.step_results[]? | select(.step_id == "read-worktree-after-assessment") | .output
                   | select(. != null) | if type == "string" then (try fromjson catch .) else . end] | last // null' \
    "$scratch/pass.json" 2>/dev/null || :)"
  rm -rf -- "$scratch"
  case "$rc" in
    0) printf '%s\n' "$answer"; return 0 ;;
    75|79) echo "ERROR: #2966: the fresh assessment run exited $rc; passed on, never retried here" >&2; return "$rc" ;;
  esac
  if [ "$rc" -ge 128 ]; then
    echo "ERROR: #2966: the fresh assessment run was stopped by a signal (exit $rc)" >&2
    return "$rc"
  fi
  printf 'ERROR: #2966: %s (nested run exit %s); its answer, as data: %s; the worktree after it, as data: %s\n' \
    "$why" "$rc" "$(printf '%s' "$answer" | jq -Rsc '.[0:2000]')" "$(printf '%s' "${after:-null}" | LC_ALL=C cut -c1-2000)" >&2
  return 1
}
