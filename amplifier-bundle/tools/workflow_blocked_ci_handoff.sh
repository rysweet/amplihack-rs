#!/usr/bin/env bash
# shellcheck disable=SC2034  # the validator's result fields and HANDOFF_* are read by the caller
# workflow_blocked_ci_handoff.sh - the BLOCKED_CI hand-off (issue #1714).
#
# Sourced, never executed. Defines constants and functions and changes nothing
# else, so it can be sourced twice. This file is the only definition of which
# terminal states auto-drive hands to its merge-ready loop.
#
# Inside auto-drive, a default-workflow build whose finalization ends
# BLOCKED_CI with a published, open PR hands that PR to the crusty loop and
# the merge-ready loop, which exist to fix failing checks. A standalone
# default-workflow still fails closed. The decision uses typed fields only
# (terminal_state, handoff, the PR number), never log text.
#
#   BLOCKED_CI is the only state handed off: the PR exists and only its
#   checks are red. Every other state is rejected, and success states need
#   no hand-off.
#
#   blocked_ci_handoff_allowed <state>
#       Return 0 only for exactly BLOCKED_CI.
#   blocked_ci_handoff_flag
#       Print `true` only when RECIPE_VAR_blocked_ci_handoff is exactly `true`,
#       else `false`. No bare environment fallback, no case folding, no trimming.
#   finalization_fail_or_handoff <state> <reason> <next> <evidence>
#       Drop-in for fail_result; callable only inside validate_finalization.
#   autodrive_build_handoff <resolved_pr>
#       autodrive-build step-03's decision. Call directly, not in $(...): it
#       sets HANDOFF_TERMINAL_STATE and HANDOFF_HANDED_OFF. Returns 0 to
#       continue, 1 after printing an ERROR naming #1714.

BLOCKED_CI_HANDOFF_FIELD="handoff"
BLOCKED_CI_HANDOFF_VALUE="merge_ready_loop"

blocked_ci_handoff_allowed() {
  case "${1-}" in
    BLOCKED_CI) return 0 ;;
    *) return 1 ;;
  esac
}

blocked_ci_handoff_flag() {
  if [ "${RECIPE_VAR_blocked_ci_handoff-}" = "true" ]; then printf 'true\n'; else printf 'false\n'; fi
}

finalization_fail_or_handoff() {
  local handed
  if blocked_ci_handoff_allowed "$1" && [ "$(blocked_ci_handoff_flag)" = "true" ] \
    && _finalization_pr_number_valid "${pr_number:-}"; then
    terminal_state="$1"
    terminal_reason="$2"
    required_next_action="$3"
    evidence_used="$4"
    terminal_success="false"
    terminal_failure="true"
    finalizer_output_valid="false"
    # Exit 0 only once a complete object exists; otherwise fail closed as before.
    if handed="$(set -o pipefail; emit_result | jq -c --arg k "$BLOCKED_CI_HANDOFF_FIELD" --arg v "$BLOCKED_CI_HANDOFF_VALUE" '. + {($k): $v}')" \
      && [ -n "$handed" ]; then
      printf '%s\n' "$handed"
      echo "INFO: #1714: BLOCKED_CI handed to the merge-ready loop (PR #$pr_number)" >&2
      exit 0
    fi
    echo "ERROR: #1714: could not write the hand-off result; failing closed" >&2
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
  # R1: no typed result (a resumed run that skipped step-02): as before.
  [ -n "$state" ] || return 0
  _handoff_known_state "$state" && HANDOFF_TERMINAL_STATE="$state"
  success="$(rv_field workflow_result terminal_success)"
  # R2: a successful build needs no hand-off.
  [ "$success" != "true" ] || return 0
  handoff="$(rv_field workflow_result handoff)"
  if ! blocked_ci_handoff_allowed "$state" || [ "$handoff" != "$BLOCKED_CI_HANDOFF_VALUE" ]; then
    # R5: every other failure ends the run.
    echo "ERROR: #1714: default-workflow ended ${HANDOFF_TERMINAL_STATE:-<unrecognised>} without a merge-ready hand-off; auto-drive stops here [$label]" >&2
    return 1
  fi
  # R3/R4: the PR is step-03's own resolution; workflow_result may only agree.
  case "$pr" in
    ''|*[!0-9]*)
      echo "ERROR: #1714: BLOCKED_CI was reported with no published open PR to hand to the merge-ready loop [$label]" >&2
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
    echo "ERROR: #1714: PR #$pr is $pr_state, not OPEN; BLOCKED_CI cannot be handed to the merge-ready loop [$label]" >&2
    return 1
  fi
  HANDOFF_HANDED_OFF="true"
  echo "INFO: #1714: phase 1 ended BLOCKED_CI; PR #$pr goes to the crusty and merge-ready loops [$label]" >&2
  return 0
}
