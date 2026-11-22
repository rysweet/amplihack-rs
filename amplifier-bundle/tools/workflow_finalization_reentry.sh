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
# end before its work does: it crashes, it is interrupted, it answers in prose,
# or it stops with the CI repair unfinished. Each of those used to reach the
# validator as no decision (or, for an unfinished repair, as
# FAILED_IMPLEMENTATION), and the run ended exit 81 on a red CI it could still
# fix. continue_ci_repair starts the assessment again instead, in a fresh
# session, for as long as its answer is not one finalization reports, and
# prints the answer that is. Nothing here reads a check, decides a terminal
# state or writes to the repository or to GitHub: the re-entered agent does
# the repair, under the same prompt and the same three permitted actions.
#
# The answer is classified, never judged. Each class is a reason to ask again,
# and the reason is handed to the next session as data (`ci_repair_reentry`):
#
#   no_decision         not one JSON object, or a terminal_state finalization
#                       does not publish: the session crashed, was cut off or
#                       answered in prose.
#   repair_unfinished   CI_REPAIR_UNFINISHED: the session stopped before the
#                       head was green and named no cause the run cannot fix.
#   blocked_ci_unowned  BLOCKED_CI without its cause or its owner's step
#                       (_finalization_decision_unowned). The session is asked
#                       for the missing sentence instead of ending the run.
#   pr_unnamed          a state that asserts a pull request, naming none
#                       (_finalization_state_names_pr).
#   worktree_dirty      a decision, but the recorded worktree now has changes
#                       and the evidence recorded it clean before the
#                       assessment ran. Every such path was written by an
#                       assessment session, so a session is asked to commit or
#                       remove what its repair left. A tree the evidence
#                       already recorded dirty predates the assessment; whose
#                       work that is cannot be told here, so it is not sent to
#                       an agent to clean and the validator still refuses it.
#   decision            anything else: printed, and the step ends.
#
# When it stops asking. There is no round or time cap, and nothing counts
# passes. repair_unfinished is always asked again: the session asserts that
# the repair can go on, and it ends only when a session decides. For every
# other class a pass that changed nothing is the end of asking: the same class
# as the pass before, with the same HEAD and the same `git status` in the
# recorded worktree. Then a no_decision is a failure of this step, which goes
# to the step's recovery agent (it may retry, which reads the pull request
# live again, or hand off with the reason); the other classes print the last
# answer and the validator reports it exactly as before this brick existed,
# loudly (FAILED_DIRTY_WORKTREE, or no decision for an unowned BLOCKED_CI
# outside auto-drive). A nested run that refuses with exit 79, stops as an
# orphan (75) or dies on a signal passes that code on, never retried here.
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

# _reentry_class <normalized answer or empty> -> one class token (header).
# The dirty-worktree class is the caller's, since it depends on the tree.
_reentry_class() {
  local state reason next pr
  [ -n "${1-}" ] || { echo no_decision; return 0; }
  state="$(printf '%s' "$1" | jq -r .terminal_state)"
  reason="$(printf '%s' "$1" | jq -r .reason)"
  next="$(printf '%s' "$1" | jq -r .next_action)"
  pr="$(printf '%s' "$1" | jq -r .pr_number)"
  if [ "$state" = CI_REPAIR_UNFINISHED ]; then echo repair_unfinished; return 0; fi
  _finalization_state_allowed "$state" || { echo no_decision; return 0; }
  if _finalization_decision_unowned "$state" "$reason" "$next" >/dev/null; then echo blocked_ci_unowned; return 0; fi
  if _finalization_state_names_pr "$state" && ! _finalization_pr_number_valid "$pr"; then echo pr_unnamed; return 0; fi
  echo decision
}

# _reentry_fingerprint -> HEAD and `git status --porcelain` in the recorded
# worktree: what a pass that made progress changes.
_reentry_fingerprint() {
  local head status
  head="$(git -C "$reentry_dir" rev-parse --verify --quiet HEAD 2>/dev/null)" || head="none"
  status="$(git -C "$reentry_dir" status --porcelain 2>/dev/null)" || status="unreadable"
  printf '%s\n%s' "$head" "$status"
}

# _reentry_dirty_lines -> the recorded worktree's `git status --porcelain`,
# printed only when that is the question to ask (header, worktree_dirty): an
# explicitly recorded worktree that git reads, that the evidence recorded
# exactly clean, and that now has changes. Prints nothing otherwise.
_reentry_dirty_lines() {
  [ "$reentry_dir_explicit" = true ] && [ "$reentry_record_clean" = true ] || return 0
  [ "$(git -C "$reentry_dir" rev-parse --is-inside-work-tree 2>/dev/null)" = true ] || return 0
  git -C "$reentry_dir" status --porcelain 2>/dev/null | LC_ALL=C tr -d '\000-\010\013-\037\177' | head -n 40 || :
}

# _reentry_note <class> <normalized previous answer or empty> <dirty lines>
# -> the `ci_repair_reentry` object the next session reads as data.
_reentry_note() {
  jq -nc --arg why "$1" --arg prev "$2" --arg dirty "$3" '{
    reentry: $why,
    previous_answer: (if $prev == "" then null else ($prev | fromjson) end),
    worktree_dirty: ($dirty != ""),
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
  local scratch raw answer class prev_class="" before="" after="" dirty rc
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
  scratch=""
  while :; do
    answer="$(_reentry_normalize "$raw")" || answer=""
    class="$(_reentry_class "$answer")"
    dirty="$(_reentry_dirty_lines)"
    [ "$class" != decision ] || [ -z "$dirty" ] || class=worktree_dirty
    if [ "$class" = decision ]; then
      [ -z "$scratch" ] || rm -rf -- "$scratch"
      printf '%s' "$raw" | jq -c .
      return 0
    fi
    if [ -n "$prev_class" ] && [ "$class" = "$prev_class" ] && [ "$class" != repair_unfinished ] && [ "$after" = "$before" ]; then
      [ -z "$scratch" ] || rm -rf -- "$scratch"
      if [ "$class" = no_decision ]; then
        echo "ERROR: #2966: assess-finalization was started again and again returned no decision, changing nothing in the worktree; step recovery decides whether a retry can reach one" >&2
        return 1
      fi
      echo "INFO: #2966: assess-finalization was asked again ($class) and changed nothing; the validator reports its answer" >&2
      printf '%s' "$raw" | jq -c .
      return 0
    fi
    if [ -z "$scratch" ]; then
      scratch="$(mktemp -d "${TMPDIR:-/tmp}/ci-repair-reentry.XXXXXX")" || { echo "ERROR: #2966: cannot create a private scratch directory" >&2; return 1; }
      chmod 700 "$scratch"
      workflow_context_read finalization_evidence >"$scratch/evidence" 2>/dev/null || : >"$scratch/evidence"
      workflow_context_read finalizer_step_status >"$scratch/step-status" 2>/dev/null || : >"$scratch/step-status"
    fi
    _reentry_note "$class" "$answer" "$dirty" >"$scratch/note"
    echo "INFO: #2966: assess-finalization ended with $class; starting it again to continue the CI repair" >&2
    before="$(_reentry_fingerprint)"
    rc=0
    raw="$(_reentry_pass "$scratch")" || rc=$?
    after="$(_reentry_fingerprint)"
    prev_class="$class"
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
