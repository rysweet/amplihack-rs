#!/usr/bin/env bash
# autodrive_merge_loop_run.sh: a one-release alias (#3049, #3129).
#
# It ran the old merge-ready loop for autodrive-merge-loop.yaml, with a round
# budget and a base sync. The shared goal loop replaced it: this alias runs
# autodrive_goal_loop.sh with the pull request goal, taking the run's values
# from RECIPE_VAR_* as before (autodrive_state_dir, repo_path, pr_number,
# task_description) and merge authority from the invocation record beside
# this launch bundle. An optional <rounds> argument
# is accepted and ignored; the loop has no round budget. Nothing here brings
# back a stop or an automatic base merge.
# docs/reference/autodrive-goal-loop.md#retired-keys-and-aliases.
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" 2>/dev/null && pwd -P)" || exit 2
echo "autodrive_merge_loop_run.sh is retired (#3129); running autodrive_goal_loop.sh with the pull request goal." >&2
INV="${HERE%/amplifier-bundle/tools}/invocation.json"; [ -f "$INV" ] || INV=""
W="$(mktemp -d)" || exit 1
trap 'rm -rf -- "$W"' EXIT
# shellcheck source=recipe_context_value.sh
. "$HERE/recipe_context_value.sh"
write_context_value task_description "$W/task" 2>/dev/null || : > "$W/task"
bash "$HERE/autodrive_goal_loop.sh" --goal-tool autodrive_pr_goal.sh \
  --goal-file skills/auto-drive-to-merge/goal-merge-pr.md --state-dir "${RECIPE_VAR_autodrive_state_dir:-}" \
  --repo "${RECIPE_VAR_repo_path:-.}" --task-file "$W/task" --invocation "$INV"
