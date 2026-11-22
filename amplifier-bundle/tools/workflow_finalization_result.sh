#!/usr/bin/env bash
# shellcheck disable=SC2034  # the result fields are read back through ${!field}
# workflow_finalization_result.sh - the finalization result object.
#
# Sourced, never executed. Defines the four functions that assemble and emit
# validate-agentic-finalization's one JSON line, and nothing else, so it can be
# sourced twice. Splitting them out of workflow_finalization_validate.sh makes
# an existing dependency explicit: workflow_finalization_handoff.sh calls
# emit_result too, and until now that function was defined inside
# validate_finalization.
#
# Every function but emit_without_jq reads the result fields of the
# validate_finalization call that is running, through bash's dynamic scoping,
# so they must be called only from inside it. They shape and emit the result;
# none of them decides what the result should be.
#
#   _finalization_state_allowed / _finalization_state_is_success
#                     the published terminal vocabulary, as two predicates
#   _finalization_action_allowed
#                     the actions assess-finalization may report taking
#   _finalization_state_names_pr
#                     the states whose definition IS an open pull request
#   _finalization_decision_unowned / _finalization_unowned_or_handoff
#                     the states whose definition IS a person's next move, and
#                     what becomes of one that named no person
#   emit_without_jq   the literal object for a host with no jq
#   emit_result       one complete, validated JSON object on stdout
#   fail_result       emit a non-success result, announce it on one line
#                     (state, reason and next action), exit 1
#   classify_success  emit a success result and exit 0

emit_without_jq() {
  printf '{"terminal_success":"false","terminal_state":"FAILED_MISSING_TOOLING","terminal_reason":"jq is required to validate finalization evidence","required_next_action":"Install jq or run from an environment with the bundled workflow tooling available.","hollow_success_detected":"false","evidence_used":"tooling.jq=missing","finalizer_schema_version":"1","finalizer_confidence":"low","finalizer_output_valid":"false","reporting_failure":"false","terminal_failure":"true"}\n'
}

# shellcheck disable=SC2154
emit_result() {
  local field separator publish_state_reached observed_phases missing_evidence
  publish_state_reached="${publish_state_override:-$(resolve_publish_state_reached)}"
  observed_phases="workflow-prep,workflow-worktree,workflow-design,workflow-tdd,workflow-refactor-review,workflow-precommit-test,workflow-publish,workflow-pr-review,workflow-finalize"
  missing_evidence=""
  # File-backed metadata and diagnostics can exceed argv's per-value limit.
  # Stream every fixed field, retaining the existing all-String schema, then
  # validate the assembled object before emitting one complete JSON result.
  {
    printf '{'
    separator=''
    for field in terminal_success terminal_state terminal_reason required_next_action hollow_success_detected evidence_used finalizer_schema_version finalizer_confidence finalizer_output_valid reporting_failure implementation_completed verification_completed publish_state_reached terminal_no_op terminal_failure pr_url pr_number observed_phases missing_evidence; do
      printf '%s"%s":' "$separator" "$field"
      printf '%s' "${!field}" | jq -n --rawfile value /dev/stdin '$value'
      separator=','
    done
    printf '}'
  } | jq -c .
}

fail_result() {
  terminal_state="$1"
  terminal_reason="$2"
  required_next_action="$3"
  evidence_used="$4"
  terminal_success="false"
  terminal_failure="true"
  finalizer_output_valid="false"
  emit_result
  # #2966: one line carrying the state, the reason and the step a person must
  # take, so the human step of an unfixable cause survives step recovery's
  # excerpt of this output. The fields are printed as data with printf '%s',
  # and any C0 control character or DEL is removed from the assembled line, so a
  # reason cannot start a second line on this channel or carry a 7-bit escape
  # sequence. The two-byte UTF-8 forms of the C1 controls are not removed: the
  # shared cleaner (_finalization_outcome_clean) leaves them too.
  {
    LC_ALL=C printf 'ERROR: workflow finalization failed closed: %s: %s; next action: %s' \
      "$terminal_state" "$terminal_reason" "$required_next_action" | LC_ALL=C tr -d '\001-\037\177'
    printf '\n'
  } >&2
  exit 1
}

classify_success() {
  terminal_state="$1"
  terminal_reason="$2"
  required_next_action="$3"
  evidence_used="$4"
  terminal_success="true"
  terminal_failure="false"
  finalizer_output_valid="true"
  finalizer_confidence="high"
  emit_result
  exit 0
}

# The two lists below are a vocabulary check, not a judgement. The assessment
# names a terminal state; these say only whether that token is one finalization
# publishes, and whether the published vocabulary calls it a success. A brick
# test keeps both inside terminal_vocabulary in
# workflow_finalization_complete.sh.
_finalization_state_allowed() {
  case "${1-}" in
    EXISTING_OPEN_PR|IMPLEMENTED_VERIFIED|ALLOW_NO_OP|NO_DIFF_SUCCESS) return 0 ;;
    BLOCKED_CI|FAILED_IMPLEMENTATION|FAILED_REPORTING|HOLLOW_SUCCESS) return 0 ;;
    FAILED_MEANINGFUL_DIFF|FAILED_CLOSED_UNMERGED|FAILED_PR_METADATA_UNAVAILABLE) return 0 ;;
    *) return 1 ;;
  esac
}
_finalization_state_is_success() {
  case "${1-}" in
    EXISTING_OPEN_PR|IMPLEMENTED_VERIFIED|ALLOW_NO_OP|NO_DIFF_SUCCESS) return 0 ;;
    *) return 1 ;;
  esac
}

# #2966 (SEC-12). The actions assess-finalization may report having taken
# before it decided: none, pushing the branch, opening the pull request, or
# bringing its CI to green. Like the lists above this is a vocabulary check and
# not a judgement. It keeps the value the validator quotes in evidence_used to
# a known token, so free text cannot add a field to that record.
_finalization_action_allowed() {
  case "${1-}" in
    none|pushed_branch|opened_pull_request|repaired_ci) return 0 ;;
    *) return 1 ;;
  esac
}

# #2819 R1. The states whose published definition IS a pull request: the state
# says one is open, so a result reporting it has to name which. This is still not a
# judgement about the run -- it does not ask whether the pull request should
# exist, only whether the decision named the thing it asserts. The other
# successes assert the opposite and name none: IMPLEMENTED_VERIFIED is for a
# remote with no GitHub pull request to read, ALLOW_NO_OP and NO_DIFF_SUCCESS
# for a run that published nothing because nothing was asked for.
_finalization_state_names_pr() {
  case "${1-}" in
    EXISTING_OPEN_PR) return 0 ;;
    *) return 1 ;;
  esac
}

# #2966. The same rule for the one state whose published definition IS a person's
# next move. BLOCKED_CI asserts a cause the run cannot fix, so it carries the
# cause (`reason`) and the step its owner must take (`next_action`); reported
# without either, it ends the run naming nobody, and the validator's placeholder
# sentences would hide that behind a pointer back at the assessment. As with a
# pull-request state that names no pull request, the decision contradicts its
# own definition, which is no decision for the recovery agent to close. This
# asks only whether the decision named what it asserts: it re-judges no fact
# and reads no check.
#
# _finalization_decision_unowned <state> <reason> <next> -> prints what is
# missing and returns 0 when the state needs an owner and was given none.
_finalization_decision_unowned() {
  local reason="${2-}" next="${3-}" missing=""
  case "${1-}" in BLOCKED_CI) ;; *) return 1 ;; esac
  [ -n "${reason//[[:space:]]/}" ] || missing="no cause"
  [ -n "${next//[[:space:]]/}" ] || missing="${missing:+$missing and }no step for its owner"
  [ -n "$missing" ] || return 1
  printf '%s\n' "$missing"
}

# _finalization_unowned_or_handoff <state> <missing> <evidence>
# What becomes of such a decision. It never returns.
#
# Inside auto-drive the state does not end the run: the pull request goes to the
# merge-ready loop, which reviews and repairs it, and no person is waiting on
# the sentences the assessment left empty. The hand-off therefore still happens,
# as it did before this check existed, with the gap recorded in place of them;
# refusing it would stop a whole drive over a missing sentence, which is the
# failure mode #2966 exists to remove. Everywhere else a person is the
# destination, so the decision is reported as no decision and the recovery agent
# closes it.
_finalization_unowned_or_handoff() {
  local reason next
  reason="assess-finalization reported $1, which names a cause the run cannot fix, but gave $2"
  next="Run finalization again so assess-finalization reports the failing check, the cause the run cannot fix and the step its owner must take, or the state the facts do support; re-running this step alone reads the same recorded answer."
  if declare -F finalization_try_handoff_open_pr >/dev/null 2>&1; then
    finalization_try_handoff_open_pr "$1" "$reason" "$next" "$3" || :
  fi
  fail_result "FAILED_MISSING_TERMINAL_EVIDENCE" "$reason" "$next" "$3"
}
