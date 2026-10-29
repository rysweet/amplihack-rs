#!/usr/bin/env bash
# shellcheck disable=SC2034  # the validator's result fields and HANDOFF_* are read by the caller
# workflow_finalization_handoff.sh - the finalization hand-off (issue #1714).
#
# Sourced, never executed. Defines constants and functions and changes nothing
# else, so it can be sourced twice. This file is the only definition of which
# terminal states auto-drive hands to its crusty and merge-ready loops.
#
# Inside auto-drive, a default-workflow build whose finalization fails while
# a published, open PR exists on the run branch hands that PR to the crusty
# loop and the merge-ready loop, which exist to review the PR and to fix what
# keeps it from merging. A standalone default-workflow still fails closed.
# The decision uses typed fields only (terminal_state, handoff, the PR
# number, the live PR state), never log text.
#
#   Handed off (FINALIZATION_HANDOFF_STATES):
#     BLOCKED_CI             the PR exists and its checks are red.
#     FAILED_IMPLEMENTATION  the PR exists and was confirmed live by the
#                            open-PR check (open, same-repo, run branch,
#                            head at or fast-forward of the local HEAD), but
#                            this run's own evidence does not prove the work
#                            complete. The crusty loop reviews exactly that.
#   Never handed off: FAILED_DIRTY_WORKTREE and FAILED_WRONG_BRANCH (the
#   local checkout is not the PR's), FAILED_MISSING_TOOLING,
#   FAILED_INVALID_EVIDENCE, FAILED_INVALID_INPUT and
#   FAILED_MISSING_TERMINAL_EVIDENCE (the evidence cannot be read),
#   HOLLOW_SUCCESS (the agent output is empty, so the PR may be too),
#   FAILED_CLOSED_UNMERGED and FAILED_MEANINGFUL_DIFF (there is no open PR
#   worth reviewing), FAILED_PR_METADATA_UNAVAILABLE (the PR is unknown),
#   FAILED_REPORTING (the fix is to rerun the reporting step, which the
#   loops do not do) and INCOMPLETE. Success states need no hand-off.
#
#   finalization_handoff_allowed <state>
#       Return 0 only for exactly one of FINALIZATION_HANDOFF_STATES.
#   finalization_handoff_flag
#       Print `true` only when RECIPE_VAR_finalization_handoff is exactly `true`,
#       else `false`. No bare environment fallback, no case folding, no trimming.
#   finalization_fail_or_handoff <state> <reason> <next> <evidence>
#       Drop-in for fail_result; callable only inside validate_finalization.
#       Hands off with the validator's current pr_number.
#   finalization_fail_or_handoff_open_pr <state> <reason> <next> <evidence>
#       As above, but hands off only when the open-PR check confirmed a PR
#       (open_pr_confirmed_number/url/facts, set by the validator); otherwise it
#       fails closed with the run's own PR evidence.
#   autodrive_build_handoff <resolved_pr>
#       autodrive-build step-03's decision. Call directly, not in $(...): it
#       sets HANDOFF_TERMINAL_STATE and HANDOFF_HANDED_OFF. Returns 0 to
#       continue, 1 after printing an ERROR naming #1714.

FINALIZATION_HANDOFF_FIELD="handoff"
FINALIZATION_HANDOFF_VALUE="merge_ready_loop"
FINALIZATION_HANDOFF_STATES="BLOCKED_CI FAILED_IMPLEMENTATION"

finalization_handoff_allowed() {
  case "${1-}" in
    BLOCKED_CI|FAILED_IMPLEMENTATION) return 0 ;;
    *) return 1 ;;
  esac
}

finalization_handoff_flag() {
  if [ "${RECIPE_VAR_finalization_handoff-}" = "true" ]; then printf 'true\n'; else printf 'false\n'; fi
}

# _finalization_handoff_emit <state> <reason> <next> <evidence>
# Exits 0 after printing a complete hand-off result for the current
# pr_number; returns 1 (having printed nothing on stdout) when it may not.
# The dispatcher runs under `set -e`, so every caller must test the return
# status (`|| :`); a bare call would end the script before fail_result.
_finalization_handoff_emit() {
  local handed
  finalization_handoff_allowed "$1" && [ "$(finalization_handoff_flag)" = "true" ] \
    && _finalization_pr_number_valid "${pr_number:-}" || return 1
  terminal_state="$1"
  terminal_reason="$2"
  required_next_action="$3"
  evidence_used="$4"
  terminal_success="false"
  terminal_failure="true"
  finalizer_output_valid="false"
  # Exit 0 only once a complete object exists; otherwise fail closed as before.
  if handed="$(set -o pipefail; emit_result | jq -c --arg k "$FINALIZATION_HANDOFF_FIELD" --arg v "$FINALIZATION_HANDOFF_VALUE" '. + {($k): $v}')" \
    && [ -n "$handed" ]; then
    printf '%s\n' "$handed"
    echo "INFO: #1714: $1 handed to the merge-ready loop (PR #$pr_number)" >&2
    exit 0
  fi
  echo "ERROR: #1714: could not write the hand-off result; failing closed" >&2
  return 1
}

finalization_fail_or_handoff() {
  _finalization_handoff_emit "$@" || :
  fail_result "$@"
}

finalization_fail_or_handoff_open_pr() {
  local own_number="${pr_number:-}" own_url="${pr_url:-}"
  if [ -n "${open_pr_confirmed_number:-}" ]; then
    pr_number="$open_pr_confirmed_number"; pr_url="${open_pr_confirmed_url:-}"; publish_state_override="true"
    _finalization_handoff_emit "$1" "$2" "$3" "$4${open_pr_confirmed_facts:+,$open_pr_confirmed_facts}" || :
    # No hand-off: report the run's own PR evidence, not the confirmed PR's.
    pr_number="$own_number"; pr_url="$own_url"; publish_state_override=""
  fi
  fail_result "$@"
}

# The 20 terminal states default-workflow can report (the finaliser's
# terminal_vocabulary; a brick test keeps the two lists equal). Anything else
# is never echoed into build_result or a log line.
_handoff_known_state() {
  case "${1-}" in
    MERGED|CLOSED_OBSOLETE|NO_DIFF_SUCCESS|FOLLOWUP_CREATED|EXISTING_OPEN_PR|SUPERSEDED|IMPLEMENTED_VERIFIED|ALLOW_NO_OP|BLOCKED_CI|FAILED_IMPLEMENTATION|FAILED_REPORTING|FAILED_MISSING_TOOLING|FAILED_PR_METADATA_UNAVAILABLE|FAILED_DIRTY_WORKTREE|FAILED_MEANINGFUL_DIFF|FAILED_CLOSED_UNMERGED|FAILED_INVALID_EVIDENCE|FAILED_MISSING_TERMINAL_EVIDENCE|HOLLOW_SUCCESS|INCOMPLETE) return 0 ;;
    *) return 1 ;;
  esac
}

autodrive_build_handoff() {
  local pr="${1-}" state success handoff result_pr shown pr_state label="autodrive-build/step-03"
  HANDOFF_TERMINAL_STATE=""; HANDOFF_HANDED_OFF="false"
  state="$(rv_field workflow_result terminal_state)"
  # R1: step-03 calls this only when workflow_result is set (a resumed run
  # that skipped step-02 leaves it empty and never gets here). A set value
  # with no readable terminal_state is a broken transport: fail closed.
  if [ -z "$state" ]; then
    echo "ERROR: #1714: workflow_result is set but has no readable terminal_state; cannot decide the finalization hand-off [$label]" >&2
    return 1
  fi
  _handoff_known_state "$state" && HANDOFF_TERMINAL_STATE="$state"
  success="$(rv_field workflow_result terminal_success)"
  # R2: a successful build needs no hand-off.
  [ "$success" != "true" ] || return 0
  handoff="$(rv_field workflow_result handoff)"
  if ! finalization_handoff_allowed "$state" || [ "$handoff" != "$FINALIZATION_HANDOFF_VALUE" ]; then
    # R5: every other failure ends the run.
    echo "ERROR: #1714: default-workflow ended ${HANDOFF_TERMINAL_STATE:-<unrecognised>} without a merge-ready hand-off; auto-drive stops here [$label]" >&2
    return 1
  fi
  # R3/R4: the PR is step-03's own resolution; workflow_result may only agree.
  case "$pr" in
    ''|*[!0-9]*)
      echo "ERROR: #1714: $state was reported with no published open PR to hand to the merge-ready loop [$label]" >&2
      return 1 ;;
  esac
  result_pr="$(rv_field workflow_result pr_number)"
  if [ -n "$result_pr" ] && [ "$result_pr" != "$pr" ]; then
    shown="$result_pr"
    case "$shown" in *[!0-9]*) shown="<invalid>" ;; esac
    echo "ERROR: #1714: workflow_result names PR #$shown but phase 1 resolved PR #$pr; refusing the hand-off [$label]" >&2
    return 1
  fi
  pr_state="UNKNOWN"
  if command -v autodrive_pr_state >/dev/null 2>&1; then
    pr_state="$(autodrive_pr_state "$pr")"
    [ "$pr_state" != "UNKNOWN" ] || pr_state="$(autodrive_pr_state "$pr")"
  fi
  if [ "$pr_state" != "OPEN" ]; then
    echo "ERROR: #1714: PR #$pr is $pr_state, not OPEN; $state cannot be handed to the merge-ready loop [$label]" >&2
    return 1
  fi
  HANDOFF_HANDED_OFF="true"
  echo "INFO: #1714: phase 1 ended $state; PR #$pr goes to the crusty and merge-ready loops [$label]" >&2
  return 0
}
