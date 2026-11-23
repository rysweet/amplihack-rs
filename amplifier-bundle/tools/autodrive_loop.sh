#!/usr/bin/env bash
# autodrive_loop.sh: a one-release alias (#3049, #3129).
#
# It drove the old Crusty and merge-ready loops, with the loop-health STUCK
# stop, the no-progress stop and the round budget. The shared goal loop
# replaced it. The old call maps on as follows:
#   --loop-name crusty        one review action (autodrive_pr_goal.sh review)
#   --loop-name merge-ready   the goal loop (autodrive_goal_loop.sh) with the
#                             pull request goal
# --repo, --state-dir, --task-file and `--context pr_number=N` carry over
# (the number reaches the goal tool as AUTODRIVE_PR); every other --context
# key, the budget keys among them, is accepted and ignored. Merge authority
# comes only from the invocation record beside this launch bundle. Another loop name prints one line naming the replacement and exits
# 0. Nothing here brings back a stop.
# docs/reference/autodrive-goal-loop.md#retired-keys-and-aliases.
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" 2>/dev/null && pwd -P)" || exit 2
NAME=""; REPO="."; STATE=""; TASK=""; PR=""
while [ $# -gt 0 ]; do
  case "$1" in
    --loop-name) NAME="${2:-}"; shift 2 ;;
    --repo) REPO="${2:-}"; shift 2 ;;
    --state-dir) STATE="${2:-}"; shift 2 ;;
    --task-file) TASK="${2:-}"; shift 2 ;;
    --context) case "${2:-}" in pr_number=*) PR="${2#pr_number=}" ;; esac; shift 2 ;;
    --round-recipe|--clean-token|--also-clean-token|--verdict-field|--before-round) shift 2 ;;
    *) shift ;;
  esac
done
[[ "$PR" =~ ^[1-9][0-9]{0,9}$ ]] || PR=""
INV="${HERE%/amplifier-bundle/tools}/invocation.json"; [ -f "$INV" ] || INV=""
echo "autodrive_loop.sh is retired (#3129); running its replacement for the '${NAME}' loop." >&2
case "$NAME" in
  crusty)
    AUTODRIVE_GOAL_CONTRACT=1 AUTODRIVE_STATE_DIR="$STATE" AUTODRIVE_REPO="$REPO" AUTODRIVE_LOOP_HOME="$HERE" \
      AUTODRIVE_TASK_FILE="${TASK:-/dev/null}" AUTODRIVE_PR="$PR" exec bash "$HERE/autodrive_pr_goal.sh" review ;;
  merge-ready)
    AUTODRIVE_PR="$PR" exec bash "$HERE/autodrive_goal_loop.sh" --goal-tool autodrive_pr_goal.sh \
      --goal-file skills/auto-drive-to-merge/goal-merge-pr.md --state-dir "$STATE" --repo "$REPO" \
      ${TASK:+--task-file "$TASK"} --invocation "$INV" ;;
  *) echo "autodrive_loop.sh: no replacement for a '${NAME}' loop; use autodrive_goal_loop.sh." >&2; exit 0 ;;
esac
