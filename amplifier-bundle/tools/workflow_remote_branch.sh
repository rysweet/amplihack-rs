#!/usr/bin/env bash
# workflow_remote_branch.sh - print the name the checked-out branch has on
# origin (issue #1762).
#
# Usage (executed, never sourced):
#   workflow_remote_branch.sh [DIR]          DIR defaults to `.`
#
# When <B> is checked out in the caller's own checkout, default-workflow step 4
# puts the task worktree on a local alias, <B>--wt-<n>, that tracks origin/<B>.
# Every step that pushes, or looks up the PR by branch, must then use <B>. This
# tool prints, for the checkout at DIR:
#   - <P>, when the branch is <P>--wt-<n> with n matching ^[1-9][0-9]*$, <P> is
#     not empty and not a default branch (main, master, the target of
#     refs/remotes/origin/HEAD, or develop when origin/HEAD does not resolve),
#     branch.<name>.remote is origin and branch.<name>.merge is refs/heads/<P>;
#   - the branch name unchanged, for any other branch;
#   - nothing, on a detached HEAD, outside a repository or for a missing DIR.
# Only the last `--wt-` suffix is parsed. The branch configuration is read
# rather than @{u}, so a missing refs/remotes/origin/<P> does not matter.
#
# It only reads (`symbolic-ref`, `rev-parse`, `config --get`). Exit status is
# 0, or 2 on a usage error.
#
# Callers fall back to the local branch when this tool is missing:
#   rb="$(bash "$R" "$dir" 2>/dev/null)" || rb="$(git -C "$dir" branch --show-current 2>/dev/null || true)"

set -euo pipefail
# S7: an inherited repository, work tree or index must not redirect the reads.
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE

if [ "$#" -gt 1 ]; then
  echo "usage: workflow_remote_branch.sh [DIR]" >&2
  exit 2
fi
dir="${1:-.}"
[ -d "$dir" ] || exit 0

head_ref="$(git -C "$dir" symbolic-ref -q HEAD 2>/dev/null)" || exit 0
case "$head_ref" in
  refs/heads/?*) name="${head_ref#refs/heads/}" ;;
  *) exit 0 ;;
esac

prefix="${name%--wt-*}"
n="${name##*--wt-}"
unchanged() { printf '%s\n' "$name"; exit 0; }
[ "$prefix" != "$name" ] && [ -n "$prefix" ] || unchanged
case "$n" in ''|0*|*[!0-9]*) unchanged ;; esac

# A default branch never has an alias, so its alias-shaped names stay as they are.
case "$prefix" in main|master) unchanged ;; esac
origin_head="$(git -C "$dir" symbolic-ref -q refs/remotes/origin/HEAD 2>/dev/null || true)"
origin_head="${origin_head#refs/remotes/origin/}"
[ -z "$origin_head" ] || [ "$prefix" != "$origin_head" ] || unchanged
if [ "$prefix" = "develop" ] \
    && ! git -C "$dir" rev-parse --verify -q "refs/remotes/origin/HEAD^{commit}" >/dev/null 2>&1; then
  unchanged
fi

remote="$(git -C "$dir" config --get "branch.$name.remote" 2>/dev/null || true)"
merge="$(git -C "$dir" config --get "branch.$name.merge" 2>/dev/null || true)"
[ "$remote" = "origin" ] && [ "$merge" = "refs/heads/$prefix" ] || unchanged
printf '%s\n' "$prefix"
