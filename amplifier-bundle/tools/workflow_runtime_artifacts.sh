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

# _amplihack_exclude_worktrees_dir <repo>
# Add `/worktrees/` to the repository's info/exclude (shared by all its
# worktrees; no tracked file changes). On the main checkout `worktrees/` holds
# task worktrees that cleanup now keeps (#1494); untracked, they would fail every
# clean-status gate there (e.g. the no-diff check in workflow_final_status.sh)
# and invite `git add -A` to stage them. Idempotent; best-effort.
_amplihack_exclude_worktrees_dir() {
  local repo="${1:-.}" common exclude
  common="$(git -C "$repo" rev-parse --git-common-dir 2>/dev/null)" || return 0
  case "$common" in /*) ;; *) common="$repo/$common" ;; esac
  exclude="$common/info/exclude"
  if [ -f "$exclude" ] && grep -qxF '/worktrees/' "$exclude" 2>/dev/null; then
    return 0
  fi
  mkdir -p "$common/info" 2>/dev/null || return 0
  # End an unterminated last line first, or the user's last rule and ours merge.
  if [ -s "$exclude" ] && [ -n "$(tail -c1 "$exclude" 2>/dev/null)" ]; then
    printf '\n' >> "$exclude" 2>/dev/null || true
  fi
  printf '/worktrees/\n' >> "$exclude" 2>/dev/null || true
  return 0
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
      # On the main checkout, `worktrees/` holds other runs' task worktrees and
      # their uncommitted work. Recipe steps do call this from the main checkout
      # (finalize preflight when a run starts there), and a bare `rm -rf` then
      # destroyed every concurrent run's worktree (issue #1494). Leave it alone.
      #
      # Issue #1528: read the worktree list fail-loud first. If it cannot be
      # read, nothing proves the directory is safe to delete, so keep it.
      if [ "$rel" = "worktrees" ]; then
        # Git's own stderr is kept: it says why the list failed.
        if ! git -C "$repo" worktree list --porcelain >/dev/null; then
          echo "WARNING: runtime cleanup kept $repo/worktrees: git worktree list failed (#1528)" >&2
          _amplihack_is_linked_worktree "$repo" || _amplihack_exclude_worktrees_dir "$repo"
          continue
        fi
        if ! _amplihack_is_linked_worktree "$repo"; then
          echo "INFO: leaving $repo/worktrees in place: on the main checkout it holds other runs' worktrees (#1494)" >&2
          _amplihack_exclude_worktrees_dir "$repo"
          continue
        fi
      fi
      # On a *dedicated* (linked) task worktree, the `worktrees/` children are
      # leaked nested scratch worktrees created by this run. Record their
      # branches (so finalization deletes them) and deregister the worktrees
      # gracefully — a bare `rm -rf worktrees/` would leave dangling
      # registrations behind in the parent repo's $GIT_DIR/worktrees/ (issue
      # #808 — a regression of #780/#755).
      if [ "$rel" = "worktrees" ]; then
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
  # On the main checkout, worktrees/ holds other runs' task worktrees (#1494).
  _amplihack_is_linked_worktree "$repo" || return 0
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

# finalize_workflow_cleanup_entry <repo_root> <worktree_path> <intended_branch>
# Recipe-facing entry point. Destructive nested-worktree + branch cleanup runs
# only against a *dedicated* (linked) per-task worktree whose canonical toplevel
# differs from the repo root; otherwise it restricts to the manifest-keyed
# branch cleanup, which never removes a worktree. Fail-soft and idempotent —
# safe to call from success and failure/early-exit paths.
finalize_workflow_cleanup_entry() {
  local repo_root="${1:-}"
  local worktree_path="${2:-}"
  local intended="${3:-}"
  [ -n "$repo_root" ] || repo_root="$(pwd)"
  if [ -n "$worktree_path" ] && [ -d "$worktree_path" ] && _amplihack_is_linked_worktree "$worktree_path"; then
    local wt_top repo_top
    wt_top="$(git -C "$worktree_path" rev-parse --show-toplevel 2>/dev/null || true)"
    repo_top="$(git -C "$repo_root" rev-parse --show-toplevel 2>/dev/null || true)"
    if [ -n "$wt_top" ] && [ "$wt_top" != "$repo_top" ]; then
      finalize_workflow_runtime_artifacts "$worktree_path" "$intended" || true
      return 0
    fi
  fi
  cleanup_run_created_branches "$repo_root" "$intended" || true
  return 0
}
# finalize_workflow_runtime_artifacts <task_worktree> [intended_branch]
# Full cleanup against a dedicated per-task worktree: removes nested worktrees,
# deletes their orphaned branches plus tracked fallback branches (remote+local),
# and sweeps runtime artifacts. Never aborts the caller.
finalize_workflow_runtime_artifacts() {
  local repo="${1:-.}"
  local intended="${2:-}"
  git -C "$repo" rev-parse --is-inside-work-tree >/dev/null 2>&1 || return 0
  [ -n "$intended" ] || intended="$(git -C "$repo" branch --show-current 2>/dev/null || true)"
  # Capture nested-worktree branches before their worktrees are removed. Only a
  # linked task worktree has nested worktrees of its own: on the main checkout
  # these would be every other run's task worktree and PR branch (#1494).
  local nested_branches=""
  if _amplihack_is_linked_worktree "$repo"; then
    nested_branches="$(_amplihack_list_nested_worktree_branches "$repo")"
  fi
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
