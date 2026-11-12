#!/usr/bin/env bash
# workflow_pr_branch.sh — print the head branch of a pull request, for
# workflow-worktree.yaml step-04-setup-worktree's `pr_number` path (#342).
#
# Moved out of that recipe (issue #1808) so it stays under the 400-line brick
# limit without being compressed; see scripts/check-brick-budget.sh.
#
# Usage (executed, never sourced, from inside the repository):
#   workflow_pr_branch.sh <pr_number>
#
# stdout : the PR's head branch name, one line.
# exit 0 : resolved.
# exit 1 : <pr_number> is not a positive integer, `gh` is not on PATH, or gh
#          returned no branch name. The reason is on stderr.
#
# SECURITY: <pr_number> must be a positive integer before it reaches gh. This
# rejects arg-injection such as '342 --repo evil/x', '-1' or '$(rm)'. gh's own
# stderr is dropped so auth or token diagnostics never reach the run log; a
# plain error is printed instead. The caller still validates the returned
# name with `git check-ref-format --branch` before using it.

set -u

pr="${1:-}"
if [ "$#" -ne 1 ] || ! [[ "$pr" =~ ^[1-9][0-9]*$ ]]; then
    echo "ERROR: pr_number '$pr' is not a valid positive integer (arg-injection guard)." >&2
    exit 1
fi
if ! command -v gh >/dev/null 2>&1; then
    echo "ERROR: pr_number is set but the 'gh' CLI is not on PATH." >&2
    exit 1
fi
branch="$(gh pr view "$pr" --json headRefName -q .headRefName 2>/dev/null || true)"
if [ -z "$branch" ]; then
    echo "ERROR: 'gh pr view $pr' returned no branch name (PR not found, no auth, or gh failure)." >&2
    exit 1
fi
printf '%s\n' "$branch"
