#!/usr/bin/env bash
# Completion consumers share authority with the verdict reader. Root objects
# win over nested scalar exports; nested completion fields remain supported.
. "$(dirname "${BASH_SOURCE[0]}")/workflow_context.sh"
WORKFLOW_CONTEXT_COHORT=finalization

workflow_finalization_field() {
  local root=$1 field=$2 legacy=${3:-} second=${4:-} tier name raw status
  case "$root:$field" in
    implementation_terminal_evidence:implementation_completed|implementation_terminal_evidence:terminal_no_op|verification_terminal_evidence:verification_completed|finalizer_step_status:status|finalizer_step_status:reporting_failure|finalization_evidence:git.dirty_worktree|finalization_evidence:tooling.missing|finalization_evidence:tooling.gh_required|finalization_evidence:prior_terminal_state.terminal_state|finalization_evidence:agent_outputs.hollow_success_signals) ;;
    workflow_result:terminal_success|workflow_result:terminal_state|workflow_result:terminal_reason|workflow_result:required_next_action|workflow_result:hollow_success_detected|workflow_result:evidence_used|workflow_result:finalizer_schema_version|workflow_result:finalizer_confidence|workflow_result:finalizer_output_valid|workflow_result:reporting_failure|workflow_result:terminal_failure|workflow_result:pr_url) ;;
    *) _workflow_context_invalid; return 3 ;;
  esac
  tier=$(_workflow_context_tier finalization)
  if [[ $tier == legacy ]]; then
    if [[ -n $legacy ]]; then
      raw=$(_workflow_context_get "$legacy" || :; printf '.')
      raw=${raw%.}
      if [[ -n $raw ]]; then printf '%s' "$raw"; return 0; fi
    fi
    if [[ -n $second ]]; then _workflow_context_get "$second"; return $?; fi
    return 2
  fi
  name="RECIPE_VAR_$root"
  if [[ $tier == canonical ]] && ! _workflow_context_has "$name"; then
    name="RECIPE_VAR_${root}__${field//./__}"
    _workflow_context_get "$name" || return 2
    return 0
  fi
  if raw=$(workflow_context_read "$root"); then
    printf '%s' "$raw" | jq -j --arg field "$field" '
      getpath($field|split(".")) | if . == null then empty elif type == "string" or type == "boolean" then if type == "string" and index("\u0000") != null then error("field") else . end else error("field") end' 2>/dev/null || { _workflow_context_invalid; return 3; }
  else
    status=$?; return "$status"
  fi
}

resolve_implementation_completed() {
  local value
  value=$(workflow_finalization_field implementation_terminal_evidence implementation_completed IMPLEMENTATION_COMPLETED IMPLEMENTATION_TERMINAL_EVIDENCE_IMPLEMENTATION_COMPLETED || :; printf '.')
  boolish "${value%.}"
}
resolve_verification_completed() {
  local value
  value=$(workflow_finalization_field verification_terminal_evidence verification_completed VERIFICATION_COMPLETED VERIFICATION_TERMINAL_EVIDENCE_VERIFICATION_COMPLETED || :; printf '.')
  boolish "${value%.}"
}
resolve_terminal_no_op() {
  local value
  value=$(workflow_finalization_field implementation_terminal_evidence terminal_no_op TERMINAL_NO_OP IMPLEMENTATION_TERMINAL_EVIDENCE_TERMINAL_NO_OP || :; printf '.')
  boolish "${value%.}"
}
resolve_allow_no_op() {
  local value
  workflow_context_capture value allow_no_op
  boolish "$value"
}
