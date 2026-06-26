#!/usr/bin/env bash
set -euo pipefail

export GIT_PAGER=cat GH_PAGER=cat PAGER=cat LESS=FRX

mode="${1:-}"

# run_finalization_cleanup (issue #808)
# Deterministic, fail-soft finalization cleanup invoked before evidence
# collection so the agentic finalizer observes a clean state: no run-created
# fallback branches left on the shared remote, no leaked nested worktrees. All
# output is redirected to stderr so the evidence JSON on stdout stays intact;
# the cleanup never aborts the caller and only ever removes worktrees from a
# dedicated per-task worktree (sibling task worktrees are never touched).
run_finalization_cleanup() {
  local here helper repo_root worktree intended
  here="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")" 2>/dev/null && pwd)" || return 0
  helper="$here/workflow_runtime_artifacts.sh"
  [ -f "$helper" ] || return 0
  repo_root="${REPO_PATH:-${RECIPE_VAR_repo_path:-$(pwd)}}"
  worktree="${WORKTREE_SETUP_WORKTREE_PATH:-${RECIPE_VAR_worktree_setup__worktree_path:-}}"
  intended="${BRANCH_NAME:-${RECIPE_VAR_branch_name:-${WORKTREE_SETUP_BRANCH_NAME:-${RECIPE_VAR_worktree_setup__branch_name:-}}}}"
  ( . "$helper" 2>/dev/null && finalize_workflow_cleanup_entry "$repo_root" "$worktree" "$intended" ) >&2 || true
  return 0
}

boolish() {
  case "$1" in
    true|1|yes|y) printf 'true' ;;
    *) printf 'false' ;;
  esac
}

collect_evidence() {
  if ! command -v jq >/dev/null 2>&1; then
    echo "ERROR: collect-finalization-evidence requires jq for structured JSON evidence" >&2
    exit 2
  fi

  target_dir="${WORKTREE_SETUP_WORKTREE_PATH:-${RECIPE_VAR_worktree_setup__worktree_path:-}}"
  if [ -z "$target_dir" ] || [ "$target_dir" = "''" ]; then
    target_dir="${REPO_PATH:-${RECIPE_VAR_repo_path:-.}}"
  fi
  [ -n "$target_dir" ] || target_dir="."

  repo_valid="false"
  dirty_worktree="unknown"
  branch_name="${BRANCH_NAME:-${RECIPE_VAR_branch_name:-}}"
  head_sha=""
  base_ref="${BASE_REF:-${RECIPE_VAR_base_ref:-}}"
  remote_host_type="${REMOTE_HOST_TYPE:-${RECIPE_VAR_remote_host_type:-other}}"
  meaningful_diff="unknown"
  commits_ahead="${TERMINAL_STATE_COMMITS_AHEAD:-${RECIPE_VAR_terminal_state__commits_ahead:-}}"
  missing_tooling=""
  github_remote="false"
  gh_required="false"

  if ! command -v git >/dev/null 2>&1; then
    missing_tooling="${missing_tooling}${missing_tooling:+,}git"
  elif [ -d "$target_dir" ] && git -C "$target_dir" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    repo_valid="true"
    if [ -z "$branch_name" ]; then
      branch_name="$(git -C "$target_dir" branch --show-current)"
    fi
    head_sha="$(git -C "$target_dir" rev-parse --verify HEAD)"
    remote_origin="$(git -C "$target_dir" config --get remote.origin.url 2>/dev/null || true)"
    case "$remote_origin" in
      git@github.com:*|ssh://git@github.com/*|https://github.com/*|http://github.com/*|https://*@github.com/*|http://*@github.com/*) github_remote="true" ;;
    esac
    if [ -n "$(git -C "$target_dir" status --porcelain)" ]; then
      dirty_worktree="true"
    else
      dirty_worktree="false"
    fi
    if [ -z "$base_ref" ]; then
      base_ref="$(git -C "$target_dir" symbolic-ref -q --short refs/remotes/origin/HEAD 2>/dev/null || printf '')"
    fi
    [ -n "$base_ref" ] || base_ref="origin/main"
    if git -C "$target_dir" rev-parse --verify --quiet "${base_ref}^{commit}" >/dev/null; then
      if git -C "$target_dir" diff --quiet "${base_ref}..HEAD"; then
        meaningful_diff="false"
      else
        meaningful_diff="true"
      fi
      if [ -z "$commits_ahead" ]; then
        commits_ahead="$(git -C "$target_dir" rev-list --count "${base_ref}..HEAD")"
      fi
    fi
  fi

  if ! command -v gh >/dev/null 2>&1; then
    missing_tooling="${missing_tooling}${missing_tooling:+,}gh"
  fi

  implementation_completed="$(boolish "${IMPLEMENTATION_COMPLETED:-${IMPLEMENTATION_TERMINAL_EVIDENCE_IMPLEMENTATION_COMPLETED:-${RECIPE_VAR_implementation_terminal_evidence__implementation_completed:-false}}}")"
  verification_completed="$(boolish "${VERIFICATION_COMPLETED:-${VERIFICATION_TERMINAL_EVIDENCE_VERIFICATION_COMPLETED:-${RECIPE_VAR_verification_terminal_evidence__verification_completed:-false}}}")"
  publish_state_reached="$(boolish "${PUBLISH_STATE_REACHED:-${PUBLISH_TERMINAL_EVIDENCE_PUBLISH_STATE_REACHED:-false}}")"
  terminal_no_op="$(boolish "${TERMINAL_NO_OP:-${IMPLEMENTATION_TERMINAL_EVIDENCE_TERMINAL_NO_OP:-${RECIPE_VAR_implementation_terminal_evidence__terminal_no_op:-false}}}")"
  allow_no_op="$(boolish "${ALLOW_NO_OP:-${RECIPE_VAR_allow_no_op:-false}}")"
  prior_terminal_state="${TERMINAL_STATE_TERMINAL_STATE:-${RECIPE_VAR_terminal_state__terminal_state:-${TERMINAL_STATE:-}}}"
  prior_terminal_success="$(boolish "${TERMINAL_STATE_TERMINAL_SUCCESS:-${RECIPE_VAR_terminal_state__terminal_success:-false}}")"
  prior_terminal_reason="${TERMINAL_STATE_TERMINAL_REASON:-${RECIPE_VAR_terminal_state__terminal_reason:-}}"
  branch_diff_status="${TERMINAL_STATE_BRANCH_DIFF_STATUS:-${RECIPE_VAR_terminal_state__branch_diff_status:-$meaningful_diff}}"
  pr_url="${TERMINAL_STATE_PR_URL:-${RECIPE_VAR_terminal_state__pr_url:-${PR_URL:-${PR_PUBLISH_RESULT_PR_URL:-${RECIPE_VAR_pr_publish_result__pr_url:-}}}}}"
  pr_number="${TERMINAL_STATE_PR_NUMBER:-${RECIPE_VAR_terminal_state__pr_number:-${PR_NUMBER:-${PR_PUBLISH_RESULT_PR_NUMBER:-${RECIPE_VAR_pr_publish_result__pr_number:-}}}}}"
  publish_status="${TERMINAL_STATE_PUBLISH_STATUS:-${RECIPE_VAR_terminal_state__publish_status:-${PR_PUBLISH_RESULT_STATE:-${RECIPE_VAR_pr_publish_result__state:-}}}}"
  if [ "$publish_status" = "FOLLOWUP_CREATED" ] || [ -n "$pr_url" ]; then
    publish_state_reached="true"
  fi
  case "$pr_url" in
    https://github.com/*|http://github.com/*|https://*@github.com/*|http://*@github.com/*) github_remote="true" ;;
  esac
  if { [ "$remote_host_type" = "github" ] || [ "$github_remote" = "true" ]; } && { [ "$meaningful_diff" != "false" ] || [ -n "$pr_url" ] || [ -n "$pr_number" ]; }; then
    gh_required="true"
  fi

  jq -nc \
    --arg schema_version "1" \
    --arg repo_path "${REPO_PATH:-${RECIPE_VAR_repo_path:-.}}" \
    --arg worktree_path "$target_dir" \
    --arg repo_valid "$repo_valid" \
    --arg branch_name "$branch_name" \
    --arg head_sha "$head_sha" \
    --arg base_ref "$base_ref" \
    --arg dirty_worktree "$dirty_worktree" \
    --arg meaningful_diff "$meaningful_diff" \
    --arg commits_ahead "$commits_ahead" \
    --arg missing_tooling "$missing_tooling" \
    --arg remote_host_type "$remote_host_type" \
    --arg github_remote "$github_remote" \
    --arg gh_required "$gh_required" \
    --arg pr_url "$pr_url" \
    --arg pr_number "$pr_number" \
    --arg publish_status "$publish_status" \
    --arg implementation_completed "$implementation_completed" \
    --arg verification_completed "$verification_completed" \
    --arg publish_state_reached "$publish_state_reached" \
    --arg terminal_no_op "$terminal_no_op" \
    --arg allow_no_op "$allow_no_op" \
    --arg prior_terminal_state "$prior_terminal_state" \
    --arg prior_terminal_success "$prior_terminal_success" \
    --arg prior_terminal_reason "$prior_terminal_reason" \
    --arg branch_diff_status "$branch_diff_status" \
    --arg observed_phases "workflow-prep,workflow-worktree,workflow-design,workflow-tdd,workflow-refactor-review,workflow-precommit-test,workflow-publish,workflow-pr-review,workflow-finalize" \
    '{
      schema_version: ($schema_version | tonumber),
      git: {
        repo_path: $repo_path,
        worktree_path: $worktree_path,
        repo_valid: $repo_valid,
        branch_name: $branch_name,
        head_sha: $head_sha,
        base_ref: $base_ref,
        dirty_worktree: $dirty_worktree,
        meaningful_diff: $meaningful_diff,
        branch_diff_status: $branch_diff_status,
        commits_ahead: $commits_ahead
      },
      tooling: {
        jq: "present",
        git_missing: ($missing_tooling | split(",") | index("git") != null),
        gh_missing: ($missing_tooling | split(",") | index("gh") != null),
        gh_required: $gh_required,
        missing: $missing_tooling
      },
      remote: {
        host_type: $remote_host_type,
        github_remote: $github_remote
      },
      pr: {
        present: (($pr_url != "") or ($pr_number != "")),
        url: $pr_url,
        number: $pr_number,
        publish_status: $publish_status
      },
      ci: {
        state: (if $prior_terminal_state == "BLOCKED_CI" then "FAILURE" else "UNKNOWN" end)
      },
      completion: {
        implementation_completed: $implementation_completed,
        verification_completed: $verification_completed,
        publish_state_reached: $publish_state_reached,
        terminal_no_op: $terminal_no_op,
        allow_no_op: $allow_no_op
      },
      prior_terminal_state: {
        terminal_success: $prior_terminal_success,
        terminal_state: $prior_terminal_state,
        terminal_reason: $prior_terminal_reason
      },
      observed_phases: ($observed_phases | split(",")),
      agent_outputs: {
        hollow_success_signals: "unknown"
      }
    }'
}

emit_without_jq() {
  printf '{"terminal_success":"false","terminal_state":"FAILED_MISSING_TOOLING","terminal_reason":"jq is required to validate agentic finalizer output","required_next_action":"Install jq or run from an environment with the bundled workflow tooling available.","hollow_success_detected":"false","evidence_used":"tooling.jq=missing","finalizer_schema_version":"0","finalizer_confidence":"low","finalizer_output_valid":"false","terminal_failure":"true"}\n'
}

validate_finalization() {
  if ! command -v jq >/dev/null 2>&1; then
    emit_without_jq
    echo "ERROR: FAILED_MISSING_TOOLING: jq is required to validate agentic finalization" >&2
    exit 1
  fi

  raw_finalizer="${AGENTIC_FINALIZER_OUTPUT:-${RECIPE_VAR_agentic_finalizer_output:-}}"
  finalization_evidence="${FINALIZATION_EVIDENCE:-${RECIPE_VAR_finalization_evidence:-}}"
  evidence_dirty_worktree="${FINALIZATION_EVIDENCE_GIT_DIRTY_WORKTREE:-${RECIPE_VAR_finalization_evidence__git__dirty_worktree:-}}"
  evidence_tooling_missing="${FINALIZATION_EVIDENCE_TOOLING_MISSING:-${RECIPE_VAR_finalization_evidence__tooling__missing:-}}"
  evidence_gh_required="${FINALIZATION_EVIDENCE_TOOLING_GH_REQUIRED:-${RECIPE_VAR_finalization_evidence__tooling__gh_required:-}}"
  evidence_prior_terminal_state="${FINALIZATION_EVIDENCE_PRIOR_TERMINAL_STATE_TERMINAL_STATE:-${RECIPE_VAR_finalization_evidence__prior_terminal_state__terminal_state:-}}"
  finalizer_output_valid="false"
  finalizer_schema_version="0"
  finalizer_confidence="low"
  terminal_state="FAILED_FINALIZER_OUTPUT"
  terminal_success="false"
  terminal_reason="agentic finalizer output was missing or malformed"
  required_next_action="Rerun finalization and inspect the agentic-finalizer step output."
  hollow_success_detected="false"
  evidence_used="finalizer.output=missing"
  terminal_failure="true"

  emit_result() {
    jq -nc \
      --arg terminal_success "$terminal_success" \
      --arg terminal_state "$terminal_state" \
      --arg terminal_reason "$terminal_reason" \
      --arg required_next_action "$required_next_action" \
      --arg hollow_success_detected "$hollow_success_detected" \
      --arg evidence_used "$evidence_used" \
      --arg finalizer_schema_version "$finalizer_schema_version" \
      --arg finalizer_confidence "$finalizer_confidence" \
      --arg finalizer_output_valid "$finalizer_output_valid" \
      --arg implementation_completed "$(boolish "${IMPLEMENTATION_COMPLETED:-${IMPLEMENTATION_TERMINAL_EVIDENCE_IMPLEMENTATION_COMPLETED:-${RECIPE_VAR_implementation_terminal_evidence__implementation_completed:-false}}}")" \
      --arg verification_completed "$(boolish "${VERIFICATION_COMPLETED:-${VERIFICATION_TERMINAL_EVIDENCE_VERIFICATION_COMPLETED:-${RECIPE_VAR_verification_terminal_evidence__verification_completed:-false}}}")" \
      --arg publish_state_reached "$(boolish "${PUBLISH_STATE_REACHED:-${PUBLISH_TERMINAL_EVIDENCE_PUBLISH_STATE_REACHED:-false}}")" \
      --arg terminal_no_op "$(boolish "${TERMINAL_NO_OP:-${IMPLEMENTATION_TERMINAL_EVIDENCE_TERMINAL_NO_OP:-${RECIPE_VAR_implementation_terminal_evidence__terminal_no_op:-false}}}")" \
      --arg terminal_failure "$terminal_failure" \
      --arg pr_url "${TERMINAL_STATE_PR_URL:-${RECIPE_VAR_terminal_state__pr_url:-${PR_URL:-${PR_PUBLISH_RESULT_PR_URL:-${RECIPE_VAR_pr_publish_result__pr_url:-}}}}}" \
      --arg pr_number "${TERMINAL_STATE_PR_NUMBER:-${RECIPE_VAR_terminal_state__pr_number:-${PR_NUMBER:-${PR_PUBLISH_RESULT_PR_NUMBER:-${RECIPE_VAR_pr_publish_result__pr_number:-}}}}}" \
      --arg observed_phases "workflow-prep,workflow-worktree,workflow-design,workflow-tdd,workflow-refactor-review,workflow-precommit-test,workflow-publish,workflow-pr-review,workflow-finalize" \
      --arg missing_evidence "" \
      '{
        terminal_success: $terminal_success,
        terminal_state: $terminal_state,
        terminal_reason: $terminal_reason,
        required_next_action: $required_next_action,
        hollow_success_detected: $hollow_success_detected,
        evidence_used: $evidence_used,
        finalizer_schema_version: $finalizer_schema_version,
        finalizer_confidence: $finalizer_confidence,
        finalizer_output_valid: $finalizer_output_valid,
        implementation_completed: $implementation_completed,
        verification_completed: $verification_completed,
        publish_state_reached: $publish_state_reached,
        terminal_no_op: $terminal_no_op,
        terminal_failure: $terminal_failure,
        pr_url: $pr_url,
        pr_number: $pr_number,
        observed_phases: $observed_phases,
        missing_evidence: $missing_evidence
      }'
  }

  fail_result() {
    terminal_state="$1"
    terminal_reason="$2"
    required_next_action="$3"
    evidence_used="$4"
    terminal_success="false"
    terminal_failure="true"
    emit_result
    echo "ERROR: workflow finalization failed closed: $terminal_state: $terminal_reason" >&2
    exit 1
  }

  [ -n "$raw_finalizer" ] || fail_result "FAILED_FINALIZER_OUTPUT" "AGENTIC_FINALIZER_OUTPUT was empty" "Rerun the agentic-finalizer step and inspect provider/runtime logs." "finalizer.output=missing"
  if ! printf '%s' "$raw_finalizer" | jq -e -s 'length == 1 and (.[0] | type == "object")' >/dev/null; then
    fail_result "FAILED_FINALIZER_OUTPUT" "agentic finalizer output was not a single JSON object" "Return one JSON object with schema_version, terminal_state, terminal_success, confidence, reason, required_next_action, hollow_success_detected, and evidence_used." "finalizer.output=malformed"
  fi
  if ! printf '%s' "$raw_finalizer" | jq -e '
    has("schema_version") and
    has("terminal_state") and (.terminal_state | type == "string") and
    has("terminal_success") and (.terminal_success | type == "boolean") and
    has("confidence") and (.confidence | type == "string") and
    has("reason") and (.reason | type == "string") and
    has("required_next_action") and (.required_next_action | type == "string") and
    has("hollow_success_detected") and (.hollow_success_detected | type == "boolean") and
    has("evidence_used") and (.evidence_used | type == "array")
  ' >/dev/null; then
    fail_result "FAILED_FINALIZER_OUTPUT" "agentic finalizer output is missing required schema fields or uses invalid field types" "Emit schema_version, terminal_state, terminal_success, confidence, reason, required_next_action, hollow_success_detected, and evidence_used with documented types." "finalizer.schema=invalid"
  fi

  finalizer_schema_version="$(printf '%s' "$raw_finalizer" | jq -er '.schema_version | tostring')"
  terminal_state="$(printf '%s' "$raw_finalizer" | jq -er '.terminal_state')"
  terminal_success="$(printf '%s' "$raw_finalizer" | jq -er '.terminal_success | tostring')"
  finalizer_confidence="$(printf '%s' "$raw_finalizer" | jq -er '.confidence')"
  terminal_reason="$(printf '%s' "$raw_finalizer" | jq -er '.reason')"
  required_next_action="$(printf '%s' "$raw_finalizer" | jq -er '.required_next_action')"
  hollow_success_detected="$(printf '%s' "$raw_finalizer" | jq -er '.hollow_success_detected | tostring')"
  evidence_used="$(printf '%s' "$raw_finalizer" | jq -er '.evidence_used | join(",")')"

  if [ -n "$finalization_evidence" ] && printf '%s' "$finalization_evidence" | jq -e 'type == "object"' >/dev/null 2>&1; then
    [ -n "$evidence_dirty_worktree" ] || evidence_dirty_worktree="$(printf '%s' "$finalization_evidence" | jq -r '.git.dirty_worktree // ""')"
    [ -n "$evidence_tooling_missing" ] || evidence_tooling_missing="$(printf '%s' "$finalization_evidence" | jq -r '.tooling.missing // ""')"
    [ -n "$evidence_gh_required" ] || evidence_gh_required="$(printf '%s' "$finalization_evidence" | jq -r '.tooling.gh_required // ""')"
    [ -n "$evidence_prior_terminal_state" ] || evidence_prior_terminal_state="$(printf '%s' "$finalization_evidence" | jq -r '.prior_terminal_state.terminal_state // ""')"
  fi

  [ "$finalizer_schema_version" = "1" ] || fail_result "FAILED_FINALIZER_OUTPUT" "unsupported schema_version: $finalizer_schema_version" "Update the finalizer to emit schema_version 1." "finalizer.schema_version=$finalizer_schema_version"
  case "$finalizer_confidence" in high|medium|low) ;; *) fail_result "FAILED_FINALIZER_OUTPUT" "confidence must be high, medium, or low" "Emit confidence as high, medium, or low." "finalizer.confidence=$finalizer_confidence" ;; esac
  [ -n "$terminal_reason" ] || fail_result "FAILED_FINALIZER_OUTPUT" "reason is empty" "Emit a non-empty evidence-backed reason." "finalizer.reason=empty"
  [ -n "$required_next_action" ] || fail_result "FAILED_FINALIZER_OUTPUT" "required_next_action is empty" "Emit an actionable next step." "finalizer.required_next_action=empty"
  if ! printf '%s' "$raw_finalizer" | jq -e '.evidence_used | length > 0 and all(.[]; type == "string" and length > 0)' >/dev/null; then
    fail_result "FAILED_FINALIZER_OUTPUT" "evidence_used must be a non-empty array of strings" "Emit at least one stable evidence key." "finalizer.evidence_used=invalid"
  fi

  expected_success="false"
  case "$terminal_state" in
    MERGED|CLOSED_OBSOLETE|NO_DIFF_SUCCESS|FOLLOWUP_CREATED|SUPERSEDED|IMPLEMENTED_VERIFIED|ALLOW_NO_OP) expected_success="true" ;;
    BLOCKED_CI|FAILED_DIRTY_WORKTREE|FAILED_MEANINGFUL_DIFF|FAILED_CLOSED_UNMERGED|FAILED_PR_METADATA_UNAVAILABLE|FAILED_MISSING_TOOLING|FAILED_INVALID_EVIDENCE|FAILED_FINALIZER_OUTPUT|FAILED_MISSING_TERMINAL_EVIDENCE|HOLLOW_SUCCESS|INCOMPLETE) expected_success="false" ;;
    *) fail_result "FAILED_FINALIZER_OUTPUT" "unknown terminal_state: $terminal_state" "Emit a terminal_state from the documented vocabulary." "finalizer.terminal_state=$terminal_state" ;;
  esac
  [ "$terminal_success" = "$expected_success" ] || fail_result "FAILED_FINALIZER_OUTPUT" "terminal_success does not match terminal_state semantics" "Align terminal_success with the terminal-state vocabulary." "finalizer.terminal_success=mismatch"
  if [ "$terminal_success" = "true" ] && [ "$finalizer_confidence" != "high" ]; then
    fail_result "FAILED_FINALIZER_OUTPUT" "terminal_success=true requires confidence=high" "Return a non-success state unless the evidence supports high confidence." "finalizer.confidence=$finalizer_confidence"
  fi
  if [ "$terminal_success" = "true" ] && [ "$hollow_success_detected" = "true" ]; then
    fail_result "HOLLOW_SUCCESS" "hollow_success_detected=true cannot produce terminal_success=true" "Continue implementation/verification or report the inaccessible/empty agent output." "finalizer.hollow_success_detected=true"
  fi
  if [ "$terminal_success" = "true" ] && [ "$evidence_dirty_worktree" = "true" ]; then
    fail_result "FAILED_DIRTY_WORKTREE" "collected evidence reported a dirty worktree" "Commit, stash, or remove uncommitted changes before finalization." "finalization_evidence.git.dirty_worktree=true"
  fi
  case ",$evidence_tooling_missing," in
    *,git,*|*,jq,*)
      if [ "$terminal_success" = "true" ]; then
        fail_result "FAILED_MISSING_TOOLING" "collected evidence reported missing deterministic tooling: $evidence_tooling_missing" "Run finalization from an environment with required git and jq tooling available." "finalization_evidence.tooling.missing=$evidence_tooling_missing"
      fi
      ;;
  esac
  if [ "$terminal_success" = "true" ] && [ "$evidence_gh_required" = "true" ]; then
    case ",$evidence_tooling_missing," in
      *,gh,*)
        fail_result "FAILED_MISSING_TOOLING" "GitHub finalization requires gh, but collected evidence reported gh missing" "Install/authenticate gh or rerun from an environment with GitHub PR tooling available." "finalization_evidence.tooling.gh=missing"
        ;;
    esac
  fi

  prior_terminal_state="${TERMINAL_STATE_TERMINAL_STATE:-${RECIPE_VAR_terminal_state__terminal_state:-}}"
  [ -n "$prior_terminal_state" ] || prior_terminal_state="$evidence_prior_terminal_state"
  prior_terminal_success="$(boolish "${TERMINAL_STATE_TERMINAL_SUCCESS:-${RECIPE_VAR_terminal_state__terminal_success:-false}}")"
  target_dir="${WORKTREE_SETUP_WORKTREE_PATH:-${RECIPE_VAR_worktree_setup__worktree_path:-${REPO_PATH:-${RECIPE_VAR_repo_path:-.}}}}"
  if command -v git >/dev/null 2>&1 && [ -d "$target_dir" ] && git -C "$target_dir" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    if [ -n "$(git -C "$target_dir" status --porcelain)" ] && [ "$terminal_success" = "true" ]; then
      fail_result "FAILED_DIRTY_WORKTREE" "dirty worktree prevents terminal success" "Commit, stash, or remove uncommitted changes before finalization." "git.dirty_worktree=true"
    fi
  fi
  case "$prior_terminal_state" in
    BLOCKED_CI|FAILED_MEANINGFUL_DIFF|FAILED_CLOSED_UNMERGED|FAILED_PR_METADATA_UNAVAILABLE|FAILED_INVALID_INPUT|FAILED_WRONG_BRANCH)
      if [ "$terminal_success" = "true" ]; then
        fail_result "$prior_terminal_state" "deterministic terminal probe reported $prior_terminal_state" "Resolve the deterministic blocker before claiming terminal success." "prior_terminal_state=$prior_terminal_state"
      fi
      ;;
  esac

  pr_url="${TERMINAL_STATE_PR_URL:-${RECIPE_VAR_terminal_state__pr_url:-${PR_URL:-${PR_PUBLISH_RESULT_PR_URL:-${RECIPE_VAR_pr_publish_result__pr_url:-}}}}}"
  pr_number="${TERMINAL_STATE_PR_NUMBER:-${RECIPE_VAR_terminal_state__pr_number:-${PR_NUMBER:-${PR_PUBLISH_RESULT_PR_NUMBER:-${RECIPE_VAR_pr_publish_result__pr_number:-}}}}}"
  branch_diff_status="${TERMINAL_STATE_BRANCH_DIFF_STATUS:-${RECIPE_VAR_terminal_state__branch_diff_status:-}}"
  implementation_completed="$(boolish "${IMPLEMENTATION_COMPLETED:-${IMPLEMENTATION_TERMINAL_EVIDENCE_IMPLEMENTATION_COMPLETED:-${RECIPE_VAR_implementation_terminal_evidence__implementation_completed:-false}}}")"
  verification_completed="$(boolish "${VERIFICATION_COMPLETED:-${VERIFICATION_TERMINAL_EVIDENCE_VERIFICATION_COMPLETED:-${RECIPE_VAR_verification_terminal_evidence__verification_completed:-false}}}")"
  allow_no_op="$(boolish "${ALLOW_NO_OP:-${RECIPE_VAR_allow_no_op:-false}}")"

  case "$terminal_state" in
    MERGED)
      [ "$prior_terminal_state" = "MERGED" ] || fail_result "FAILED_INVALID_EVIDENCE" "MERGED requires deterministic merge evidence" "Wait for merge evidence or use a non-success state." "terminal_state=MERGED,prior_terminal_state=$prior_terminal_state"
      ;;
    NO_DIFF_SUCCESS|CLOSED_OBSOLETE)
      if [ "$prior_terminal_success" != "true" ] || { [ "$prior_terminal_state" != "NO_DIFF_SUCCESS" ] && [ "$prior_terminal_state" != "CLOSED_OBSOLETE" ]; }; then
        fail_result "FAILED_INVALID_EVIDENCE" "$terminal_state requires deterministic clean no-diff proof" "Resolve or publish meaningful diffs before claiming no-diff/obsolete success." "terminal_state=$terminal_state,branch_diff_status=$branch_diff_status"
      fi
      ;;
    FOLLOWUP_CREATED)
      [ -n "$pr_url" ] || [ -n "$pr_number" ] || fail_result "FAILED_INVALID_EVIDENCE" "FOLLOWUP_CREATED requires a durable PR or follow-up identifier" "Create or discover the follow-up PR/issue before finalization." "terminal_state=FOLLOWUP_CREATED,pr.present=false"
      ;;
    SUPERSEDED)
      [ -n "$pr_url" ] || [ -n "$pr_number" ] || fail_result "FAILED_INVALID_EVIDENCE" "SUPERSEDED requires a durable replacement PR or issue identifier" "Link the superseding PR/issue before finalization." "terminal_state=SUPERSEDED,replacement.present=false"
      ;;
    IMPLEMENTED_VERIFIED)
      [ "$implementation_completed" = "true" ] && [ "$verification_completed" = "true" ] || fail_result "FAILED_MISSING_TERMINAL_EVIDENCE" "IMPLEMENTED_VERIFIED requires implementation and verification evidence" "Complete implementation and verification or choose a more specific failure state." "implementation_completed=$implementation_completed,verification_completed=$verification_completed"
      ;;
    ALLOW_NO_OP)
      [ "$allow_no_op" = "true" ] || fail_result "FAILED_INVALID_EVIDENCE" "ALLOW_NO_OP requires explicit allow_no_op=true" "Set allow_no_op only for eligible non-code-change tasks with a reason." "allow_no_op=$allow_no_op"
      ;;
  esac

  finalizer_output_valid="true"
  terminal_failure="false"
  if [ "$terminal_success" = "false" ]; then
    terminal_failure="true"
  fi
  emit_result
  if [ "$terminal_success" = "false" ]; then
    echo "ERROR: workflow finalization reached non-success terminal state: $terminal_state: $terminal_reason" >&2
    exit 1
  fi
}

complete_workflow() {
  if ! command -v jq >/dev/null 2>&1; then
    echo "ERROR: workflow-complete requires jq to report workflow_result" >&2
    exit 2
  fi
  jq -n \
    --arg task "${TASK_DESCRIPTION:-}" \
    --arg issue "${ISSUE_NUMBER:-}" \
    --arg pr "${WORKFLOW_RESULT_PR_URL:-${RECIPE_VAR_workflow_result__pr_url:-${PR_URL:-${PR_PUBLISH_RESULT_PR_URL:-${RECIPE_VAR_pr_publish_result__pr_url:-}}}}}" \
    --arg terminal_state "${WORKFLOW_RESULT_TERMINAL_STATE:-${RECIPE_VAR_workflow_result__terminal_state:-FAILED_FINALIZER_OUTPUT}}" \
    --arg terminal_success "${WORKFLOW_RESULT_TERMINAL_SUCCESS:-${RECIPE_VAR_workflow_result__terminal_success:-false}}" \
    --arg terminal_reason "${WORKFLOW_RESULT_TERMINAL_REASON:-${RECIPE_VAR_workflow_result__terminal_reason:-validated workflow_result missing from context}}" \
    --arg required_next_action "${WORKFLOW_RESULT_REQUIRED_NEXT_ACTION:-${RECIPE_VAR_workflow_result__required_next_action:-Inspect validate-agentic-finalization output.}}" \
    --arg hollow_success_detected "${WORKFLOW_RESULT_HOLLOW_SUCCESS_DETECTED:-${RECIPE_VAR_workflow_result__hollow_success_detected:-false}}" \
    --arg evidence_used "${WORKFLOW_RESULT_EVIDENCE_USED:-${RECIPE_VAR_workflow_result__evidence_used:-workflow_result=missing}}" \
    --arg finalizer_schema_version "${WORKFLOW_RESULT_FINALIZER_SCHEMA_VERSION:-${RECIPE_VAR_workflow_result__finalizer_schema_version:-0}}" \
    --arg finalizer_confidence "${WORKFLOW_RESULT_FINALIZER_CONFIDENCE:-${RECIPE_VAR_workflow_result__finalizer_confidence:-low}}" \
    --arg finalizer_output_valid "${WORKFLOW_RESULT_FINALIZER_OUTPUT_VALID:-${RECIPE_VAR_workflow_result__finalizer_output_valid:-false}}" \
    --arg terminal_failure "${WORKFLOW_RESULT_TERMINAL_FAILURE:-${RECIPE_VAR_workflow_result__terminal_failure:-true}}" \
    '{
      workflow: "default-workflow",
      version: "2.0.0",
      task: $task,
      issue_number: $issue,
      pr_url: $pr,
      terminal_outcome: $terminal_state,
      terminal_state: $terminal_state,
      terminal_success: $terminal_success,
      terminal_reason: $terminal_reason,
      workflow_result: {
        terminal_success: $terminal_success,
        terminal_state: $terminal_state,
        terminal_reason: $terminal_reason,
        required_next_action: $required_next_action,
        hollow_success_detected: $hollow_success_detected,
        evidence_used: $evidence_used,
        finalizer_schema_version: $finalizer_schema_version,
        finalizer_confidence: $finalizer_confidence,
        finalizer_output_valid: $finalizer_output_valid,
        terminal_failure: $terminal_failure
      },
      required_next_action: $required_next_action,
      hollow_success_detected: $hollow_success_detected,
      evidence_used: $evidence_used,
      finalizer_schema_version: $finalizer_schema_version,
      finalizer_confidence: $finalizer_confidence,
      finalizer_output_valid: $finalizer_output_valid,
      terminal_failure: $terminal_failure,
      terminal_vocabulary: ["MERGED", "CLOSED_OBSOLETE", "NO_DIFF_SUCCESS", "FOLLOWUP_CREATED", "SUPERSEDED", "IMPLEMENTED_VERIFIED", "ALLOW_NO_OP", "BLOCKED_CI", "FAILED_FINALIZER_OUTPUT", "FAILED_MISSING_TOOLING", "FAILED_PR_METADATA_UNAVAILABLE", "FAILED_DIRTY_WORKTREE", "FAILED_MEANINGFUL_DIFF", "FAILED_CLOSED_UNMERGED", "FAILED_INVALID_EVIDENCE", "FAILED_MISSING_TERMINAL_EVIDENCE", "HOLLOW_SUCCESS", "INCOMPLETE"]
    }'
}

case "$mode" in
  collect) run_finalization_cleanup; collect_evidence ;;
  validate) validate_finalization ;;
  complete) complete_workflow ;;
  *)
    echo "ERROR: workflow_agentic_finalization.sh requires mode: collect, validate, or complete" >&2
    exit 2
    ;;
esac
