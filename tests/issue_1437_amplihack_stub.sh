#!/usr/bin/env bash
# issue_1437_amplihack_stub.sh — the `amplihack` the issue #1437 tests put first
# on PATH. Not a test: tests/issue_1437_autodrive_*.sh copy it into a private
# bin directory.
#
#   orch helper   exec $REAL_AMPLIHACK when it is set; otherwise a POSIX
#                 stand-in for `extract-json --require-field F` (pass the blob
#                 through when it names F) and `extract-field --field F
#                 --default D` (a string or bare value; JSON \n and \t decoded).
#   recipe show   exit 0.
#   recipe run    the loop names recipes by absolute launch-side path (#1543),
#                 so the recipe is keyed on its base name. Every call is logged
#                 to $STUB_STATE/calls.
#     loop-health-evaluator, call h:
#                 saves loop_history to $STUB_STATE/history-h, prints
#                 $STUB_HEALTH_EXTRA_h, then the runner's step-04 block (#1512)
#                 with $STUB_HEALTH_h (default CONTINUE for h=1, DONE after),
#                 and exits $STUB_HEALTH_RC_h (default 0).
#     any other recipe, call n:
#                 writes $STUB_RECORD_n to the autodrive_round_record file
#                 (default CONCERNS for n=1, CLEAN after), prints $STUB_OUT_n
#                 and exits $STUB_RC_n (default 0).
set -u
STATE="${STUB_STATE:?STUB_STATE is not set}"

count() { # count <name>: increments and prints $STATE/<name>
  local n
  n="$(cat "${STATE}/$1" 2>/dev/null || echo 0)"
  n=$((n + 1))
  printf '%s' "$n" > "${STATE}/$1"
  printf '%s' "$n"
}
knob() { # knob <prefix> <n> <default>
  local name="$1$2"
  printf '%s' "${!name-$3}"
}

if [ "${1:-} ${2:-}" = "orch helper" ]; then
  if [ -n "${REAL_AMPLIHACK:-}" ]; then exec "$REAL_AMPLIHACK" "$@"; fi
  MODE="${3:-}"; FIELD=""; DEFAULT=""
  shift 3 || true
  while [ $# -gt 0 ]; do
    case "$1" in
      --require-field|--field) FIELD="${2:-}"; shift 2 ;;
      --default) DEFAULT="${2:-}"; shift 2 ;;
      *) shift ;;
    esac
  done
  BLOB="$(cat)"
  case "$MODE" in
    extract-json)
      case "$BLOB" in *"\"${FIELD}\""*) printf '%s' "$BLOB" ;; esac
      exit 0 ;;
    extract-field)
      V="$(printf '%s' "$BLOB" | sed -n "s/.*\"${FIELD}\"[[:space:]]*:[[:space:]]*\"\([^\"]*\)\".*/\1/p" | head -n 1)"
      if [ -z "$V" ]; then
        V="$(printf '%s' "$BLOB" | sed -n "s/.*\"${FIELD}\"[[:space:]]*:[[:space:]]*\([A-Za-z0-9._-]*\).*/\1/p" | head -n 1)"
      fi
      [ -n "$V" ] || V="$DEFAULT"
      V="${V//\\n/$'\n'}"; V="${V//\\t/$'\t'}"
      printf '%s' "$V"
      exit 0 ;;
  esac
  exit 0
fi

if [ "${1:-} ${2:-}" = "recipe show" ]; then exit 0; fi

if [ "${1:-} ${2:-}" = "recipe run" ]; then
  RECIPE="$(basename -- "${3:-}" .yaml)"
  printf '%s\n' "$RECIPE" >> "${STATE}/calls"
  RECORD=""; HISTORY=""; prev=""
  for a in "$@"; do
    if [ "$prev" = "-c" ]; then
      case "$a" in
        autodrive_round_record=*) RECORD="${a#*=}" ;;
        loop_history=*) HISTORY="${a#*=}" ;;
      esac
    fi
    prev="$a"
  done
  if [ "$RECIPE" = "loop-health-evaluator" ]; then
    h="$(count health)"
    printf '%s' "$HISTORY" > "${STATE}/history-${h}"
    if [ "$h" = 1 ]; then d=CONTINUE; else d=DONE; fi
    extra="$(knob STUB_HEALTH_EXTRA_ "$h" "")"
    [ -n "$extra" ] && printf '%s\n' "$extra"
    printf '  \342\234\223 step-04-enforce-loop-verdict: completed\n    Output: LOOP_HEALTH: %s\n' \
      "$(knob STUB_HEALTH_ "$h" "$d")"
    exit "$(knob STUB_HEALTH_RC_ "$h" 0)"
  fi
  n="$(count rounds)"
  if [ "$n" = 1 ]; then d='{"crusty_verdict":"CONCERNS"}'; else d='{"crusty_verdict":"CLEAN"}'; fi
  [ -n "$RECORD" ] && { knob STUB_RECORD_ "$n" "$d" > "$RECORD"; : > "${RECORD}.findings"; }
  printf '%s\n' "$(knob STUB_OUT_ "$n" "round ran")"
  exit "$(knob STUB_RC_ "$n" 0)"
fi

echo "issue_1437_amplihack_stub: unhandled: $*" >&2
exit 3
