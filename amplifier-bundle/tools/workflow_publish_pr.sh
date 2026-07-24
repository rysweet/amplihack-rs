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
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PR_SCOPE_HELPER="${WORKFLOW_PR_SCOPE_HELPER:-${SCRIPT_DIR}/workflow_pr_scope.sh}"
# workflow_pr_scope.sh validates headRefName, baseRefName, headRefOid,
# isCrossRepository, expected_pr_title_prefix, and created_after.

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

# Best-effort: apply caller-configured labels to the GitHub PR that was just
# published (created OR matched as an existing open PR for this branch). The
# labels come from the comma-separated `WORKFLOW_PR_LABELS` env var, so the
# generic bundle stays policy-free — the caller (e.g. an autonomous agent that
# needs a durable "eligible for gated self-merge" marker) decides what to apply.
#
# This NEVER fails the publish: a label that does not exist in the repo, a
# transient GitHub API error, or a missing `gh` must not block the PR flow, so
# every failure is warn-and-continue and the function always returns 0. It is a
# no-op unless the host is GitHub, a numeric PR number was resolved, and at
# least one label was requested.
apply_pr_labels_best_effort() {
  [ -n "${WORKFLOW_PR_LABELS:-}" ] || return 0
  [ "${HOST_TYPE:-}" = "github" ] || return 0
  case "${PR_NUMBER_RESULT:-}" in '' | *[!0-9]*) return 0 ;; esac
  # Only label PRs that are newly created or currently OPEN. A MERGED/CLOSED
  # terminal is a no-op success (nothing to gate for the merge queue), and
  # labeling a closed PR would be pointless churn. PR_STATE is empty on the
  # create-new path (no pre-existing PR was found), so this still labels
  # freshly created PRs.
  case "${PR_STATE:-}" in MERGED | CLOSED) return 0 ;; esac
  command -v gh >/dev/null 2>&1 || return 0

  # `gh pr edit --add-label` splits comma-separated labels natively, so the raw
  # CSV is passed straight through in a single call — no bespoke parsing needed.
  # `timeout 60` mirrors every other gh call in this script: a hung `gh pr edit`
  # must never block finish_publish from emitting its JSON.
  if ! timeout 60 gh pr edit "$PR_NUMBER_RESULT" --add-label "$WORKFLOW_PR_LABELS" >/dev/null 2>&1; then
    echo "WARNING: workflow_publish_pr.sh: best-effort labels '${WORKFLOW_PR_LABELS}' not applied to PR #${PR_NUMBER_RESULT} (a label may not exist in this repo, GitHub API unavailable, or timed out)" >&2
  fi
  return 0
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
  if [ "$terminal_status" = "success" ]; then
    apply_pr_labels_best_effort
  fi
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

sanitize_provider_stderr() {
  sed -E 's#(https?://)[^@[:space:]]+@#\1REDACTED@#g' "$1" | tr '\n' ' ' | head -c 500
}

azdo_common_args=()
configure_azdo_args() {
  local org_url project_name
  org_url="${AZDO_ORG_URL:-${SYSTEM_COLLECTIONURI:-}}"
  project_name="${AZDO_PROJECT:-${SYSTEM_TEAMPROJECT:-}}"
  azdo_common_args=()
  [ -n "$org_url" ] && azdo_common_args+=(--org "$org_url")
  [ -n "$project_name" ] && azdo_common_args+=(--project "$project_name")
}

az_repos_pr_list_with_retry() {
  local stderr_file output status attempt delay=1
  configure_azdo_args
  for attempt in 1 2 3; do
    stderr_file=$(mktemp -t step16-az-pr-list-XXXXXX)
    if output=$(timeout 60 az repos pr list "${azdo_common_args[@]}" --status active --source-branch "$CURRENT_BRANCH" --target-branch "$BASE_BRANCH" --output json 2>"$stderr_file"); then
      rm -f "$stderr_file"; printf '%s\n' "$output"; return 0
    else
      status=$?
    fi
    if [ "$attempt" -lt 3 ] && is_transient_gh_error "$stderr_file"; then
      echo "WARNING: az repos pr list failed transiently (exit ${status}); retrying (${attempt}/3): $(sanitize_provider_stderr "$stderr_file")" >&2
      rm -f "$stderr_file"; sleep "$delay"; delay=$((delay * 2)); continue
    fi
    echo "ERROR: az repos pr list failed (exit ${status}); refusing to risk duplicate PR creation." >&2
    [ ! -s "$stderr_file" ] || echo "az repos pr list stderr: $(sanitize_provider_stderr "$stderr_file")" >&2
    rm -f "$stderr_file"
    return "$status"
  done
}

az_repos_pr_create_with_retry() {
  local stderr_file output status attempt delay=1
  configure_azdo_args
  for attempt in 1 2 3; do
    stderr_file=$(mktemp -t step16-az-pr-create-XXXXXX)
    if output=$(timeout 60 az repos pr create "${azdo_common_args[@]}" --source-branch "$CURRENT_BRANCH" --target-branch "$BASE_BRANCH" --title "$PR_TITLE" --description "$PR_BODY" --output json 2>"$stderr_file"); then
      rm -f "$stderr_file"; printf '%s\n' "$output"; return 0
    else
      status=$?
    fi
    if [ "$attempt" -lt 3 ] && is_transient_gh_error "$stderr_file"; then
      echo "WARNING: az repos pr create failed transiently (exit ${status}); retrying (${attempt}/3): $(sanitize_provider_stderr "$stderr_file")" >&2
      rm -f "$stderr_file"; sleep "$delay"; delay=$((delay * 2)); continue
    fi
    echo "ERROR: az repos pr create failed (exit ${status}); provider state is ambiguous." >&2
    [ ! -s "$stderr_file" ] || echo "az repos pr create stderr: $(sanitize_provider_stderr "$stderr_file")" >&2
    rm -f "$stderr_file"
    return "$status"
  done
}

select_matching_azdo_pr() {
  local pr_list_json="$1"
  printf '%s' "$pr_list_json" | jq -c \
    --arg src "$CURRENT_BRANCH" \
    --arg src_ref "refs/heads/$CURRENT_BRANCH" \
    --arg tgt "$BASE_BRANCH" \
    --arg tgt_ref "refs/heads/$BASE_BRANCH" \
    'map(select(
      ((.sourceRefName // "") == $src or (.sourceRefName // "") == $src_ref) and
      ((.targetRefName // "") == $tgt or (.targetRefName // "") == $tgt_ref) and
      (((.status // "") | ascii_downcase) == "active")
    )) | .[0] // empty'
}

load_azdo_pr_fields() {
  local pr_json="$1"
  mapfile -t AZDO_PR_FIELDS < <(
    printf '%s' "$pr_json" | jq -r '.url // "", ((.pullRequestId // "") | tostring), .status // "", .sourceRefName // "", .targetRefName // ""'
  )
  PR_URL_RESULT="${AZDO_PR_FIELDS[0]:-}"
  PR_NUMBER_RESULT="${AZDO_PR_FIELDS[1]:-}"
}

scoped_pr_lookup() {
  local scope_output reason created_after expected_pr_title_prefix
  [ -x "$PR_SCOPE_HELPER" ] || finish_publish "FAILED_INVALID_INPUT" "invalid-input" "failure" "workflow_pr_scope.sh is missing or not executable at $PR_SCOPE_HELPER" 1
  created_after="${WORKFLOW_STARTED_AT:-${RECIPE_STARTED_AT:-${TASK_STARTED_AT:-}}}"
  expected_pr_title_prefix="${EXPECTED_PR_TITLE_PREFIX:-${PR_EXPECTED_TITLE_PREFIX:-Update}}"
  scope_args=(
    --repo "$REPO_IDENTITY"
    --head "$CURRENT_BRANCH"
    --base "$BASE_BRANCH"
    --issue "$ISSUE_NUM"
    --work-item "$ISSUE_NUM"
    --expected-pr-title-prefix "$expected_pr_title_prefix"
    --head-sha "$LOCAL_HEAD"
  )
  [ -n "$created_after" ] && scope_args+=(--created-after "$created_after")
  if scope_output="$("$PR_SCOPE_HELPER" "${scope_args[@]}")"; then
    printf '%s\n' "$scope_output"
    return 0
  fi
  reason="$(printf '%s' "$scope_output" | jq -r '.reason // ""' 2>/dev/null || true)"
  if [ "$reason" = "no_scoped_pr" ]; then
    printf '{}\n'
    return 0
  fi
  finish_publish "FAILED_PR_IDENTITY" "invalid-pr-identity" "failure" "scoped PR lookup failed: ${reason:-unknown}" 1
}

# Test seam: when this file is *sourced* with WORKFLOW_PUBLISH_PR_LIB_ONLY set,
# return after defining the helper functions above so unit tests can exercise
# pure helpers (e.g. apply_pr_labels_best_effort) without running the publish
# flow. Guarded by `BASH_SOURCE[0] != $0` so it can ONLY short-circuit when
# sourced — a leaked env var can never abort a directly-executed production run.
if [ -n "${WORKFLOW_PUBLISH_PR_LIB_ONLY:-}" ] && [ "${BASH_SOURCE[0]}" != "${0}" ]; then
  return 0
fi

HOST_TYPE="${REMOTE_HOST_TYPE:-other}"
CURRENT_BRANCH=$(git branch --show-current)
ISSUE_NUM="$ISSUE_NUMBER"
if ! [[ "$ISSUE_NUM" =~ ^[0-9]+$ ]]; then echo "ERROR: issue_number is not numeric: $ISSUE_NUM" >&2; exit 1; fi
[ -n "$CURRENT_BRANCH" ] || finish_publish "FAILED_INVALID_INPUT" "invalid-input" "failure" "current branch is empty; cannot publish from detached HEAD" 1
LOCAL_HEAD="$(git rev-parse --verify HEAD 2>/dev/null)" || finish_publish "FAILED_INVALID_INPUT" "invalid-input" "failure" "local HEAD does not resolve" 1

BASE_REF="$(resolve_pr_base_ref)"
BASE_BRANCH="${BASE_REF#origin/}"
RUNTIME_ARTIFACT_HELPER="${WORKFLOW_RUNTIME_ARTIFACT_HELPER:-${SCRIPT_DIR}/workflow_runtime_artifacts.sh}"
[ -f "$RUNTIME_ARTIFACT_HELPER" ] || { echo "ERROR: workflow runtime artifact helper not found: $RUNTIME_ARTIFACT_HELPER" >&2; exit 2; }
# shellcheck source=/dev/null
. "$RUNTIME_ARTIFACT_HELPER"
preflight_known_workflow_runtime_artifacts .
if [ -n "$(git status --porcelain)" ]; then
  finish_publish "FAILED_DIRTY_WORKTREE" "dirty-worktree" "failure" "dirty worktree blocks no-diff or obsolete terminal success; commit or discard changes explicitly" 1
fi
if git diff --quiet "${BASE_REF}..HEAD"; then BRANCH_DIFF_STATUS="no-diff"; else BRANCH_DIFF_STATUS="has-diff"; fi

if [ "$HOST_TYPE" = "azdo" ]; then
  if [ "$BRANCH_DIFF_STATUS" = "no-diff" ]; then
    finish_publish "NO_DIFF_SUCCESS" "no-diff" "success" "branch has no diff against base; no PR created"
  fi
  COMMITS_AHEAD=$(git rev-list --count "${BASE_REF}..HEAD" 2>/dev/null || echo "0")
  if [ "$COMMITS_AHEAD" -eq 0 ]; then
    BRANCH_DIFF_STATUS="no-diff"
    finish_publish "NO_DIFF_SUCCESS" "no-diff" "success" "0 commits ahead of ${BASE_BRANCH}; no PR created"
  fi
  if ! command -v az >/dev/null 2>&1; then
    finish_publish "BLOCKED_PROVIDER" "azdo-cli-missing" "failure" "Azure DevOps CLI ('az') is required to publish AzDO pull requests" 1
  fi
  if ! AZDO_PR_LIST_JSON="$(az_repos_pr_list_with_retry)"; then
    finish_publish "BLOCKED_PROVIDER" "azdo-pr-list-failed" "failure" "AzDO active PR lookup failed; provider/auth state is ambiguous" 1
  fi
  if AZDO_PR_JSON="$(select_matching_azdo_pr "$AZDO_PR_LIST_JSON")" && [ -n "$AZDO_PR_JSON" ]; then
    load_azdo_pr_fields "$AZDO_PR_JSON"
    finish_publish "EXISTING_OPEN_PR" "existing-open-pr" "success" "existing active AzDO PR found for branch"
  fi

  CHANGED_FILES=$(git diff --name-only "${BASE_REF}..HEAD" 2>/dev/null | head -40 || true)
  CHANGED_FILES_BODY=$(printf '%s\n' "$CHANGED_FILES" | sed '/^$/d; s/^/- /')
  [ -n "$CHANGED_FILES_BODY" ] || CHANGED_FILES_BODY="- No changed files detected by git diff"
  DIFF_STAT=$(git diff --stat "${BASE_REF}..HEAD" 2>/dev/null | tail -20 || true)
  DIFF_STAT_BODY="${DIFF_STAT:-No diff stat available}"
  PR_TITLE="Update ${CURRENT_BRANCH} (AB#${ISSUE_NUM})"
  PR_TITLE="${PR_TITLE:0:200}"
  PR_BODY=$(printf '## Summary\nWorkflow-generated Azure DevOps PR for AB#%s.\n\n## Changed files\n%s\n\n## Diff stat\n```text\n%s\n```\n' "$ISSUE_NUM" "$CHANGED_FILES_BODY" "$DIFF_STAT_BODY")
  if ! AZDO_CREATED_JSON="$(az_repos_pr_create_with_retry)"; then
    finish_publish "BLOCKED_PROVIDER" "azdo-pr-create-failed" "failure" "AzDO PR creation failed; provider state is ambiguous" 1
  fi
  load_azdo_pr_fields "$AZDO_CREATED_JSON"
  [ -n "$PR_NUMBER_RESULT" ] || finish_publish "BLOCKED_PROVIDER" "azdo-pr-create-invalid" "failure" "AzDO PR creation returned no pullRequestId" 1
  PUBLISH_STATE="FOLLOWUP_CREATED"
  LEGACY_PUBLISH_STATE="create-new-pr"
  TERMINAL_STATUS="success"
  MESSAGE="AzDO PR created"
  emit_publish_result
  exit 0
fi

if [ "$HOST_TYPE" != "github" ]; then
  finish_publish "non-github" "non-github" "success" "non-GitHub host does not use provider PR creation"
fi

if ! command -v gh >/dev/null 2>&1; then echo "ERROR: workflow_publish_pr.sh requires the GitHub CLI ('gh') on PATH." >&2; exit 127; fi
REPO_IDENTITY="$(parse_github_repo_identity "$(git config --get remote.origin.url 2>/dev/null || true)" || true)"
[ -n "$REPO_IDENTITY" ] || finish_publish "FAILED_PR_IDENTITY" "invalid-pr-identity" "failure" "unable to determine current GitHub repo identity from origin remote" 1

PR_JSON="$(scoped_pr_lookup)"
load_pr_fields "$PR_JSON"

if [ -n "$PR_URL_RESULT" ]; then
  if [ -z "$PR_STATE" ]; then
    VIEW_JSON="$(gh_pr_view_with_retry "$PR_URL_RESULT" --json url,number,state,mergedAt,headRefName,baseRefName,headRefOid,headRepositoryOwner,headRepository,isCrossRepository)"
    load_pr_fields "$VIEW_JSON"
  fi
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
# Issue #929: derive the PR-title scope from the first *substantive* changed
# file, ignoring generated/lockfiles. The lockfile-sync step (#915) commits
# Cargo.lock, which sorts first in `git diff --name-only`; without this filter a
# full feature diff gets mislabeled "Update Cargo.lock". Only a genuinely
# lockfile-only diff falls back to the lockfile scope. Filenames are treated as
# data: quoted expansions, basename matched via a static case, never eval'd.
is_generated_scope_file() {
  case "$(basename -- "$1")" in
    Cargo.lock|*.lock|package-lock.json|npm-shrinkwrap.json|yarn.lock|pnpm-lock.yaml|go.sum|Pipfile.lock|poetry.lock|composer.lock|Gemfile.lock|flake.lock) return 0 ;;
    *) return 1 ;;
  esac
}
SUBSTANTIVE_FIRST=""
while IFS= read -r changed_path; do
  [ -n "$changed_path" ] || continue
  if ! is_generated_scope_file "$changed_path"; then
    SUBSTANTIVE_FIRST="$changed_path"
    break
  fi
done <<EOF
$CHANGED_FILES
EOF
if [ -n "$SUBSTANTIVE_FIRST" ]; then
  FIRST_CHANGED="$SUBSTANTIVE_FIRST"
else
  FIRST_CHANGED=$(printf '%s\n' "$CHANGED_FILES" | awk 'NF {print; exit}')
fi
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
PR_NUMBER_RESULT="$(printf '%s\n' "$PR_URL_RESULT" | awk -F/ '/\/pull\/[0-9]+/ {print $NF; exit}')"
finish_publish "$PUBLISH_STATE" "$LEGACY_PUBLISH_STATE" "success" "draft PR created"
