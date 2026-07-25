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

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
GH_RETRY_HELPER="${WORKFLOW_GH_RETRY_HELPER:-${SCRIPT_DIR}/workflow_gh_retry.sh}"
if [ ! -f "$GH_RETRY_HELPER" ]; then
  echo '{"ok":false,"reason":"missing_gh_retry_helper"}'
  echo "ERROR: workflow_pr_scope.sh requires the shared retry helper at $GH_RETRY_HELPER" >&2
  exit 2
fi
# shellcheck source=/dev/null
. "$GH_RETRY_HELPER"

# Thin wrapper over the shared rate-limit-aware retry driver. A rate-limit is no
# longer treated as a fast 3x transient: the driver waits for the authoritative
# reset and, when a REST fallback is provided (read paths), serves the request
# on the core budget instead of blocking. GH_RETRY_REST_FALLBACK is set by the
# caller only for READ existence lookups.
gh_with_retry() {
  local label="$1"
  shift
  _gh_retry_core "$label" "$@"
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

# REST (core-budget) fallbacks for the read paths. When GraphQL is rate-limited
# but core still has budget, the shared driver serves the existence/inspection
# lookup via these instead of failing the scope probe closed. Each fallback
# echoes JSON in the same shape the matching `gh` command would return.
# shellcheck disable=SC2317  # invoked indirectly via GH_RETRY_REST_FALLBACK
_scope_rest_list() { gh_pr_exists_rest "$REPO" "$HEAD_REF"; }
# shellcheck disable=SC2317  # invoked indirectly via GH_RETRY_REST_FALLBACK
_scope_rest_view_number() { gh_pr_view_rest "$REPO" "$PR_NUMBER"; }
# shellcheck disable=SC2317  # invoked indirectly via GH_RETRY_REST_FALLBACK
_scope_rest_view_url() {
  local n
  n="$(gh_pr_number_from_url "$PR_URL")" || return 1
  gh_pr_view_rest "$REPO" "$n"
}

if [ -n "$PR_URL" ]; then
  raw_json="$(GH_RETRY_REST_FALLBACK=_scope_rest_view_url gh_with_retry "pr view" pr view "$PR_URL" --repo "$REPO" --json "$fields")" \
    || emit_failure "pr_metadata_unavailable" "unable to inspect explicit PR URL"
  raw_json="$(jq -nc --argjson pr "$raw_json" '[$pr]')"
elif [ -n "$PR_NUMBER" ]; then
  raw_json="$(GH_RETRY_REST_FALLBACK=_scope_rest_view_number gh_with_retry "pr view" pr view "$PR_NUMBER" --repo "$REPO" --json "$fields")" \
    || emit_failure "pr_metadata_unavailable" "unable to inspect explicit PR number"
  raw_json="$(jq -nc --argjson pr "$raw_json" '[$pr]')"
else
  raw_json="$(GH_RETRY_REST_FALLBACK=_scope_rest_list gh_with_retry "pr list" pr list --repo "$REPO" --head "$HEAD_REF" --state all --json "$fields")" \
    || emit_failure "pr_metadata_unavailable" "unable to list scoped PR candidates"
  if [ -z "${raw_json//[[:space:]]/}" ]; then
    raw_json="[]"
  fi
fi

if ! printf '%s' "$raw_json" | jq -e 'type == "array"' >/dev/null 2>&1; then
  emit_failure "invalid_pr_metadata" "GitHub PR metadata was not a JSON array"
fi

# PRIMARY KEY: (headRefName, baseRefName, same-repo, non-cross-repository).
# GitHub forbids a second OPEN PR for the same head->base in the same repo, so
# this tuple is already a reliable unique key for the recipe's own PR. Issue
# tokens / head-sha / title-prefix are NOT primary discriminators: layering them
# as hard rejections (a stale local tracking issue, or a remote head that has
# advanced past the captured sha) makes the lookup fail closed, then collide on
# `gh pr create` and hard-fail the recipe (issue #1017, PR #1015). When
# --pr-number/--pr-url is given the primary key also enforces that explicit
# identity so an authoritative lookup still targets exactly the named PR.
match_by_primary_key() {
  printf '%s' "$1" | jq -c \
    --arg repo "$REPO" \
    --arg headRefName "$HEAD_REF" \
    --arg baseRefName "$BASE_REF" \
    --arg prNumber "$PR_NUMBER" \
    --arg prUrl "$PR_URL" '
      def owner_name:
        (.headRepositoryOwner.login // .headRepositoryOwner.name // .headRepositoryOwner // "");
      def repo_name:
        (.headRepository.name // ((.headRepository.nameWithOwner // "") | split("/") | .[-1]) // "");
      [
        .[]
        | select((.headRefName // "") == $headRefName)
        | select((.baseRefName // "") == $baseRefName)
        | select((.isCrossRepository // false) == false)
        | select(((owner_name + "/" + repo_name) == $repo))
        | select(($prNumber == "") or (((.number // "") | tostring) == $prNumber))
        | select(($prUrl == "") or ((.url // "") == $prUrl))
      ]'
}

# Secondary discriminators. Applied as HARD filters only in authoritative
# (--pr-number/--pr-url) mode — where a stale explicitly-named PR must fail
# closed — and as TIE-BREAKERS in discovery mode only when >1 candidate shares
# the same head+base (rare open/closed variants).
apply_discriminators() {
  printf '%s' "$1" | jq -c \
    --arg headRefOid "$HEAD_SHA" \
    --arg issueId "$ISSUE_ID" \
    --arg workItemId "$WORK_ITEM_ID" \
    --arg recipeRunId "$RECIPE_RUN_ID" \
    --arg treeId "$TREE_ID" \
    --arg workstreamId "$WORKSTREAM_ID" \
    --arg expected_pr_title_prefix "$EXPECTED_PR_TITLE_PREFIX" \
    --arg created_after "$CREATED_AFTER" '
      def text:
        ((.title // "") + " " + (.body // ""));
      def has_token($token):
        ($token == "") or (text | contains($token));
      [
        .[]
        | select(($headRefOid == "") or ((.headRefOid // "") == $headRefOid))
        | select(($expected_pr_title_prefix == "") or ((.title // "") | startswith($expected_pr_title_prefix)))
        | select(($created_after == "") or ((.createdAt // "") >= $created_after))
        | select(($issueId == "") or (((.number // "") | tostring) == $issueId) or has_token("#" + $issueId) or has_token("issue-" + $issueId))
        | select(($workItemId == "") or (((.number // "") | tostring) == $workItemId) or has_token("#" + $workItemId) or has_token("AB#" + $workItemId) or has_token("work-item-" + $workItemId))
        | select(($recipeRunId == "") or has_token($recipeRunId))
        | select(($treeId == "") or has_token($treeId))
        | select(($workstreamId == "") or has_token($workstreamId))
      ]'
}

open_candidates() {
  printf '%s' "$1" | jq -c '[ .[] | select(((.state // "") | ascii_upcase) == "OPEN") ]'
}

array_length() {
  printf '%s' "$1" | jq 'length'
}

emit_multiple_scoped_prs() {
  jq -nc --arg reason "multiple_scoped_prs" --argjson candidates "$1" '{ok:false,reason:$reason,candidates:$candidates}'
  echo "ERROR: workflow_pr_scope.sh: multiple_scoped_prs: more than one PR matched the explicit workflow scope" >&2
  exit 1
}

emit_scoped_match() {
  printf '%s' "$1" | jq -c '.[0] + {ok:true, scoped:true}'
  exit 0
}

# Discovery-mode fallback. Prefer a single OPEN candidate over failing closed on
# the recipe's own PR; a genuine >=2-OPEN set (GitHub's one-open-PR-per-head->base
# invariant broken) is a real anomaly and fails loud. Always terminates.
prefer_single_open_or_fail() {
  local open
  open="$(open_candidates "$1")"
  if [ "$(array_length "$open")" -eq 1 ]; then
    emit_scoped_match "$open"
  fi
  emit_multiple_scoped_prs "$1"
}

primary="$(match_by_primary_key "$raw_json")"
primary_count="$(array_length "$primary")"

if [ "$primary_count" -eq 0 ]; then
  emit_failure "no_scoped_pr" "no PR matched the explicit workflow scope"
fi

if [ -n "$PR_NUMBER" ] || [ -n "$PR_URL" ]; then
  # Authoritative mode: the explicitly named PR must still satisfy every
  # historical metadata guard (stale head-sha, title-prefix, issue token, ...).
  # This preserves the fail-closed contract for a caller-supplied PR identity.
  matches="$(apply_discriminators "$primary")"
  match_count="$(array_length "$matches")"
  if [ "$match_count" -eq 0 ]; then
    emit_failure "no_scoped_pr" "no PR matched the explicit workflow scope"
  fi
  if [ "$match_count" -gt 1 ]; then
    emit_multiple_scoped_prs "$matches"
  fi
  emit_scoped_match "$matches"
fi

# Discovery mode: (head, base, same-repo, non-cross) is authoritative on its own.
if [ "$primary_count" -eq 1 ]; then
  emit_scoped_match "$primary"
fi

# More than one candidate shares the same head+base (rare open/closed variants).
# Use the discriminators only to break the tie.
tie="$(apply_discriminators "$primary")"
tie_count="$(array_length "$tie")"

if [ "$tie_count" -eq 1 ]; then
  emit_scoped_match "$tie"
fi

# tie_count > 1: discriminators narrowed but did not resolve -> prefer OPEN.
if [ "$tie_count" -gt 1 ]; then
  prefer_single_open_or_fail "$tie"
fi

# tie_count == 0: discriminators eliminated the recipe's own PR (stale issue
# token or advanced head-sha). Fall back to the primary key and prefer OPEN
# rather than failing closed on a PR that is genuinely ours.
prefer_single_open_or_fail "$primary"
