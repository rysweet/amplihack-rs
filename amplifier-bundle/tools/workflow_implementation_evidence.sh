#!/usr/bin/env bash
set -euo pipefail

normalize_bool() {
  case "$(printf '%s' "$1" | tr '[:upper:]' '[:lower:]')" in
    "true"|"1"|"yes"|"y") printf 'true\n' ;;
    *) printf 'false\n' ;;
  esac
}

emit_evidence() {
  jq -nc \
    --arg implementation_completed "$1" \
    --arg terminal_no_op "$2" \
    --arg terminal_state "$3" \
    --arg terminal_reason "$4" \
    '{implementation_completed:$implementation_completed,terminal_no_op:$terminal_no_op,terminal_state:$terminal_state,terminal_reason:$terminal_reason}'
}

allow_no_op="$(normalize_bool "${ALLOW_NO_OP:-${RECIPE_VAR_allow_no_op:-false}}")"
impl="${IMPLEMENTATION:-}"
verdict_raw="${VERDICT_JSON:-}"
orchestration_sentinel='No files modified — orchestration task'

if [ "$allow_no_op" = "true" ]; then
  emit_evidence "false" "true" "ALLOW_NO_OP" "allow_no_op was explicitly selected for a non-code-change path"
  exit 0
fi
if [ -n "$impl" ] && printf '%s' "$impl" | grep -qF -- "$orchestration_sentinel"; then
  emit_evidence "false" "true" "ALLOW_NO_OP" "implementation output contained the explicit orchestration no-op sentinel"
  exit 0
fi

verdict_line="$(printf '%s\n' "$verdict_raw" | grep -E '^[[:space:]]*\{.*"verdict"' | tail -1 || true)"
if [ -n "$verdict_line" ]; then
  verdict="$(printf '%s' "$verdict_line" | jq -r '.verdict // ""' 2>/dev/null || true)"
else
  verdict=""
fi

case "$verdict" in
  WORK_VERIFIED|VERIFIED|SUCCESS|APPROVED|PASS|PASSED)
    emit_evidence "true" "false" "IMPLEMENTATION_COMPLETED" "step-08c work-verifier approved concrete implementation artifacts"
    ;;
  *)
    emit_evidence "false" "false" "IMPLEMENTATION_UNPROVEN" "step-08c did not produce WORK_VERIFIED implementation evidence"
    ;;
esac
