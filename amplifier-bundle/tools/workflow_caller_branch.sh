#!/usr/bin/env bash
# workflow_caller_branch.sh - decide whether default-workflow step 4 should
# continue the caller's own branch (issue #1969).
#
# Usage (executed, never sourced):
#   workflow_caller_branch.sh <repo_path>
#
# A top-level run whose repo_path is a linked worktree on its own feature
# branch <X> continues <X> through the #1762 alias path instead of silently
# starting an unrelated branch. Checks, in order; the first that decides wins:
#   0. not exactly one argument, or repo_path is not a directory inside a git
#      repository: ERROR line, exit 2;
#   1. repo_path is in the main clone (absolute git dir equals the common git
#      dir, compared as physical paths): exit 1;
#   2. HEAD is detached: exit 1;
#   3. `git check-ref-format --branch <X>` does not print <X> back unchanged:
#      ERROR line, exit 2;
#   4. <X> is a default branch (workflow_default_branch.sh exits 0): exit 1;
#      the helper failing or missing: ERROR line, exit 2;
#   5. AMPLIHACK_SESSION_DEPTH is a whole number >= 2 (a nested run, #1134):
#      the nested-run INFO line, exit 1;
#   6. otherwise: the continuation INFO line on stderr, <X> alone on stdout,
#      exit 0.
#
# AMPLIHACK_SESSION_DEPTH is matched as text and never evaluated: anything that
# is not a plain whole number is treated as unset (top-level).
#
# It only reads (`rev-parse`, `symbolic-ref`, `check-ref-format`) and never runs
# `git status`, so a core.fsmonitor hook in the caller's repository is not
# started.

set -euo pipefail
# S7: an inherited repository, work tree or index must not redirect the reads.
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE

usage() {
  echo "ERROR: usage: workflow_caller_branch.sh <repo_path> (#1969)" >&2
  exit 2
}
[ "$#" -eq 1 ] || usage
dir="$1"
[ -n "$dir" ] && [ -d "$dir" ] || { echo "ERROR: repo_path '$dir' is not a directory (#1969)." >&2; exit 2; }
git -C "$dir" rev-parse --git-dir >/dev/null 2>&1 \
  || { echo "ERROR: repo_path '$dir' is not inside a git repository (#1969)." >&2; exit 2; }

physical() { (cd -P -- "$1" 2>/dev/null && pwd -P); }

# 1. The main clone is never a caller branch to continue.
git_dir="$(git -C "$dir" rev-parse --absolute-git-dir 2>/dev/null)" \
  || { echo "ERROR: cannot read the git directory of '$dir' (#1969)." >&2; exit 2; }
common_dir="$(git -C "$dir" rev-parse --path-format=absolute --git-common-dir 2>/dev/null)" \
  || { echo "ERROR: cannot read the common git directory of '$dir' (#1969)." >&2; exit 2; }
git_dir_real="$(physical "$git_dir")" || { echo "ERROR: cannot resolve '$git_dir' (#1969)." >&2; exit 2; }
common_real="$(physical "$common_dir")" || { echo "ERROR: cannot resolve '$common_dir' (#1969)." >&2; exit 2; }
[ "$git_dir_real" != "$common_real" ] || exit 1

# 2. A detached HEAD has no branch to continue.
head_ref="$(git -C "$dir" symbolic-ref -q HEAD 2>/dev/null)" || exit 1
case "$head_ref" in
  refs/heads/?*) branch="${head_ref#refs/heads/}" ;;
  *) exit 1 ;;
esac

# 3. The name must come back unchanged; `--branch` expands @{-N}.
if [ "$(git -C "$dir" check-ref-format --branch "$branch" 2>/dev/null || true)" != "$branch" ]; then
  echo "ERROR: the branch checked out at '$dir' is not a valid branch name; not continuing it (#1969)." >&2
  exit 2
fi

# 4. A default branch is never continued. The rule lives only in
# workflow_default_branch.sh; a missing or failing helper stops here.
default_rc=0
bash "$(dirname -- "${BASH_SOURCE[0]}")/workflow_default_branch.sh" "$dir" "$branch" || default_rc=$?
case "$default_rc" in
  0) exit 1 ;;
  1) ;;
  *) echo "ERROR: cannot decide whether '$branch' is the default branch (workflow_default_branch.sh exit $default_rc) (#1969)." >&2; exit 2 ;;
esac

# The branch as one shell-inert word for copy-paste hints: 'X' with every
# single quote written as '\''.
quoted="'$(printf '%s' "$branch" | sed "s/'/'\\\\''/g")'"

# 5. A nested run keeps the #1134 behaviour. The depth is text: only a plain
# whole number counts, leading zeros are stripped without arithmetic.
depth="${AMPLIHACK_SESSION_DEPTH:-}"
case "$depth" in
  ''|*[!0-9]*) depth="" ;;
  *) depth="${depth#"${depth%%[!0]*}"}" ;;
esac
case "$depth" in
  ''|1) ;;
  *)
    echo "INFO: nested run (depth $depth); not continuing caller branch '$branch' (#1969, #1134). Pass -c existing_branch=$quoted to continue it." >&2
    exit 1 ;;
esac

# 6. Continue the caller's branch.
echo "INFO: repo_path is a linked worktree on branch '$branch'; continuing caller branch '$branch' (#1969). The task worktree starts from origin/$branch, so only commits pushed there are carried into it. To start a new branch instead, run from the main clone or name a branch with a 'Branch:' line." >&2
printf '%s\n' "$branch"
