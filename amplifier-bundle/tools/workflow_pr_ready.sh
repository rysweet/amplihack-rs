#!/usr/bin/env bash
set -euo pipefail

export GIT_PAGER=cat GH_PAGER=cat PAGER=cat LESS=FRX
PR_URL="${PR_URL:-${RECIPE_VAR_pr_url:-${PR_PUBLISH_RESULT_PR_URL:-${RECIPE_VAR_pr_publish_result__pr_url:-}}}}"
PR_NUMBER="${PR_NUMBER:-${RECIPE_VAR_pr_number:-${PR_PUBLISH_RESULT_PR_NUMBER:-${RECIPE_VAR_pr_publish_result__pr_number:-}}}}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PR_SCOPE_HELPER="${WORKFLOW_PR_SCOPE_HELPER:-${SCRIPT_DIR}/workflow_pr_scope.sh}"
# workflow_pr_scope.sh validates headRefName, baseRefName, headRefOid,
# isCrossRepository, expected_pr_title_prefix, and created_after.

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

scoped_pr_json=""
if ! git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "INFO: [skip] not a git repo — skipping gh pr ready" >&2
  exit 0
fi
current_branch=$(git branch --show-current 2>/dev/null || true)
local_head="$(git rev-parse --verify HEAD 2>/dev/null || true)"
repo_identity="$(parse_github_repo_identity "$(git config --get remote.origin.url 2>/dev/null || true)" || true)"
expected_base="$(resolve_expected_base_branch || true)"
[ -n "$repo_identity" ] || { echo "ERROR: unable to determine current GitHub repo identity from origin remote" >&2; exit 1; }
[ -n "$current_branch" ] || { echo "ERROR: current branch is empty; refusing ambiguous PR readiness mutation" >&2; exit 1; }
[ -n "$local_head" ] || { echo "ERROR: local HEAD does not resolve; refusing ambiguous PR readiness mutation" >&2; exit 1; }
[ -n "$expected_base" ] || { echo "ERROR: unable to resolve expected base branch for scoped PR validation" >&2; exit 1; }
[ -x "$PR_SCOPE_HELPER" ] || { echo "ERROR: workflow_pr_scope.sh missing or not executable at $PR_SCOPE_HELPER" >&2; exit 1; }
if [[ "$PR_NUMBER" =~ [^[:space:]] ]] && ! [[ "$PR_NUMBER" =~ ^[1-9][0-9]*$ ]]; then
  echo "ERROR: invalid PR_NUMBER '$PR_NUMBER' for workflow_pr_ready.sh" >&2
  exit 1
fi

scoped_issue_id="${ISSUE_NUMBER:-${RECIPE_VAR_issue_number:-}}"
# issue_number can carry a non-numeric local tracking reference (e.g.
# local-5d904cff4398) when the workflow fell back to local tracking (#815,
# #804). PR-scope matching only understands numeric ids, so coerce a
# non-numeric ref to empty: a local ref must not filter out the current-work PR.
case "$scoped_issue_id" in ''|*[!0-9]*) scoped_issue_id="" ;; esac
scope_args=(
  --repo "$repo_identity"
  --head "$current_branch"
  --base "$expected_base"
  --issue "$scoped_issue_id"
  --work-item "$scoped_issue_id"
  --head-sha "$local_head"
)
title_prefix="${EXPECTED_PR_TITLE_PREFIX:-${PR_EXPECTED_TITLE_PREFIX:-}}"
[ -n "$title_prefix" ] && scope_args+=(--expected-pr-title-prefix "$title_prefix")
[[ "$PR_URL" =~ [^[:space:]] ]] && scope_args+=(--pr-url "$PR_URL")
[[ "$PR_NUMBER" =~ ^[1-9][0-9]*$ ]] && scope_args+=(--pr-number "$PR_NUMBER")
if [[ "${WORKFLOW_STARTED_AT:-${RECIPE_STARTED_AT:-${TASK_STARTED_AT:-}}}" =~ [^[:space:]] ]]; then
  scope_args+=(--created-after "${WORKFLOW_STARTED_AT:-${RECIPE_STARTED_AT:-${TASK_STARTED_AT:-}}}")
fi
if ! scoped_pr_json="$("$PR_SCOPE_HELPER" "${scope_args[@]}")"; then
  reason="$(printf '%s' "$scoped_pr_json" | jq -r '.reason // ""' 2>/dev/null || true)"
  if [ "$reason" = "no_scoped_pr" ] && [ -z "$PR_URL" ] && [ -z "$PR_NUMBER" ]; then
    echo "INFO: [skip] no scoped PR matched current workflow identity — skipping gh pr ready" >&2
    exit 0
  fi
  echo "ERROR: scoped PR validation failed before ready mutation: ${reason:-unknown}" >&2
  exit 1
fi
pr_target="$(printf '%s' "$scoped_pr_json" | jq -r '.url // .number // empty')"
[ -n "$pr_target" ] || { echo "ERROR: scoped PR validation returned no PR target" >&2; exit 1; }

terminal_status="active-pr"
closed_unmerged_seen="false"
for pr_target in "$pr_target"; do
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
