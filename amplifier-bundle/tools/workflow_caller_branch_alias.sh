#!/usr/bin/env bash
# workflow_caller_branch_alias.sh - choose the local alias branch for a task
# worktree whose target branch is checked out in the caller's own checkout
# (issue #1762).
#
# Usage (executed, never sourced):
#   workflow_caller_branch_alias.sh <caller_path> <main_repo_path> <branch>
#
# Git allows a branch in only one worktree, and #858 forbids using the caller's
# checkout as the task worktree. default-workflow step 4 therefore puts the task
# worktree on a local alias, <branch>--wt-<n>, tracking origin/<branch>. This
# tool decides whether that is safe and which alias to use; step 4 creates it.
#
# Checks, in order; the first failure is the only one reported:
#   1. <branch> is a default branch (main, master, the target of
#      refs/remotes/origin/HEAD, or develop when origin/HEAD does not resolve):
#      INFO line, exit 3. Step 4 then gives the #858 refusal.
#   2. The caller has uncommitted changes to tracked files, or its status
#      cannot be read: exit 1.
#   3. refs/remotes/origin/<branch> does not resolve: exit 1.
#   4. refs/heads/<branch> has commits that are not on origin/<branch>, or the
#      count cannot be read: exit 1.
# Then the alias:
#   5. Reuse: the lowest n whose refs/heads/<branch>--wt-<n> tracks
#      origin/<branch> and is checked out at <main_repo_path>/worktrees/<alias>
#      (compared as physical paths).
#   6. Create: the lowest n for which no such ref, no file, directory or
#      symlink at the path, and no registered worktree at the path exists.
# On success the INFO line goes to stderr, the alias alone to stdout, exit 0.
#
# Exit status: 0 alias chosen; 1 stop (one ERROR line on stderr); 2 usage
# (wrong argument count, a missing directory, or a <branch> that
# `git check-ref-format --branch` does not print back unchanged, such as
# @{-1}); 3 default branch.
#
# It only reads. It never fetches, pushes, or creates branches, worktrees,
# configuration or refs: step 4 does that with the answer. Every check whose
# empty or zero result would mean "safe" stops when the command itself fails.
# Messages never contain a remote URL, file names or diffs.

set -euo pipefail
# S7: an inherited repository, work tree or index must not redirect the checks.
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE

usage() {
  echo "usage: workflow_caller_branch_alias.sh <caller_path> <main_repo_path> <branch>" >&2
  exit 2
}
[ "$#" -eq 3 ] || usage
caller="$1" main="$2" branch="$3"
[ -d "$caller" ] && [ -d "$main" ] || usage
# S5: the name must come back unchanged; `--branch` expands @{-N}.
[ "$(git -C "$caller" check-ref-format --branch "$branch" 2>/dev/null || true)" = "$branch" ] || usage

stop() {
  echo "ERROR: $1 (issue #1762)." >&2
  exit 1
}
at_caller="branch '$branch' is checked out in the caller repository at $caller"

# 1. The default branch never gets an alias.
origin_head="$(git -C "$caller" symbolic-ref -q refs/remotes/origin/HEAD 2>/dev/null || true)"
origin_head="${origin_head#refs/remotes/origin/}"
is_default=false
case "$branch" in main|master) is_default=true ;; esac
[ -z "$origin_head" ] || [ "$branch" != "$origin_head" ] || is_default=true
if [ "$branch" = "develop" ] \
    && ! git -C "$caller" rev-parse --verify -q "refs/remotes/origin/HEAD^{commit}" >/dev/null 2>&1; then
  is_default=true
fi
if [ "$is_default" = true ]; then
  echo "INFO: branch '$branch' is the default branch; it gets no alias worktree (issue #1762)." >&2
  exit 3
fi

# 2. Uncommitted changes to tracked files. --no-optional-locks keeps the
# caller's index as it is.
dirty="$(git -C "$caller" --no-optional-locks status --porcelain --untracked-files=no 2>/dev/null)" \
  || stop "cannot read the status of the caller repository at $caller for branch '$branch'; refusing to choose an alias worktree"
[ -z "$dirty" ] || stop "$at_caller and has uncommitted changes to tracked files. The task worktree starts from origin/$branch and would not see those changes. Commit and push them, or discard them, then re-run"

# 3. The branch must be on origin for the alias to track it.
git -C "$caller" rev-parse --verify -q "refs/remotes/origin/$branch^{commit}" >/dev/null 2>&1 \
  || stop "$at_caller but origin/$branch does not exist. The task worktree tracks origin/$branch and would not see the branch's commits. Push the branch with 'git push -u origin $branch', or work from a checkout that is not on '$branch', then re-run"

# 4. Local commits missing from origin/<branch>.
ahead="$(git -C "$caller" rev-list --count "refs/remotes/origin/$branch..refs/heads/$branch" 2>/dev/null)" || ahead="" # empty or non-numeric stops at the case below
case "$ahead" in
  ''|*[!0-9]*) stop "cannot compare branch '$branch' in the caller repository at $caller with origin/$branch; refusing to choose an alias worktree" ;;
esac
[ "$ahead" -eq 0 ] || stop "$at_caller and has $ahead commit(s) that are not on origin/$branch. The task worktree starts from origin/$branch and would not see those commits. Push them with 'git push origin $branch' (run 'git pull --rebase' first if origin/$branch has moved on), or discard them, then re-run"

physical() { (cd -P -- "$1" 2>/dev/null && pwd -P); }
main_real="$(physical "$main")" || usage
worktrees="$(git -C "$main" worktree list --porcelain)" \
  || stop "cannot list the worktrees of $main for branch '$branch'; refusing to choose an alias worktree"
# The worktree path git records for local branch $1, or nothing.
checked_out_at() {
  local line path=""
  while IFS= read -r line; do
    case "$line" in
      "worktree "*) path="${line#worktree }" ;;
      "branch refs/heads/$1") printf '%s\n' "$path"; return 0 ;;
    esac
  done <<EOF
$worktrees
EOF
  return 0
}
# True when git has a worktree registered at path $1, which need not exist.
registered_at() {
  local line
  while IFS= read -r line; do
    [ "$line" != "worktree $1" ] || return 0
  done <<EOF
$worktrees
EOF
  return 1
}
announce() {
  echo "INFO: $at_caller; the task worktree uses local branch '$1' with upstream origin/$branch (issue #1762)." >&2
  printf '%s\n' "$1"
  exit 0
}

# 5. Reuse pass, lowest n first.
refs="$(git -C "$main" for-each-ref --format='%(refname)' "refs/heads/$branch--wt-*")" \
  || stop "cannot list the local branches of $main for branch '$branch'; refusing to choose an alias worktree"
numbers=""
while IFS= read -r ref; do
  n="${ref#"refs/heads/$branch--wt-"}"
  case "$n" in ''|0*|*[!0-9]*) continue ;; esac
  numbers="$numbers$n
"
done <<EOF
$refs
EOF
for n in $(printf '%s' "$numbers" | LC_ALL=C sort -n); do
  alias_name="$branch--wt-$n"
  [ "$(git -C "$main" config --get "branch.$alias_name.remote" 2>/dev/null || true)" = "origin" ] || continue
  [ "$(git -C "$main" config --get "branch.$alias_name.merge" 2>/dev/null || true)" = "refs/heads/$branch" ] || continue
  want="$(physical "$main_real/worktrees/$alias_name")" || continue
  have="$(checked_out_at "$alias_name")"
  [ -n "$have" ] && [ "$(physical "$have")" = "$want" ] || continue
  announce "$alias_name"
done

# 6. Create pass: the lowest n with nothing in the way.
n=1
while :; do
  alias_name="$branch--wt-$n"
  path="$main_real/worktrees/$alias_name"
  if git -C "$main" show-ref --verify --quiet "refs/heads/$alias_name" \
      || [ -e "$path" ] || [ -L "$path" ] || registered_at "$path"; then
    n=$((n + 1))
    continue
  fi
  [ "$(git -C "$main" check-ref-format --branch "$alias_name" 2>/dev/null || true)" = "$alias_name" ] \
    || stop "alias '$alias_name' for branch '$branch' is not a valid branch name; refusing to choose an alias worktree"
  announce "$alias_name"
done
