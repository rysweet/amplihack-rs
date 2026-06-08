#!/usr/bin/env bash
set -euo pipefail

export GIT_PAGER=cat GH_PAGER=cat PAGER=cat LESS=FRX
PR_URL="${PR_URL:-${RECIPE_VAR_pr_url:-${PR_PUBLISH_RESULT_PR_URL:-${RECIPE_VAR_pr_publish_result__pr_url:-}}}}"
PR_NUMBER="${PR_NUMBER:-${RECIPE_VAR_pr_number:-${PR_PUBLISH_RESULT_PR_NUMBER:-${RECIPE_VAR_pr_publish_result__pr_number:-}}}}"

if ! command -v gh >/dev/null 2>&1; then
  echo "ERROR: gh CLI not found — cannot verify or mutate GitHub PR readiness" >&2
  exit 127
fi
if ! timeout 30 gh auth status >/dev/null 2>&1; then
  echo "ERROR: gh CLI is unavailable or unauthenticated — cannot verify or mutate GitHub PR readiness" >&2
  exit 1
fi
if ! command -v jq >/dev/null 2>&1; then
  echo "ERROR: jq CLI not found — cannot validate GitHub PR readiness metadata" >&2
  exit 127
fi

sanitize_gh_stderr() {
  sed -E 's#(https?://)[^@[:space:]]+@#\1REDACTED@#g' "$1" | tr '\n' ' ' | head -c 500
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
    else
      status=$?
    fi
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

validate_pr_identity_before_mutation() {
  local pr_json="$1" pr_url="$2" current_branch local_head repo_identity pr_base_identity expected_base
  local pr_head_ref pr_base_ref pr_head_oid pr_head_owner pr_head_repo pr_cross_repo

  git rev-parse --is-inside-work-tree >/dev/null 2>&1 || {
    echo "ERROR: refusing to mutate PR '$pr_url'; workflow_pr_ready.sh is not running inside a git worktree" >&2
    return 1
  }
  current_branch="$(git branch --show-current 2>/dev/null || true)"
  [ -n "$current_branch" ] || {
    echo "ERROR: refusing to mutate PR '$pr_url'; current branch is empty" >&2
    return 1
  }
  local_head="$(git rev-parse --verify HEAD 2>/dev/null)" || {
    echo "ERROR: refusing to mutate PR '$pr_url'; local HEAD does not resolve" >&2
    return 1
  }
  repo_identity="$(parse_github_repo_identity "$(git config --get remote.origin.url 2>/dev/null || true)" || true)"
  [ -n "$repo_identity" ] || {
    echo "ERROR: refusing to mutate PR '$pr_url'; unable to determine current GitHub repo identity from origin remote" >&2
    return 1
  }
  pr_base_identity="$(parse_github_repo_identity "$pr_url" || true)"
  if [ "$pr_base_identity" != "$repo_identity" ]; then
    echo "ERROR: refusing to mutate PR '$pr_url'; PR URL repo '${pr_base_identity:-unknown}' does not match current repo '$repo_identity'" >&2
    return 1
  fi

  pr_head_ref=$(printf '%s' "$pr_json" | jq -r '.headRefName // ""')
  pr_base_ref=$(printf '%s' "$pr_json" | jq -r '.baseRefName // ""')
  pr_head_oid=$(printf '%s' "$pr_json" | jq -r '.headRefOid // ""')
  pr_head_owner=$(printf '%s' "$pr_json" | jq -r '.headRepositoryOwner.login // .headRepositoryOwner.name // .headRepositoryOwner // ""')
  pr_head_repo=$(printf '%s' "$pr_json" | jq -r '.headRepository.name // ((.headRepository.nameWithOwner // "") | split("/") | .[-1]) // ""')
  pr_cross_repo=$(printf '%s' "$pr_json" | jq -r '.isCrossRepository // false')

  if [ -z "$pr_head_ref" ] || [ -z "$pr_base_ref" ] || [ -z "$pr_head_oid" ] || [ -z "$pr_head_owner" ] || [ -z "$pr_head_repo" ]; then
    echo "ERROR: refusing to mutate PR '$pr_url'; PR metadata is incomplete" >&2
    return 1
  fi
  if [ "$pr_head_ref" != "$current_branch" ]; then
    echo "ERROR: refusing to mutate PR '$pr_url'; headRefName '$pr_head_ref' does not match current branch '$current_branch'" >&2
    return 1
  fi
  if [ "$pr_head_oid" != "$local_head" ]; then
    echo "ERROR: refusing to mutate PR '$pr_url'; headRefOid '$pr_head_oid' does not match local HEAD '$local_head'" >&2
    return 1
  fi
  if [ "$pr_cross_repo" = "true" ] || [ "$pr_head_owner/$pr_head_repo" != "$repo_identity" ]; then
    echo "ERROR: refusing to mutate PR '$pr_url'; PR head repo '$pr_head_owner/$pr_head_repo' does not match current repo '$repo_identity'" >&2
    return 1
  fi
  if ! expected_base="$(resolve_expected_base_branch)"; then
    echo "ERROR: refusing to mutate PR '$pr_url'; unable to resolve expected base branch for baseRefName validation" >&2
    return 1
  fi
  if [ "$pr_base_ref" != "$expected_base" ]; then
    echo "ERROR: refusing to mutate PR '$pr_url'; baseRefName '$pr_base_ref' does not match expected base '$expected_base'" >&2
    return 1
  fi
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
      echo "ERROR: unable to discover PRs for branch '$current_branch'; refusing to treat ambiguous GitHub state as no PR found" >&2
      exit 1
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
  if ! pr_json="$(gh_with_retry "pr view" pr view "$pr_target" --json number,state,isDraft,mergedAt,url,headRefName,baseRefName,headRefOid,headRepositoryOwner,headRepository,isCrossRepository)"; then
    echo "ERROR: unable to inspect PR '$pr_target' with gh; refusing to mutate ambiguous PR state" >&2
    exit 1
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

  validate_pr_identity_before_mutation "$pr_json" "$pr_url" || exit 1

  if [ "$pr_is_draft" = "true" ]; then
    if ready_output="$(gh_with_retry "pr ready" pr ready "$pr_url")"; then
      printf '%s\n' "$ready_output"
    else
      echo "ERROR: gh pr ready failed for '$pr_url'; refusing to report successful finalization after mutation failure" >&2
      exit 1
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
