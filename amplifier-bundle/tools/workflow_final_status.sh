#!/usr/bin/env bash
set -euo pipefail

echo "=== WORKFLOW COMPLETE ==="
echo ""

export GH_PAGER=cat PAGER=cat LESS=FRX

HOST_TYPE="${REMOTE_HOST_TYPE:-other}"
PR_URL="${PR_URL:-${PR_PUBLISH_RESULT_PR_URL:-${RECIPE_VAR_pr_publish_result__pr_url:-}}}"
PR_NUMBER="${PR_NUMBER:-${PR_PUBLISH_RESULT_PR_NUMBER:-${RECIPE_VAR_pr_publish_result__pr_number:-}}}"
PUBLISH_STATE="${PR_PUBLISH_RESULT_STATE:-${RECIPE_VAR_pr_publish_result__state:-}}"
TASK_DESC="${TASK_DESCRIPTION:-}"
ISSUE_NUMBER="${ISSUE_NUMBER:-}"

terminal_success="false"
terminal_state_out=""
terminal_reason_out=""
terminal_failure="false"
implementation_completed="false"
verification_completed="false"
publish_state_reached="false"
terminal_no_op="false"
missing_evidence=""
invalid_evidence=""
normalized_bool="false"
terminal_status="${PUBLISH_STATE:-active-pr}"
final_status_rc=0
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PR_SCOPE_HELPER="${WORKFLOW_PR_SCOPE_HELPER:-${SCRIPT_DIR}/workflow_pr_scope.sh}"
# workflow_pr_scope.sh validates headRefName, baseRefName, headRefOid,
# isCrossRepository, expected_pr_title_prefix, and created_after.

sanitize_gh_stderr() {
  sed -E 's#(https?://)[^@[:space:]]+@#\1REDACTED@#g' "$1" | tr '\n' ' ' | head -c 500
}

is_transient_gh_error() {
  [ -s "$1" ] && grep -Eiq 'HTTP 5[0-9][0-9]|(^|[^0-9])(502|503|504)([^0-9]|$)|rate limit|timed out|timeout|temporar|connection reset|connection refused|TLS handshake|network|server error' "$1"
}

gh_pr_view_with_retry() {
  local stderr_file output status attempt delay=1
  for attempt in 1 2 3; do
    stderr_file=$(mktemp -t step22b-gh-pr-view-XXXXXX)
    if output=$(timeout 60 gh pr view "$@" 2>"$stderr_file"); then
      rm -f "$stderr_file"
      printf '%s\n' "$output"
      return 0
    else
      status=$?
    fi
    if [ "$attempt" -lt 3 ] && is_transient_gh_error "$stderr_file"; then
      echo "WARNING: final PR status lookup failed transiently (exit ${status}); retrying (${attempt}/3): $(sanitize_gh_stderr "$stderr_file")" >&2
      rm -f "$stderr_file"
      sleep "$delay"
      delay=$((delay * 2))
      continue
    fi
    echo "WARNING: final PR status lookup failed (exit ${status}); continuing with terminal-state result" >&2
    [ ! -s "$stderr_file" ] || echo "gh pr view stderr: $(sanitize_gh_stderr "$stderr_file")" >&2
    rm -f "$stderr_file"
    return "$status"
  done
}

normalize_bool() {
  local name="$1"
  local raw="$2"
  local value
  value="$(printf '%s' "$raw" | tr '[:upper:]' '[:lower:]')"
  case "$value" in
    ""|"false"|"0"|"no"|"n") normalized_bool="false" ;;
    "true"|"1"|"yes"|"y") normalized_bool="true" ;;
    *)
      invalid_evidence="${invalid_evidence}${invalid_evidence:+,}${name}"
      normalized_bool="false"
      ;;
  esac
}

join_missing_evidence() {
  local result=""
  [ "$implementation_completed" = "true" ] || result="${result}${result:+,}implementation_completed"
  [ "$verification_completed" = "true" ] || result="${result}${result:+,}verification_completed"
  [ "$publish_state_reached" = "true" ] || result="${result}${result:+,}publish_state_reached"
  [ "$terminal_no_op" = "true" ] || result="${result}${result:+,}terminal_no_op"
  printf '%s\n' "$result"
}

emit_terminal_evidence() {
  echo "terminal_success=$terminal_success"
  echo "terminal_state=$terminal_state_out"
  [ -z "$terminal_reason_out" ] || echo "terminal_reason=$terminal_reason_out"
  echo "implementation_completed=$implementation_completed"
  echo "verification_completed=$verification_completed"
  echo "publish_state_reached=$publish_state_reached"
  echo "terminal_no_op=$terminal_no_op"
  echo "terminal_failure=$terminal_failure"
  [ -z "${OBSERVED_PHASES:-}" ] || echo "observed_phases=${OBSERVED_PHASES}"
  [ -z "$missing_evidence" ] || echo "missing_evidence=$missing_evidence"
  [ -z "$invalid_evidence" ] || echo "invalid_evidence=$invalid_evidence"
}

impl_input="${IMPLEMENTATION_COMPLETED:-${IMPLEMENTATION_TERMINAL_EVIDENCE_IMPLEMENTATION_COMPLETED:-${RECIPE_VAR_implementation_terminal_evidence__implementation_completed:-}}}"
verify_input="${VERIFICATION_COMPLETED:-${VERIFICATION_TERMINAL_EVIDENCE_VERIFICATION_COMPLETED:-${RECIPE_VAR_verification_terminal_evidence__verification_completed:-}}}"
publish_input="${PUBLISH_STATE_REACHED:-${PUBLISH_TERMINAL_EVIDENCE_PUBLISH_STATE_REACHED:-${RECIPE_VAR_publish_terminal_evidence__publish_state_reached:-}}}"
no_op_input="${TERMINAL_NO_OP:-${IMPLEMENTATION_TERMINAL_EVIDENCE_TERMINAL_NO_OP:-${RECIPE_VAR_implementation_terminal_evidence__terminal_no_op:-${VERIFICATION_TERMINAL_EVIDENCE_TERMINAL_NO_OP:-${RECIPE_VAR_verification_terminal_evidence__terminal_no_op:-}}}}}"
failure_input="${TERMINAL_FAILURE:-${TERMINAL_STATE_TERMINAL_FAILURE:-${RECIPE_VAR_terminal_state__terminal_failure:-}}}"
allow_no_op_input="${ALLOW_NO_OP:-${RECIPE_VAR_allow_no_op:-false}}"
probe_success_input="${TERMINAL_STATE_TERMINAL_SUCCESS:-${RECIPE_VAR_terminal_state__terminal_success:-}}"
probe_state_input="${TERMINAL_STATE:-${TERMINAL_STATE_TERMINAL_STATE:-${RECIPE_VAR_terminal_state__terminal_state:-}}}"

normalize_bool "IMPLEMENTATION_COMPLETED" "$impl_input"
implementation_completed="$normalized_bool"
normalize_bool "VERIFICATION_COMPLETED" "$verify_input"
verification_completed="$normalized_bool"
normalize_bool "PUBLISH_STATE_REACHED" "$publish_input"
publish_state_reached="$normalized_bool"
normalize_bool "TERMINAL_NO_OP" "$no_op_input"
terminal_no_op="$normalized_bool"
normalize_bool "TERMINAL_FAILURE" "$failure_input"
terminal_failure="$normalized_bool"
normalize_bool "ALLOW_NO_OP" "$allow_no_op_input"
allow_no_op="$normalized_bool"
normalize_bool "TERMINAL_STATE_TERMINAL_SUCCESS" "$probe_success_input"
probe_success="$normalized_bool"

if [ "$allow_no_op" = "true" ]; then
  terminal_no_op="true"
fi

if [ "$probe_success" = "true" ]; then
  case "$probe_state_input" in
    MERGED|CLOSED_OBSOLETE|NO_DIFF_SUCCESS)
      terminal_no_op="true"
      ;;
  esac
fi

case "$PUBLISH_STATE" in
  no-diff|NO_DIFF_SUCCESS|CLOSED_OBSOLETE)
    if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
      RUNTIME_ARTIFACT_HELPER="${WORKFLOW_RUNTIME_ARTIFACT_HELPER:-${SCRIPT_DIR}/workflow_runtime_artifacts.sh}"
      [ -f "$RUNTIME_ARTIFACT_HELPER" ] || { echo "ERROR: workflow runtime artifact helper not found: $RUNTIME_ARTIFACT_HELPER" >&2; exit 2; }
      # shellcheck source=/dev/null
      . "$RUNTIME_ARTIFACT_HELPER"
      preflight_known_workflow_runtime_artifacts .
      base_ref="$(git symbolic-ref -q --short refs/remotes/origin/HEAD 2>/dev/null || true)"
      [ -n "$base_ref" ] || base_ref="origin/main"
      if git rev-parse --verify --quiet "${base_ref}^{commit}" >/dev/null && [ -z "$(git status --porcelain)" ] && git diff --quiet "${base_ref}..HEAD"; then
        if [ "$PUBLISH_STATE" = "no-diff" ]; then
          PUBLISH_STATE="NO_DIFF_SUCCESS"
        fi
      else
        echo "ERROR: publish reported terminal no-diff/obsolete state but final clean-worktree diff could not confirm that state" >&2
        exit 1
      fi
    else
      echo "ERROR: publish reported terminal no-diff/obsolete state but final clean-worktree diff could not confirm that state" >&2
      exit 1
    fi
    ;;
esac

parse_github_repo_identity() {
  local url="$1" path owner repo
  case "$url" in
    git@github.com:*) path="${url#git@github.com:}" ;;
    ssh://git@github.com/*) path="${url#ssh://git@github.com/}" ;;
    https://*@github.com/*|http://*@github.com/*) path="${url#*://}"; path="${path#*@github.com/}" ;;
    https://github.com/*) path="${url#https://github.com/}" ;;
    http://github.com/*) path="${url#http://github.com/}" ;;
    *) return 1 ;;
  esac
  path="${path%%\?*}"
  path="${path%%#*}"
  path="${path%.git}"
  case "$path" in */*) ;; *) return 1 ;; esac
  owner="${path%%/*}"
  repo="${path#*/}"
  repo="${repo%%/*}"
  [ -n "$owner" ] && [ -n "$repo" ] || return 1
  printf '%s/%s\n' "$owner" "$repo"
}

resolve_expected_base_branch() {
  local candidate
  candidate="$(git symbolic-ref -q --short refs/remotes/origin/HEAD 2>/dev/null || true)"
  if [ -n "$candidate" ] && git rev-parse --verify --quiet "${candidate}^{commit}" >/dev/null; then printf '%s\n' "${candidate#origin/}"; return 0; fi
  for candidate in origin/main origin/master origin/develop; do
    if git rev-parse --verify --quiet "${candidate}^{commit}" >/dev/null; then printf '%s\n' "${candidate#origin/}"; return 0; fi
  done
  return 1
}

validate_final_pr_scope() {
  local current_branch local_head repo_identity expected_base scoped_json reason created_after
  [ -x "$PR_SCOPE_HELPER" ] || { echo "ERROR: workflow_pr_scope.sh missing or not executable at $PR_SCOPE_HELPER" >&2; return 1; }
  command -v jq >/dev/null 2>&1 || { echo "ERROR: jq CLI not found — cannot validate scoped final PR metadata" >&2; return 127; }
  current_branch="$(git branch --show-current 2>/dev/null || true)"
  local_head="$(git rev-parse --verify HEAD 2>/dev/null || true)"
  repo_identity="$(parse_github_repo_identity "$(git config --get remote.origin.url 2>/dev/null || true)" || true)"
  expected_base="$(resolve_expected_base_branch || true)"
  [ -n "$repo_identity" ] && [ -n "$current_branch" ] && [ -n "$local_head" ] && [ -n "$expected_base" ] || {
    echo "ERROR: scoped final PR validation lacks repo, branch, headRefOid, or baseRefName context" >&2
    return 1
  }
  created_after="${WORKFLOW_STARTED_AT:-${RECIPE_STARTED_AT:-${TASK_STARTED_AT:-}}}"
  scope_args=(
    --repo "$repo_identity"
    --head "$current_branch"
    --base "$expected_base"
    --issue "${ISSUE_NUMBER:-${RECIPE_VAR_issue_number:-}}"
    --work-item "${ISSUE_NUMBER:-${RECIPE_VAR_issue_number:-}}"
    --head-sha "$local_head"
  )
  title_prefix="${EXPECTED_PR_TITLE_PREFIX:-${PR_EXPECTED_TITLE_PREFIX:-}}"
  [ -n "$title_prefix" ] && scope_args+=(--expected-pr-title-prefix "$title_prefix")
  [ -n "$PR_URL" ] && scope_args+=(--pr-url "$PR_URL")
  [[ "$PR_NUMBER" =~ ^[1-9][0-9]*$ ]] && scope_args+=(--pr-number "$PR_NUMBER")
  [ -n "$created_after" ] && scope_args+=(--created-after "$created_after")
  if scoped_json="$("$PR_SCOPE_HELPER" "${scope_args[@]}")"; then
    PR_URL="$(printf '%s' "$scoped_json" | jq -r '.url // empty')"
    PR_NUMBER="$(printf '%s' "$scoped_json" | jq -r '(.number // "") | tostring')"
    return 0
  fi
  reason="$(printf '%s' "$scoped_json" | jq -r '.reason // ""' 2>/dev/null || true)"
  echo "ERROR: scoped final PR validation failed: ${reason:-unknown}" >&2
  return 1
}

case "$PUBLISH_STATE" in
  FOLLOWUP_CREATED|MERGED|CLOSED_OBSOLETE|NO_DIFF_SUCCESS)
    publish_state_reached="true"
    ;;
esac
if [ -n "$PR_URL" ]; then
  publish_state_reached="true"
fi

if [ -n "$invalid_evidence" ]; then
  terminal_success="false"
  terminal_state_out="FAILED_INVALID_EVIDENCE"
  terminal_reason_out="invalid boolean evidence marker(s): $invalid_evidence"
  emit_terminal_evidence
  echo "ERROR: workflow terminal evidence contains invalid boolean value(s): $invalid_evidence" >&2
  exit 2
fi

if [ "$terminal_failure" = "true" ]; then
  terminal_success="false"
  terminal_state_out="${TERMINAL_STATE:-${TERMINAL_STATE_TERMINAL_STATE:-${RECIPE_VAR_terminal_state__terminal_state:-TERMINAL_FAILURE}}}"
  terminal_reason_out="${TERMINAL_REASON:-${TERMINAL_STATE_TERMINAL_REASON:-${RECIPE_VAR_terminal_state__terminal_reason:-explicit terminal failure evidence was reported}}}"
  emit_terminal_evidence
  echo "ERROR: workflow terminal failure: $terminal_state_out: $terminal_reason_out" >&2
  exit 1
fi

if [ "$terminal_no_op" = "true" ]; then
  terminal_success="true"
  terminal_state_out="${TERMINAL_STATE:-${TERMINAL_STATE_TERMINAL_STATE:-${RECIPE_VAR_terminal_state__terminal_state:-ALLOW_NO_OP}}}"
  terminal_reason_out="${TERMINAL_REASON:-${TERMINAL_STATE_TERMINAL_REASON:-${RECIPE_VAR_terminal_state__terminal_reason:-allow_no_op was explicitly selected for a non-code-change path}}}"
elif [ "$publish_state_reached" = "true" ]; then
  terminal_success="true"
  terminal_state_out="${TERMINAL_STATE:-${TERMINAL_STATE_TERMINAL_STATE:-${RECIPE_VAR_terminal_state__terminal_state:-${PUBLISH_STATE:-FOLLOWUP_CREATED}}}}"
  terminal_reason_out="${TERMINAL_REASON:-${TERMINAL_STATE_TERMINAL_REASON:-${RECIPE_VAR_terminal_state__terminal_reason:-${PR_PUBLISH_RESULT_MESSAGE:-${RECIPE_VAR_pr_publish_result__message:-publish/PR state reached}}}}}"
elif [ "$implementation_completed" = "true" ] && [ "$verification_completed" = "true" ]; then
  terminal_success="true"
  terminal_state_out="${TERMINAL_STATE:-${TERMINAL_STATE_TERMINAL_STATE:-${RECIPE_VAR_terminal_state__terminal_state:-IMPLEMENTED_VERIFIED}}}"
  terminal_reason_out="${TERMINAL_REASON:-${TERMINAL_STATE_TERMINAL_REASON:-${RECIPE_VAR_terminal_state__terminal_reason:-implementation and verification evidence present}}}"
else
  terminal_success="false"
  terminal_state_out="FAILED_MISSING_TERMINAL_EVIDENCE"
  terminal_reason_out="development workflow reached closure without implementation+verification, publish/PR state, or explicit no-op evidence"
  missing_evidence="$(join_missing_evidence)"
  emit_terminal_evidence
  echo "ERROR: workflow terminal evidence is incomplete for a development/code-change workflow." >&2
  echo "ERROR: missing_evidence=$missing_evidence" >&2
  echo "ERROR: observed_phases=${OBSERVED_PHASES:-unknown}" >&2
  echo "ERROR: expected one of: implementation_completed=true with verification_completed=true; publish_state_reached=true; terminal_no_op=true; or terminal_failure=true." >&2
  exit 1
fi

emit_terminal_evidence

echo ""
case "$terminal_state_out" in
  MERGED|CLOSED_OBSOLETE|NO_DIFF_SUCCESS|FOLLOWUP_CREATED|IMPLEMENTED_VERIFIED|ALLOW_NO_OP|closed-after-merge|already-merged|no-diff)
    echo "terminal_status=$terminal_state_out"
    ;;
esac

if [ -z "$PR_URL" ]; then
  if [ "$HOST_TYPE" = "azdo" ]; then
    echo "PR Status: N/A (manual creation required)" >&2
  else
    echo "PR Status: N/A (no remote provider)" >&2
  fi
elif [ "$HOST_TYPE" = "github" ]; then
  echo "PR Status:"
  if command -v gh >/dev/null 2>&1; then
    if validate_final_pr_scope; then
      gh_pr_view_with_retry "$PR_URL" --json state,mergeable,reviews,statusCheckRollup || true
    else
      final_status_rc=1
    fi
  else
    echo "WARNING: gh CLI not found; skipping final PR status lookup" >&2
  fi
else
  echo "WARNING: PR_URL set but host is '$HOST_TYPE'; skipping gh pr view" >&2
fi

echo ""
printf '=== Task: %s ===\n' "$TASK_DESC"
case "$HOST_TYPE" in
  github) echo "=== Issue: #${ISSUE_NUMBER} ===" ;;
  azdo) echo "=== Issue: AB#${ISSUE_NUMBER} ===" ;;
  *) echo "=== Issue: Ref #${ISSUE_NUMBER} ===" ;;
esac

if [ -n "$PR_URL" ]; then
  printf '=== PR: %s ===\n' "$PR_URL"
elif [ "$HOST_TYPE" = "azdo" ]; then
  echo "=== PR: N/A (manual creation required) ==="
else
  echo "=== PR: N/A (no remote provider) ==="
fi

echo ""
if [ "$final_status_rc" -ne 0 ]; then
  echo "Workflow final status failed; terminal success was not proven." >&2
  exit "$final_status_rc"
fi

echo "All 23 workflow steps completed successfully."
