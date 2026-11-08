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
# Without --strict-fetch, exit status is always 0. A failed fetch, a detached HEAD, a branch that was
# never pushed, or a refused fast-forward prints one WARNING or INFO line on
# stderr and changes nothing more; step-20b then fails or succeeds on its own
# terms, exactly as without this tool.

set -uo pipefail

# Checkpoint callers must retain a failed transport instead of treating it as
# optional enrichment. Other callers keep the original best-effort contract.
strict_fetch=false
probe_origin_ref=false
case "${1:-}" in
  --strict-fetch)
    strict_fetch=true
    if [ "$#" -eq 2 ] && [ "$2" = --probe-origin-ref ]; then probe_origin_ref=true
    elif [ "$#" -ne 1 ]; then exit 2; fi ;;
  '') [ "$#" -eq 0 ] || exit 2 ;;
  *) echo 'ERROR: invalid workflow_pr_head_sync argument' >&2; exit 2 ;;
esac

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=workflow_pr_head_fetch.sh
. "$here/workflow_pr_head_fetch.sh"

local_branch="$(git branch --show-current 2>/dev/null || :)"
# #1762: an alias (<B>--wt-<n> tracking origin/<B>) syncs with origin/<B>.
if [ -z "$local_branch" ]; then
  echo "WARNING: workflow_pr_head_sync: detached HEAD; nothing to sync (#1714)" >&2
  exit 0
fi
branch="$(bash "$here/workflow_remote_branch.sh" .)" || {
  echo "WARNING: workflow_pr_head_sync: workflow_remote_branch.sh is missing or failed; nothing to sync (#1762)" >&2
  exit 0
}

bound=""
! command -v timeout >/dev/null 2>&1 || bound="timeout 60"
ssh_cmd=""
_finalization_batch_ssh_command . ssh_cmd
# Owner-approved #1554 early mode (17.116 amendment): this fetch-origin read
# is opt-in for the earlier finalization caller only. Cleanup does not select it.
if [ "$probe_origin_ref" = true ]; then
  probe_output=$(mktemp "${TMPDIR:?assigned TMPDIR required}/early-sync-origin.XXXXXX") || exit $?
  probe_rc=0
  GIT_TERMINAL_PROMPT=0 GIT_SSH_COMMAND="$ssh_cmd" $bound git \
    -c http.lowSpeedLimit=1000 -c http.lowSpeedTime=30 \
    ls-remote --exit-code -- origin </dev/null >"$probe_output" || probe_rc=$?
  if [ "$probe_rc" -ne 0 ] && [ "$probe_rc" -ne 2 ]; then
    rm -f -- "$probe_output"
    echo "ERROR: workflow_pr_head_sync: origin ref inspection failed (exit $probe_rc)" >&2
    exit "$probe_rc"
  fi
  inspection_rc=0
  # The checkpoint engine validates the saved advertisement (#1554, decision
  # 17.126): 0 present, 3 absent, 78 malformed. A missing engine stops here.
  command -v -- "${AMPLIHACK_BIN:-amplihack}" >/dev/null 2>&1 || {
    rm -f -- "$probe_output"
    echo "ERROR: workflow_pr_head_sync: workflow checkpoint engine not found (AMPLIHACK_BIN=${AMPLIHACK_BIN:-amplihack}); checkpoint stopped" >&2
    exit 78
  }
  "${AMPLIHACK_BIN:-amplihack}" internal workflow-checkpoint advertised-ref "$probe_output" "refs/heads/$branch" "$probe_rc" || inspection_rc=$?
  rm -f -- "$probe_output"
  if [ "$inspection_rc" -eq 3 ]; then
    echo "INFO: workflow_pr_head_sync: origin task ref is absent; skipping early sync before first publication" >&2
    exit 0
  fi
  if [ "$inspection_rc" -ne 0 ]; then
    echo "ERROR: workflow_pr_head_sync: malformed origin task-ref advertisement (transport exit $probe_rc; inspection exit $inspection_rc)" >&2
    exit "$inspection_rc"
  fi
fi

workflow_pr_head_sync_fetch() {
  GIT_TERMINAL_PROMPT=0 GIT_SSH_COMMAND="$ssh_cmd" $bound git \
    -c http.lowSpeedLimit=1000 -c http.lowSpeedTime=30 \
    fetch --quiet --no-tags --no-write-fetch-head \
    origin "+refs/heads/$branch:refs/remotes/origin/$branch" </dev/null
}
if [ "$strict_fetch" = true ]; then
  fetch_output=$(mktemp "${TMPDIR:?assigned TMPDIR required}/checkpoint-sync-fetch.XXXXXX") || exit $?
  fetch_rc=0
  workflow_pr_head_sync_fetch >"$fetch_output" 2>&1 || fetch_rc=$?
  # The strict caller captures and redacts this private output before emission.
  cat "$fetch_output" >&2
  rm -f -- "$fetch_output"
  if [ "$fetch_rc" -ne 0 ]; then
    echo "ERROR: workflow_pr_head_sync: fetch failed (exit $fetch_rc); checkpoint stopped" >&2
    exit "$fetch_rc"
  fi
elif ! workflow_pr_head_sync_fetch >/dev/null 2>&1; then
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
