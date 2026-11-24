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
ALLOW_HEAD_BEHIND="false"
ALLOW_HEAD_AHEAD="false"
# Set only from argv, never from the environment, so an inherited variable
# cannot turn cross-base selection on for this helper's other callers (#2784).
ANY_BASE_OPEN="false"

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
  --allow-head-behind   accept an OPEN named PR whose head is an ancestor of --head-sha (#1467)
  --allow-head-ahead    accept an OPEN named PR whose head is a fast-forward of --head-sha (#1714)
  --any-base-open       discovery only: when no OPEN PR from --head targets --base, match the one
                        OPEN same-repository PR from --head into another base (#2784); two or more
                        give multiple_scoped_prs. No effect with --pr-number or --pr-url.
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
    --allow-head-behind) ALLOW_HEAD_BEHIND="true"; shift ;;
    --allow-head-ahead) ALLOW_HEAD_AHEAD="true"; shift ;;
    --any-base-open) ANY_BASE_OPEN="true"; shift ;;
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
GITHUB_REPO_HELPER="${WORKFLOW_GITHUB_REPO_HELPER:-${SCRIPT_DIR}/workflow_github_repo.sh}"
if [ ! -f "$GITHUB_REPO_HELPER" ] || [ ! -r "$GITHUB_REPO_HELPER" ]; then
  echo '{"ok":false,"reason":"missing_github_repo_helper"}'
  echo "ERROR: workflow_pr_scope.sh requires the shared GitHub repo identity parser at $GITHUB_REPO_HELPER" >&2
  exit 2
fi
# Defines parse_github_repo_identity (issue #1642).
# shellcheck source=workflow_github_repo.sh disable=SC1090
. "$GITHUB_REPO_HELPER"

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

# The head repository's owner/name, shared by every same-repository filter.
HEAD_REPO_JQ_DEFS='
      def owner_name:
        (.headRepositoryOwner.login // .headRepositoryOwner.name // .headRepositoryOwner // "");
      def repo_name:
        (.headRepository.name // ((.headRepository.nameWithOwner // "") | split("/") | .[-1]) // "");
'

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
    --arg prUrl "$PR_URL" "$HEAD_REPO_JQ_DEFS"'
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

# Issue #2787: on no_scoped_pr, which filters removed each candidate. One jq
# function over every filter the lookup applies: the six of the primary key
# (head, base, cross-repository, repository, pr-number, pr-url) and, when a PR
# was named, the eight secondary discriminators (head-sha, title-prefix,
# created-after, issue-token, work-item-token, recipe-run-id, tree-id,
# workstream-id). A head SHA accepted under --allow-head-behind or
# --allow-head-ahead (HEAD_SHA_ACCEPTED) does not count as a head-sha
# rejection. Each finding carries the compared values only when they are refs,
# SHAs, numbers, repository names or times; title and body tokens carry no
# value, since that text is the PR author's. Control characters are removed
# and every value is cut at 200 characters, because a candidate's head can
# come from a fork. Prints `[{number, rejected_by:[...], findings:[...]}]`.
HEAD_SHA_ACCEPTED=""
rejected_candidates() {
  local authoritative=false
  { [ -n "$PR_NUMBER" ] || [ -n "$PR_URL" ]; } && authoritative=true
  printf '%s' "$1" | jq -c \
    --arg repo "$REPO" --arg headRefName "$HEAD_REF" --arg baseRefName "$BASE_REF" \
    --arg prNumber "$PR_NUMBER" --arg prUrl "$PR_URL" --argjson auth "$authoritative" \
    --arg headRefOid "$HEAD_SHA" --arg acceptedOid "$HEAD_SHA_ACCEPTED" \
    --arg issueId "$ISSUE_ID" --arg workItemId "$WORK_ITEM_ID" --arg recipeRunId "$RECIPE_RUN_ID" \
    --arg treeId "$TREE_ID" --arg workstreamId "$WORKSTREAM_ID" \
    --arg expected_pr_title_prefix "$EXPECTED_PR_TITLE_PREFIX" --arg created_after "$CREATED_AFTER" \
    "$HEAD_REPO_JQ_DEFS"'
      def clean: tostring | gsub("[[:cntrl:]]"; "") | .[0:200];
      def text: ((.title // "") + " " + (.body // ""));
      def has_token($token): ($token == "") or (text | contains($token));
      def f($name; $value; $want): {filter:$name, value:($value | clean), want:($want | clean)};
      def rejected_by:
        [ (if (.headRefName // "") != $headRefName then f("head"; .headRefName // ""; $headRefName) else empty end),
          (if (.baseRefName // "") != $baseRefName then f("base"; .baseRefName // ""; $baseRefName) else empty end),
          (if (.isCrossRepository // false) != false then f("cross-repository"; "true"; "false") else empty end),
          (if (owner_name + "/" + repo_name) != $repo then f("repository"; owner_name + "/" + repo_name; $repo) else empty end),
          (if $prNumber != "" and ((.number // "") | tostring) != $prNumber then f("pr-number"; .number // ""; $prNumber) else empty end),
          (if $prUrl != "" and (.url // "") != $prUrl then f("pr-url"; .number // ""; $prUrl) else empty end),
          (if $auth then
            (if $headRefOid != "" and (.headRefOid // "") != $headRefOid and ($acceptedOid == "" or (.headRefOid // "") != $acceptedOid)
               then f("head-sha"; .headRefOid // ""; $headRefOid) else empty end),
            (if $expected_pr_title_prefix != "" and ((.title // "") | startswith($expected_pr_title_prefix) | not)
               then {filter:"title-prefix"} else empty end),
            (if $created_after != "" and ((.createdAt // "") < $created_after)
               then f("created-after"; .createdAt // ""; $created_after) else empty end),
            (if $issueId != "" and ((((.number // "") | tostring) == $issueId) or has_token("#" + $issueId) or has_token("issue-" + $issueId) | not)
               then {filter:"issue-token", want:("#" + $issueId | clean)} else empty end),
            (if $workItemId != "" and ((((.number // "") | tostring) == $workItemId) or has_token("#" + $workItemId) or has_token("AB#" + $workItemId) or has_token("work-item-" + $workItemId) | not)
               then {filter:"work-item-token", want:("#" + $workItemId | clean)} else empty end),
            (if $recipeRunId != "" and (has_token($recipeRunId) | not) then {filter:"recipe-run-id"} else empty end),
            (if $treeId != "" and (has_token($treeId) | not) then {filter:"tree-id"} else empty end),
            (if $workstreamId != "" and (has_token($workstreamId) | not) then {filter:"workstream-id"} else empty end)
          else empty end)
        ];
      [ .[] | rejected_by as $r | select($r | length > 0)
        | {number:(.number // null), rejected_by:($r | map(.filter)), findings:$r} ] | .[:50]'
}

# emit_no_scoped_pr <candidates JSON>: the no_scoped_pr failure, with the same
# reason, message, first stderr line and exit status as emit_failure, plus the
# `rejected` list in the JSON and one INFO line per candidate after the ERROR
# line (#2787).
emit_no_scoped_pr() {
  local rejected
  rejected="$(rejected_candidates "$1" 2>/dev/null)" || rejected='[]'
  printf '%s' "$rejected" | jq -e 'type == "array"' >/dev/null 2>&1 || rejected='[]'
  jq -nc --arg reason "no_scoped_pr" --arg message "no PR matched the explicit workflow scope" --argjson rejected "$rejected" \
    '{ok:false, reason:$reason, message:$message, rejected:[$rejected[] | {number, rejected_by}]}'
  echo "ERROR: workflow_pr_scope.sh: no_scoped_pr: no PR matched the explicit workflow scope" >&2
  printf '%s' "$rejected" | jq -r '.[] | "INFO: workflow_pr_scope.sh: candidate #\(.number // "?") was removed by: "
    + ([.findings[] | .filter + (if .value != null and .want != null then " (\(.value), wanted \(.want))"
        elif .want != null then " (wanted \(.want))" else "" end)] | join("; "))' >&2 || :
  if [ "$(printf '%s' "$rejected" | jq 'length')" = 0 ]; then
    echo "INFO: workflow_pr_scope.sh: GitHub listed no pull request for head $(printf '%s' "$HEAD_REF" | LC_ALL=C tr -d '\000-\037\177') in ${REPO}" >&2
  fi
  exit 1
}

open_candidates() {
  printf '%s' "$1" | jq -c '[ .[] | select(((.state // "") | ascii_upcase) == "OPEN") ]'
}

array_length() {
  printf '%s' "$1" | jq 'length'
}

# Issue #2784 (--any-base-open): OPEN same-repository PRs from --head into a
# base other than --base. The same-repository conditions are the primary
# key's; only the base differs, and only OPEN PRs count.
match_open_on_other_base() {
  printf '%s' "$1" | jq -c \
    --arg repo "$REPO" \
    --arg headRefName "$HEAD_REF" \
    --arg baseRefName "$BASE_REF" "$HEAD_REPO_JQ_DEFS"'
      [
        .[]
        | select((.headRefName // "") == $headRefName)
        | select((.baseRefName // "") != "")
        | select((.baseRefName // "") != $baseRefName)
        | select(((.state // "") | ascii_upcase) == "OPEN")
        | select((.isCrossRepository // false) == false)
        | select(((owner_name + "/" + repo_name) == $repo))
      ]'
}

# "#<number> into <base>" for each PR, comma-separated: diagnostics carry
# numbers and base names only, never titles, bodies or raw JSON.
pr_numbers_and_bases() {
  printf '%s' "$1" | jq -r '[ .[] | "#\(.number // "") into \(.baseRefName // "")" ] | join(", ")'
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

# Issue #2784: with base_branch empty, step-16 resolves --base from origin/HEAD,
# and an open PR from this head into another base (dev, say) was missed, so the
# caller created a second PR. Opt-in and discovery only; it applies when no
# primary-key candidate is OPEN, so an OPEN PR into --base still wins, and an
# OPEN PR into another base wins over a MERGED or CLOSED one into --base. Two
# or more share one head SHA, so nothing can choose between them: they fail.
# No extra GitHub call: `pr list --head --state all` has no base filter. Each
# value is assigned in its own statement so set -e stops on a jq failure
# instead of reading it as "no match".
if [ "$ANY_BASE_OPEN" = "true" ] && [ -z "$PR_NUMBER" ] && [ -z "$PR_URL" ]; then
  primary_open="$(open_candidates "$primary")"
  primary_open_count="$(array_length "$primary_open")"
  if [ "$primary_open_count" -eq 0 ]; then
    other_base="$(match_open_on_other_base "$raw_json")"
    other_count="$(array_length "$other_base")"
    if [ "$other_count" -gt 0 ]; then
      other_listing="$(pr_numbers_and_bases "$other_base")"
      if [ "$other_count" -eq 1 ]; then
        echo "INFO: workflow_pr_scope.sh: no open PR from $HEAD_REF into $BASE_REF; using open PR $other_listing (#2784)" >&2
        emit_scoped_match "$other_base"
      fi
      echo "INFO: workflow_pr_scope.sh: no open PR from $HEAD_REF into $BASE_REF and more than one into other bases: $other_listing; not choosing one (#2784)" >&2
      emit_multiple_scoped_prs "$other_base"
    fi
  fi
fi

if [ "$primary_count" -eq 0 ]; then
  emit_no_scoped_pr "$raw_json"
fi

if [ -n "$PR_NUMBER" ] || [ -n "$PR_URL" ]; then
  # Authoritative mode: the explicitly named PR must still satisfy every
  # historical metadata guard (stale head-sha, title-prefix, issue token, ...).
  # This preserves the fail-closed contract for a caller-supplied PR identity.
  matches="$(apply_discriminators "$primary")"
  match_count="$(array_length "$matches")"
  # Issue #1467, opt-in (--allow-head-behind): a named PR that is OPEN and whose
  # head is an ancestor of local HEAD is behind, not stale. Accept that head in
  # place of HEAD_SHA; every other discriminator stays hard. Two callers pass
  # it: the terminal-state probe, which runs before the push steps, and PR
  # ready (#2787), where the run has just pushed and GitHub can still report
  # the previous head for a moment. Behind there means GitHub has not caught up
  # yet, not that the PR lacks the run's work; the final status probe does not
  # pass it.
  if [ "$ALLOW_HEAD_BEHIND" = "true" ] && [ "$match_count" -eq 0 ] && [[ "$HEAD_SHA" =~ ^[0-9a-f]{40}$ ]]; then
    behind_oid="$(open_candidates "$primary" | jq -r 'if length == 1 then (.[0].headRefOid // "") else "" end')"
    if [[ "$behind_oid" =~ ^[0-9a-f]{40}$ ]] && git merge-base --is-ancestor "$behind_oid" "$HEAD_SHA" 2>/dev/null; then
      echo "INFO: workflow_pr_scope.sh: PR head $behind_oid is behind local HEAD $HEAD_SHA; treating it as unpushed work, not stale (#1467)" >&2
      HEAD_SHA_ACCEPTED="$behind_oid"
      matches="$(HEAD_SHA="$behind_oid" apply_discriminators "$primary")"
      match_count="$(array_length "$matches")"
    fi
  fi
  # Issue #1714, opt-in (--allow-head-ahead; only the terminal-state probe in
  # workflow-finalize): a named PR that is OPEN and whose head is a
  # fast-forward of local HEAD (a merge of main pushed from elsewhere) holds
  # every local commit, so it is the same PR. The test, and the one bounded,
  # non-interactive fetch it may need, are workflow_pr_head_fetch.sh's, shared
  # with the open-PR check. A diverged or rewritten head is still no match.
  if [ "$ALLOW_HEAD_AHEAD" = "true" ] && [ "$match_count" -eq 0 ] && [[ "$HEAD_SHA" =~ ^[0-9a-f]{40}$ ]]; then
    ahead_oid="$(open_candidates "$primary" | jq -r 'if length == 1 then (.[0].headRefOid // "") else "" end')"
    if [[ "$ahead_oid" =~ ^[0-9a-f]{40}$ ]] && [ -f "$SCRIPT_DIR/workflow_pr_head_fetch.sh" ]; then
      # shellcheck source=workflow_pr_head_fetch.sh
      . "$SCRIPT_DIR/workflow_pr_head_fetch.sh"
      if _finalization_pr_head_descends . "$HEAD_REF" "$HEAD_SHA" "$ahead_oid"; then
        echo "INFO: workflow_pr_scope.sh: PR head $ahead_oid is a fast-forward of local HEAD $HEAD_SHA; treating it as the same PR (#1714)" >&2
        HEAD_SHA_ACCEPTED="$ahead_oid"
        matches="$(HEAD_SHA="$ahead_oid" apply_discriminators "$primary")"
        match_count="$(array_length "$matches")"
      fi
    fi
  fi
  if [ "$match_count" -eq 0 ]; then
    emit_no_scoped_pr "$raw_json"
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
