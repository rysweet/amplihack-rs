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
#   6. <X> has the #1762 alias form <P>--wt-<n> (the last `--wt-`, n matching
#      ^[1-9][0-9]*$, <P> not empty) and <P> is not a default branch
#      (workflow_default_branch.sh exits 1 for <P>; exit 0 means a default
#      branch, which never gets an alias, so <X> is an ordinary branch and
#      falls through to 7; any other status: ERROR line, exit 2). That is the
#      shape workflow_remote_branch.sh maps, here with or without the upstream
#      configuration: two ERROR lines, exit 3. An alias worktree publishes to origin/<P> and is never pushed
#      or continued as a branch of its own. Continuing <P> from inside it is
#      not done either: the planner would reuse this very worktree (#858).
#      The lines name <P> and the checkout to re-run from, and never suggest
#      pushing <X>;
#   7. otherwise: the continuation INFO line on stderr, <X> alone on stdout,
#      exit 0.
#
# Exit status: 0 continue <X>; 1 not applicable (start a new branch); 2 error;
# 3 stop, repo_path is an alias worktree. Step 4 exits 1 on 2 and 3.
#
# AMPLIHACK_SESSION_DEPTH is matched as text and never evaluated: anything that
# is not a plain whole number is treated as unset (top-level).
#
# It only reads (`rev-parse`, `symbolic-ref`, `check-ref-format`, and
# `worktree list` for the alias stop's hint) and never runs `git status`, so a
# core.fsmonitor hook in the caller's repository is not started.

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

# $1 as one shell-inert word for copy-paste hints: 'X' with every single
# quote written as '\''.
shell_word() { printf "'%s'" "$(printf '%s' "$1" | sed "s/'/'\\\\''/g")"; }
quoted="$(shell_word "$branch")"

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

# 6. A #1762 alias worktree is not a branch of its own. This comes after the
# depth check, so a nested run started in a parent's alias task worktree keeps
# the #1134 behaviour above. A default branch never gets an alias, so
# <default>--wt-<n> is an ordinary branch here exactly as it is in
# workflow_remote_branch.sh; asking the same helper keeps step 4 and step 15
# agreeing on what an alias is, and keeps the hint below from ever naming a
# default branch as a push target or as existing_branch.
prefix="${branch%--wt-*}"
n="${branch##*--wt-}"
case "$n" in ''|0*|*[!0-9]*) prefix="$branch" ;; esac
if [ -n "$prefix" ] && [ "$prefix" != "$branch" ]; then
  prefix_default_rc=0
  bash "$(dirname -- "${BASH_SOURCE[0]}")/workflow_default_branch.sh" "$dir" "$prefix" || prefix_default_rc=$?
  case "$prefix_default_rc" in
    0) prefix="$branch" ;;
    1) ;;
    *) echo "ERROR: cannot decide whether '$prefix' is the default branch (workflow_default_branch.sh exit $prefix_default_rc) (#1969)." >&2; exit 2 ;;
  esac
fi
if [ -n "$prefix" ] && [ "$prefix" != "$branch" ]; then
  # The main clone is the first entry; <P>'s checkout, if any, is named in the hint.
  worktrees="$(git -C "$dir" worktree list --porcelain 2>/dev/null)" || worktrees=""
  main_path="" prefix_path="" path=""
  while IFS= read -r line; do
    case "$line" in
      "worktree "*) path="${line#worktree }"; [ -n "$main_path" ] || main_path="$path" ;;
      "branch refs/heads/$prefix") [ -n "$prefix_path" ] || prefix_path="$path" ;;
    esac
  done <<EOF
$worktrees
EOF
  p_word="$(shell_word "$prefix")"
  if [ -z "$prefix_path" ]; then
    where="check $p_word out in a linked worktree and re-run from there"
  elif [ -n "$main_path" ] && [ "$(physical "$prefix_path" || true)" = "$(physical "$main_path" || true)" ]; then
    # Gate C: the main clone on <P> is the run's caller, so this goes through the planner.
    where="re-run from the main clone with -c repo_path=$(shell_word "$main_path") -c existing_branch=$p_word"
  else
    where="re-run from $(shell_word "$prefix_path"), which has $p_word checked out"
  fi
  echo "ERROR: repo_path is a #1762 alias worktree: branch $quoted is the alias form <branch>--wt-<n> of $p_word. An alias worktree publishes to origin/$prefix and is never pushed or continued as a branch of its own, so step 4 is not continuing it and creates nothing (#1969)." >&2
  echo "ERROR: To continue $p_word, $where. To start a new branch, re-run with -c repo_path=$(shell_word "${main_path:-<main clone>}"). Commits made here that are not on origin/$prefix belong there: \"git push origin $(shell_word "HEAD:refs/heads/$prefix")\" (#1969, #1762)." >&2
  exit 3
fi

# 7. Continue the caller's branch.
echo "INFO: repo_path is a linked worktree on branch '$branch'; continuing caller branch '$branch' (#1969). The task worktree starts from origin/$branch, so only commits pushed there are carried into it. To start a new branch instead, run from the main clone or name a branch with a 'Branch:' line." >&2
printf '%s\n' "$branch"
