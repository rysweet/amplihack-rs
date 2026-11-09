#!/usr/bin/env bash
# workflow_push_plan.sh - classify HEAD against refs/remotes/origin/<target>
# for default-workflow step 15 (issue #1969, #978).
#
# Usage (executed, never sourced; runs in the current directory):
#   workflow_push_plan.sh <target>
#
# Prints exactly one line on stdout:
#   first-push ahead=<N>          refs/remotes/origin/<target> does not exist;
#                                 N = git rev-list --count HEAD --not --remotes=origin
#   already-pushed ahead=0 behind=<B>
#                                 HEAD has nothing origin/<target> lacks
#   diverged ahead=<A> behind=<B> merge_base=<sha|none> upstream=origin/<target>
#                                 both sides have commits the other lacks
#   fast-forward ahead=<A> behind=0
#                                 origin/<target> is an ancestor of HEAD
#
# Exit status: 0 with one line; 2 with nothing on stdout on a usage error, a
# <target> that `git check-ref-format --branch` does not print back unchanged,
# or any count or merge-base that fails or prints something unexpected.
#
# It only reads: it never fetches (step 15 does), never reads @{u}, and names
# every ref fully qualified, so a tag or local branch called origin/<target>
# cannot change the answer.

set -euo pipefail
# S7: an inherited repository, work tree or index must not redirect the reads.
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE

fail() {
  echo "ERROR: workflow_push_plan.sh: $1 (#1969)." >&2
  exit 2
}
[ "$#" -eq 1 ] || fail "usage: workflow_push_plan.sh <target>"
target="$1"
[ -n "$target" ] || fail "the push target is empty"
# S5: the name must come back unchanged; `--branch` expands @{-N}.
[ "$(git check-ref-format --branch "$target" 2>/dev/null || true)" = "$target" ] \
  || fail "'$target' is not a valid branch name"
git rev-parse --verify -q "HEAD^{commit}" >/dev/null 2>&1 || fail "HEAD does not name a commit"

# A count is a plain whole number or an error, never a silent zero.
count() {
  local n
  n="$(git rev-list --count "$@" 2>/dev/null)" || return 1
  case "$n" in ''|*[!0-9]*) return 1 ;; esac
  printf '%s\n' "$n"
}

upstream="refs/remotes/origin/$target"
if ! git rev-parse --verify -q "$upstream^{commit}" >/dev/null 2>&1; then
  ahead="$(count HEAD --not --remotes=origin --)" || fail "cannot count the commits of HEAD that are on no origin branch"
  printf 'first-push ahead=%s\n' "$ahead"
  exit 0
fi

ahead="$(count "$upstream..HEAD" --)" || fail "cannot count the commits of HEAD that are not on origin/$target"
behind="$(count "HEAD..$upstream" --)" || fail "cannot count the commits of origin/$target that are not in HEAD"

if [ "$ahead" = 0 ]; then
  printf 'already-pushed ahead=0 behind=%s\n' "$behind"
elif [ "$behind" = 0 ]; then
  printf 'fast-forward ahead=%s behind=0\n' "$ahead"
else
  mb_rc=0
  merge_base="$(git merge-base HEAD "$upstream" 2>/dev/null)" || mb_rc=$?
  if [ "$mb_rc" -eq 1 ] && [ -z "$merge_base" ]; then
    merge_base="none"
  elif [ "$mb_rc" -ne 0 ] || ! printf '%s\n' "$merge_base" | grep -Eqx '[0-9a-f]{40}|[0-9a-f]{64}'; then
    fail "cannot compute the merge base of HEAD and origin/$target"
  fi
  printf 'diverged ahead=%s behind=%s merge_base=%s upstream=origin/%s\n' "$ahead" "$behind" "$merge_base" "$target"
fi
