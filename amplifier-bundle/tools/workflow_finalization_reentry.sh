#!/usr/bin/env bash
# workflow_finalization_reentry.sh - re-entering the CI repair (#2966).
#
# Sourced, never executed. Defines functions and one constant and changes
# nothing else, so it can be sourced twice. The dispatcher sources it after
# the result brick and calls continue_ci_repair for its `reassess` mode. That
# mode is run by continue-ci-repair, the step after the agent step in
# workflow-finalization-assessment.yaml, so it ends before
# validate-agentic-finalization in workflow-finalize reads the answer.
#
# Why it exists. assess-finalization is one agent session, and a session can
# end before its work does: it crashes, it is cut off, it answers in prose, or
# it stops with the CI repair unfinished (CI_REPAIR_UNFINISHED). Each of those
# used to reach the validator as no decision, and the run ended exit 81 on a
# red CI it could still fix. continue_ci_repair starts the assessment again
# instead, in a fresh session, and prints the answer once it is a decision.
#
# What it asks of an answer. One question, the validator's: would
# validate-agentic-finalization publish this as the assessment's decision?
# The tests are the validator's own predicates (_finalization_state_allowed,
# _finalization_decision_unowned, _finalization_state_names_pr with
# _finalization_pr_number_valid, and the worktree its probe reads), so the two
# cannot disagree. An answer the validator could only report as no decision
# cannot be read as one, however well-formed its JSON, so it is started again
# like an unparseable one. _reentry_gap says why in one fixed sentence, and the
# sentence and the last answer are handed to the next session as data
# (`ci_repair_reentry`). What a session should do about each reason is the
# prompt's ("When finalization starts you again" in the recipe), and whether to
# ask again after that is the recovery agent's. This file keeps no classes,
# compares nothing between passes and counts no passes. It reads no check and
# writes nothing to the repository or to GitHub.
#
# When it stops asking.
#   - A decision is printed, and the step ends.
#   - CI_REPAIR_UNFINISHED is the session's own statement that the repair goes
#     on, so it is started again every time a pass returns it.
#   - Any other answer that is not a decision is asked about once: a fresh
#     session is started with the reason. When that session's answer is again
#     not a decision, and not CI_REPAIR_UNFINISHED, the step fails with one
#     ERROR line naming both reasons, and its recovery agent judges whether
#     another pass can reach one. A retry runs this step again, which starts a
#     fresh session that reads the pull request live; a handoff ends the run
#     exit 81 with the reason on record. Nothing here ends a run, and no
#     missing sentence reaches the validator's terminal failure.
# A nested run that refuses with exit 79, stops as an orphan (75) or dies on a
# signal passes that code on, never retried here. Any other failure of the
# nested run fails the step for its recovery agent the same way.
#
# The worktree reason is narrow. It applies only to an explicitly recorded
# worktree that the evidence recorded exactly clean, so every change in it was
# written by a pass of the assessment. A tree the evidence already recorded
# dirty predates the assessment; whose work that is cannot be told here, so it
# is not sent to an agent to clean, and the validator refuses it
# (FAILED_DIRTY_WORKTREE).
#
# Requires workflow_context.sh, workflow_finalization_metadata.sh,
# workflow_finalization_pr_signal.sh (_finalization_pr_number_valid) and
# workflow_finalization_result.sh to be sourced first, as the dispatcher does.
# The recipe it re-runs is the bundle's own, beside this file, never one from
# the worktree under repair (#1543).

FINALIZATION_REENTRY_TOOLS_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" 2>/dev/null && pwd)"

# _reentry_normalize <raw> -> the five answer fields as one JSON object of
# strings, or status 1 when <raw> is not exactly one JSON object. A leaf of the
# wrong type, or a string holding NUL, is read as empty, as the validator's
# workflow_finalization_field reads it; pr_number may be a JSON number (#2819
# R3). Control characters are removed and each field is cut to 1200
# characters, so the note handed to the next session cannot carry an escape
# sequence or grow without bound.
_reentry_normalize() {
  [ -n "${1-}" ] || return 1
  printf '%s' "$1" | jq -cs '
    if length == 1 and (.[0] | type) == "object" then .[0] else error("not one object") end
    | . as $a
    | def leaf($k): $a[$k] as $v
        | if ($v | type) == "string" then (if ($v | index("\u0000")) == null then $v else "" end)
          elif ($v | type) == "boolean" then ($v | tostring)
          elif ($v | type) == "number" and $k == "pr_number" then ($v | tostring)
          else "" end
        | gsub("[\u0000-\u001f\u007f]"; "") | .[0:1200];
      {terminal_state: leaf("terminal_state"), action_taken: leaf("action_taken"),
       pr_number: leaf("pr_number"), reason: leaf("reason"), next_action: leaf("next_action")}' 2>/dev/null
}

# _reentry_gap <normalized answer or empty> <worktree lines> -> one sentence
# saying why the validator would not publish the answer as the assessment's
# decision, or nothing when it would (header). The sentences are fixed text:
# none quotes the agent.
_reentry_gap() {
  local state reason next pr missing
  [ -n "${1-}" ] || { echo "it returned no answer finalization can read"; return 0; }
  state="$(printf '%s' "$1" | jq -r .terminal_state)"
  reason="$(printf '%s' "$1" | jq -r .reason)"
  next="$(printf '%s' "$1" | jq -r .next_action)"
  pr="$(printf '%s' "$1" | jq -r .pr_number)"
  if [ "$state" = CI_REPAIR_UNFINISHED ]; then
    echo "it reported CI_REPAIR_UNFINISHED, so the CI repair goes on"
  elif ! _finalization_state_allowed "$state"; then
    echo "its terminal_state is not one finalization publishes"
  elif missing="$(_finalization_decision_unowned "$state" "$reason" "$next")"; then
    echo "it reported BLOCKED_CI with $missing"
  elif _finalization_state_names_pr "$state" && ! _finalization_pr_number_valid "$pr"; then
    echo "it reported $state and named no pull request number"
  elif [ -n "${2-}" ]; then
    echo "it left changes in the recorded worktree, which was clean before the assessment ran"
  fi
}

# _reentry_dirty_lines -> the recorded worktree's `git status --porcelain`,
# printed only when it is the validator's reason to refuse a decision that a
# pass can answer (header): an explicitly recorded worktree that git reads,
# that the evidence recorded exactly clean, and that now has changes. Prints
# nothing otherwise.
_reentry_dirty_lines() {
  [ "$reentry_dir_explicit" = true ] && [ "$reentry_record_clean" = true ] || return 0
  [ "$(git -C "$reentry_dir" rev-parse --is-inside-work-tree 2>/dev/null)" = true ] || return 0
  git -C "$reentry_dir" status --porcelain 2>/dev/null | LC_ALL=C tr -d '\000-\010\013-\037\177' | head -n 40 || :
}

# _reentry_note <why> <normalized previous answer or empty> <worktree lines>
# -> the `ci_repair_reentry` object the next session reads as data.
_reentry_note() {
  jq -nc --arg why "$1" --arg prev "$2" --arg dirty "$3" '{
    reentry: $why,
    previous_answer: (if $prev == "" then null else ($prev | fromjson) end),
    worktree_status: ($dirty | split("\n") | map(select(. != "")))}'
}

# _reentry_pass <scratch> -> runs the bundle's assessment recipe once more and
# prints the raw answer of its assess-finalization step (empty when there is
# none). Returns the nested run's exit code. Values reach it through files in
# <scratch>, never argv. The nested run gets no RECIPE_VAR_* and no context
# file from this step: the recipe declares every key it reads (#1561).
_reentry_pass() {
  local scratch="$1" rc=0
  local -a ctx=(--context-from-file "ci_repair_reentry=$scratch/note")
  [ ! -s "$scratch/evidence" ] || ctx+=(--context-from-file "finalization_evidence=$scratch/evidence")
  [ ! -s "$scratch/step-status" ] || ctx+=(--context-from-file "finalizer_step_status=$scratch/step-status")
  [ -z "$reentry_repo_path" ] || ctx+=(-c "repo_path=$reentry_repo_path")
  [ -z "$reentry_base_branch" ] || ctx+=(-c "base_branch=$reentry_base_branch")
  (
    while IFS= read -r v; do
      case "$v" in RECIPE_VAR_*|AMPLIHACK_CONTEXT_FILE) unset "$v" ;; esac
    done < <(compgen -e)
    exec amplihack recipe run "$reentry_recipe" --working-dir "$PWD" -f json "${ctx[@]}"
  ) >"$scratch/pass.json" || rc=$?
  jq -r '[.step_results[]? | select(.step_id == "assess-finalization") | .output
          | select(. != null) | if type == "string" then . else tojson end] | last // ""' \
    "$scratch/pass.json" 2>/dev/null || :
  return "$rc"
}

# continue_ci_repair: the `reassess` mode. Prints one JSON answer on stdout for
# the step's `finalization_assessment` output and exits 0, or exits non-zero
# for step recovery with an ERROR line naming why (header).
continue_ci_repair() {
  local scratch="" raw answer state gap dirty asked="" rc
  local reentry_dir reentry_dir_explicit=true reentry_record_clean=false reentry_recipe
  local reentry_repo_path reentry_base_branch
  command -v jq >/dev/null 2>&1 || { echo "ERROR: #2966: continue-ci-repair requires jq" >&2; return 1; }
  reentry_recipe="${FINALIZATION_REENTRY_TOOLS_DIR%/*}/recipes/workflow-finalization-assessment.yaml"
  reentry_dir="$(workflow_finalization_metadata worktree_path || :)"
  [ -n "$reentry_dir" ] || reentry_dir="$(workflow_finalization_metadata repo_path || :)"
  [ -n "$reentry_dir" ] || { reentry_dir="."; reentry_dir_explicit=false; }
  [ "$(workflow_finalization_field finalization_evidence git.dirty_worktree FINALIZATION_EVIDENCE_GIT_DIRTY_WORKTREE || :)" != false ] \
    || reentry_record_clean=true
  reentry_repo_path="$(workflow_finalization_metadata repo_path || :)"
  reentry_base_branch="${RECIPE_VAR_base_branch:-}"
  raw="$(workflow_context_read finalization_assessment || :)"
  while :; do
    answer="$(_reentry_normalize "$raw")" || answer=""
    dirty="$(_reentry_dirty_lines)"
    gap="$(_reentry_gap "$answer" "$dirty")"
    if [ -z "$gap" ]; then
      [ -z "$scratch" ] || rm -rf -- "$scratch"
      printf '%s' "$raw" | jq -c .
      return 0
    fi
    state=""
    [ -z "$answer" ] || state="$(printf '%s' "$answer" | jq -r .terminal_state)"
    # `asked` is the reason the last pass was started for, when that was a
    # missing decision; it is empty after a pass started to continue a repair.
    if [ "$state" = CI_REPAIR_UNFINISHED ]; then
      asked=""
    elif [ -n "$asked" ]; then
      [ -z "$scratch" ] || rm -rf -- "$scratch"
      echo "ERROR: #2966: assess-finalization was started again because $asked, and the new session's answer is still not one finalization reports: $gap; step recovery judges whether another pass can reach a decision" >&2
      return 1
    else
      asked="$gap"
    fi
    if [ -z "$scratch" ]; then
      scratch="$(mktemp -d "${TMPDIR:-/tmp}/ci-repair-reentry.XXXXXX")" || { echo "ERROR: #2966: cannot create a private scratch directory" >&2; return 1; }
      chmod 700 "$scratch"
      workflow_context_read finalization_evidence >"$scratch/evidence" 2>/dev/null || : >"$scratch/evidence"
      workflow_context_read finalizer_step_status >"$scratch/step-status" 2>/dev/null || : >"$scratch/step-status"
    fi
    _reentry_note "$gap" "$answer" "$dirty" >"$scratch/note"
    echo "INFO: #2966: assess-finalization's answer is not one finalization reports ($gap); starting it again" >&2
    rc=0
    raw="$(_reentry_pass "$scratch")" || rc=$?
    case "$rc" in
      0) ;;
      75|79) rm -rf -- "$scratch"; echo "ERROR: #2966: the re-entered assessment run exited $rc; not retried" >&2; return "$rc" ;;
      *) rm -rf -- "$scratch"
         if [ "$rc" -ge 128 ]; then echo "ERROR: #2966: the re-entered assessment run was stopped by a signal (exit $rc)" >&2; return "$rc"; fi
         echo "ERROR: #2966: the re-entered assessment run failed (exit $rc) before it could answer; step recovery decides whether a retry can reach a decision" >&2
         return 1 ;;
    esac
  done
}
