#!/usr/bin/env bash
# Trusted recipe selection remains independent of the PR worktree's cwd.
# Reviewers use that worktree directly; subsequent agents assess any edits.
set -uo pipefail
export GIT_PAGER=cat GH_PAGER=cat PAGER=cat LESS=FRX
. "$(dirname -- "${BASH_SOURCE[0]}")/git-repo-env.sh"
amplihack_clear_git_repo_env
unset CDPATH
stop() { echo "STOP: $* (#1528)" >&2; exit 1; }
real() { [ -n "${1:-}" ] && (cd -- "$1" 2>/dev/null && pwd -P); }
# inside <path> <tree>: <path> is <tree> or lies under it (both real paths).
inside() { [ -n "$2" ] && case "$1/" in "$2"/*) return 0 ;; esac; return 1; }

# launch_recipe <name> <repo-real>: stdout the launch-side path, or fail.
launch_recipe() {
  local name="$1" bundle path
  case "$name" in
    ''|*/*|*.yaml|*.yml)
      echo "ERROR: recipe '${name}' must be a bare name; it is resolved in the launch-side bundle, never by path (#1528)." >&2
      return 1 ;;
  esac
  bundle="$(real "$(dirname -- "${BASH_SOURCE[0]}")/../recipes")"
  [ -n "$bundle" ] || { echo "ERROR: no amplifier-bundle/recipes next to $(dirname -- "${BASH_SOURCE[0]}") (#1528)." >&2; return 1; }
  path="${bundle}/${name}.yaml"
  [ -f "$path" ] || { echo "ERROR: required recipe '${name}' is not in the launch-side bundle (${bundle}) (#1528)." >&2; return 1; }
  if inside "$path" "$2"; then
    echo "ERROR: recipe '${name}' resolved to ${path}, inside the tree under review ($2). The reviewed tree never supplies its own reviewer or stop rule (#1528)." >&2
    return 1
  fi
  printf '%s\n' "$path"
}

cmd_loop_preflight() {
  local loop="${1:-}" repo="${2:-.}" round="${3:-}" key="${4:-}" repo_real round_path
  # Preserve the launch-side recipe validation; cwd is always the named PR worktree.
  case "$key" in
    '') ;;
    *) printf '{"loop":"%s","loop_result":"UNTRUSTED_RECIPE","reason":"reserved loop context"}\n' "$loop"; return 1 ;;
  esac
  repo_real="$(real "$repo")" || return 1
  round_path="$(launch_recipe "$round" "$repo_real")" || {
    printf '{"loop":"%s","loop_result":"UNTRUSTED_RECIPE","round_label":"preflight","reason":"round recipe does not resolve in the launch-side bundle"}\n' "$loop"
    return 1
  }
  printf '%s\n%s\n' "$round_path" "$(launch_recipe loop-health-evaluator "$repo_real" 2>/dev/null)"
}

cmd_commit_dir() {
  local pr="${1:-}" gd cd pr_head
  case "$pr" in ''|*[!0-9]*) stop "commit-dir needs the PR number" ;; esac
  # In the main checkout the git dir and the common dir are the same directory.
  gd="$(git rev-parse --absolute-git-dir)" && cd="$(git rev-parse --path-format=absolute --git-common-dir)" \
    && [ "$(git rev-parse --show-toplevel)" = "$(pwd -P)" ] \
    && case "$cd" in /*) [ "$gd" != "$cd" ] ;; *) false ;; esac \
    || stop "not a linked worktree; never stage or commit in the main checkout or another checkout"
  # An empty answer never matches.
  pr_head="$(gh pr view "$pr" --json headRefName --jq .headRefName)" && [ -n "$pr_head" ] \
    && [ "$(git branch --show-current)" = "$pr_head" ] \
    || stop "this worktree is not on the PR head branch"
}

sub="${1:-}"; [ $# -gt 0 ] && shift
case "$sub" in
  loop-preflight) cmd_loop_preflight "$@" ;;
  commit-dir) cmd_commit_dir "$@" ;;
  *) echo "usage: autodrive_worktree_guard.sh loop-preflight <loop> <repo> <round-recipe> [<context-key>] | commit-dir <pr>" >&2; exit 2 ;;
esac
