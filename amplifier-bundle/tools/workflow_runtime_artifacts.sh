#!/usr/bin/env bash
# Shared lifecycle helper for workflow-generated runtime artifacts.
#
# Artifact Guard remains strict and read-only. This helper only removes the
# workflow-owned artifacts that recipes themselves may generate before guarded
# lifecycle phases run.

workflow_runtime_artifacts_error() {
  echo "ERROR: workflow runtime artifact cleanup: $*" >&2
}

workflow_runtime_artifacts_info() {
  echo "INFO: workflow runtime artifact cleanup: $*" >&2
}

workflow_runtime_artifacts_resolve_repo() {
  local candidate="${1:-}"
  local top

  if [ -z "$candidate" ]; then
    candidate="${WORKTREE_SETUP_WORKTREE_PATH:-}"
  fi
  if [ -z "$candidate" ]; then
    candidate="${RECIPE_VAR_worktree_setup__worktree_path:-}"
  fi
  if [ -z "$candidate" ]; then
    candidate="${REPO_PATH:-}"
  fi
  if [ -z "$candidate" ]; then
    candidate="."
  fi

  if [ "$candidate" = "/" ]; then
    workflow_runtime_artifacts_error "refusing to use '/' as the cleanup target"
    return 1
  fi
  if [ ! -d "$candidate" ]; then
    workflow_runtime_artifacts_error "target is not a directory: $candidate"
    return 1
  fi
  if ! top="$(git -C "$candidate" rev-parse --show-toplevel 2>/dev/null)"; then
    workflow_runtime_artifacts_error "target is not a git worktree: $candidate"
    return 1
  fi
  if [ -z "$top" ] || [ "$top" = "/" ]; then
    workflow_runtime_artifacts_error "resolved git top-level is unsafe: ${top:-<empty>}"
    return 1
  fi
  if ! top="$(cd "$top" && pwd -P)"; then
    workflow_runtime_artifacts_error "could not canonicalize git top-level for $candidate"
    return 1
  fi
  if [ "$top" = "/" ]; then
    workflow_runtime_artifacts_error "refusing to clean repository root '/'"
    return 1
  fi

  printf '%s\n' "$top"
}

workflow_runtime_artifacts_canonical_existing() {
  local path="$1"

  if [ ! -e "$path" ]; then
    workflow_runtime_artifacts_error "cannot canonicalize missing path: $path"
    return 1
  fi
  if [ -d "$path" ] && [ ! -L "$path" ]; then
    (cd "$path" && pwd -P)
  else
    local parent
    parent="$(dirname "$path")"
    printf '%s/%s\n' "$(cd "$parent" && pwd -P)" "$(basename "$path")"
  fi
}

workflow_runtime_artifacts_assert_inside_repo() {
  local repo="$1"
  local path="$2"
  local label="$3"
  local canonical

  canonical="$(workflow_runtime_artifacts_canonical_existing "$path")" || return 1
  case "$canonical" in
    "$repo"|"$repo"/*) return 0 ;;
    *)
      workflow_runtime_artifacts_error "$label escapes repository root: $canonical (repo: $repo)"
      return 1
      ;;
  esac
}

workflow_runtime_artifacts_tracked_under() {
  local repo="$1"
  local rel="$2"

  git -C "$repo" ls-files -- "$rel"
}

workflow_runtime_artifacts_cleanup_runtime_dir() {
  local repo="$1"
  local runtime="$repo/.claude/runtime"
  local tracked

  [ -e "$runtime" ] || return 0
  if [ -L "$runtime" ]; then
    workflow_runtime_artifacts_error ".claude/runtime is a symlink; refusing cleanup"
    return 1
  fi
  if [ ! -d "$runtime" ]; then
    workflow_runtime_artifacts_error ".claude/runtime exists but is not a directory"
    return 1
  fi
  workflow_runtime_artifacts_assert_inside_repo "$repo" "$runtime" ".claude/runtime" || return 1

  tracked="$(workflow_runtime_artifacts_tracked_under "$repo" ".claude/runtime")"
  if [ -n "$tracked" ]; then
    workflow_runtime_artifacts_error "tracked files exist under .claude/runtime; refusing to delete tracked content"
    printf '%s\n' "$tracked" | sed 's/^/  tracked: /' >&2
    return 1
  fi

  workflow_runtime_artifacts_info "removing generated .claude/runtime"
  rm -rf -- "$runtime"
}

workflow_runtime_artifacts_path_is_owned_or_ancestor() {
  local path="$1"
  shift
  local owner

  for owner in "$@"; do
    case "$path" in
      "$owner"|"$owner"/*) return 0 ;;
    esac
    if [ -d "$path" ]; then
      case "$owner" in
        "$path"/*) return 0 ;;
      esac
    fi
  done
  return 1
}

workflow_runtime_artifacts_cleanup_nested_worktrees() {
  local repo="$1"
  local worktrees="$repo/worktrees"
  local tracked
  local marker
  local owner
  local path
  local -a markers=()
  local -a owners=()

  [ -e "$worktrees" ] || return 0
  if [ -L "$worktrees" ]; then
    workflow_runtime_artifacts_error "worktrees is a symlink; refusing cleanup"
    return 1
  fi
  if [ ! -d "$worktrees" ]; then
    workflow_runtime_artifacts_error "worktrees exists but is not a directory"
    return 1
  fi
  workflow_runtime_artifacts_assert_inside_repo "$repo" "$worktrees" "worktrees" || return 1

  tracked="$(workflow_runtime_artifacts_tracked_under "$repo" "worktrees")"
  if [ -n "$tracked" ]; then
    workflow_runtime_artifacts_error "tracked files exist under worktrees; refusing to delete tracked content"
    printf '%s\n' "$tracked" | sed 's/^/  tracked: /' >&2
    return 1
  fi

  while IFS= read -r marker; do
    markers+=("$marker")
    owner="$(dirname "$marker")"
    if [ "$owner" = "$worktrees" ]; then
      workflow_runtime_artifacts_error "ownership marker at worktrees root is unsafe; marker must live inside a nested worktree"
      return 1
    fi
    if [ -L "$owner" ]; then
      workflow_runtime_artifacts_error "marked nested worktree is a symlink: ${owner#"$repo"/}"
      return 1
    fi
    workflow_runtime_artifacts_assert_inside_repo "$repo" "$owner" "marked nested worktree" || return 1
    owners+=("$owner")
  done < <(find "$worktrees" -name ".amplihack-workflow-worktree" -type f -print)

  while IFS= read -r path; do
    [ -n "$path" ] || continue
    if [ "${#owners[@]}" -gt 0 ] && workflow_runtime_artifacts_path_is_owned_or_ancestor "$path" "${owners[@]}"; then
      continue
    fi
    workflow_runtime_artifacts_error "unmarked nested worktree content remains under worktrees: ${path#"$repo"/}"
    return 1
  done < <(find "$worktrees" -mindepth 1 -print)

  for owner in "${owners[@]}"; do
    workflow_runtime_artifacts_info "removing workflow-owned nested worktree ${owner#"$repo"/}"
    rm -rf -- "$owner"
  done

  if [ -d "$worktrees" ]; then
    find "$worktrees" -depth -type d -empty -exec rmdir {} +
  fi
}

cleanup_known_workflow_runtime_artifacts() {
  local repo

  repo="$(workflow_runtime_artifacts_resolve_repo "${1:-}")" || return 1
  workflow_runtime_artifacts_cleanup_runtime_dir "$repo" || return 1
  workflow_runtime_artifacts_cleanup_nested_worktrees "$repo" || return 1
}

preflight_known_workflow_runtime_artifacts() {
  local repo

  repo="$(workflow_runtime_artifacts_resolve_repo "${1:-}")" || return 1
  cleanup_known_workflow_runtime_artifacts "$repo" || return 1

  if [ -e "$repo/.claude/runtime" ]; then
    workflow_runtime_artifacts_error ".claude/runtime remains after cleanup"
    return 1
  fi
  if [ -d "$repo/worktrees" ] && find "$repo/worktrees" -name ".amplihack-workflow-worktree" -type f -print -quit | grep -q .; then
    workflow_runtime_artifacts_error "marked workflow nested worktrees remain after cleanup"
    return 1
  fi
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  case "${1:-preflight}" in
    cleanup)
      shift
      cleanup_known_workflow_runtime_artifacts "${1:-}"
      ;;
    preflight)
      shift || true
      preflight_known_workflow_runtime_artifacts "${1:-}"
      ;;
    *)
      echo "Usage: $0 [cleanup|preflight] [repo]" >&2
      exit 2
      ;;
  esac
fi
