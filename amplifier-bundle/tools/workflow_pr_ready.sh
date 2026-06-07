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

sanitize_gh_stderr() {
  sed -E 's#https://[^@[:space:]]+@#https://REDACTED@#g' "$1" | tr '\n' ' ' | head -c 500
}

is_transient_gh_error() {
  [ -s "$1" ] && grep -Eiq 'HTTP 5[0-9][0-9]|(^|[^0-9])(502|503|504)([^0-9]|$)|rate limit|timed out|timeout|temporar|connection reset|connection refused|TLS handshake|network|server error' "$1"
}

gh_with_retry() {
  local label="$1" stderr_file output status attempt delay=1
  shift
  for attempt in 1 2 3; do
    stderr_file=$(mktemp -t step21-gh-XXXXXX)
    if output=$(timeout 60 gh "$@" 2>"$stderr_file"); then
      rm -f "$stderr_file"
      printf '%s\n' "$output"
      return 0
    fi
    status=$?
    if [ "$attempt" -lt 3 ] && is_transient_gh_error "$stderr_file"; then
      echo "WARNING: gh $label failed transiently (exit ${status}); retrying (${attempt}/3): $(sanitize_gh_stderr "$stderr_file")" >&2
      rm -f "$stderr_file"
      sleep "$delay"
      delay=$((delay * 2))
      continue
    fi
    echo "WARNING: gh $label failed (exit ${status})" >&2
    [ ! -s "$stderr_file" ] || echo "gh $label stderr: $(sanitize_gh_stderr "$stderr_file")" >&2
    rm -f "$stderr_file"
    return "$status"
  done
}

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
    if branch_pr_numbers="$(gh_with_retry "pr list" pr list --head "$current_branch" --state all --json number --jq '.[].number')"; then
      while IFS= read -r branch_pr_number; do add_pr_target "$branch_pr_number"; done <<< "$branch_pr_numbers"
    else
      echo "WARNING: unable to discover PRs for branch '$current_branch'; skipping branch discovery" >&2
    fi
  fi
fi
if [ "${#pr_targets[@]}" -eq 0 ]; then
  echo "INFO: [skip] not a git repo or no PR_URL, valid PR_NUMBER, or branch PR found — skipping gh pr ready" >&2
  exit 0
fi

terminal_status="active-pr"
closed_unmerged_seen="false"
for pr_target in "${pr_targets[@]}"; do
  if ! pr_json="$(gh_with_retry "pr view" pr view "$pr_target" --json number,state,isDraft,mergedAt,url)"; then
    echo "WARNING: unable to inspect PR '$pr_target' with gh; skipping this PR" >&2
    continue
  fi
  pr_url=$(printf '%s' "$pr_json" | jq -r '.url // ""')
  pr_state=$(printf '%s' "$pr_json" | jq -r '.state // ""')
  pr_is_draft=$(printf '%s' "$pr_json" | jq -r '.isDraft // false')
  pr_merged_at=$(printf '%s' "$pr_json" | jq -r '.mergedAt // ""')

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
    if ready_output="$(gh_with_retry "pr ready" pr ready "$pr_url")"; then
      printf '%s\n' "$ready_output"
    else
      echo "WARNING: gh pr ready failed for '$pr_url'; continuing with visible skip" >&2
      continue
    fi
  else
    echo "INFO: PR is already ready for review"
  fi

  ready_body="## Ready for Final Review

Workflow steps completed: requirements, design, implementation, tests, code review, philosophy compliance, cleanup, and quality audit.

Ready for merge approval."
  if comment_output="$(gh_with_retry "pr comment" pr comment "$pr_url" --body "$ready_body")"; then
    printf '%s\n' "$comment_output"
  else
    echo "WARNING: gh pr comment failed for '$pr_url'; PR ready state was still evaluated" >&2
  fi
done

if [ "$closed_unmerged_seen" = "true" ]; then
  exit 1
fi
echo "=== PR Ready Step Complete (terminal_status=$terminal_status) ==="
