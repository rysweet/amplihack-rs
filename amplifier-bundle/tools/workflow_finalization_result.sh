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
# Every function but emit_without_jq and the four predicates reads the result
# fields of the validate_finalization call that is running, through bash's
# dynamic scoping, so those must be called only from inside it. They shape and
# emit the result; none of them decides what the result should be.
#
#   _finalization_state_allowed / _finalization_state_is_success
#                     the published terminal vocabulary, as two predicates
#   _finalization_state_names_pr
#                     the states whose definition IS an open pull request
#   _finalization_assessment_decides
#                     whether a recorded assessment carries a decision at all
#   emit_without_jq   the literal object for a host with no jq
#   emit_result       one complete, validated JSON object on stdout
#   fail_result       emit a non-success result, announce it, exit 1
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
  echo "ERROR: workflow finalization failed closed: $terminal_state: $terminal_reason" >&2
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

# #2819 R3. Whether a recorded assess-finalization answer carries a decision
# finalization can report. The two conditions are the ones validate_finalization
# refuses on, asked through the same three predicates, so "is there a decision"
# and the refusal of one cannot drift apart: a state outside the published
# vocabulary is no decision, and so is a state whose definition IS an open pull
# request reported without a pull request number.
#
# It is asked before the validator runs, by the assessment-status mode, so a
# failed or non-JSON model call can be given one more attempt rather than
# ending a finished run on a gap in finalization itself (#2847 item 1). It
# reads nothing but its two arguments, reaches no verdict about the run, and
# never decides a terminal state: the answer is only whether there is one to
# report. Prints the reason and returns 1 when there is none.
_finalization_assessment_decides() {
  local state="${1-}" pr="${2-}"
  if ! _finalization_state_allowed "$state"; then
    if [ -z "$state" ]; then
      printf 'assess-finalization produced no terminal state'
    else
      printf 'assess-finalization returned a terminal state finalization does not publish'
    fi
    return 1
  fi
  if _finalization_state_names_pr "$state" && ! _finalization_pr_number_valid "$pr"; then
    printf '%s asserts an open pull request and named no number for it' "$state"
    return 1
  fi
  return 0
}
