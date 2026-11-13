#!/usr/bin/env bash
# issue_1437_amplihack_stub.sh — the `amplihack` the issue #1437 tests put first
# on PATH. Not a test: tests/issue_1437_autodrive_*.sh copy it into a private
# bin directory.
#
#   orch helper   exec $REAL_AMPLIHACK when it is set; otherwise a POSIX
#                 stand-in for `extract-json --require-field F` (pass the blob
#                 through when it names F) and `extract-field --field F
#                 --default D [--require-boolean]` (a string or bare value;
#                 JSON \n and \t decoded, boolean types checked when required).
#   recipe show   exit 0.
#   recipe run    the loop names recipes by absolute launch-side path (#1543),
#                 so the recipe is keyed on its base name. Every call is logged
#                 to $STUB_STATE/calls.
#     loop-health-evaluator, call h:
#                 saves loop_history to $STUB_STATE/history-h, prints
#                 $STUB_HEALTH_EXTRA_h, then the runner's step-04 block (#1512)
#                 with $STUB_HEALTH_h (default CONTINUE for h=1, DONE after),
#                 and exits $STUB_HEALTH_RC_h (default 0).
#                 STUB_REAL_HEALTH_ROOT opts into the shipped collector,
#                 resolver and enforcement commands; only the model
#                 assessment comes from STUB_HEALTH_h in that mode.
#     any other recipe, call n:
#                 writes $STUB_RECORD_n to the autodrive_round_record file
#                 (default NOT_READY for n=1, MERGE_READY after), prints $STUB_OUT_n
#                 and exits $STUB_RC_n (default 0).
#   internal review-*   (#2096) the loop owns a private review checkout. POSIX
#                 stand-ins with exact argument counts, each call logged as
#                 one line to $STUB_STATE/review-calls:
#     review-allocate REPO STATE   prints REPO; exits $STUB_REVIEW_ALLOCATE_RC
#                                  (default 0, nothing printed when non-zero).
#     review-next REPO CHECKOUT    prints CHECKOUT.
#     review-launch CHECKOUT       prints CHECKOUT.
#     review-head CHECKOUT         prints `git rev-parse --verify HEAD` there;
#                                  git's exit status is passed through.
#     review-release CHECKOUT      prints nothing; exits $STUB_REVIEW_RELEASE_RC
#                                  (default 0) and, on success, increments
#                                  $STUB_STATE/review-release.
#                 Any other internal command or argument count is unhandled.
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
  MODE="${3:-}"; FIELD=""; DEFAULT=""; REQUIRE_BOOLEAN=false
  shift 3 || true
  while [ $# -gt 0 ]; do
    case "$1" in
      --require-field|--field) FIELD="${2:-}"; shift 2 ;;
      --default) DEFAULT="${2:-}"; shift 2 ;;
      --require-boolean) REQUIRE_BOOLEAN=true; shift ;;
      *) shift ;;
    esac
  done
  BLOB="$(cat)"
  case "$MODE" in
    normalise-loop-verdict)
      case "$BLOB" in CONTINUE|DONE|STUCK) printf '%s' "$BLOB" ;; *) printf STUCK ;; esac
      exit 0 ;;
    extract-json)
      if [ -z "$FIELD" ]; then
        case "$BLOB" in \{*\}) printf '%s' "$BLOB" ;; *) printf '{}' ;; esac
      else
        case "$BLOB" in *"\"${FIELD}\""*) printf '%s' "$BLOB" ;; *) printf '{}' ;; esac
      fi
      exit 0 ;;
    extract-field)
      if [ "$REQUIRE_BOOLEAN" = true ]; then
        V="$(printf '%s' "$BLOB" | sed -n "s/.*\"${FIELD}\"[[:space:]]*:[[:space:]]*\(true\|false\)[[:space:]]*[,}].*/\1/p" | head -n 1)"
        printf '%s\n' "${V:-$DEFAULT}"
        exit 0
      fi
      V="$(printf '%s' "$BLOB" | sed -n "s/.*\"${FIELD}\"[[:space:]]*:[[:space:]]*\"\([^\"]*\)\".*/\1/p" | head -n 1)"
      if [ -z "$V" ]; then
        V="$(printf '%s' "$BLOB" | sed -n "s/.*\"${FIELD}\"[[:space:]]*:[[:space:]]*\([A-Za-z0-9._-]*\).*/\1/p" | head -n 1)"
      fi
      [ -n "$V" ] || V="$DEFAULT"
      V="${V//\\n/$'\n'}"; V="${V//\\t/$'\t'}"
      printf '%s\n' "$V"
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
      case "$a" in
        loop_*=*)
          context_name="$(printf '%s' "${a%%=*}" | tr '[:lower:]' '[:upper:]')"
          export "$context_name=${a#*=}"
          ;;
      esac
    fi
    prev="$a"
  done
  if [ "$RECIPE" = "loop-health-evaluator" ]; then
    h="$(count health)"
    printf '%s' "$HISTORY" > "${STATE}/history-${h}"
    if [ "$h" = 1 ]; then d=CONTINUE; else d=DONE; fi
    # Opt in to the shipped deterministic evaluator commands. Only the model
    # assessment is supplied by the test; collection, resolution and enforcement
    # execute verbatim, so transcript false positives cannot hide behind a stub.
    if [ -n "${STUB_REAL_HEALTH_ROOT:-}" ]; then
      export AMPLIHACK_HOME="$STUB_REAL_HEALTH_ROOT"
      recipe_dir="$STUB_REAL_HEALTH_ROOT/amplifier-bundle/recipes"
      step_command() {
        awk -v id="$2" '
          /^  - id:/ { if (body) exit; active = index($0, "id: \"" id "\"") > 0 }
          active && /^    command: \|/ { body=1; next }
          body { if ($0 ~ /^    [a-zA-Z_]+:/) exit; sub(/^      /, ""); print }
        ' "$1"
      }
      RECIPE_VAR_loop_evidence="$(bash -c "$(step_command "$recipe_dir/loop-evidence-collector.yaml" step-01-collect-loop-evidence)")" || exit $?
      export RECIPE_VAR_loop_evidence
      terminal="$(printf '%s' "$RECIPE_VAR_loop_evidence" | "$0" orch helper extract-field --field terminal_refusal --default true)"
      export RECIPE_VAR_loop_health_assessment=""
      if [ "$terminal" = false ]; then
        : > "${STATE}/health-agent-called-${h}"
        RECIPE_VAR_loop_health_assessment="{\"loop_verdict\":\"$(knob STUB_HEALTH_ "$h" "$d")\"}"
        export RECIPE_VAR_loop_health_assessment
      fi
      RECIPE_VAR_loop_health="$(bash -c "$(step_command "$recipe_dir/loop-health-evaluator.yaml" step-03-resolve-loop-verdict)")" || exit $?
      export RECIPE_VAR_loop_health
      printf '  \342\234\223 step-04-enforce-loop-verdict: completed\n    Output: '
      bash -c "$(step_command "$recipe_dir/loop-health-evaluator.yaml" step-04-enforce-loop-verdict)"
      exit $?
    fi
    extra="$(knob STUB_HEALTH_EXTRA_ "$h" "")"
    [ -n "$extra" ] && printf '%s\n' "$extra"
    printf '  \342\234\223 step-04-enforce-loop-verdict: completed\n    Output: LOOP_HEALTH: %s\n' \
      "$(knob STUB_HEALTH_ "$h" "$d")"
    exit "$(knob STUB_HEALTH_RC_ "$h" 0)"
  fi
  n="$(count rounds)"
  if [ "$n" = 1 ]; then d='{"merge_ready_verdict":"NOT_READY"}'; else d='{"merge_ready_verdict":"MERGE_READY"}'; fi
  [ -n "$RECORD" ] && { knob STUB_RECORD_ "$n" "$d" > "$RECORD"; : > "${RECORD}.findings"; }
  printf '%s\n' "$(knob STUB_OUT_ "$n" "round ran")"
  exit "$(knob STUB_RC_ "$n" 0)"
fi

if [ "${1:-}" = internal ]; then
  case "${2:-}:$#" in
    review-allocate:4|review-next:4|review-launch:3|review-head:3|review-release:3)
      printf '%s\n' "${*:2}" >> "${STATE}/review-calls"
      case "$2" in
        review-allocate)
          rc="${STUB_REVIEW_ALLOCATE_RC:-0}"
          [ "$rc" = 0 ] || exit "$rc"
          printf '%s\n' "$3"
          exit 0 ;;
        review-next) printf '%s\n' "$4"; exit 0 ;;
        review-launch) printf '%s\n' "$3"; exit 0 ;;
        review-head) exec git -C "$3" rev-parse --verify HEAD ;;
        review-release)
          rc="${STUB_REVIEW_RELEASE_RC:-0}"
          [ "$rc" = 0 ] || exit "$rc"
          count review-release >/dev/null
          exit 0 ;;
      esac ;;
  esac
fi

echo "issue_1437_amplihack_stub: unhandled: $*" >&2
exit 3
