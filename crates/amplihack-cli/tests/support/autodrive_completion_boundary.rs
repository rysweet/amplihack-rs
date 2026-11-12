//! Recipe/agent boundary doubles for issue #2336. Production helpers stay real.

pub const SHIM: &str = r#"#!/usr/bin/env bash
set -uo pipefail
if [ "${1:-} ${2:-}" = 'orch helper' ]; then
  if [[ "${3:-}" == autodrive-completion-* ]] && [ -f "$FIXTURE/no-completion-helper" ]; then
    echo 'error: unsupported completion helper; update amplihack' >&2; exit 2
  fi
  exec "$REAL_AMPLIHACK" "$@"
fi
if [ "${1:-} ${2:-}" = 'recipe show' ]; then exit 0; fi
[ "${1:-} ${2:-}" = 'recipe run' ] || exit 2
name="$(basename "$3" .yaml)"; shift 3
if [ "$name" = loop-health-evaluator ]; then
  jq -cn --arg depth "$AMPLIHACK_SESSION_DEPTH" --arg ceiling "$AMPLIHACK_MAX_DEPTH" \
    --arg cwd "$PWD" --args '$ARGS.positional as $argv | {depth:$depth,ceiling:$ceiling,cwd:$cwd,argv:$argv}' -- "$@" >> "$FAKE_LOG/evaluator.jsonl"
  if [ -f "$FIXTURE/eval-rc" ]; then
    [ ! -f "$FIXTURE/eval-message" ] || cat "$FIXTURE/eval-message"
    exit "$(cat "$FIXTURE/eval-rc")"
  fi
  verdict="$(cat "$FIXTURE/eval-verdict")"
  printf '  ✓ step-04-enforce-loop-verdict: completed\n    Output: LOOP_HEALTH: %s\n' "$verdict"
  [ "$verdict" != STUCK ]; exit $?
fi
[ "$name" = autodrive-crusty-round ] || exit 2
while [ $# -gt 0 ]; do
  case "$1" in
    -c) key="$(printf '%s' "${2%%=*}" | tr '[:lower:]' '[:upper:]')"; val="${2#*=}"; export "$key=$val"; shift 2 ;;
    --working-dir) cd "$2" || exit 1; shift 2 ;;
    *) shift ;;
  esac
done
printf '%s\n' "$AUTODRIVE_ROUND_LABEL" >> "$FAKE_LOG/rounds"
bash "$FIXTURE/round.sh"
"#;

pub const ROUND_SCRIPT: &str = r#"set -euo pipefail
export RECIPE_VAR_review_index="$(bash "$FIXTURE/guard.sh")"
export RECIPE_VAR_crusty_round_context="$(bash "$FIXTURE/context.sh")"
export RECIPE_VAR_crusty_review="$(cat "$FIXTURE/review.json")"
bash "$FIXTURE/guard-check.sh"
export RECIPE_VAR_crusty_verdict="$(bash "$FIXTURE/classify.sh")"
bash "$FIXTURE/record.sh"
if [ -n "${AUTODRIVE_COMPLETION_STATE:-}" ]; then
  amplihack orch helper autodrive-completion-write --private-state "$AUTODRIVE_COMPLETION_STATE" \
    --action "$(cat "$FIXTURE/action")" --reason 'Review objective assessed against preserved evidence'
fi
[ ! -f "$FIXTURE/round-message" ] || cat "$FIXTURE/round-message"
exit "$(cat "$FIXTURE/round-rc")"
"#;
