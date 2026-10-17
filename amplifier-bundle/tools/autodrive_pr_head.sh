#!/usr/bin/env bash
# autodrive_pr_head.sh — resolve a pull request's head and the local worktree
# that has its branch checked out, so auto-drive's evidence steps measure the
# PR head instead of whatever directory the run was started from (#1543).
#
#   autodrive_pr_head.sh --pr N [--branch B] [--repo R]
#
# The head comes from the platform (gh pr view headRefOid/headRefName). B,
# when given, must equal headRefName: a different B would measure (and the
# round would push from) a tree on the wrong branch, so the tool refuses. The
# worktree for headRefName is searched in this order:
#
#   1. R itself, when headRefName is the branch checked out there;
#   2. the `git worktree list --porcelain` entries of R's repository whose
#      branch is refs/heads/<headRefName> — exactly one, or the run refuses.
#
# READ-ONLY. This tool never creates, fetches into, checks out in, commits in
# or pushes from anything. It measures. A worktree whose HEAD differs from the
# PR head is reported (head_matches=false plus a stderr WARNING), not refused:
# the evidence records the tree's real HEAD and the merge gate refuses
# evidence that disagrees with the head it merges.
#
# On success stdout is ONE JSON line:
#   {"pr","head_sha","head_ref","worktree","worktree_head","head_matches","reason":"ok"}
# On failure stdout is empty and stderr says
#   autodrive_pr_head: <reason>: <detail>
#
#   Exit 0 — resolved.
#   Exit 2 — usage error.
#   Exit 3 — no-pr-worktree or ambiguous-pr-worktree.
#   Exit 4 — pr-lookup-failed: gh pr view failed or returned an unusable head.
#   Exit 5 — branch-mismatch: --branch is not the PR's headRefName.

set -uo pipefail
unset GIT_DIR GIT_WORK_TREE
export GIT_PAGER=cat GH_PAGER=cat PAGER=cat

die() { # die <exit> <reason> <detail>
  printf 'autodrive_pr_head: %s: %s\n' "$2" "$3" >&2
  exit "$1"
}

# json_esc: escape for a JSON string; control characters are refused.
json_esc() {
  case "$1" in *[[:cntrl:]]*) return 1 ;; esac
  local s="${1//\\/\\\\}"
  printf '%s' "${s//\"/\\\"}"
}

PR=""; BRANCH=""; REPO="."
while [ $# -gt 0 ]; do
  case "$1" in
    --pr|--branch|--repo)
      [ $# -ge 2 ] || die 2 usage "$1 needs a value"
      case "$1" in
        --pr) PR="$2" ;;
        --branch) BRANCH="$2" ;;
        --repo) REPO="$2" ;;
      esac
      shift 2 ;;
    *) die 2 usage "unknown argument '$1' (usage: autodrive_pr_head.sh --pr N [--branch B] [--repo R])" ;;
  esac
done
[[ "$PR" =~ ^[1-9][0-9]*$ ]] || die 2 usage "--pr must be a positive pull request number, got '${PR}'"
[ -n "$REPO" ] || REPO="."

ROOT="$(git -C "$REPO" rev-parse --show-toplevel 2>/dev/null)" \
  || die 3 no-pr-worktree "--repo '${REPO}' is not inside a git worktree"
ROOT="$(cd "$ROOT" && pwd -P)" || die 3 no-pr-worktree "cannot enter '${ROOT}'"
COMMON="$(cd "$ROOT" && cd "$(git rev-parse --git-common-dir)" 2>/dev/null && pwd -P)" \
  || die 3 no-pr-worktree "cannot read the git common dir of '${ROOT}'"

# --- the PR head, from the platform ------------------------------------------
LINE="$(cd "$ROOT" && gh pr view "$PR" --json headRefOid,headRefName,state \
  --jq '[.headRefOid, .headRefName, .state] | @tsv' 2>/dev/null)"
RC=$?
[ "$RC" -eq 0 ] || die 4 pr-lookup-failed "gh pr view failed (rc=${RC}) for PR #${PR}"
IFS=$'\t' read -r HEAD_SHA HEAD_REF _STATE <<<"$LINE"
[[ "${HEAD_SHA:-}" =~ ^([0-9a-f]{40}|[0-9a-f]{64})$ ]] \
  || die 4 pr-lookup-failed "PR #${PR} headRefOid is not a 40- or 64-hex commit id"
[ -n "${HEAD_REF:-}" ] || die 4 pr-lookup-failed "PR #${PR} has no headRefName"

if [ -n "$BRANCH" ] && [ "$BRANCH" != "$HEAD_REF" ]; then
  die 5 branch-mismatch "--branch '${BRANCH}' is not PR #${PR}'s head branch '${HEAD_REF}'; pass existing_branch=${HEAD_REF} or leave it empty"
fi

# same_repo <dir>: the directory belongs to the repository R belongs to.
same_repo() {
  local c
  c="$(cd "$1" 2>/dev/null && cd "$(git rev-parse --git-common-dir 2>/dev/null)" 2>/dev/null && pwd -P)" || return 1
  [ "$c" = "$COMMON" ]
}

# --- 1. R itself --------------------------------------------------------------
WT=""
CURRENT="$(git -C "$ROOT" branch --show-current 2>/dev/null)"
[ -n "$CURRENT" ] && [ "$CURRENT" = "$HEAD_REF" ] && WT="$ROOT"

# --- 2. the repository's worktree list ---------------------------------------
if [ -z "$WT" ]; then
  LIST="$(git -C "$ROOT" worktree list --porcelain 2>/dev/null)" \
    || die 3 no-pr-worktree "cannot list the worktrees of '${ROOT}'"
  MATCHES=()
  path=""; ref=""; skip="false"
  while IFS= read -r line; do
    case "$line" in
      "worktree "*) path="${line#worktree }" ;;
      "branch "*) ref="${line#branch }" ;;
      bare|prunable|"prunable "*) skip="true" ;;
      "")
        if [ -n "$path" ] && [ "$skip" = "false" ] && [ "$ref" = "refs/heads/$HEAD_REF" ] \
           && [ "${path:0:1}" = "/" ] && [ -d "$path" ] && same_repo "$path"; then
          MATCHES+=("$(cd "$path" && pwd -P)")
        fi
        path=""; ref=""; skip="false"
        ;;
    esac
  done <<<"$LIST"$'\n'
  if [ "${#MATCHES[@]}" -gt 1 ]; then
    printf 'autodrive_pr_head: ambiguous-pr-worktree: PR #%s branch %s is checked out in %s worktrees; check another branch out in all but one, or remove the extras\n' \
      "$PR" "'${HEAD_REF}'" "${#MATCHES[@]}" >&2
    printf '  %s\n' "${MATCHES[@]}" >&2
    exit 3
  fi
  [ "${#MATCHES[@]}" -eq 1 ] && WT="${MATCHES[0]}"
fi

[ -n "$WT" ] || die 3 no-pr-worktree "PR #${PR} head branch '${HEAD_REF}' has no worktree (searched --repo '${ROOT}' and git worktree list)"

WT_HEAD="$(git -C "$WT" rev-parse HEAD 2>/dev/null)" \
  || die 3 no-pr-worktree "cannot read HEAD in worktree '${WT}'"
MATCH="true"
if [ "$WT_HEAD" != "$HEAD_SHA" ]; then
  MATCH="false"
  echo "WARNING: worktree ${WT} is at ${WT_HEAD}, PR #${PR} head is ${HEAD_SHA}; run: git -C ${WT} pull --ff-only" >&2
fi

E_WT="$(json_esc "$WT")" || die 3 no-pr-worktree "worktree path contains control characters"
E_REF="$(json_esc "$HEAD_REF")" || die 4 pr-lookup-failed "headRefName contains control characters"
printf '{"pr":"%s","head_sha":"%s","head_ref":"%s","worktree":"%s","worktree_head":"%s","head_matches":"%s","reason":"ok"}\n' \
  "$PR" "$HEAD_SHA" "$E_REF" "$E_WT" "$WT_HEAD" "$MATCH"
