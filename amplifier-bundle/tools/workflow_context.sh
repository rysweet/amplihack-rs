#!/usr/bin/env bash
# Private model-neutral context transport. Presence selects a whole cohort;
# invalid authoritative data never falls back to a stale secondary source.
# Source once in the caller before command substitutions to freeze the file.
_workflow_context_has_file=${AMPLIHACK_CONTEXT_FILE+x}
_workflow_context_file_status=2
_workflow_context_file_snapshot=''
if [[ ${AMPLIHACK_CONTEXT_FILE+x} ]]; then
  _workflow_context_file_status=3
  if [[ -n $AMPLIHACK_CONTEXT_FILE && -f $AMPLIHACK_CONTEXT_FILE && ! -L $AMPLIHACK_CONTEXT_FILE ]] && command -v jq >/dev/null 2>&1; then
    if _workflow_context_file_snapshot=$(jq -cs 'if length == 1 and (.[0]|type) == "object" then .[0] else error("context") end' 2>/dev/null < "$AMPLIHACK_CONTEXT_FILE"); then
      _workflow_context_file_status=0
    fi
  fi
fi

# Snapshot only this private contract's names, never unrelated environment data.
# Indexed arrays also work with the system Bash shipped on macOS.
# A sentinel avoids empty-array nounset differences in older Bash releases.
_workflow_context_names=('')
_workflow_context_values=('')
_workflow_context_upper() { printf '%s' "$1" | tr '[:lower:]' '[:upper:]'; }
_workflow_context_store() {
  _workflow_context_names[${#_workflow_context_names[@]}]=$1
  _workflow_context_values[${#_workflow_context_values[@]}]=$2
}
_workflow_context_has() {
  local name
  for name in "${_workflow_context_names[@]}"; do [[ $name == "$1" ]] && return 0; done
  return 1
}
_workflow_context_get() {
  local i
  for ((i=0; i<${#_workflow_context_names[@]}; i++)); do
    if [[ ${_workflow_context_names[$i]} == "$1" ]]; then printf '%s' "${_workflow_context_values[$i]}"; return 0; fi
  done
  return 2
}
_workflow_context_roots='verdict_json implementation allow_no_op doc_review_feedback precommit_results local_testing_gate agentic_finalizer_narrative implementation_terminal_evidence verification_terminal_evidence finalizer_step_status finalization_evidence workflow_result repo_path worktree_setup branch_name base_ref remote_host_type terminal_state pr_publish_result pr_url pr_number task_description issue_number publish_state_reached publish_terminal_evidence'
for _wc_key in $_workflow_context_roots; do
  _wc_name="$(_workflow_context_upper "$_wc_key")"
  if [[ ${!_wc_name+x} ]]; then _workflow_context_store "$_wc_name" "${!_wc_name}"; fi
  while IFS= read -r _wc_name; do
    [[ -n $_wc_name ]] || continue
    if [[ $_wc_name == "RECIPE_VAR_$_wc_key" || $_wc_name == "RECIPE_VAR_${_wc_key}__"* ]]; then
      _workflow_context_store "$_wc_name" "${!_wc_name}"
    fi
  done < <(compgen -A variable "RECIPE_VAR_$_wc_key")
done
for _wc_name in IMPLEMENTATION_COMPLETED IMPLEMENTATION_TERMINAL_EVIDENCE_IMPLEMENTATION_COMPLETED VERIFICATION_COMPLETED VERIFICATION_TERMINAL_EVIDENCE_VERIFICATION_COMPLETED TERMINAL_NO_OP IMPLEMENTATION_TERMINAL_EVIDENCE_TERMINAL_NO_OP FINALIZER_REPORTING_FAILURE FINALIZATION_EVIDENCE_GIT_DIRTY_WORKTREE FINALIZATION_EVIDENCE_TOOLING_MISSING FINALIZATION_EVIDENCE_TOOLING_GH_REQUIRED FINALIZATION_EVIDENCE_PRIOR_TERMINAL_STATE_TERMINAL_STATE FINALIZATION_EVIDENCE_AGENT_OUTPUTS_HOLLOW_SUCCESS_SIGNALS WORKTREE_SETUP_WORKTREE_PATH WORKTREE_SETUP_BRANCH_NAME BRANCH_NAME TERMINAL_STATE_PR_URL TERMINAL_STATE_PR_NUMBER TERMINAL_STATE_COMMITS_AHEAD TERMINAL_STATE_TERMINAL_STATE TERMINAL_STATE_TERMINAL_SUCCESS TERMINAL_STATE_TERMINAL_REASON TERMINAL_STATE_BRANCH_DIFF_STATUS TERMINAL_STATE_PUBLISH_STATUS PR_PUBLISH_RESULT_PR_URL PR_PUBLISH_RESULT_PR_NUMBER PR_PUBLISH_RESULT_STATE PUBLISH_TERMINAL_EVIDENCE_PUBLISH_STATE_REACHED; do
  if [[ ${!_wc_name+x} ]]; then _workflow_context_store "$_wc_name" "${!_wc_name}"; fi
done
for _wc_key in terminal_success terminal_state terminal_reason required_next_action hollow_success_detected evidence_used finalizer_schema_version finalizer_confidence finalizer_output_valid reporting_failure terminal_failure pr_url; do
  _wc_name="WORKFLOW_RESULT_$(_workflow_context_upper "$_wc_key")"
  if [[ ${!_wc_name+x} ]]; then _workflow_context_store "$_wc_name" "${!_wc_name}"; fi
done
unset _wc_key _wc_name

_workflow_context_contract() {
  case "$1" in
    verdict_json) _wc_cohort=implementation; _wc_type=object; _wc_legacy=VERDICT_JSON ;;
    implementation) _wc_cohort=implementation; _wc_type=string; _wc_legacy=IMPLEMENTATION ;;
    allow_no_op) _wc_cohort=${WORKFLOW_CONTEXT_COHORT:-implementation}; _wc_type=bool; _wc_legacy=ALLOW_NO_OP ;;
    doc_review_feedback) _wc_cohort=documentation; _wc_type=object; _wc_legacy=DOC_REVIEW_FEEDBACK ;;
    precommit_results) _wc_cohort=verification; _wc_type=string; _wc_legacy=PRECOMMIT_RESULTS ;;
    local_testing_gate) _wc_cohort=verification; _wc_type=string; _wc_legacy=LOCAL_TESTING_GATE ;;
    agentic_finalizer_narrative) _wc_cohort=reporting; _wc_type=string; _wc_legacy=AGENTIC_FINALIZER_NARRATIVE ;;
    pr_number|issue_number) _wc_cohort=metadata; _wc_type=number; _wc_legacy=$(_workflow_context_upper "$1") ;;
    repo_path|branch_name|base_ref|remote_host_type|pr_url|task_description)
      _wc_cohort=metadata; _wc_type=string; _wc_legacy=$(_workflow_context_upper "$1") ;;
    publish_state_reached) _wc_cohort=metadata; _wc_type=bool; _wc_legacy=PUBLISH_STATE_REACHED ;;
    worktree_setup|terminal_state|pr_publish_result|publish_terminal_evidence)
      _wc_cohort=metadata; _wc_type=object; _wc_legacy=$(_workflow_context_upper "$1") ;;
    implementation_terminal_evidence|verification_terminal_evidence|finalizer_step_status|finalization_evidence|workflow_result)
      _wc_cohort=finalization; _wc_type=object; _wc_legacy=$(_workflow_context_upper "$1") ;;
    *) return 3 ;;
  esac
}

_workflow_context_tier() {
  local key name variable
  if [[ $_workflow_context_has_file ]]; then printf 'file'; return; fi
  local keys
  case "$1" in
    implementation) keys='verdict_json implementation allow_no_op' ;;
    documentation) keys='doc_review_feedback' ;;
    verification) keys='precommit_results local_testing_gate allow_no_op' ;;
    reporting) keys='agentic_finalizer_narrative' ;;
    metadata) keys='repo_path worktree_setup branch_name base_ref remote_host_type terminal_state pr_publish_result pr_url pr_number task_description issue_number publish_state_reached publish_terminal_evidence' ;;
    finalization) keys='implementation_terminal_evidence verification_terminal_evidence allow_no_op finalization_evidence finalizer_step_status workflow_result' ;;
    *) return 3 ;;
  esac
  for key in $keys; do
    name="RECIPE_VAR_$key"
    if _workflow_context_has "$name"; then printf 'canonical'; return; fi
    # Inspect names only. Nested namespaces select authority but cannot
    # fabricate complete verdict/documentation objects.
    for variable in "${_workflow_context_names[@]}"; do
      if [[ $variable == "${name}__"* ]]; then printf 'canonical'; return; fi
    done
  done
  printf 'legacy'
}

_workflow_context_invalid() {
  printf 'WARN: invalid authoritative workflow context\n' >&2
  return 3
}

workflow_context_read() {
  local _wc_cohort _wc_type _wc_legacy tier raw name value
  _workflow_context_contract "${1:-}" || { _workflow_context_invalid; return 3; }
  tier=$(_workflow_context_tier "$_wc_cohort") || { _workflow_context_invalid; return 3; }
  if [[ $tier == legacy ]]; then
    _workflow_context_get "$_wc_legacy" || return 2
    return 0
  fi
  if [[ $tier == file ]]; then
    [[ $_workflow_context_file_status == 0 ]] || { _workflow_context_invalid; return 3; }
    if ! raw=$(printf '%s' "$_workflow_context_file_snapshot" | jq -c --arg key "$1" 'if has($key) then .[$key] else empty end'); then
      _workflow_context_invalid; return 3
    fi
    [[ -n $raw ]] || return 2
  else
    name="RECIPE_VAR_$1"
    raw=$(_workflow_context_get "$name" && printf '.') || return 2
    raw=${raw%.}
    # Runner scalar Strings are raw; objects are complete JSON documents.
    if [[ $_wc_type == string || $_wc_type == bool || $_wc_type == number ]]; then
      printf '%s' "$raw"; return 0
    fi
  fi
  case "$_wc_type" in
    object)
      if ! value=$(printf '%s' "$raw" | jq -cs 'if length != 1 then error("context") else .[0] end | if type == "string" then fromjson else . end | if type == "object" then if type == "string" and index("\u0000") != null then error("context") else . end else error("context") end' 2>/dev/null); then
        _workflow_context_invalid; return 3
      fi ;;
    string|bool|number)
      if ! value=$(printf '%s' "$raw" | jq -j --arg kind "$_wc_type" 'if type == "string" or ($kind == "bool" and type == "boolean") or ($kind == "number" and type == "number") then if type == "string" and index("\u0000") != null then error("context") else . end else error("context") end' 2>/dev/null && printf '.'); then
        _workflow_context_invalid; return 3
      fi
      value=${value%.} ;;
  esac
  printf '%s' "$value"
}

# Capture scalar bytes without command substitution's trailing-LF removal.
# The destination is a private caller variable; the key still uses the allowlist.
workflow_context_capture() {
  local _wc_capture_value
  _wc_capture_value=$(workflow_context_read "$2" || :; printf '.')
  printf -v "$1" '%s' "${_wc_capture_value%.}"
}
