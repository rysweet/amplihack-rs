#!/usr/bin/env bash
# Narrow cleanup for workflow-generated runtime artifacts.
#
# This helper deliberately removes only artifacts created by workflow execution
# inside a task worktree. It does not weaken Artifact Guard and does not remove
# unrelated user files.

set -euo pipefail

is_git_worktree() {
  local repo="$1"
  git -C "$repo" rev-parse --is-inside-work-tree >/dev/null 2>&1
}

path_is_tracked() {
  local repo="$1"
  local rel="$2"
  git -C "$repo" ls-files --error-unmatch -- "$rel" >/dev/null 2>&1
}

cleanup_known_workflow_runtime_artifacts() {
  local repo="${1:-.}"
  if ! is_git_worktree "$repo"; then
    echo "ERROR: runtime artifact cleanup requires a git worktree: $repo" >&2
    return 2
  fi

  local rel
  for rel in ".claude/runtime" "worktrees"; do
    if [ -e "$repo/$rel" ]; then
      if path_is_tracked "$repo" "$rel"; then
        echo "ERROR: refusing to remove tracked path during runtime cleanup: $rel" >&2
        return 1
      fi
      rm -rf -- "$repo/$rel"
    fi
  done
}

preflight_known_workflow_runtime_artifacts() {
  cleanup_known_workflow_runtime_artifacts "$@"
}
