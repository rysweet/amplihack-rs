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

# _amplihack_is_linked_worktree <repo>
# True when <repo> is a *linked* git worktree (its `.git` is a gitdir-pointer
# file), false for the main worktree (its `.git` is a directory). Only a linked
# worktree's `worktrees/` children are guaranteed to be leaked nested scratch
# worktrees rather than other concurrent runs' task worktrees, so destructive
# nested cleanup is gated on this signal.
_amplihack_is_linked_worktree() {
  local repo="${1:-.}"
  [ -f "$repo/.git" ]
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
      # On a *dedicated* (linked) task worktree, the `worktrees/` children are
      # leaked nested scratch worktrees created by this run. Record their
      # branches (so finalization deletes them) and deregister the worktrees
      # gracefully — a bare `rm -rf worktrees/` would leave dangling
      # registrations behind in the parent repo's $GIT_DIR/worktrees/ (issue
      # #808 — a regression of #780/#755). This graceful path is intentionally
      # skipped on the main checkout, where `worktrees/` children are other
      # runs' task worktrees: it must never deregister or record those. (The
      # pre-existing `rm -rf` below is left unchanged; it is only ever pointed
      # at a per-task worktree by the recipe steps that cd into the worktree.)
      if [ "$rel" = "worktrees" ] && _amplihack_is_linked_worktree "$repo"; then
        _amplihack_record_nested_worktree_branches "$repo"
        cleanup_nested_worktrees "$repo"
      fi
      rm -rf -- "$repo/$rel"
    fi
  done
}

preflight_known_workflow_runtime_artifacts() {
  cleanup_known_workflow_runtime_artifacts "$@"
}

# ---------------------------------------------------------------------------
# Deterministic finalization cleanup (issue #808)
#
# When a default-workflow run hits a denied force-push, its push-fallback path
# could spray throwaway branches to the shared remote and leave nested
# worktrees behind, with no finalization cleanup to remove them. The helpers
# below provide a deterministic, idempotent, fail-soft cleanup that runs in
# both success and failure/early-exit paths:
#
#   * record_run_created_branch        — track a fallback/intermediate branch
#   * cleanup_run_created_branches     — delete tracked branches (remote+local)
#   * cleanup_nested_worktrees         — remove nested worktrees + prune
#   * finalize_workflow_runtime_artifacts — orchestrate all of the above
#
# Every function is defensive: individual git failures never abort the caller,
# and the intended PR branch plus protected branches are never deleted.
# ---------------------------------------------------------------------------

# Path to the per-run manifest of run-created fallback branches that
# finalization must delete. Stored under the common git dir (shared by the main
# repo and all of its linked worktrees) and scoped by the recipe run id so that
# concurrent runs never consume each other's entries (each finalize reads and
# deletes only its own run's manifest).
_amplihack_run_branch_manifest() {
  local repo="${1:-.}"
  local common
  common="$(git -C "$repo" rev-parse --git-common-dir 2>/dev/null || true)"
  [ -n "$common" ] || return 1
  case "$common" in
    /*) : ;;
    *) common="$repo/$common" ;;
  esac
  local scope="${AMPLIHACK_RECIPE_RUN_ID:-default}"
  # Reduce the scope to filename-safe characters (no path traversal / odd chars).
  scope="$(printf '%s' "$scope" | tr -c 'A-Za-z0-9._-' '_')"
  [ -n "$scope" ] || scope="default"
  printf '%s/amplihack/run-created-branches-%s\n' "$common" "$scope"
}

# record_run_created_branch <repo> <branch>
# Track a fallback/intermediate branch so finalization can delete it later.
# Idempotent (deduplicated); never fails the caller.
record_run_created_branch() {
  local repo="${1:-.}"
  local branch="${2:-}"
  [ -n "$branch" ] || return 0
  case "$branch" in
    -*) return 0 ;;
  esac
  local manifest
  manifest="$(_amplihack_run_branch_manifest "$repo" 2>/dev/null || true)"
  [ -n "$manifest" ] || return 0
  mkdir -p "$(dirname "$manifest")" 2>/dev/null || return 0
  if [ -f "$manifest" ] && grep -qxF "$branch" "$manifest" 2>/dev/null; then
    return 0
  fi
  printf '%s\n' "$branch" >> "$manifest" 2>/dev/null || true
  return 0
}

# _amplihack_record_nested_worktree_branches <repo>
# Record the branch of every worktree nested under <repo>/worktrees/ into the
# run manifest, so finalization can delete those branches even if the nested
# worktree itself is removed (by preflight) before finalization runs.
_amplihack_record_nested_worktree_branches() {
  local repo="${1:-.}"
  local b
  while IFS= read -r b; do
    [ -n "$b" ] || continue
    record_run_created_branch "$repo" "$b"
  done <<RUNBRANCHES
$(_amplihack_list_nested_worktree_branches "$repo")
RUNBRANCHES
  return 0
}

# cleanup_nested_worktrees <repo>
# Remove every registered git worktree that lives under <repo>/worktrees/ and
# prune dangling administrative registrations. Idempotent; fail-soft.
cleanup_nested_worktrees() {
  local repo="${1:-.}"
  git -C "$repo" rev-parse --is-inside-work-tree >/dev/null 2>&1 || return 0
  local top
  top="$(git -C "$repo" rev-parse --show-toplevel 2>/dev/null || true)"
  [ -n "$top" ] || return 0
  local prefix="$top/worktrees/"
  local wt
  while IFS= read -r wt; do
    [ -n "$wt" ] || continue
    case "$wt" in
      "$prefix"*)
        git -C "$repo" worktree remove --force "$wt" >/dev/null 2>&1 \
          || rm -rf -- "$wt" 2>/dev/null || true
        ;;
    esac
  done < <(git -C "$repo" worktree list --porcelain 2>/dev/null | sed -n 's/^worktree //p')
  git -C "$repo" worktree prune >/dev/null 2>&1 || true
  return 0
}

# _amplihack_default_branch <repo>
# Best-effort name of the repository's default branch (e.g. resolved from
# origin/HEAD). Empty when it cannot be determined. Used to widen protection
# beyond the hard-coded base names so a configured default like `trunk` is never
# deleted even if it were mistakenly recorded.
_amplihack_default_branch() {
  local repo="${1:-.}"
  local ref
  ref="$(git -C "$repo" symbolic-ref -q --short refs/remotes/origin/HEAD 2>/dev/null || true)"
  printf '%s' "${ref#origin/}"
}

# _amplihack_delete_run_branch <repo> <branch> <intended>
# Delete a single run-created branch from the shared remote and locally, while
# never touching the intended PR branch, a protected base branch, the resolved
# default branch, or a branch still checked out in some worktree. Fail-soft.
_amplihack_delete_run_branch() {
  local repo="$1"
  local branch="$2"
  local intended="$3"
  [ -n "$branch" ] || return 0
  case "$branch" in
    -*|main|master|develop|trunk|HEAD) return 0 ;;
  esac
  [ "$branch" != "$intended" ] || return 0
  local default_branch
  default_branch="$(_amplihack_default_branch "$repo")"
  [ -z "$default_branch" ] || [ "$branch" != "$default_branch" ] || return 0
  if git -C "$repo" worktree list --porcelain 2>/dev/null | grep -qxF "branch refs/heads/$branch"; then
    return 0
  fi
  # Remote first (shared-remote clutter is the primary leak), then local.
  git -C "$repo" push origin --delete "$branch" >/dev/null 2>&1 || true
  git -C "$repo" branch -D "$branch" >/dev/null 2>&1 || true
  return 0
}

# _amplihack_list_nested_worktree_branches <repo>
# Print the branch checked out by each worktree that lives under
# <repo>/worktrees/ (one per line). Used to delete the run-created branch that
# a nested worktree leaves behind (issue #808: the nested worktree "and its
# branch" are both leaked).
_amplihack_list_nested_worktree_branches() {
  local repo="${1:-.}"
  local top
  top="$(git -C "$repo" rev-parse --show-toplevel 2>/dev/null || true)"
  [ -n "$top" ] || return 0
  local prefix="$top/worktrees/"
  local wt="" branch="" line
  while IFS= read -r line; do
    case "$line" in
      "worktree "*) wt="${line#worktree }" ;;
      "branch refs/heads/"*) branch="${line#branch refs/heads/}" ;;
      "")
        case "$wt" in
          "$prefix"*) [ -n "$branch" ] && printf '%s\n' "$branch" ;;
        esac
        wt=""; branch="" ;;
    esac
  done < <(git -C "$repo" worktree list --porcelain 2>/dev/null; printf '\n')
  return 0
}

# cleanup_run_created_branches <repo> [intended_branch]
# Delete every tracked run-created branch from the shared remote and locally,
# preserving the intended PR branch, any branch currently checked out, and the
# protected base branches. Idempotent; fail-soft.
cleanup_run_created_branches() {
  local repo="${1:-.}"
  local intended="${2:-}"
  git -C "$repo" rev-parse --is-inside-work-tree >/dev/null 2>&1 || return 0
  local manifest
  manifest="$(_amplihack_run_branch_manifest "$repo" 2>/dev/null || true)"
  [ -n "$manifest" ] || return 0
  [ -f "$manifest" ] || return 0
  [ -n "$intended" ] || intended="$(git -C "$repo" branch --show-current 2>/dev/null || true)"
  local branch
  while IFS= read -r branch; do
    _amplihack_delete_run_branch "$repo" "$branch" "$intended"
  done < "$manifest"
  # Idempotent: the manifest is consumed once finalization has processed it.
  rm -f -- "$manifest" 2>/dev/null || true
  return 0
}

# finalize_workflow_runtime_artifacts <repo> [intended_branch]
# Deterministic finalization cleanup entry point. Safe to call from success and
# failure/early-exit paths (e.g. an EXIT trap); never aborts the caller.
finalize_workflow_runtime_artifacts() {
  local repo="${1:-.}"
  local intended="${2:-}"
  git -C "$repo" rev-parse --is-inside-work-tree >/dev/null 2>&1 || return 0
  [ -n "$intended" ] || intended="$(git -C "$repo" branch --show-current 2>/dev/null || true)"
  # Capture nested-worktree branches before their worktrees are removed.
  local nested_branches
  nested_branches="$(_amplihack_list_nested_worktree_branches "$repo")"
  # 1. Remove nested worktrees + prune dangling registrations.
  cleanup_nested_worktrees "$repo" || true
  # 2. Delete the now-orphaned nested-worktree branches (remote + local).
  local b
  while IFS= read -r b; do
    [ -n "$b" ] || continue
    _amplihack_delete_run_branch "$repo" "$b" "$intended"
  done <<RUNBRANCHES
$nested_branches
RUNBRANCHES
  # 3. Delete explicitly tracked fallback branches (remote + local).
  cleanup_run_created_branches "$repo" "$intended" || true
  # 4. Sweep remaining runtime artifacts, but never let its fail-closed
  #    tracked-path guard abort finalization.
  cleanup_known_workflow_runtime_artifacts "$repo" >/dev/null 2>&1 || true
  return 0
}
