#!/usr/bin/env bash
# autodrive_run_pr.sh — find the pull request an agent step opened for the run.
#
# Issue #1676: a development run can open its PR from an agent step instead of
# the publish step, so phase 1 can finish with a PR that its own publish output
# does not name. autodrive-build step-03 used to look up the preflight's branch
# only, which is REPO_PATH's branch (main, detached, or unrelated), never the
# branch default-workflow created in its own worktree, and stopped the run
# with "phase 1 finished without a pull request".
#
# This applies the open-PR check workflow-finalize step-22b uses (#1596) to the
# run's own worktree:
#   - the worktree exists and is a git work tree;
#   - its current branch is the run's branch (worktree_setup.branch_name when
#     given), never main, master, a detached HEAD or an option-like name;
#   - the PR is open, from this repository (not a fork), on that branch;
#   - the PR's head commit is the worktree's HEAD, so a PR that does not hold
#     this run's work never counts.
# gh resolves the repository from the worktree's own remote.
#
# Fail closed: any guard, gh error or malformed answer prints nothing. Prints
# at most the PR number on stdout. Always returns 0 and never exits, so it is
# safe under `set -euo pipefail`.
#
# This file only DEFINES functions; sourcing it has no side effects.

# autodrive_open_pr_for_run_branch <run_dir> [<expected_branch>]
autodrive_open_pr_for_run_branch() {
  local dir="${1:-}" want="${2:-}" branch head json
  [ -n "$dir" ] && [ -d "$dir" ] || return 0
  command -v gh >/dev/null 2>&1 || return 0
  command -v jq >/dev/null 2>&1 || return 0
  git -C "$dir" rev-parse --is-inside-work-tree >/dev/null 2>&1 || return 0
  branch="$(git -C "$dir" branch --show-current 2>/dev/null || true)"
  [ -n "$branch" ] || return 0
  [ -z "$want" ] || [ "$branch" = "$want" ] || return 0
  case "$branch" in -*|main|master) return 0 ;; esac
  git check-ref-format --branch "$branch" >/dev/null 2>&1 || return 0
  head="$(git -C "$dir" rev-parse --verify HEAD 2>/dev/null || true)"
  [[ "$head" =~ ^[0-9a-f]{40}$ ]] || return 0
  # Raw gh stderr is dropped: it is untrusted and may carry PR content.
  json="$(cd "$dir" && gh pr list --head "$branch" --state open \
            --json number,headRefName,headRefOid,isCrossRepository 2>/dev/null)" || return 0
  printf '%s' "$json" | jq -r --arg b "$branch" --arg h "$head" '
    if type != "array" then empty else
      [ .[] | objects
        | select((.isCrossRepository // false) == false)
        | select(.headRefName == $b and .headRefOid == $h)
        | (.number | tostring)
        | select(test("^[1-9][0-9]*$")) ][0] // empty
    end' 2>/dev/null || true
  return 0
}
