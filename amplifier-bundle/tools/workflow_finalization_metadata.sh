#!/usr/bin/env bash
# Fixed deterministic metadata, separate from completion and verdict policy.
workflow_finalization_metadata() {
  local root field='' aliases name tier raw
  case "$1" in
    branch_name) root=branch_name; aliases='BRANCH_NAME WORKTREE_SETUP_BRANCH_NAME' ;;
    repo_path|base_ref|remote_host_type|task_description|issue_number)
      root=$1; aliases=$(_workflow_context_upper "$1") ;;
    worktree_path) root=worktree_setup; field=worktree_path; aliases=WORKTREE_SETUP_WORKTREE_PATH ;;
    pr_url) root=terminal_state; field=pr_url; aliases='TERMINAL_STATE_PR_URL PR_URL PR_PUBLISH_RESULT_PR_URL' ;;
    pr_number) root=terminal_state; field=pr_number; aliases='TERMINAL_STATE_PR_NUMBER PR_NUMBER PR_PUBLISH_RESULT_PR_NUMBER' ;;
    publish_state_reached) root=publish_terminal_evidence; field=publish_state_reached; aliases='PUBLISH_STATE_REACHED PUBLISH_TERMINAL_EVIDENCE_PUBLISH_STATE_REACHED' ;;
    prior_terminal_state) root=terminal_state; field=terminal_state; aliases='TERMINAL_STATE_TERMINAL_STATE TERMINAL_STATE' ;;
    prior_terminal_success) root=terminal_state; field=terminal_success; aliases=TERMINAL_STATE_TERMINAL_SUCCESS ;;
    prior_terminal_reason) root=terminal_state; field=terminal_reason; aliases=TERMINAL_STATE_TERMINAL_REASON ;;
    branch_diff_status) root=terminal_state; field=branch_diff_status; aliases=TERMINAL_STATE_BRANCH_DIFF_STATUS ;;
    commits_ahead) root=terminal_state; field=commits_ahead; aliases=TERMINAL_STATE_COMMITS_AHEAD ;;
    publish_status) root=terminal_state; field=publish_status; aliases='TERMINAL_STATE_PUBLISH_STATUS PR_PUBLISH_RESULT_STATE' ;;
    *) _workflow_context_invalid; return 3 ;;
  esac
  tier=$(_workflow_context_tier metadata)
  if [[ $tier == legacy ]]; then
    for name in $aliases; do
      raw=$(_workflow_context_get "$name" || :)
      if [[ -n $raw ]]; then printf '%s' "$raw"; return 0; fi
    done
    return 2
  fi
  if [[ -z $field ]]; then workflow_context_read "$root"; return $?; fi
  if raw=$(workflow_context_read "$root"); then
    printf '%s' "$raw" | jq -r --arg key "$field" '.[$key] | if . == null then empty elif type == "string" or (($key == "pr_number" or $key == "commits_ahead") and type == "number") or (($key == "terminal_success" or $key == "publish_state_reached" or $key == "branch_diff_status") and type == "boolean") then if type == "string" and index("\u0000") != null then error("metadata") else . end else error("metadata") end' 2>/dev/null || { _workflow_context_invalid; return 3; }
    return
  fi
  # A supplied complete root cannot borrow even a missing member from aliases.
  name="RECIPE_VAR_$root"
  if [[ $tier == file ]]; then
    [[ $_workflow_context_file_status == 0 ]] || return 3
    if printf '%s' "$_workflow_context_file_snapshot" | jq -e --arg key "$root" 'has($key)' >/dev/null; then return 2; fi
  elif _workflow_context_has "$name"; then return 2;
  fi
  name="${name}__${field}"
  if [[ $tier == canonical ]] && _workflow_context_has "$name"; then
    _workflow_context_get "$name"; return $?
  fi
  # These are existing canonical publishing synonyms, all within one tier.
  case "$1" in
    pr_url|pr_number) workflow_context_read "$1" && return; root=pr_publish_result; field=$1 ;;
    publish_state_reached) workflow_context_read publish_state_reached; return $? ;;
    publish_status) root=pr_publish_result; field=state ;;
    *) return 2 ;;
  esac
  name="RECIPE_VAR_$root"
  if [[ $tier == file ]] || _workflow_context_has "$name"; then
    raw=$(workflow_context_read "$root") || return $?
    printf '%s' "$raw" | jq -r --arg key "$field" '.[$key] | if . == null then empty elif type == "string" or ($key == "pr_number" and type == "number") then if type == "string" and index("\u0000") != null then error("metadata") else . end else error("metadata") end' 2>/dev/null || { _workflow_context_invalid; return 3; }
  else
    name="${name}__${field}"
    _workflow_context_get "$name" || return 2
  fi
}
