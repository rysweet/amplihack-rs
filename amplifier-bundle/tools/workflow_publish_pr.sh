#!/usr/bin/env bash
set -euo pipefail

if ! command -v jq >/dev/null 2>&1; then
  echo "ERROR: jq is required by workflow_publish_pr.sh." >&2
  exit 2
fi

PUBLISH_STATE="unknown"
LEGACY_PUBLISH_STATE="unknown"
TERMINAL_STATUS="failure"
PR_URL_RESULT=""
PR_NUMBER_RESULT=""
BRANCH_DIFF_STATUS="unknown"
MESSAGE="not classified"

emit_publish_result() {
  jq -nc \
    --arg state "$PUBLISH_STATE" \
    --arg legacy_state "$LEGACY_PUBLISH_STATE" \
    --arg terminal_status "$TERMINAL_STATUS" \
    --arg pr_url "$PR_URL_RESULT" \
    --arg pr_number "$PR_NUMBER_RESULT" \
    --arg branch_diff_status "$BRANCH_DIFF_STATUS" \
    --arg message "$MESSAGE" \
    '{state:$state,legacy_state:$legacy_state,terminal_status:$terminal_status,pr_url:$pr_url,pr_number:$pr_number,branch_diff_status:$branch_diff_status,message:$message}'
}

finish_publish() {
  local state="$1"
  local legacy_state="$2"
  local terminal_status="$3"
  local message="$4"
  local exit_code="${5:-0}"

  PUBLISH_STATE="$state"
  LEGACY_PUBLISH_STATE="$legacy_state"
  TERMINAL_STATUS="$terminal_status"
  MESSAGE="$message"
  emit_publish_result
  exit "$exit_code"
}

load_pr_fields() {
  local pr_json="$1"
  mapfile -t PR_FIELDS < <(
    printf '%s' "$pr_json" | jq -r '.url // "", ((.number // "") | tostring), .state // "", .mergedAt // "", .headRefName // "", .baseRefName // "", .headRefOid // "", (.headRepositoryOwner.login // .headRepositoryOwner.name // .headRepositoryOwner // ""), (.headRepository.name // ((.headRepository.nameWithOwner // "") | split("/") | .[-1]) // ""), (.isCrossRepository // false | tostring)'
  )
  PR_URL_RESULT="${PR_FIELDS[0]:-}"
  PR_NUMBER_RESULT="${PR_FIELDS[1]:-}"
  PR_STATE="${PR_FIELDS[2]:-}"
  PR_MERGED_AT="${PR_FIELDS[3]:-}"
  PR_HEAD_REF="${PR_FIELDS[4]:-}"
  PR_BASE_REF="${PR_FIELDS[5]:-}"
  PR_HEAD_OID="${PR_FIELDS[6]:-}"
  PR_HEAD_OWNER="${PR_FIELDS[7]:-}"
  PR_HEAD_REPO="${PR_FIELDS[8]:-}"
  PR_IS_CROSS_REPO="${PR_FIELDS[9]:-false}"
}

resolve_pr_base_ref() {
  local candidate
  candidate="$(git symbolic-ref -q --short refs/remotes/origin/HEAD 2>/dev/null || true)"
  if [ -n "$candidate" ] && git rev-parse --verify --quiet "${candidate}^{commit}" >/dev/null; then printf '%s\n' "$candidate"; return 0; fi
  git remote set-head origin -a >/dev/null 2>&1 || true
  candidate="$(git symbolic-ref -q --short refs/remotes/origin/HEAD 2>/dev/null || true)"
  if [ -n "$candidate" ] && git rev-parse --verify --quiet "${candidate}^{commit}" >/dev/null; then printf '%s\n' "$candidate"; return 0; fi
  for candidate in origin/main origin/master origin/develop; do
    if git rev-parse --verify --quiet "${candidate}^{commit}" >/dev/null; then printf '%s\n' "$candidate"; return 0; fi
  done
  echo "ERROR: no supported remote base ref found. Expected origin/HEAD, origin/main, origin/master, or origin/develop." >&2
  return 1
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

validate_pr_identity() {
  local expected_base="$1"
  if [ -z "$REPO_IDENTITY" ]; then
    finish_publish "FAILED_PR_IDENTITY" "invalid-pr-identity" "failure" "unable to determine current GitHub repo identity from origin remote" 1
  fi
  if [ -z "$PR_HEAD_REF" ] || [ -z "$PR_BASE_REF" ] || [ -z "$PR_HEAD_OID" ]; then
    finish_publish "FAILED_PR_IDENTITY" "invalid-pr-identity" "failure" "existing PR metadata is incomplete; refusing to classify terminal state" 1
  fi
  if [ "$PR_HEAD_REF" != "$CURRENT_BRANCH" ]; then
    finish_publish "FAILED_PR_IDENTITY" "invalid-pr-identity" "failure" "existing PR headRefName '$PR_HEAD_REF' does not match current branch '$CURRENT_BRANCH'" 1
  fi
  if [ "$PR_BASE_REF" != "$expected_base" ]; then
    finish_publish "FAILED_PR_IDENTITY" "invalid-pr-identity" "failure" "existing PR baseRefName '$PR_BASE_REF' does not match base '$expected_base'" 1
  fi
  if [ "$PR_HEAD_OID" != "$LOCAL_HEAD" ]; then
    finish_publish "FAILED_PR_IDENTITY" "invalid-pr-identity" "failure" "existing PR headRefOid '$PR_HEAD_OID' does not match local HEAD '$LOCAL_HEAD'" 1
  fi
  PR_BASE_IDENTITY="$(parse_github_repo_identity "$PR_URL_RESULT" || true)"
  if [ -z "$PR_HEAD_OWNER" ] || [ -z "$PR_HEAD_REPO" ] || [ -z "$PR_BASE_IDENTITY" ]; then
    finish_publish "FAILED_PR_IDENTITY" "invalid-pr-identity" "failure" "existing PR metadata is missing repository identity" 1
  fi
  if [ "$PR_IS_CROSS_REPO" = "true" ] || [ "$PR_HEAD_OWNER/$PR_HEAD_REPO" != "$REPO_IDENTITY" ] || [ "$PR_BASE_IDENTITY" != "$REPO_IDENTITY" ]; then
    finish_publish "FAILED_PR_IDENTITY" "invalid-pr-identity" "failure" "existing PR repo '$PR_HEAD_OWNER/$PR_HEAD_REPO' -> '$PR_BASE_IDENTITY' does not match current repo '$REPO_IDENTITY'" 1
  fi
}

sanitize_gh_stderr() {
  sed -E 's#(https?://)[^@[:space:]]+@#\1REDACTED@#g' "$1" | tr '\n' ' ' | head -c 500
}

is_transient_gh_error() {
  [ -s "$1" ] && grep -Eiq 'HTTP 5[0-9][0-9]|(^|[^0-9])(502|503|504)([^0-9]|$)|rate limit|timed out|timeout|temporar|connection reset|connection refused|TLS handshake|network|server error' "$1"
}

gh_pr_list_with_retry() {
  local stderr_file output status attempt delay=1
  for attempt in 1 2 3; do
    stderr_file=$(mktemp -t step16-gh-pr-list-XXXXXX)
    if output=$(timeout 60 gh pr list "$@" 2>"$stderr_file"); then
      rm -f "$stderr_file"; printf '%s\n' "$output"; return 0
    else
      status=$?
    fi
    if [ "$attempt" -lt 3 ] && is_transient_gh_error "$stderr_file"; then
      echo "WARNING: gh pr list failed transiently (exit ${status}); retrying (${attempt}/3): $(sanitize_gh_stderr "$stderr_file")" >&2
      rm -f "$stderr_file"; sleep "$delay"; delay=$((delay * 2)); continue
    fi
    echo "ERROR: gh pr list failed (exit ${status}); refusing to risk duplicate PR creation." >&2
    [ ! -s "$stderr_file" ] || echo "gh pr list stderr: $(sanitize_gh_stderr "$stderr_file")" >&2
    rm -f "$stderr_file"
    return "$status"
  done
}

gh_pr_view_with_retry() {
  local stderr_file output status attempt delay=1
  for attempt in 1 2 3; do
    stderr_file=$(mktemp -t step16-gh-pr-view-XXXXXX)
    if output=$(timeout 60 gh pr view "$@" 2>"$stderr_file"); then
      rm -f "$stderr_file"; printf '%s\n' "$output"; return 0
    else
      status=$?
    fi
    if [ "$attempt" -lt 3 ] && is_transient_gh_error "$stderr_file"; then
      echo "WARNING: gh pr view failed transiently (exit ${status}); retrying (${attempt}/3): $(sanitize_gh_stderr "$stderr_file")" >&2
      rm -f "$stderr_file"; sleep "$delay"; delay=$((delay * 2)); continue
    fi
    echo "ERROR: gh pr view failed (exit ${status}); existing PR state is ambiguous." >&2
    [ ! -s "$stderr_file" ] || echo "gh pr view stderr: $(sanitize_gh_stderr "$stderr_file")" >&2
    rm -f "$stderr_file"
    return "$status"
  done
}

gh_pr_create_with_retry() {
  local stderr_file output status attempt delay=1
  for attempt in 1 2 3; do
    stderr_file=$(mktemp -t step16-gh-pr-create-XXXXXX)
    if output=$(timeout 60 gh pr create --draft --title "$PR_TITLE" --body "$PR_BODY" 2>"$stderr_file"); then
      rm -f "$stderr_file"; printf '%s\n' "$output"; return 0
    else
      status=$?
    fi
    if [ "$attempt" -lt 3 ] && is_transient_gh_error "$stderr_file"; then
      echo "WARNING: gh pr create failed transiently (exit ${status}); retrying (${attempt}/3): $(sanitize_gh_stderr "$stderr_file")" >&2
      rm -f "$stderr_file"; sleep "$delay"; delay=$((delay * 2)); continue
    fi
    echo "ERROR: gh pr create failed (exit $status) — PR may already exist for this branch or GitHub API is unavailable" >&2
    [ ! -s "$stderr_file" ] || echo "gh pr create stderr: $(sanitize_gh_stderr "$stderr_file")" >&2
    rm -f "$stderr_file"
    return "$status"
  done
}

HOST_TYPE="${REMOTE_HOST_TYPE:-other}"
if [ "$HOST_TYPE" != "github" ]; then
  finish_publish "non-github" "non-github" "success" "non-GitHub host does not use gh pr create"
fi

CURRENT_BRANCH=$(git branch --show-current)
ISSUE_NUM="$ISSUE_NUMBER"
if ! [[ "$ISSUE_NUM" =~ ^[0-9]+$ ]]; then echo "ERROR: issue_number is not numeric: $ISSUE_NUM" >&2; exit 1; fi
if ! command -v gh >/dev/null 2>&1; then echo "ERROR: workflow_publish_pr.sh requires the GitHub CLI ('gh') on PATH." >&2; exit 127; fi
[ -n "$CURRENT_BRANCH" ] || finish_publish "FAILED_INVALID_INPUT" "invalid-input" "failure" "current branch is empty; cannot publish from detached HEAD" 1
LOCAL_HEAD="$(git rev-parse --verify HEAD 2>/dev/null)" || finish_publish "FAILED_INVALID_INPUT" "invalid-input" "failure" "local HEAD does not resolve" 1
REPO_IDENTITY="$(parse_github_repo_identity "$(git config --get remote.origin.url 2>/dev/null || true)" || true)"

BASE_REF="$(resolve_pr_base_ref)"
BASE_BRANCH="${BASE_REF#origin/}"
if git diff --quiet "${BASE_REF}..HEAD"; then BRANCH_DIFF_STATUS="no-diff"; else BRANCH_DIFF_STATUS="has-diff"; fi

if ! PR_JSON="$(gh_pr_list_with_retry --head "$CURRENT_BRANCH" --state all --json url,number,state,mergedAt,headRefName,baseRefName,headRefOid,headRepositoryOwner,headRepository,isCrossRepository --jq '.[0] // {}')"; then
  finish_publish "FAILED_PR_METADATA_UNAVAILABLE" "pr-metadata-unavailable" "failure" "unavailable PR metadata for branch; refusing to risk duplicate PR creation" 1
fi
load_pr_fields "$PR_JSON"

if [ -n "$PR_URL_RESULT" ]; then
  if [ -z "$PR_STATE" ]; then
    VIEW_JSON="$(gh_pr_view_with_retry "$PR_URL_RESULT" --json url,number,state,mergedAt,headRefName,baseRefName,headRefOid,headRepositoryOwner,headRepository,isCrossRepository)"
    load_pr_fields "$VIEW_JSON"
  fi
  validate_pr_identity "$BASE_BRANCH"
  case "$PR_STATE:$PR_MERGED_AT" in
    OPEN:*) finish_publish "existing-open-pr" "existing-open-pr" "success" "existing open PR found for branch" ;;
    MERGED:*) finish_publish "MERGED" "already-merged" "success" "branch PR is already merged" ;;
    CLOSED:?*) finish_publish "MERGED" "closed-after-merge" "success" "branch PR is closed after merge" ;;
    CLOSED:*)
      if [ "$BRANCH_DIFF_STATUS" = "has-diff" ]; then
        finish_publish "FAILED_CLOSED_UNMERGED" "closed-unmerged-with-diff" "failure" "existing PR was closed without merge and branch still has diff; reopen it or create a new branch intentionally" 1
      fi
      finish_publish "CLOSED_OBSOLETE" "no-diff" "success" "closed unmerged PR exists but branch has no diff against base" ;;
  esac
fi

if [ "$BRANCH_DIFF_STATUS" = "no-diff" ]; then
  finish_publish "NO_DIFF_SUCCESS" "no-diff" "success" "branch has no diff against base; no PR created"
fi

COMMITS_AHEAD=$(git rev-list --count "${BASE_REF}..HEAD" 2>/dev/null || echo "0")
if [ "$COMMITS_AHEAD" -eq 0 ]; then
  BRANCH_DIFF_STATUS="no-diff"
  finish_publish "NO_DIFF_SUCCESS" "no-diff" "success" "0 commits ahead of ${BASE_BRANCH}; no PR created"
fi

CHANGED_FILES=$(git diff --name-only "${BASE_REF}..HEAD" 2>/dev/null | head -40 || true)
CHANGED_COUNT=$(printf '%s\n' "$CHANGED_FILES" | sed '/^$/d' | wc -l | tr -d ' ')
DIFF_STAT=$(git diff --stat "${BASE_REF}..HEAD" 2>/dev/null | tail -20 || true)
RECENT_COMMITS=$(git log --oneline --no-decorate "${BASE_REF}..HEAD" -6 2>/dev/null || true)
FIRST_CHANGED=$(printf '%s\n' "$CHANGED_FILES" | sed '/^$/d' | head -1)
case "$FIRST_CHANGED" in amplifier-bundle/recipes/*) PR_SCOPE="workflow recipes" ;; crates/amplihack-cli/*) PR_SCOPE="amplihack CLI" ;; crates/*) PR_SCOPE="$(printf '%s' "$FIRST_CHANGED" | cut -d/ -f2)" ;; tests/*) PR_SCOPE="regression coverage" ;; docs/*) PR_SCOPE="documentation" ;; "") PR_SCOPE="workflow changes" ;; *) PR_SCOPE="$(printf '%s' "$FIRST_CHANGED" | cut -d/ -f1)" ;; esac
if [ "$CHANGED_COUNT" -gt 1 ]; then PR_TITLE="Update ${PR_SCOPE} with ${CHANGED_COUNT} changed files"; else PR_TITLE="Update ${PR_SCOPE}"; fi
PR_TITLE="${PR_TITLE} (#${ISSUE_NUM})"
PR_TITLE="${PR_TITLE:0:200}"
CHANGED_FILES_BODY=$(printf '%s\n' "$CHANGED_FILES" | sed '/^$/d; s/^/- /')
[ -n "$CHANGED_FILES_BODY" ] || CHANGED_FILES_BODY="- No changed files detected by git diff"
VALIDATION_SOURCE="${LOCAL_TESTING_GATE:-${local_testing_gate:-${PRECOMMIT_RESULTS:-${precommit_results:-}}}}"
if [ -n "$VALIDATION_SOURCE" ]; then VALIDATION_BODY=$(printf '%s\n' "$VALIDATION_SOURCE" | sed -n '1,12p'); else VALIDATION_BODY="step-13 local testing gate is expected before ready-for-review; no structured step-13 output was available to step-16."; fi
if [ -n "$RECENT_COMMITS" ]; then BEHAVIOR_BODY=$(printf 'Implemented behavior through these branch commits:\n%s' "$RECENT_COMMITS"); else BEHAVIOR_BODY="Branch is ${COMMITS_AHEAD} commit(s) ahead of ${BASE_BRANCH}; behavior impact is represented by the changed files and diff stat."; fi
case "$CHANGED_FILES" in *amplifier-bundle/recipes/*) RISK_BODY="Workflow behavior changed; review recipe gates and regression coverage carefully." ;; *crates/amplihack-cli/*) RISK_BODY="CLI behavior changed; verify command help, exit codes, and JSON/table output contracts." ;; *) RISK_BODY="No high-risk subsystem pattern detected from changed paths." ;; esac
DIFF_STAT_BODY="${DIFF_STAT:-No diff stat available}"
ISSUE_LINK="Closes #${ISSUE_NUM}"
PR_BODY=$(printf '## Summary\nConcise workflow-generated PR for %s.\n\n## Issue\n%s\n\n## Changed files\n%s\n\n## Diff stat\n```text\n%s\n```\n\n## Behavior\n%s\n\n## Validation\n%s\n\n## Risk\n%s\n\n## Checklist\n- [x] Branch has %s commit(s) ahead of %s\n- [ ] Code review completed\n- [ ] Philosophy check passed\n\n---\n*This PR was created as a draft for review before merging.*\n' "$PR_SCOPE" "$ISSUE_LINK" "$CHANGED_FILES_BODY" "$DIFF_STAT_BODY" "$BEHAVIOR_BODY" "$VALIDATION_BODY" "$RISK_BODY" "$COMMITS_AHEAD" "$BASE_BRANCH")

PUBLISH_STATE="FOLLOWUP_CREATED"
LEGACY_PUBLISH_STATE="create-new-pr"
if ! PR_URL_RESULT="$(gh_pr_create_with_retry)"; then
  finish_publish "FAILED_PR_CREATE" "create-pr-failed" "failure" "draft PR creation failed; GitHub API state is ambiguous" 1
fi
PR_NUMBER_RESULT="$(printf '%s\n' "$PR_URL_RESULT" | sed -nE 's#.*/pull/([0-9]+).*#\1#p' | head -1)"
finish_publish "$PUBLISH_STATE" "$LEGACY_PUBLISH_STATE" "success" "draft PR created"
