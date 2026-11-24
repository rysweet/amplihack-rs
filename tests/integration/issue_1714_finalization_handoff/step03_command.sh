set -uo pipefail
T="${AMPLIHACK_HOME}/amplifier-bundle/tools"
[ -f "$T/workflow_finalization_handoff.sh" ] || { echo "ERROR: #1714: workflow_finalization_handoff.sh not found under AMPLIHACK_HOME" >&2; exit 1; }
. "$T/recipe_var.sh" || exit 1
. "$T/autodrive_state.sh" || exit 1
. "$T/workflow_finalization_handoff.sh" || exit 1
PR="$(printf '%s' "$RECIPE_VAR_build_preflight" | jq -r '.pr')" || exit 1
HANDOFF_TERMINAL_STATE=""; HANDOFF_HANDED_OFF=false
if [ -n "${RECIPE_VAR_workflow_result:-}" ]; then autodrive_build_handoff "$PR" || exit 1; fi
case "$PR" in ''|*[!0-9]*) echo "ERROR: #1714: phase 1 finished without a pull request" >&2; exit 1 ;; esac
URL="$(gh pr view "$PR" --json url --jq .url)" || exit 1
jq -n --arg pr "$PR" --arg url "$URL" --arg state "$HANDOFF_TERMINAL_STATE" --arg handoff "$HANDOFF_HANDED_OFF" '{pr:$pr,pr_url:$url,terminal_state:$state,handed_off:$handoff,merged_before_run:"false",state_dir:""}'
