#!/usr/bin/env bash
set -euo pipefail

export GIT_PAGER=cat GH_PAGER=cat PAGER=cat LESS=FRX

if ! command -v jq >/dev/null 2>&1; then
  echo '{"ok":false,"reason":"missing_jq"}'
  echo "ERROR: jq is required by workflow_pr_scope.sh" >&2
  exit 2
fi
if ! command -v gh >/dev/null 2>&1; then
  echo '{"ok":false,"reason":"missing_gh"}'
  echo "ERROR: gh is required by workflow_pr_scope.sh" >&2
  exit 127
fi

REPO=""
HEAD_REF=""
BASE_REF=""
PR_NUMBER=""
PR_URL=""
ISSUE_ID=""
WORK_ITEM_ID=""
RECIPE_RUN_ID=""
TREE_ID=""
WORKSTREAM_ID=""
EXPECTED_PR_TITLE_PREFIX=""
CREATED_AFTER=""
HEAD_SHA=""

usage() {
  cat >&2 <<'USAGE'
usage: workflow_pr_scope.sh --repo OWNER/REPO --head BRANCH --base BRANCH [scope...]

Scope options:
  --pr-number NUMBER
  --pr-url URL
  --issue ID
  --work-item ID
  --recipe-run-id ID
  --tree-id ID
  --workstream-id ID
  --expected-pr-title-prefix PREFIX
  --created-after RFC3339_TIME
  --head-sha SHA
USAGE
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --repo) REPO="${2:-}"; shift 2 ;;
    --head) HEAD_REF="${2:-}"; shift 2 ;;
    --base) BASE_REF="${2:-}"; shift 2 ;;
    --pr-number) PR_NUMBER="${2:-}"; shift 2 ;;
    --pr-url) PR_URL="${2:-}"; shift 2 ;;
    --issue) ISSUE_ID="${2:-}"; shift 2 ;;
    --work-item) WORK_ITEM_ID="${2:-}"; shift 2 ;;
    --recipe-run-id) RECIPE_RUN_ID="${2:-}"; shift 2 ;;
    --tree-id) TREE_ID="${2:-}"; shift 2 ;;
    --workstream-id) WORKSTREAM_ID="${2:-}"; shift 2 ;;
    --expected-pr-title-prefix) EXPECTED_PR_TITLE_PREFIX="${2:-}"; shift 2 ;;
    --created-after) CREATED_AFTER="${2:-}"; shift 2 ;;
    --head-sha) HEAD_SHA="${2:-}"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *)
      jq -nc --arg reason "invalid_arg" --arg arg "$1" '{ok:false,reason:$reason,arg:$arg}'
      usage
      exit 2
      ;;
  esac
done

emit_failure() {
  local reason="$1"
  local message="$2"
  jq -nc --arg reason "$reason" --arg message "$message" '{ok:false,reason:$reason,message:$message}'
  echo "ERROR: workflow_pr_scope.sh: $reason: $message" >&2
  exit 1
}

sanitize_gh_stderr() {
  sed -E 's#(https?://)[^@[:space:]]+@#\1REDACTED@#g' "$1" | tr '\n' ' ' | awk '{print substr($0, 1, 500)}'
}

is_transient_gh_error() {
  [ -s "$1" ] && grep -Eiq 'HTTP 5[0-9][0-9]|(^|[^0-9])(502|503|504)([^0-9]|$)|rate limit|timed out|timeout|temporar|connection reset|connection refused|TLS handshake|network|server error' "$1"
}

gh_with_retry() {
  local label="$1" stderr_file output status attempt delay=1
  shift
  for attempt in 1 2 3; do
    stderr_file=$(mktemp -t workflow-pr-scope-gh-XXXXXX)
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
    echo "ERROR: gh $label failed (exit ${status})" >&2
    [ ! -s "$stderr_file" ] || echo "gh $label stderr: $(sanitize_gh_stderr "$stderr_file")" >&2
    rm -f "$stderr_file"
    return "$status"
  done
}

parse_github_repo_identity() {
  local url="$1" path owner repo
  case "$url" in
    https://github.com/*) path="${url#https://github.com/}" ;;
    http://github.com/*) path="${url#http://github.com/}" ;;
    git@github.com:*) path="${url#git@github.com:}" ;;
    ssh://git@github.com/*) path="${url#ssh://git@github.com/}" ;;
    https://*@github.com/*|http://*@github.com/*) path="${url#*://}"; path="${path#*@github.com/}" ;;
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

if [ -z "$REPO" ]; then
  REPO="$(parse_github_repo_identity "$(git config --get remote.origin.url 2>/dev/null || true)" || true)"
fi
if [ -z "$HEAD_REF" ]; then
  HEAD_REF="$(git branch --show-current 2>/dev/null || true)"
fi
if [ -z "$HEAD_SHA" ] && git rev-parse --verify HEAD >/dev/null 2>&1; then
  HEAD_SHA="$(git rev-parse --verify HEAD)"
fi
if [ -z "$REPO" ] || [ -z "$HEAD_REF" ] || [ -z "$BASE_REF" ]; then
  emit_failure "missing_scope" "repo, head, and base are required"
fi
if [ -n "$PR_NUMBER" ] && ! [[ "$PR_NUMBER" =~ ^[1-9][0-9]*$ ]]; then
  emit_failure "invalid_pr_number" "pr-number is not a positive integer"
fi
if [ -n "$PR_URL" ] && ! [[ "$PR_URL" =~ ^https://github\.com/[^[:space:]]+/[^[:space:]]+/pull/[1-9][0-9]*$ ]]; then
  emit_failure "invalid_pr_url" "pr-url is not a GitHub pull request URL"
fi

fields="number,title,body,state,createdAt,mergedAt,url,headRefName,baseRefName,headRefOid,headRepositoryOwner,headRepository,isCrossRepository,statusCheckRollup,isDraft,mergeable,reviews"
raw_json=""
if [ -n "$PR_URL" ]; then
  raw_json="$(gh_with_retry "pr view" pr view "$PR_URL" --repo "$REPO" --json "$fields")" \
    || emit_failure "pr_metadata_unavailable" "unable to inspect explicit PR URL"
  raw_json="$(jq -nc --argjson pr "$raw_json" '[$pr]')"
elif [ -n "$PR_NUMBER" ]; then
  raw_json="$(gh_with_retry "pr view" pr view "$PR_NUMBER" --repo "$REPO" --json "$fields")" \
    || emit_failure "pr_metadata_unavailable" "unable to inspect explicit PR number"
  raw_json="$(jq -nc --argjson pr "$raw_json" '[$pr]')"
else
  raw_json="$(gh_with_retry "pr list" pr list --repo "$REPO" --head "$HEAD_REF" --state all --json "$fields")" \
    || emit_failure "pr_metadata_unavailable" "unable to list scoped PR candidates"
  if [ -z "${raw_json//[[:space:]]/}" ]; then
    raw_json="[]"
  fi
fi

if ! printf '%s' "$raw_json" | jq -e 'type == "array"' >/dev/null 2>&1; then
  emit_failure "invalid_pr_metadata" "GitHub PR metadata was not a JSON array"
fi

matches="$(
  printf '%s' "$raw_json" | jq -c \
    --arg repo "$REPO" \
    --arg headRefName "$HEAD_REF" \
    --arg baseRefName "$BASE_REF" \
    --arg headRefOid "$HEAD_SHA" \
    --arg prNumber "$PR_NUMBER" \
    --arg prUrl "$PR_URL" \
    --arg issueId "$ISSUE_ID" \
    --arg workItemId "$WORK_ITEM_ID" \
    --arg recipeRunId "$RECIPE_RUN_ID" \
    --arg treeId "$TREE_ID" \
    --arg workstreamId "$WORKSTREAM_ID" \
    --arg expected_pr_title_prefix "$EXPECTED_PR_TITLE_PREFIX" \
    --arg created_after "$CREATED_AFTER" '
      def owner_name:
        (.headRepositoryOwner.login // .headRepositoryOwner.name // .headRepositoryOwner // "");
      def repo_name:
        (.headRepository.name // ((.headRepository.nameWithOwner // "") | split("/") | .[-1]) // "");
      def text:
        ((.title // "") + " " + (.body // ""));
      def has_token($token):
        ($token == "") or (text | contains($token));
      [
        .[]
        | select((.headRefName // "") == $headRefName)
        | select((.baseRefName // "") == $baseRefName)
        | select(($headRefOid == "") or ((.headRefOid // "") == $headRefOid))
        | select(($prNumber == "") or (((.number // "") | tostring) == $prNumber))
        | select(($prUrl == "") or ((.url // "") == $prUrl))
        | select((.isCrossRepository // false) == false)
        | select(((owner_name + "/" + repo_name) == $repo))
        | select(($expected_pr_title_prefix == "") or ((.title // "") | startswith($expected_pr_title_prefix)))
        | select(($created_after == "") or ((.createdAt // "") >= $created_after))
        | select(($issueId == "") or (((.number // "") | tostring) == $issueId) or has_token("#" + $issueId) or has_token("issue-" + $issueId))
        | select(($workItemId == "") or (((.number // "") | tostring) == $workItemId) or has_token("#" + $workItemId) or has_token("AB#" + $workItemId) or has_token("work-item-" + $workItemId))
        | select(($recipeRunId == "") or has_token($recipeRunId))
        | select(($treeId == "") or has_token($treeId))
        | select(($workstreamId == "") or has_token($workstreamId))
      ]'
)"

match_count="$(printf '%s' "$matches" | jq 'length')"
if [ "$match_count" -eq 0 ]; then
  emit_failure "no_scoped_pr" "no PR matched the explicit workflow scope"
fi
if [ "$match_count" -gt 1 ]; then
  jq -nc --arg reason "multiple_scoped_prs" --argjson candidates "$matches" '{ok:false,reason:$reason,candidates:$candidates}'
  echo "ERROR: workflow_pr_scope.sh: multiple_scoped_prs: more than one PR matched the explicit workflow scope" >&2
  exit 1
fi

printf '%s' "$matches" | jq -c '.[0] + {ok:true, scoped:true}'
