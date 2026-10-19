#!/usr/bin/env bash
# Report the genuine validated object without losing fields.
complete_workflow() {
  if ! command -v jq >/dev/null 2>&1; then
    echo "ERROR: workflow-complete requires jq to report workflow_result" >&2
    exit 2
  fi
  local result value key tier
  tier=$(_workflow_context_tier finalization)
  result="$(workflow_context_read workflow_result || :)"
  if ! printf '%s' "$result" | jq -e 'type == "object"' >/dev/null 2>&1; then result='{}'; fi
  for key in terminal_success terminal_state terminal_reason required_next_action hollow_success_detected evidence_used finalizer_schema_version finalizer_confidence finalizer_output_valid reporting_failure terminal_failure pr_url; do
    if ! printf '%s' "$result" | jq -e --arg key "$key" 'has($key)' >/dev/null; then
      value="$(workflow_finalization_field workflow_result "$key" "WORKFLOW_RESULT_$(_workflow_context_upper "$key")" || :)"
      if [ -z "$value" ]; then
        case "$key" in
          terminal_success) value='false' ;;
          terminal_state) value='FAILED_INVALID_EVIDENCE' ;;
          terminal_reason) value='validated workflow_result missing from context' ;;
          required_next_action) value='Inspect validate-agentic-finalization output.' ;;
          hollow_success_detected) value='false' ;;
          evidence_used) value='workflow_result=missing' ;;
          finalizer_schema_version) value='0' ;;
          finalizer_confidence) value='low' ;;
          finalizer_output_valid) value='false' ;;
          reporting_failure) value='false' ;;
          terminal_failure) value='true' ;;
          pr_url) value=''; if [[ $tier == legacy ]]; then value="$(resolve_pr_url)"; fi ;;
        esac
      fi
      result="$(printf '%s' "$result" | jq -c --arg key "$key" --arg value "$value" '. + {($key):$value}')"
    fi
  done
  # --rawfile preserves UTF-8 across jq raw-parser chunk boundaries.
  # Preserve every validated field and its genuine type; metadata is added to
  # the envelope, never flattened into exported success aliases.
  {
    printf '{"workflow_result":%s,"task":' "$result"
    { workflow_finalization_metadata task_description || :; } | jq -n --rawfile value /dev/stdin '$value'
    printf ',"issue_number":'
    { workflow_finalization_metadata issue_number || :; } | jq -n --rawfile value /dev/stdin '$value'
    printf '}'
  } | jq '
    .workflow_result as $result | {
      workflow:"default-workflow", version:"2.0.0", task:.task, issue_number:.issue_number,
      pr_url:$result.pr_url, terminal_outcome:$result.terminal_state,
      workflow_result:$result
    } + ($result | {terminal_state, terminal_success, terminal_reason,
      required_next_action, hollow_success_detected, evidence_used,
      finalizer_schema_version, finalizer_confidence, finalizer_output_valid,
      reporting_failure, terminal_failure}) + {terminal_vocabulary:["MERGED", "CLOSED_OBSOLETE", "NO_DIFF_SUCCESS", "FOLLOWUP_CREATED", "SUPERSEDED", "IMPLEMENTED_VERIFIED", "ALLOW_NO_OP", "BLOCKED_CI", "FAILED_IMPLEMENTATION", "FAILED_REPORTING", "FAILED_MISSING_TOOLING", "FAILED_PR_METADATA_UNAVAILABLE", "FAILED_DIRTY_WORKTREE", "FAILED_MEANINGFUL_DIFF", "FAILED_CLOSED_UNMERGED", "FAILED_INVALID_EVIDENCE", "FAILED_MISSING_TERMINAL_EVIDENCE", "HOLLOW_SUCCESS", "INCOMPLETE"]}'
}
