#!/usr/bin/env bash
set -euo pipefail

echo "=== WORKFLOW COMPLETE ==="
echo ""

export GH_PAGER=cat PAGER=cat LESS=FRX
HOST_TYPE="${REMOTE_HOST_TYPE:-other}"
PR_URL="${PR_URL:-${PR_PUBLISH_RESULT_PR_URL:-${RECIPE_VAR_pr_publish_result__pr_url:-}}}"
PUBLISH_STATE="${PR_PUBLISH_RESULT_STATE:-${RECIPE_VAR_pr_publish_result__state:-}}"
terminal_status="${PUBLISH_STATE:-active-pr}"
final_status_rc=0

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

case "$terminal_status" in
  MERGED|CLOSED_OBSOLETE|NO_DIFF_SUCCESS|closed-after-merge|already-merged|no-diff) echo "terminal_status=$terminal_status" ;;
esac

if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  BASE_REF="$(git symbolic-ref -q --short refs/remotes/origin/HEAD 2>/dev/null || true)"
  [ -n "$BASE_REF" ] || BASE_REF="origin/main"
  if git rev-parse --verify --quiet "${BASE_REF}^{commit}" >/dev/null && [ -z "$(git status --porcelain)" ] && git diff --quiet "${BASE_REF}..HEAD"; then
    if [ "$terminal_status" = "CLOSED_OBSOLETE" ]; then
      echo "terminal_status=CLOSED_OBSOLETE"
    else
      terminal_status="NO_DIFF_SUCCESS"
      echo "terminal_status=NO_DIFF_SUCCESS"
    fi
  elif [ "$terminal_status" = "no-diff" ] || [ "$terminal_status" = "NO_DIFF_SUCCESS" ] || [ "$terminal_status" = "CLOSED_OBSOLETE" ]; then
    echo "ERROR: publish reported terminal no-diff/obsolete state but final clean-worktree diff could not confirm that state" >&2
    final_status_rc=1
  fi
fi

if [ -z "$PR_URL" ]; then
  if [ "$HOST_TYPE" = "azdo" ]; then
    echo "PR Status: N/A (manual creation required)" >&2
  else
    echo "PR Status: N/A (no remote provider)" >&2
  fi
elif [ "$HOST_TYPE" = "github" ]; then
  echo "PR Status:"
  if command -v gh >/dev/null 2>&1; then
    gh_pr_view_with_retry "$PR_URL" --json state,mergeable,reviews,statusCheckRollup || true
  else
    echo "WARNING: gh CLI not found; skipping final PR status lookup" >&2
  fi
else
  echo "WARNING: PR_URL set but host is '$HOST_TYPE'; skipping gh pr view" >&2
fi

echo ""
TASK_DESC="$TASK_DESCRIPTION"
ISSUE_NUMBER="$ISSUE_NUMBER"
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
