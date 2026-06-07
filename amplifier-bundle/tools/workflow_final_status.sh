#!/usr/bin/env bash
set -euo pipefail

echo "=== WORKFLOW COMPLETE ==="
echo ""

export GH_PAGER=cat PAGER=cat LESS=FRX
HOST_TYPE="${REMOTE_HOST_TYPE:-other}"
PR_URL="${PR_URL:-${PR_PUBLISH_RESULT_PR_URL:-${RECIPE_VAR_pr_publish_result__pr_url:-}}}"
PUBLISH_STATE="${PR_PUBLISH_RESULT_STATE:-${RECIPE_VAR_pr_publish_result__state:-}}"
terminal_status="${PUBLISH_STATE:-active-pr}"

case "$terminal_status" in
  closed-after-merge|already-merged|no-diff) echo "terminal_status=$terminal_status" ;;
esac

if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  BASE_REF="$(git symbolic-ref -q --short refs/remotes/origin/HEAD 2>/dev/null || true)"
  [ -n "$BASE_REF" ] || BASE_REF="origin/main"
  if git rev-parse --verify --quiet "${BASE_REF}^{commit}" >/dev/null && git diff --quiet "${BASE_REF}..HEAD"; then
    terminal_status="no-diff"
    echo "terminal_status=no-diff"
  elif [ "$terminal_status" = "no-diff" ]; then
    echo "WARNING: publish reported no-diff but final diff could not confirm that state" >&2
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
    timeout 60 gh pr view "$PR_URL" --json state,mergeable,reviews,statusCheckRollup
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
echo "All 23 workflow steps completed successfully."
