#!/usr/bin/env bash
# workflow_review_mode_step.sh - the body of `review-mode` in
# amplifier-bundle/recipes/workflow-refactor-review.yaml (issue #1619).
#
# Prints `true` when the code, security and philosophy passes may run as one
# combined review, and `false` when they run separately. The step's `output:`
# captures stdout; the reason goes to stderr. Every failure answers `false`
# and exits 0: a review mode is a preference, never a reason to stop the phase.
#
# Combined review needs the user-only `review.combined_small_diff = true` and
# a diff of at most `review.combined_diff_threshold` lines (default 200)
# against the merge base with RECIPE_VAR_base_branch (default origin/main),
# counting committed and uncommitted lines.
#
# worktree_setup_path.sh is looked for under $AMPLIHACK_HOME, ~/.copilot and
# ~/.amplihack only. A copy in the repository under review, in the working
# directory or under REPO_PATH is that repository's own code and is never run.

set -euo pipefail
export GIT_PAGER=cat PAGER=cat LESS=FRX

decide() {
  printf '%s\n' "$1"
  [ -z "${2:-}" ] || echo "review mode: $2" >&2
  if [ "$1" = "true" ]; then
    echo "review mode: one combined pass" >&2
  else
    echo "review mode: separate code, security and philosophy passes" >&2
  fi
  exit 0
}

WT_TOOL=""
for c in "${AMPLIHACK_HOME:-}" "${HOME:-/root}/.copilot" "${HOME:-/root}/.amplihack"; do
  if [ -n "$c" ] && [ -f "$c/amplifier-bundle/tools/worktree_setup_path.sh" ]; then
    WT_TOOL="$c/amplifier-bundle/tools/worktree_setup_path.sh"
    break
  fi
done
WT=""
[ -n "$WT_TOOL" ] && WT="$(bash "$WT_TOOL" review-mode 2>/dev/null || true)"
[ -n "$WT" ] || decide false "no worktree path to measure a diff in"
cd -P "$WT" 2>/dev/null || decide false "the worktree path is not reachable"
git rev-parse --is-inside-work-tree >/dev/null 2>&1 || decide false "[skip] not a git repo; the mode is decided from a diff"
command -v amplihack >/dev/null 2>&1 || decide false "amplihack is not on PATH, so the setting cannot be read"

ON="$(amplihack config get review.combined_small_diff 2>/dev/null | tail -1 || true)"
[ "$ON" = "true" ] || decide false "review.combined_small_diff is not on"
LIMIT="$(amplihack config get review.combined_diff_threshold 2>/dev/null | tail -1 || true)"
case "$LIMIT" in '' | *[!0-9]*) LIMIT=200 ;; esac

BASE="${RECIPE_VAR_base_branch:-}"
[ -n "$BASE" ] || BASE="origin/main"
MB="$(git merge-base HEAD "$BASE" 2>/dev/null || true)"
[ -n "$MB" ] || decide false "no merge base against $BASE"

# Committed and uncommitted lines together: the passes review the tree, not
# the commits.
CHANGED="$( { git diff --numstat "$MB" HEAD; git diff --numstat HEAD; } 2>/dev/null | awk '{a=$1; d=$2; if (a=="-") a=0; if (d=="-") d=0; s+=a+d} END {print s+0}')" \
  || decide false "the diff could not be measured"
echo "review mode: $CHANGED changed lines, threshold $LIMIT" >&2
[ "$CHANGED" -le "$LIMIT" ] || decide false "the diff is above the threshold"
decide true
