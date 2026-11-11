#!/usr/bin/env bash
# Both Crusty entry points finalize through the same receipt gate. Dispatch
# completion ends iteration; only recorded follow-ups authorize the next phase.
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)" || exit 1
DIR="${1:?state dir}"; PR="${2:-}"; TASK_ID="${3:-}"; TASK_FILE="${4:-}"
. "$HERE/autodrive_state.sh" || exit 1
autodrive_clear_phase "$DIR" crusty-loop || exit 1
FU="$HERE/autodrive_followups.sh"
[ -f "$FU" ] || { echo "ERROR: followups-unfiled: required follow-up helper missing" >&2; exit 1; }
bash "$FU" collect --state-dir "$DIR" \
  || { echo "ERROR: followups-unfiled: collection failed; phase remains incomplete" >&2; exit 1; }
if [ -n "$PR" ]; then
  if [ "${AUTODRIVE_DRY_RUN:-false}" != true ]; then
    bash "$FU" file --state-dir "$DIR" --pr "$PR" --task-id "$TASK_ID" --task-text-file "$TASK_FILE"
    RC=$?
    [ "$RC" -ne 79 ] || exit 79
    [ "$RC" -eq 0 ] || { echo "ERROR: followups-unfiled: filing failed; phase remains incomplete" >&2; exit 1; }
  fi
  bash "$FU" check --state-dir "$DIR" --pr "$PR" \
    || { echo "ERROR: followups-unfiled: receipts invalid; phase remains incomplete" >&2; exit 1; }
elif [ -s "$DIR/followups.jsonl" ]; then
  echo "ERROR: followups-unfiled: PR identity required for mandatory follow-ups" >&2
  exit 1
fi
autodrive_record_crusty_loop_done "$DIR" || exit 1
echo "INFO: crusty loop recorded complete after follow-up verification." >&2
