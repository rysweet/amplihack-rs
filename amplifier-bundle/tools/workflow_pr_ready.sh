#!/usr/bin/env bash
set -euo pipefail

export GIT_PAGER=cat GH_PAGER=cat PAGER=cat LESS=FRX
PR_URL="${PR_URL:-${RECIPE_VAR_pr_url:-${PR_PUBLISH_RESULT_PR_URL:-${RECIPE_VAR_pr_publish_result__pr_url:-}}}}"
PR_NUMBER="${PR_NUMBER:-${RECIPE_VAR_pr_number:-${PR_PUBLISH_RESULT_PR_NUMBER:-${RECIPE_VAR_pr_publish_result__pr_number:-}}}}"

if ! command -v gh >/dev/null 2>&1; then
  echo "WARNING: gh CLI not found — skipping PR ready conversion" >&2
  exit 0
fi
if ! timeout 30 gh auth status >/dev/null 2>&1; then
  echo "WARNING: gh CLI is unavailable or unauthenticated — skipping PR ready conversion" >&2
  exit 0
fi
if ! command -v jq >/dev/null 2>&1; then
  echo "WARNING: jq CLI not found — skipping PR ready conversion" >&2
  exit 0
fi

pr_targets=()
add_pr_target() {
  local target="$1" existing
  [[ "$target" =~ [^[:space:]] ]] || return 0
  for existing in "${pr_targets[@]}"; do [ "$existing" = "$target" ] && return 0; done
  pr_targets+=("$target")
}

[[ "$PR_URL" =~ [^[:space:]] ]] && add_pr_target "$PR_URL"
if [[ "$PR_NUMBER" =~ ^[1-9][0-9]*$ ]]; then
  add_pr_target "$PR_NUMBER"
elif [[ "$PR_NUMBER" =~ [^[:space:]] ]]; then
  echo "WARNING: ignoring invalid PR_NUMBER '$PR_NUMBER' for workflow_pr_ready.sh" >&2
fi
if [ "${#pr_targets[@]}" -eq 0 ] && git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  current_branch=$(git branch --show-current 2>/dev/null || true)
  if [[ "$current_branch" =~ [^[:space:]] ]]; then
    while IFS= read -r branch_pr_number; do add_pr_target "$branch_pr_number"; done < <(timeout 60 gh pr list --head "$current_branch" --state all --json number --jq '.[].number' 2>/dev/null || true)
  fi
fi
if [ "${#pr_targets[@]}" -eq 0 ]; then
  echo "INFO: [skip] not a git repo or no PR_URL, valid PR_NUMBER, or branch PR found — skipping gh pr ready" >&2
  exit 0
fi

terminal_status="active-pr"
closed_unmerged_seen="false"
for pr_target in "${pr_targets[@]}"; do
  pr_json_file=$(mktemp -t step21-pr-view-XXXXXX)
  if ! timeout 60 gh pr view "$pr_target" --json number,state,isDraft,mergedAt,url >"$pr_json_file" 2>&1; then
    view_rc=$?
    echo "WARNING: unable to inspect PR '$pr_target' with gh (exit $view_rc); skipping this PR" >&2
    cat "$pr_json_file" >&2
    rm -f "$pr_json_file"
    continue
  fi
  pr_url=$(jq -r '.url // ""' "$pr_json_file")
  pr_state=$(jq -r '.state // ""' "$pr_json_file")
  pr_is_draft=$(jq -r '.isDraft // false' "$pr_json_file")
  pr_merged_at=$(jq -r '.mergedAt // ""' "$pr_json_file")
  rm -f "$pr_json_file"

  if [ "$pr_state" = "MERGED" ] || [ -n "$pr_merged_at" ]; then
    terminal_status="already-merged"
    [ "$pr_state" = "CLOSED" ] && terminal_status="closed-after-merge"
    echo "INFO: terminal_status=$terminal_status; PR is already merged — no ready-for-review action needed"
    continue
  fi
  if [ "$pr_state" = "CLOSED" ]; then
    terminal_status="closed-unmerged"
    closed_unmerged_seen="true"
    echo "ERROR: terminal_status=closed-unmerged; PR is closed without merge — reopen it or create a new branch intentionally" >&2
    continue
  fi

  if [ "$pr_is_draft" = "true" ]; then
    ready_output_file=$(mktemp -t step21-pr-ready-XXXXXX)
    if timeout 60 gh pr ready "$pr_url" >"$ready_output_file" 2>&1; then
      cat "$ready_output_file"
      rm -f "$ready_output_file"
    else
      ready_rc=$?
      echo "WARNING: gh pr ready failed for '$pr_url' (exit $ready_rc); continuing with visible skip" >&2
      cat "$ready_output_file" >&2
      rm -f "$ready_output_file"
      continue
    fi
  else
    echo "INFO: PR is already ready for review"
  fi

  ready_body="## Ready for Final Review

Workflow steps completed: requirements, design, implementation, tests, code review, philosophy compliance, cleanup, and quality audit.

Ready for merge approval."
  comment_output_file=$(mktemp -t step21-pr-comment-XXXXXX)
  if timeout 60 gh pr comment "$pr_url" --body "$ready_body" >"$comment_output_file" 2>&1; then
    cat "$comment_output_file"
  else
    comment_rc=$?
    echo "WARNING: gh pr comment failed for '$pr_url' (exit $comment_rc); PR ready state was still evaluated" >&2
    cat "$comment_output_file" >&2
  fi
  rm -f "$comment_output_file"
done

if [ "$closed_unmerged_seen" = "true" ]; then
  exit 1
fi
echo "=== PR Ready Step Complete (terminal_status=$terminal_status) ==="
