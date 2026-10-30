#!/usr/bin/env bash
# workflow_pr_head_sync.sh - bring the run's worktree up to its pushed branch
# before workflow-finalize edits it (issue #1714, case 2).
#
# Usage (executed, never sourced), from the top of the run's linked worktree:
#   workflow_pr_head_sync.sh
#
# workflow-finalize runs the terminal-state probe with
# accept_pr_head_ahead=true, so an open PR whose head is a fast-forward of the
# local HEAD (main merged into the PR from another checkout, or GitHub's
# "Update branch" button) counts as this run's PR. The steps after the probe
# (step-20 cleanup, step-20c quality audit) edit the tree and step-20b
# pushes. Without this tool they would edit a tree missing the PR head's
# commits, and step-20b would push to a remote that is ahead of it.
#
# What it does, in the current directory, where <branch> is the branch's name
# on origin (workflow_remote_branch.sh: <B> for an alias <B>--wt-<n>, #1762):
#   1. Fetches origin refs/heads/<branch> into refs/remotes/origin/<branch>,
#      with the same no-prompt, bounded options as workflow_pr_head_fetch.sh.
#   2. If the branch has no upstream and origin/<branch> exists, sets the
#      upstream to origin/<branch>, so step-20b pulls before it pushes.
#   3. If origin/<branch> is a strict descendant of HEAD, runs
#      `git merge --ff-only origin/<branch>`. HEAD is an ancestor by
#      construction, so this cannot conflict with history; it can only be
#      refused by uncommitted changes to the same files.
# A local HEAD that is equal to, ahead of or diverged from origin/<branch> is
# left alone: step-20b's `git pull --rebase` handles it, as before.
#
# Exit status is always 0. A failed fetch, a detached HEAD, a branch that was
# never pushed, or a refused fast-forward prints one WARNING or INFO line on
# stderr and changes nothing more; step-20b then fails or succeeds on its own
# terms, exactly as without this tool.

set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=workflow_pr_head_fetch.sh
. "$here/workflow_pr_head_fetch.sh"

local_branch="$(git branch --show-current 2>/dev/null || :)"
# #1762: an alias (<B>--wt-<n> tracking origin/<B>) syncs with origin/<B>.
branch="$(bash "$here/workflow_remote_branch.sh" . 2>/dev/null)" || branch="$local_branch"
if [ -z "$local_branch" ]; then
  echo "WARNING: workflow_pr_head_sync: detached HEAD; nothing to sync (#1714)" >&2
  exit 0
fi

bound=""
! command -v timeout >/dev/null 2>&1 || bound="timeout 60"
ssh_cmd=""
_finalization_batch_ssh_command . ssh_cmd
if ! GIT_TERMINAL_PROMPT=0 GIT_SSH_COMMAND="$ssh_cmd" $bound git \
    -c http.lowSpeedLimit=1000 -c http.lowSpeedTime=30 \
    fetch --quiet --no-tags --no-write-fetch-head \
    origin "+refs/heads/$branch:refs/remotes/origin/$branch" </dev/null >/dev/null 2>&1; then
  echo "WARNING: workflow_pr_head_sync: could not fetch origin/$branch; worktree left as it is (#1714)" >&2
  exit 0
fi

remote_ref="refs/remotes/origin/$branch"
remote_oid="$(git rev-parse --verify -q "$remote_ref^{commit}" 2>/dev/null || :)"
if [ -z "$remote_oid" ]; then
  echo "INFO: workflow_pr_head_sync: origin/$branch does not exist; nothing to sync (#1714)" >&2
  exit 0
fi

if ! git rev-parse --abbrev-ref --symbolic-full-name '@{u}' >/dev/null 2>&1; then
  if git branch --quiet --set-upstream-to="origin/$branch" "$local_branch" >/dev/null 2>&1; then
    echo "INFO: workflow_pr_head_sync: set the upstream of $local_branch to origin/$branch (#1714)" >&2
  else
    echo "WARNING: workflow_pr_head_sync: could not set the upstream of $local_branch to origin/$branch (#1714)" >&2
  fi
fi

local_oid="$(git rev-parse HEAD)"
if [ "$local_oid" = "$remote_oid" ] || ! git merge-base --is-ancestor "$local_oid" "$remote_oid" 2>/dev/null; then
  exit 0
fi

count="$(git rev-list --count "$local_oid..$remote_oid")"
if git merge --ff-only --quiet "$remote_oid" >/dev/null 2>&1; then
  echo "INFO: workflow_pr_head_sync: fast-forwarded $local_branch by $count commit(s) to the PR head $remote_oid (#1714)" >&2
else
  echo "WARNING: workflow_pr_head_sync: could not fast-forward $local_branch to $remote_oid (uncommitted changes in the way?); step-20b will pull --rebase (#1714)" >&2
fi
exit 0
