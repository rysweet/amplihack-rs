#!/usr/bin/env bash
set -euo pipefail

export GIT_PAGER=cat GH_PAGER=cat PAGER=cat LESS=FRX
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Usage: workflow_pr_ready.sh [--base BRANCH]
#   --base BRANCH  The run's base_branch, which step-16 passed to
#                  `gh pr create --base`. A set value is never replaced.
#                  Empty or absent: the base of the pull request this run
#                  recorded, from step-16's publish result or the pull
#                  request's own baseRefName (delivery_run_base in
#                  workflow_delivery_inputs.sh). origin/HEAD, then origin/main,
#                  origin/master, origin/develop, is used only when the run
#                  recorded no pull request at all
#                  (docs/reference/workflow-delivery-inputs.md).
#
# Issue #2787: a finished run's own PR is marked ready, or the tool says why
# not and leaves it alone. It never ends a run whose work is done.
#   - The branch and head are read from the PR's own worktree: the
#     `git worktree list --porcelain` entry on worktree_setup.branch_name, then
#     on the PR's headRefName (or a <B>--wt-<n> alias of either), found from
#     the caller's folder or REPO_PATH. A checkout on the base branch is never
#     taken for the PR's branch. existing_branch is not read: workflow-finalize,
#     the one recipe that calls this tool, does not declare it (#1561), and
#     step-04 records a given existing_branch as worktree_setup.branch_name.
#   - A PR head one or more commits behind the local head (GitHub not yet
#     reporting the push this run just made) is accepted: the scope lookup gets
#     --allow-head-behind and the identity check accepts an ancestor. The
#     repository, cross-repository, base and state checks are unchanged, and a
#     cross-repository PR is never marked ready.
#   - When the run's PR is known (pr_url or pr_number) but the scoped lookup or
#     the identity check rejects it, the tool prints a WARNING with the reason,
#     leaves the draft flag alone, and exits 0. The PR stays as it is for the
#     next step or agent to judge.
# Unchanged exits: 2 usage, 127 no gh or jq, 1 for gh auth failure, a closed
# unmerged PR, a failed ready call, or a scope failure when no PR is known.
EXPECTED_BASE_INPUT=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --base)
      [ "$#" -ge 2 ] || { echo "ERROR: workflow_pr_ready.sh: --base needs a value" >&2; exit 2; }
      EXPECTED_BASE_INPUT="$2"; shift 2 ;;
    *) echo "ERROR: workflow_pr_ready.sh: unknown argument '$1' (usage: workflow_pr_ready.sh [--base BRANCH])" >&2; exit 2 ;;
  esac
done
if [ -n "$EXPECTED_BASE_INPUT" ] && ! git check-ref-format --branch "$EXPECTED_BASE_INPUT" >/dev/null 2>&1; then
  printf 'ERROR: workflow_pr_ready.sh: --base %q is not a valid branch name\n' "$EXPECTED_BASE_INPUT" >&2
  exit 2
fi

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

# rv_field <key> <field>: this run's own field from the base RECIPE_VAR_<key> (#1561).
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/recipe_var.sh" \
  || { echo "ERROR: workflow_pr_ready.sh: recipe_var.sh not found beside it" >&2; exit 2; }

PR_URL="${RECIPE_VAR_pr_url:-$(rv_field pr_publish_result pr_url)}"
PR_NUMBER="${RECIPE_VAR_pr_number:-$(rv_field pr_publish_result pr_number)}"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
GH_RETRY_HELPER="${WORKFLOW_GH_RETRY_HELPER:-${SCRIPT_DIR}/workflow_gh_retry.sh}"
if [ ! -f "$GH_RETRY_HELPER" ]; then
  echo "ERROR: workflow_pr_ready.sh requires the shared retry helper at $GH_RETRY_HELPER" >&2
  exit 2
fi
# shellcheck source=/dev/null
. "$GH_RETRY_HELPER"
GITHUB_REPO_HELPER="${WORKFLOW_GITHUB_REPO_HELPER:-${SCRIPT_DIR}/workflow_github_repo.sh}"
if [ ! -f "$GITHUB_REPO_HELPER" ] || [ ! -r "$GITHUB_REPO_HELPER" ]; then
  echo "ERROR: workflow_pr_ready.sh requires the shared GitHub repo identity parser at $GITHUB_REPO_HELPER" >&2
  exit 2
fi
# Defines parse_github_repo_identity (issue #1642).
# shellcheck source=workflow_github_repo.sh disable=SC1090
. "$GITHUB_REPO_HELPER"
# Defines delivery_run_base, the one rule for the base of this run's PR (#2520).
DELIVERY_INPUTS_HELPER="${WORKFLOW_DELIVERY_INPUTS_HELPER:-${SCRIPT_DIR}/workflow_delivery_inputs.sh}"
if [ ! -f "$DELIVERY_INPUTS_HELPER" ] || [ ! -r "$DELIVERY_INPUTS_HELPER" ]; then
  echo "ERROR: workflow_pr_ready.sh requires the delivery-input helper at $DELIVERY_INPUTS_HELPER" >&2
  exit 2
fi
# shellcheck source=workflow_delivery_inputs.sh disable=SC1090
. "$DELIVERY_INPUTS_HELPER"

# Rate-limit-aware retry. This helper both reads (pr view) and MUTATES (pr ready,
# pr comment); per the workflow contract mutation paths must never fall back to
# REST, so no REST fallback is configured here — a genuine rate limit waits for
# the authoritative reset and then retries. Auth errors are never retried.
gh_with_retry() {
  local label="$1"
  shift
  _gh_retry_core "$label" "$@"
}

# The base is worked out once, after the repository and branch are known, and
# that one value is used by both the scoped lookup and the identity check.
EXPECTED_BASE=""
resolve_expected_base_branch() {
  local repo="$1" branch="$2"
  delivery_run_base "$EXPECTED_BASE_INPUT" "$(rv_field pr_publish_result base_branch)" \
    "${PR_URL:-$PR_NUMBER}" "$repo" "$branch" "${RECIPE_VAR_forbidden_base_branches:-}"
}

validate_pr_identity_before_mutation() {
  local pr_json="$1" pr_url="$2" current_branch local_head repo_identity pr_base_identity expected_base="$EXPECTED_BASE"
  local pr_head_ref pr_base_ref pr_head_oid pr_head_owner pr_head_repo pr_cross_repo

  git rev-parse --is-inside-work-tree >/dev/null 2>&1 || {
    echo "WARNING: not marking PR '$pr_url' ready; workflow_pr_ready.sh is not running inside a git worktree" >&2
    return 1
  }
  # #1762: an alias worktree (<B>--wt-<n> tracking origin/<B>) is on <B>.
  current_branch="$(bash "$SCRIPT_DIR/workflow_remote_branch.sh" .)" || {
    echo "WARNING: not marking PR '$pr_url' ready; workflow_remote_branch.sh is missing or failed at $SCRIPT_DIR" >&2
    return 1
  }
  [ -n "$current_branch" ] || {
    echo "WARNING: not marking PR '$pr_url' ready; current branch is empty" >&2
    return 1
  }
  local_head="$(git rev-parse --verify HEAD 2>/dev/null)" || {
    echo "WARNING: not marking PR '$pr_url' ready; local HEAD does not resolve" >&2
    return 1
  }
  repo_identity="$(parse_github_repo_identity "$(git config --get remote.origin.url 2>/dev/null || true)" || true)"
  [ -n "$repo_identity" ] || {
    echo "WARNING: not marking PR '$pr_url' ready; unable to determine current GitHub repo identity from origin remote" >&2
    return 1
  }
  pr_base_identity="$(parse_github_repo_identity "$pr_url" || true)"
  if [ "$pr_base_identity" != "$repo_identity" ]; then
    echo "WARNING: not marking PR '$pr_url' ready; PR URL repo '${pr_base_identity:-unknown}' does not match current repo '$repo_identity'" >&2
    return 1
  fi

  pr_head_ref=$(printf '%s' "$pr_json" | jq -r '.headRefName // ""')
  pr_base_ref=$(printf '%s' "$pr_json" | jq -r '.baseRefName // ""')
  pr_head_oid=$(printf '%s' "$pr_json" | jq -r '.headRefOid // ""')
  pr_head_owner=$(printf '%s' "$pr_json" | jq -r '.headRepositoryOwner.login // .headRepositoryOwner.name // .headRepositoryOwner // ""')
  pr_head_repo=$(printf '%s' "$pr_json" | jq -r '.headRepository.name // ((.headRepository.nameWithOwner // "") | split("/") | .[-1]) // ""')
  pr_cross_repo=$(printf '%s' "$pr_json" | jq -r '.isCrossRepository // false')

  if [ -z "$pr_head_ref" ] || [ -z "$pr_base_ref" ] || [ -z "$pr_head_oid" ] || [ -z "$pr_head_owner" ] || [ -z "$pr_head_repo" ]; then
    echo "WARNING: not marking PR '$pr_url' ready; PR metadata is incomplete" >&2
    return 1
  fi
  if [ "$pr_head_ref" != "$current_branch" ]; then
    echo "WARNING: not marking PR '$pr_url' ready; headRefName '$pr_head_ref' does not match current branch '$current_branch'" >&2
    return 1
  fi
  # #2787: only the head SHA check is relaxed. A PR head that is the local head
  # or an ancestor of it is this run's PR, with GitHub not yet showing a push
  # made moments ago. A head that is neither (rewritten, diverged, unknown
  # here) is still refused.
  if [ "$pr_head_oid" != "$local_head" ]; then
    if [[ "$pr_head_oid" =~ ^[0-9a-f]{40}([0-9a-f]{24})?$ ]] && git merge-base --is-ancestor "$pr_head_oid" "$local_head" 2>/dev/null; then
      echo "INFO: PR '$pr_url' head $pr_head_oid is an ancestor of local HEAD $local_head; GitHub has not caught up with the push yet, so it is accepted (#2787)" >&2
    else
      echo "WARNING: not marking PR '$pr_url' ready; headRefOid '$pr_head_oid' is neither local HEAD '$local_head' nor an ancestor of it" >&2
      return 1
    fi
  fi
  if [ "$pr_cross_repo" = "true" ] || [ "$pr_head_owner/$pr_head_repo" != "$repo_identity" ]; then
    echo "WARNING: not marking PR '$pr_url' ready; PR head repo '$pr_head_owner/$pr_head_repo' does not match current repo '$repo_identity'" >&2
    return 1
  fi
  if [ -z "$expected_base" ]; then
    echo "WARNING: not marking PR '$pr_url' ready; unable to resolve expected base branch for baseRefName validation" >&2
    return 1
  fi
  if [ "$pr_base_ref" != "$expected_base" ]; then
    echo "WARNING: not marking PR '$pr_url' ready; baseRefName '$pr_base_ref' does not match expected base '$expected_base'" >&2
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

# --- #2787: read the branch and head from the PR's own worktree --------------
# A 1.2.213 step-21 that cannot resolve worktree_setup falls back to REPO_PATH,
# the launch checkout, which is usually on the base branch; the scope lookup
# then looked for <base> -> <base> and missed the run's open PR. The worktree
# on the PR's branch is found from git's own list instead.
#
# pr_worktree_for <branch> <list-from>: the path of the linked worktree on
# <branch>, or on its <branch>--wt-<n> alias (#1762), listed from <list-from>.
pr_worktree_for() {
  local want="$1" from="$2" line path="" br
  [ -n "$want" ] && [ -d "$from" ] || return 1
  case "$want" in -*) return 1 ;; esac
  git check-ref-format --branch "$want" >/dev/null 2>&1 || return 1
  while IFS= read -r line; do
    case "$line" in
      "worktree "*) path="${line#worktree }" ;;
      "branch refs/heads/"*)
        br="${line#branch refs/heads/}"
        if { [ "$br" = "$want" ] || [[ "$br" =~ ^"$want"--wt-[1-9][0-9]*$ ]]; } && [ -d "$path" ]; then
          printf '%s\n' "$path"; return 0
        fi ;;
    esac
  done < <(git -C "$from" worktree list --porcelain 2>/dev/null)
  return 1
}
WT_LIST_FROM="."
git rev-parse --is-inside-work-tree >/dev/null 2>&1 || WT_LIST_FROM="${REPO_PATH:-}"
if [ -n "$WT_LIST_FROM" ] && [ -d "$WT_LIST_FROM" ]; then
  pr_wt=""; pr_wt_from=""
  for src in worktree_setup pr_head; do
    case "$src" in
      worktree_setup) want="$(rv_field worktree_setup branch_name)" ;;
      pr_head)
        want=""
        if [[ "${PR_URL:-}" =~ ^https://github\.com/[^[:space:]]+/pull/[1-9][0-9]*$ ]] || [[ "${PR_NUMBER:-}" =~ ^[1-9][0-9]*$ ]]; then
          want="$(cd "$WT_LIST_FROM" && gh pr view "${PR_URL:-$PR_NUMBER}" --json headRefName --jq '.headRefName // ""' 2>/dev/null)" || want=""
        fi ;;
    esac
    want="${want%%$'\n'*}"
    if pr_wt="$(pr_worktree_for "$want" "$WT_LIST_FROM")"; then pr_wt_from="$src"; break; fi
  done
  if [ -n "$pr_wt" ]; then
    if [ "$(cd "$pr_wt" && pwd -P)" != "$(pwd -P)" ]; then
      echo "INFO: reading the branch and head from the PR worktree $pr_wt, found from ${pr_wt_from}, not from $(pwd -P) (#2787)" >&2
    else
      echo "INFO: this folder is the PR worktree, found from ${pr_wt_from} (#2787)" >&2
    fi
    cd "$pr_wt"
  fi
fi

# cannot_validate <reason>: the facts the scoped lookup needs are missing. With
# the run's PR known (pr_url or pr_number) that is a reason not to mark it
# ready, never a reason to end a run whose work is done (#2787): WARNING, the
# PR left as it is, exit 0. With no PR known it stays an error, as before.
cannot_validate() {
  if [[ "$PR_URL" =~ [^[:space:]] ]] || [[ "$PR_NUMBER" =~ ^[1-9][0-9]*$ ]]; then
    echo "WARNING: the run's PR ${PR_URL:-#$PR_NUMBER} was not marked ready: $1. It is left as it is for the next step or agent to judge (#2787)." >&2
    exit 0
  fi
  echo "ERROR: $1; refusing ambiguous PR readiness mutation" >&2
  exit 1
}

scoped_pr_json=""
if ! git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "INFO: [skip] not a git repo — skipping gh pr ready" >&2
  exit 0
fi
current_branch="$(bash "$SCRIPT_DIR/workflow_remote_branch.sh" .)" || { echo "ERROR: workflow_remote_branch.sh is missing or failed at $SCRIPT_DIR; refusing PR readiness mutation" >&2; exit 1; }
local_head="$(git rev-parse --verify HEAD 2>/dev/null || true)"
repo_identity="$(parse_github_repo_identity "$(git config --get remote.origin.url 2>/dev/null || true)" || true)"
[ -x "$PR_SCOPE_HELPER" ] || { echo "ERROR: workflow_pr_scope.sh missing or not executable at $PR_SCOPE_HELPER" >&2; exit 1; }
if [[ "$PR_NUMBER" =~ [^[:space:]] ]] && ! [[ "$PR_NUMBER" =~ ^[1-9][0-9]*$ ]]; then
  echo "ERROR: invalid PR_NUMBER '$PR_NUMBER' for workflow_pr_ready.sh" >&2
  exit 1
fi
[ -n "$repo_identity" ] || cannot_validate "unable to determine current GitHub repo identity from origin remote"
[ -n "$current_branch" ] || cannot_validate "current branch is empty"
[ -n "$local_head" ] || cannot_validate "local HEAD does not resolve"
if ! EXPECTED_BASE="$(resolve_expected_base_branch "$repo_identity" "$current_branch")" || [ -z "$EXPECTED_BASE" ]; then
  cannot_validate "unable to resolve expected base branch for scoped PR validation"
fi

scoped_issue_id="${RECIPE_VAR_issue_number:-}"
# issue_number can carry a non-numeric local tracking reference (e.g.
# local-5d904cff4398) when the workflow fell back to local tracking (#815,
# #804). PR-scope matching only understands numeric ids, so coerce a
# non-numeric ref to empty: a local ref must not filter out the current-work PR.
case "$scoped_issue_id" in ''|*[!0-9]*) scoped_issue_id="" ;; esac
scope_args=(
  --repo "$repo_identity"
  --head "$current_branch"
  --base "$EXPECTED_BASE"
  --issue "$scoped_issue_id"
  --work-item "$scoped_issue_id"
  --head-sha "$local_head"
  --allow-head-behind
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
  if [[ "$PR_URL" =~ [^[:space:]] ]] || [[ "$PR_NUMBER" =~ ^[1-9][0-9]*$ ]]; then
    # #2787: the run's own PR is known, so a failed lookup is not a reason to
    # end a run whose work is done. The PR is left as it is.
    echo "WARNING: the run's PR ${PR_URL:-#$PR_NUMBER} was not marked ready: the scoped lookup rejected it (${reason:-unknown}; the lines above name each check that removed it). It is left as it is for the next step or agent to judge (#2787)." >&2
    exit 0
  fi
  echo "ERROR: scoped PR validation failed before ready mutation: ${reason:-unknown}" >&2
  exit 1
fi
pr_target="$(printf '%s' "$scoped_pr_json" | jq -r '.url // .number // empty')"
[ -n "$pr_target" ] || { echo "ERROR: scoped PR validation returned no PR target" >&2; exit 1; }

terminal_status="active-pr"
closed_unmerged_seen="false"
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
  echo "=== PR Ready Step Complete (terminal_status=$terminal_status) ==="
  exit 0
fi
if [ "$pr_state" = "CLOSED" ]; then
  terminal_status="closed-unmerged"
  closed_unmerged_seen="true"
  echo "ERROR: terminal_status=closed-unmerged; PR is closed without merge — reopen it or create a new branch intentionally" >&2
fi

if [ "$closed_unmerged_seen" = "true" ]; then
  exit 1
fi

if ! validate_pr_identity_before_mutation "$pr_json" "$pr_url"; then
  # #2787: the reason is the WARNING line above. Nothing is changed on the PR.
  echo "WARNING: the run's PR '$pr_url' was not marked ready; it is left as it is for the next step or agent to judge (#2787)." >&2
  exit 0
fi

if [ "$pr_is_draft" = "true" ]; then
  if ready_output="$(gh_with_retry "pr ready" pr ready "$pr_url")"; then
    printf '%s\n' "$ready_output"
  else
    ready_rc=$?
    echo "ERROR: gh pr ready failed for '$pr_url' (exit ${ready_rc}); refusing to report successful finalization after mutation failure" >&2
    exit "$ready_rc"
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

if [ "$closed_unmerged_seen" = "true" ]; then
  exit 1
fi
echo "=== PR Ready Step Complete (terminal_status=$terminal_status) ==="
