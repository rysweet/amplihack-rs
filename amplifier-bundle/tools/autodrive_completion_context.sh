# shellcheck shell=bash
# Driver-only, file-backed evaluator data. No action selection lives here.
CTX_DIR="$(mktemp -d "$STATE_DIR/.completion-context.XXXXXX")" || return 2
CTX_FILES=()
completion_context() {
  ( umask 077 && set -C && printf '%s' "$2" > "$CTX_DIR/$1" ) || return 2
  CTX_FILES+=(--context-from-file "$1=$CTX_DIR/$1")
}
completion_context loop_history "$HISTORY" &&
completion_context loop_baseline_ref "$BASELINE" &&
completion_context loop_findings_previous "$PREV_FINDINGS" &&
completion_context loop_test_signal_previous "$PREV_TEST" &&
completion_context loop_ci_signal_previous "$PREV_CI" &&
completion_context loop_clean_token "$TOKEN_SAFE" || { rm -rf -- "$CTX_DIR"; return 2; }
PREPARED="$("$AMPLIHACK_BIN" orch helper autodrive-completion-prepare \
  --state-dir "$STATE_DIR" --round-label "$ROUND_LABEL" --record "$RECORD" \
  --repo "$REPO" --review-dir "$REVIEW_REAL" --launch-tools "$LOOP_HOME" "${CTX_FILES[@]}")"
CTX_RC=$?
rm -rf -- "$CTX_DIR"
[ "$CTX_RC" -eq 0 ] || return 2
AUTODRIVE_COMPLETION_STATE="$(field "$PREPARED" private_state '')"
[ -n "$AUTODRIVE_COMPLETION_STATE" ] || { echo "ERROR: completion helper returned no private state" >&2; return 2; }
export AUTODRIVE_COMPLETION_STATE
COMPLETION_CTX=(-c "autodrive_completion_state=$AUTODRIVE_COMPLETION_STATE")
