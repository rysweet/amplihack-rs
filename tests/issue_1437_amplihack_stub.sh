#!/usr/bin/env bash
# issue_1437_amplihack_stub.sh — the `amplihack` the issue #1437 tests put first
# on PATH. Not a test: tests/issue_1437_autodrive_*.sh copy it into a private
# bin directory.
#
#   orch helper   exec $REAL_AMPLIHACK when it is set; otherwise a POSIX
#                 stand-in for `extract-json --require-field F` (pass the blob
#                 through when it names F) and `extract-field --field F
#                 --default D [--require-boolean]` (a string or bare value;
#                 JSON \n and \t decoded, boolean types checked when required),
#                 and for `normalise-loop-verdict` (pass CONTINUE, DONE or
#                 STUCK through; anything else becomes STUCK).
#
#                 stdin rule (#2582): the mode is dispatched BEFORE stdin is
#                 opened, and only these five modes read it —
#                   normalise-loop-verdict  extract-json  extract-field
#                   normalise-verdict       normalise-type
#                 Every other mode answers 0 without reading, so a caller that
#                 passes an open stdin cannot block on a channel the stub does
#                 not use. `autodrive-completion-seal` is the mode that hung
#                 tests/issue_1437_autodrive_loop.sh this way.
#
#                 The last two have no stand-in and answer nothing, but the
#                 tree pipes into them, so they must keep draining: a mode that
#                 is piped into belongs on that list even with no stand-in,
#                 or its writer takes SIGPIPE and the pipeline ends 141 under
#                 `set -o pipefail`. Adding a mode means two decisions: whether
#                 it joins the list above, and whether it needs a stand-in
#                 below. See docs/howto/keep-test-stubs-from-blocking-on-stdin.md.
#   recipe show   exit 0.
#   recipe run    the loop names recipes by absolute launch-side path (#1543),
#                 so the recipe is keyed on its base name. Every call is logged
#                 to $STUB_STATE/calls.
#     loop-health-evaluator, call h:
#                 saves loop_history (from -c or --context-from-file,
#                 #2582) to $STUB_STATE/history-h, prints
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
#                 With STUB_LEAVE_BASELINE set, it also creates a
#                 pre-review.stub directory beside the round record, standing
#                 in for the round's pre-review evidence, so a test can see
#                 whether the loop reclaims it (#2808).
#     every recipe run that names --working-dir logs one line
#                 `round-workdir <path>` to $STUB_STATE/round-workdirs.
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
  MODE="${3:-}"
  # Answer and leave, before stdin or the arguments are touched, for every mode
  # this stub has no stand-in for: see the stdin rule in this file's header.
  # Only the five modes below carry a blob, and only they use the arguments.
  case "$MODE" in
    normalise-loop-verdict|extract-json|extract-field|normalise-verdict|normalise-type) : ;;
    *) exit 0 ;;
  esac
  FIELD=""; DEFAULT=""; REQUIRE_BOOLEAN=false
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
    [ "$prev" = "--working-dir" ] && printf 'round-workdir %s\n' "$a" >> "${STATE}/round-workdirs"
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
    # #2582: the loop hands the evaluator's free-text keys to the nested run
    # by file, so they are no longer `-c key=value`. Read the file and treat
    # the value exactly as the `-c` form above does, or this stub sees an
    # empty history and the tests that read it assert nothing.
    if [ "$prev" = "--context-from-file" ]; then
      ctx_key="${a%%=*}"
      ctx_file="${a#*=}"
      ctx_value=""
      [ -f "$ctx_file" ] && ctx_value="$(cat -- "$ctx_file")"
      case "$ctx_key" in
        loop_history) HISTORY="$ctx_value" ;;
      esac
      case "$ctx_key" in
        loop_*)
          context_name="$(printf '%s' "$ctx_key" | tr '[:lower:]' '[:upper:]')"
          export "$context_name=$ctx_value"
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
  # #2808: stand in for step-01c's pre-review evidence, which the round writes
  # into the state directory beside the round record.
  if [ -n "${STUB_LEAVE_BASELINE:-}" ] && [ -n "$RECORD" ]; then
    mkdir -p "${RECORD%/*}/pre-review.stub" \
      && printf 'untracked secret\n' > "${RECORD%/*}/pre-review.stub/contents.tar"
  fi
  printf '%s\n' "$(knob STUB_OUT_ "$n" "round ran")"
  exit "$(knob STUB_RC_ "$n" 0)"
fi


echo "issue_1437_amplihack_stub: unhandled: $*" >&2
exit 3
