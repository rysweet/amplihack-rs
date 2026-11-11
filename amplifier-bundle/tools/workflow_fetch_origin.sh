#!/usr/bin/env bash
# workflow_fetch_origin.sh — fetch origin for a recipe's worktree setup step,
# retrying a transient failure.
#
# Moved out of consensus-issue-worktree.yaml step3-setup-worktree and
# workflow-worktree.yaml step-04-setup-worktree (issue #1808) so those recipes
# stay under the 400-line brick limit without being compressed; see
# scripts/check-brick-budget.sh.
#
# Usage (executed, never sourced, from inside the repository):
#   workflow_fetch_origin.sh
#
# Runs `git fetch origin --no-tags` up to three times, waiting 1 s and then
# 2 s between attempts. Git's output goes to stderr. Always exits 0: a fetch
# that keeps failing only means the caller continues with the remote refs it
# already has, and the caller's own base-ref resolution (#858) still fails
# closed when none of them resolves. A caller that cannot find this tool runs
# a single `git fetch origin --no-tags` itself.

set -u

max_attempts=3
delay=1
for attempt in 1 2 3; do
    if git fetch origin --no-tags >&2; then
        exit 0
    fi
    status=$?
    if [ "$attempt" -lt "$max_attempts" ]; then
        echo "WARNING: git fetch origin failed (exit ${status}); retrying (${attempt}/${max_attempts})." >&2
        sleep "$delay"
        delay=$((delay * 2))
        continue
    fi
    echo "WARNING: git fetch origin failed after ${max_attempts} attempts (exit ${status}); continuing with local refs." >&2
done
exit 0
