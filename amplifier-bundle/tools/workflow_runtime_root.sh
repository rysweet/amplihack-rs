#!/usr/bin/env bash
# workflow_runtime_root.sh — print and create the per-branch runtime root for
# workflow-worktree.yaml step-04-setup-worktree.
#
# Moved out of that recipe (issue #1808) so it stays under the 400-line brick
# limit without being compressed; see scripts/check-brick-budget.sh.
#
# Usage (executed, never sourced):
#   workflow_runtime_root.sh <branch_name>
#
# The root is $AMPLIHACK_RUNTIME_ROOT when that is set, otherwise
# $XDG_RUNTIME_DIR/amplihack-runtime/<branch_name>, otherwise
# /tmp/amplihack-runtime/<branch_name>. The locks, reflection, logs, metrics
# and provenance folders are created under it.
#
# stdout : the runtime root, one line.
# exit 0 : the folders exist.
# exit 1 : no branch name was given, or the folders could not be created.

set -u

branch="${1:-}"
if [ "$#" -ne 1 ] || [ -z "$branch" ]; then
    echo "ERROR: usage: workflow_runtime_root.sh <branch_name>" >&2
    exit 1
fi

root="${AMPLIHACK_RUNTIME_ROOT:-}"
if [ -z "$root" ]; then
    if [ -n "${XDG_RUNTIME_DIR:-}" ]; then
        root="${XDG_RUNTIME_DIR}/amplihack-runtime/${branch}"
    else
        root="/tmp/amplihack-runtime/${branch}"
    fi
fi

mkdir -p \
    "${root}/locks" \
    "${root}/reflection" \
    "${root}/logs" \
    "${root}/metrics" \
    "${root}/provenance" || exit 1

printf '%s\n' "$root"
