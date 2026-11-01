#!/usr/bin/env bash
# workflow_alias_reuse.sh - prepare an existing alias worktree for reuse by
# default-workflow step 4 (issue #1762).
#
# Usage (executed, never sourced):
#   workflow_alias_reuse.sh <worktree_path> <alias> <branch>
#
# Step 4 calls this when refs/heads/<alias> already exists. A reused alias
# must start where a newly created one would:
# at origin/<branch>, which step 4 has just fetched. Otherwise the planner's
# "push them, then re-run" remedy would leave the pushed commits out of the
# task worktree, step 15 would find the branch diverged, and anything an
# earlier run left behind would be staged into this run's commit.
#
# Checks, in order; the first failure is the only one reported:
#   0. <alias> is checked out at <worktree_path>. If not, another run may be
#      creating it, so stop and leave it alone.
#   1. The worktree has no uncommitted changes and no untracked files
#      (ignored files are not counted), and its status can be read.
#   2. <alias> has no commits that origin/<branch> lacks, and the count can be
#      read.
#   3. `git merge --ff-only origin/<branch>` succeeds in the worktree.
#
# Exit status: 0 the alias is clean and at origin/<branch>; 1 stop (one ERROR
# line on stderr, nothing changed); 2 usage.
#
# The only write is the fast-forward in step 3. It never fetches, pushes,
# discards changes or moves any other branch.

set -euo pipefail
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE

[ "$#" -eq 3 ] || { echo "usage: workflow_alias_reuse.sh <worktree_path> <alias> <branch>" >&2; exit 2; }
wt="$1" alias="$2" branch="$3"

stop() {
  echo "ERROR: cannot reuse alias worktree '$alias' at $wt for '$branch': $1 (issue #1762)." >&2
  exit 1
}

[ "$(git -C "$wt" symbolic-ref -q --short HEAD 2>/dev/null)" = "$alias" ] || {
  echo "ERROR: alias '$alias' exists but is not checked out at $wt; another run may be creating it. Wait for that run to finish, then re-run (issue #1762)." >&2
  exit 1
}
dirty="$(git -C "$wt" status --porcelain --untracked-files=all 2>/dev/null)" \
  || stop "git status failed there"
[ -z "$dirty" ] \
  || stop "it has uncommitted changes or untracked files, perhaps from an earlier or still-running run. Commit and push them, remove them, or remove the worktree, then re-run"
ahead="$(git -C "$wt" rev-list --count "refs/remotes/origin/$branch..refs/heads/$alias" 2>/dev/null)" \
  || stop "it cannot be compared with origin/$branch"
[ "$ahead" = 0 ] \
  || stop "it has $ahead commit(s) that origin/$branch does not. Push them to '$branch' or remove the worktree, then re-run"
git -C "$wt" merge -q --ff-only "refs/remotes/origin/$branch" >&2 \
  || stop "it cannot be fast-forwarded to origin/$branch"
echo "INFO: Reusing existing worktree directory at '$wt' (branch matches, clean, fast-forwarded to origin/$branch)." >&2
