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
# ${TMPDIR:-/tmp}/amplihack-runtime/<branch_name> (issue #1978: the run's own
# TMPDIR before the shared /tmp). In that last case the amplihack-runtime
# parent is created mode 0700, and a parent or root that is a symlink or is
# not owned by the caller is refused. The locks, reflection, logs, metrics
# and provenance folders are created under the root.
#
# stdout : the runtime root, one line.
# exit 0 : the folders exist.
# exit 1 : no branch name was given, the fallback root is unsafe, or the
#          folders could not be created.

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
        parent="${TMPDIR:-/tmp}/amplihack-runtime"
        if [ -L "$parent" ]; then
            echo "ERROR: runtime parent ${parent} is a symlink; refusing it" >&2
            exit 1
        fi
        # -m applies to the last component only, which is the one this creates.
        # shellcheck disable=SC2174
        mkdir -p -m 700 "$parent" || exit 1
        if [ ! -O "$parent" ]; then
            echo "ERROR: runtime parent ${parent} is not owned by $(id -un); refusing it" >&2
            exit 1
        fi
        root="${parent}/${branch}"
        if [ -L "$root" ]; then
            echo "ERROR: runtime root ${root} is a symlink; refusing it" >&2
            exit 1
        fi
        if [ -e "$root" ] && [ ! -O "$root" ]; then
            echo "ERROR: runtime root ${root} is not owned by $(id -un); refusing it" >&2
            exit 1
        fi
    fi
fi

mkdir -p \
    "${root}/locks" \
    "${root}/reflection" \
    "${root}/logs" \
    "${root}/metrics" \
    "${root}/provenance" || exit 1

printf '%s\n' "$root"
