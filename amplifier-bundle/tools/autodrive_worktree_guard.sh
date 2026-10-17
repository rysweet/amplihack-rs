#!/usr/bin/env bash
# autodrive_worktree_guard.sh — the checks that keep the pull request under
# review from supplying its own reviewer, and keep rounds committing only in
# that pull request's linked worktree (issue #1528).
#
# Phases 2 and 3 work in the PR's linked worktree (`--repo` of
# autodrive_loop.sh, `repo_path` of a round). Three things must still come from
# OUTSIDE that tree, because the branch under review would otherwise judge
# itself, and a round's fix-up commit could rewrite the next round's reviewer:
#
#   - the round and evaluator recipes. `recipe run <name> --working-dir DIR`
#     looks the name up in DIR's own amplifier-bundle/recipes first
#     (recipe_search_dirs, resolve.rs), so the loop runs them by absolute path
#     in the bundle that holds this script. A recipe run by path searches its
#     own directory first for sub-recipes (build_search_dirs, run/mod.rs), so
#     loop-evidence-collector and autodrive-merge-evidence come from there too;
#   - the judging agents' working directory. The agent CLI loads AGENTS.md,
#     CLAUDE.md and .claude hooks from it, so the crusty review, the
#     merge-ready assessment and the loop-health analyzer start in the review
#     dir (the launch checkout), never in or under the worktree;
#   - these tools. autodrive_loop.sh hands its own directory to each round as
#     `autodrive_launch_tools`; a round never looks for this file in the tree
#     it judges.
#
# Usage (executed, never sourced):
#   autodrive_worktree_guard.sh loop-preflight <loop> <repo> <review-dir> <round-recipe> [<context-key>]
#       autodrive_loop.sh's check, before round 1. On success prints three
#       lines: the review dir's real path, the round recipe's absolute path in
#       the launch-side bundle, and loop-health-evaluator's (an empty third
#       line when it does not resolve; the loop reports that as
#       MISSING_DEPENDENCY). Otherwise exit 1 with one JSON line on stdout,
#       loop_result UNTRUSTED_REVIEW_DIR or UNTRUSTED_RECIPE, when:
#         - <context-key> is given: review_dir or autodrive_launch_tools came
#           in as a round --context, and only the loop sets those;
#         - <review-dir> (default: the current directory) is not a directory,
#           or is <repo> or inside it;
#         - <round-recipe> is a path, is not in the bundle beside this script,
#           or that bundle lies inside <repo> (this script was itself picked
#           up from the tree under review).
#   autodrive_worktree_guard.sh round-guard
#       A round's step-00: the review-dir check on $REVIEW_DIR and $REPO_PATH,
#       with an empty $REVIEW_DIR refused (there is no default; "." would be
#       the worktree). Exit 1 with a STOP line.
#   autodrive_worktree_guard.sh commit-dir <pr>
#       The round commit block's check, run in the block's shell after it
#       enters repo_path: exit 1 with a STOP line unless the current directory
#       is the top of a LINKED worktree (its git dir differs from the absolute
#       common dir; never the main checkout) on PR <pr>'s head branch.
# Every refusal names #1528. Exit 2 is a usage error. Nothing here writes.

set -uo pipefail
export GIT_PAGER=cat GH_PAGER=cat PAGER=cat LESS=FRX

stop() { echo "STOP: $* (#1528)" >&2; exit 1; }
real() { (cd -- "$1" 2>/dev/null && pwd -P); }

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
  local loop="${1:-}" repo="${2:-.}" dir="${3:-}" round="${4:-}" key="${5:-}" repo_real rd round_path
  refuse() { # refuse <loop_result> <reason>
    echo "ERROR: loop '${loop}': $2. The reviewed tree never supplies its reviewer's instructions (#1528)." >&2
    printf '{"loop":"%s","loop_result":"%s","round_label":"preflight","reason":"%s"}\n' "$loop" "$1" "$(printf '%s' "$2" | tr -d '"\\')"
    exit 1
  }
  case "$key" in
    '') ;;
    review_dir) refuse UNTRUSTED_REVIEW_DIR "review_dir is set only by --review-dir, never by --context" ;;
    *) refuse UNTRUSTED_RECIPE "${key} is set only by the loop, never by --context" ;;
  esac
  repo_real="$(real "$repo")"
  [ -n "$dir" ] || dir="$(pwd -P)"
  rd="$(real "$dir")"
  [ -n "$rd" ] || refuse UNTRUSTED_REVIEW_DIR "review dir '${dir}' is not a directory"
  inside "$rd" "$repo_real" \
    && refuse UNTRUSTED_REVIEW_DIR "review dir ${rd} is the tree under review (${repo_real}) or inside it; launch auto-drive from a checkout other than the PR worktree"
  round_path="$(launch_recipe "$round" "$repo_real")" \
    || refuse UNTRUSTED_RECIPE "round recipe ${round} does not resolve in the launch-side bundle"
  printf '%s\n%s\n%s\n' "$rd" "$round_path" "$(launch_recipe loop-health-evaluator "$repo_real" 2>/dev/null)"
}

cmd_round_guard() {
  local rd repo_real
  [ -n "${REVIEW_DIR:-}" ] || stop "review_dir is required; the judging agent never starts in the tree under review"
  rd="$(real "$REVIEW_DIR")" || rd=""
  [ -n "$rd" ] || stop "review_dir '${REVIEW_DIR}' is not a directory"
  repo_real="$(real "${REPO_PATH:-.}")" || repo_real=""
  [ -n "$repo_real" ] || stop "cannot enter repo_path '${REPO_PATH:-.}'"
  inside "$rd" "$repo_real" \
    && stop "review_dir ${rd} is the tree under review (${repo_real}) or inside it; the reviewed tree never instructs its reviewer"
  echo "INFO: judging agent starts in ${rd}; commits happen only in ${repo_real}" >&2
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
  round-guard) cmd_round_guard ;;
  commit-dir) cmd_commit_dir "$@" ;;
  *) echo "usage: autodrive_worktree_guard.sh loop-preflight <loop> <repo> <review-dir> <round-recipe> [<context-key>] | round-guard | commit-dir <pr>" >&2; exit 2 ;;
esac
