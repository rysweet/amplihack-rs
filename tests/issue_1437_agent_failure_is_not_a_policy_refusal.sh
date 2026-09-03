#!/usr/bin/env bash
# issue_1437_agent_failure_is_not_a_policy_refusal.sh
#
# `auto-drive-to-merge` surfaced an ordinary agent/API failure as exit 79 and
# stopped a run permanently.
#
# WHAT ACTUALLY HAPPENED (issue #1437, PR #1416). The merge-ready builder agent
# did the substantive work, committed and pushed `9d521815`, and only then lost
# its Copilot session:
#
#     Execution failed: 400 The resource you requested was not found.
#
# The nested recipe reported that correctly — `amplihack copilot failed
# (exit 1)`. `autodrive_loop.sh` nevertheless announced
#
#     ERROR: exit 79 terminal policy refusal from 'autodrive-merge-round'
#     AUTO_DRIVE_LOOP: TERMINAL_POLICY_REFUSAL
#
# and exited 79, because its terminal test grepped the WHOLE ROUND TRANSCRIPT
# for the literal word `BLOCKED_TERMINAL`. A round transcript contains every
# file the round's agents read. In the captured session for that run the word
# appears 98 times — as the CONTENT of the repository's own contract test and
# reference docs (`BLOCKED_TERMINAL is TERMINAL: surfaced, and never retried
# into`, `BLOCKED_TERMINAL not detected (terminal_refusal='${T}')`) and as
# refusals a descendant received and the agent then handled inline, exactly as
# the refusal's own text instructs. None of it was a refusal of the round.
#
# The two directions this locks down:
#
#   NOT TERMINAL — an agent/API failure whose transcript merely quotes a
#                  refusal must leave the loop free to run another round.
#   STILL TERMINAL — a real structural guard refusal must still stop the loop
#                  and exit 79, whether it arrives as the child's exit status
#                  or as the child's own `structural_refusal` classification.
#
# Both fail before the fix. The first because the transcript grep fires; the
# second because that grep is case-sensitive on `BLOCKED_TERMINAL` and a
# refusal that reaches the parent only through the (lower-case) classification
# marker was missed entirely.
#
# Usage: bash tests/issue_1437_agent_failure_is_not_a_policy_refusal.sh
# Exit codes: 0 = pass, 1 = fail, 2 = harness error.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
LOOP="${REPO_ROOT}/amplifier-bundle/tools/autodrive_loop.sh"
[ -f "$LOOP" ] || { echo "HARNESS-ERROR: missing ${LOOP}" >&2; exit 2; }

PASS_COUNT=0
FAIL_COUNT=0
pass() { PASS_COUNT=$((PASS_COUNT + 1)); echo "  PASS[$1]: $2"; }
fail() { FAIL_COUNT=$((FAIL_COUNT + 1)); echo "  FAIL[$1]: $2" >&2; }

echo "=== issue #1437: an agent/API failure is not a policy refusal ==="

WORK="$(mktemp -d)"
trap 'rm -rf "${WORK}"' EXIT
STUB_BIN="${WORK}/bin"; mkdir -p "${STUB_BIN}"

# --- the JSON helper the loop reads verdicts with ---------------------------
# Prefer a REAL amplihack when one is on hand, so the verdict pipeline under
# test is the real one. CI's lint job builds no Rust, so a small POSIX stand-in
# covers the two calls the loop makes: `extract-json --require-field F` (pass
# the object through only when it carries F) and `extract-field --field F
# --default D`.
supports_helper() {
  printf '{"a":"b"}' | "$1" orch helper extract-field --field a --default X >/dev/null 2>&1 \
    && printf '{"a":"b"}' | "$1" orch helper extract-json --require-field a >/dev/null 2>&1
}
REAL_AMPLIHACK=""
for cand in "${REPO_ROOT}/target/release/amplihack" "${REPO_ROOT}/target/debug/amplihack" \
            "${CARGO_TARGET_DIR:-}/release/amplihack" "${CARGO_TARGET_DIR:-}/debug/amplihack" \
            "$(command -v amplihack 2>/dev/null || true)"; do
  [ -n "${cand}" ] && [ -x "${cand}" ] || continue
  if supports_helper "${cand}"; then REAL_AMPLIHACK="${cand}"; break; fi
done
export REAL_AMPLIHACK
if [ -n "$REAL_AMPLIHACK" ]; then
  echo "  (verdict pipeline: real binary ${REAL_AMPLIHACK})"
else
  echo "  (verdict pipeline: POSIX stand-in — no built amplihack on this host)"
fi

cat > "${STUB_BIN}/amplihack" <<'STUB'
#!/usr/bin/env bash
# --- orch helper -----------------------------------------------------------
if [ "${1:-}" = "orch" ]; then
  if [ -n "${REAL_AMPLIHACK:-}" ]; then exec "$REAL_AMPLIHACK" "$@"; fi
  FIELD=""; DEFAULT=""; MODE="${3:-}"
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
      case "$BLOB" in *"\"${FIELD}\""*) printf '%s' "$BLOB" ;; *) : ;; esac
      exit 0 ;;
    extract-field)
      # "field":"value"  or  "field":value  (bare number / true / false / null)
      V="$(printf '%s' "$BLOB" | sed -n "s/.*\"${FIELD}\"[[:space:]]*:[[:space:]]*\"\([^\"]*\)\".*/\1/p" | head -1)"
      if [ -z "$V" ]; then
        V="$(printf '%s' "$BLOB" | sed -n "s/.*\"${FIELD}\"[[:space:]]*:[[:space:]]*\([A-Za-z0-9._-]*\).*/\1/p" | head -1)"
      fi
      [ -n "$V" ] || V="$DEFAULT"
      printf '%s' "$V"
      exit 0 ;;
  esac
  exit 0
fi
# --- recipe show -----------------------------------------------------------
if [ "${1:-}" = "recipe" ] && [ "${2:-}" = "show" ]; then exit 0; fi
# --- recipe run ------------------------------------------------------------
if [ "${1:-}" = "recipe" ] && [ "${2:-}" = "run" ]; then
  RECIPE="${3:-}"
  RECORD=""; for a in "$@"; do case "$a" in autodrive_round_record=*) RECORD="${a#*=}" ;; esac; done
  if [ "$RECIPE" = "loop-health-evaluator" ]; then
    echo "$RECIPE" >> "${STUB_HEALTH_CALLS:-/dev/null}"
    N="$(grep -c . "${STUB_HEALTH_CALLS:-/dev/null}" 2>/dev/null || echo 1)"
    if [ "$N" = "1" ]; then
      printf '%s\n' "${STUB_HEALTH_1:-LOOP_HEALTH: CONTINUE — keep going}"
      exit "${STUB_HEALTH_RC_1:-0}"
    fi
    printf '%s\n' "${STUB_HEALTH_N:-LOOP_HEALTH: DONE — converged}"
    exit "${STUB_HEALTH_RC_N:-0}"
  fi
  echo "$RECIPE" >> "${STUB_ROUND_CALLS:-/dev/null}"
  N="$(grep -c . "${STUB_ROUND_CALLS:-/dev/null}" 2>/dev/null || echo 1)"
  if [ "$N" = "1" ]; then
    [ -n "$RECORD" ] && { printf '%s' "${STUB_RECORD_1:-{\"crusty_verdict\":\"CONCERNS\"}}" > "$RECORD"; : > "${RECORD}.findings"; }
    printf '%s\n' "${STUB_STDOUT_1:-round ran}"
    exit "${STUB_RC_1:-0}"
  fi
  [ -n "$RECORD" ] && { printf '%s' "${STUB_RECORD_N:-{\"crusty_verdict\":\"CLEAN\"}}" > "$RECORD"; : > "${RECORD}.findings"; }
  printf '%s\n' "${STUB_STDOUT_N:-round ran}"
  exit "${STUB_RC_N:-0}"
fi
echo "stub amplihack: unhandled: $*" >&2
exit 3
STUB
chmod +x "${STUB_BIN}/amplihack"

LOOP_DIR=""; LOOP_OUT=""; LOOP_ERR=""
# Sets LOOP_RC / LOOP_OUT / LOOP_ERR. The loop's exit status is CAPTURED rather
# than returned, so this stays correct under the CI shell's `-e` (issue #1434):
# a scenario whose expected outcome is a non-zero exit must not kill the run.
LOOP_RC=0
run_loop() { # run_loop <name>
  LOOP_DIR="${WORK}/loop-$1"; mkdir -p "${LOOP_DIR}"
  export STUB_ROUND_CALLS="${LOOP_DIR}/round-calls" STUB_HEALTH_CALLS="${LOOP_DIR}/health-calls"
  : > "${STUB_ROUND_CALLS}"; : > "${STUB_HEALTH_CALLS}"
  LOOP_RC=0
  PATH="${STUB_BIN}:${PATH}" AMPLIHACK_BIN="${STUB_BIN}/amplihack" \
    bash "${LOOP}" --loop-name "crusty" --round-recipe "autodrive-crusty-round" \
      --clean-token "CLEAN" --verdict-field "crusty_verdict" \
      --repo "${WORK}" --state-dir "${LOOP_DIR}" \
      >"${LOOP_DIR}/out" 2>"${LOOP_DIR}/err" || LOOP_RC=$?
  LOOP_OUT="$(cat "${LOOP_DIR}/out")"
  LOOP_ERR="$(cat "${LOOP_DIR}/err")"
}
# `grep -c` prints 0 AND exits 1 when nothing matches, so `|| echo 0` would
# emit the count twice. Take the count and ignore the status.
count_lines() { local n=""; n="$(grep -c . "$1" 2>/dev/null)" || true; printf '%s' "${n:-0}"; }
round_calls() { count_lines "${LOOP_DIR}/round-calls"; }
health_calls() { count_lines "${LOOP_DIR}/health-calls"; }

reset_stub() {
  export STUB_RC_1=0 STUB_RC_N=0 STUB_HEALTH_RC_1=0 STUB_HEALTH_RC_N=0
  export STUB_RECORD_1='{"crusty_verdict":"CONCERNS"}' STUB_RECORD_N='{"crusty_verdict":"CLEAN"}'
  export STUB_STDOUT_1="round ran" STUB_STDOUT_N="round ran"
  export STUB_HEALTH_1="LOOP_HEALTH: CONTINUE — keep going"
  export STUB_HEALTH_N="LOOP_HEALTH: DONE — converged"
}

# ---------------------------------------------------------------------------
# 1. NOT TERMINAL — the reported incident, transcript and all.
# ---------------------------------------------------------------------------
# Round 1's transcript carries the literal word `BLOCKED_TERMINAL` three times,
# in exactly the shapes the real captured session carried it: repository file
# content the agent read, and a refusal handed to a DESCENDANT that the agent
# handled inline before doing the work. The round's actual terminating cause is
# the agent's API session dying, and the classification says so.
reset_stub
export STUB_RC_1=1
export STUB_STDOUT_1='● Search (grep) "BLOCKED_TERMINAL" (amplifier-bundle/recipes/tests) — 12 lines found
  test-issue-1337-loop-health-evaluator.sh:20:#   - exit 79 / BLOCKED_TERMINAL is TERMINAL: surfaced, and never retried into
● Bash: amplihack recipe run default-workflow
  BLOCKED_TERMINAL orchestration_unavailable: depth 3 of max 3 (issue #964/#1326).
  DO: complete this step inline and return your result.
I completed the work inline as instructed, committed 9d521815 and pushed.
Execution failed: 400 The resource you requested was not found.
ERROR: recipe run terminated on attempt 1 — classified `agent_api`.
amplihack.recipe.failure_class {"schema_version":1,"issue":1267,"class":"agent_api","signal":"execution failed: 400","action":"terminal","attempt":1,"retryable":false,"structural_refusal":false}'
run_loop incident; rc="${LOOP_RC}"

if [ "$rc" -ne 79 ]; then
  pass "AGENT-API-not-79" "an agent/API failure does not exit 79 (rc=${rc})"
else
  fail "AGENT-API-not-79" "an agent/API failure was reported as the exit-79 policy refusal"
fi
if ! printf '%s' "${LOOP_ERR}" | grep -qF 'TERMINAL_POLICY_REFUSAL'; then
  pass "AGENT-API-not-refusal" "no TERMINAL_POLICY_REFUSAL is claimed for an agent/API failure"
else
  fail "AGENT-API-not-refusal" "the loop claimed a terminal policy refusal that no guard issued"
fi
if [ "$(round_calls)" -ge 2 ]; then
  pass "AGENT-API-continues" "the loop ran another round ($(round_calls) rounds) instead of stopping dead"
else
  fail "AGENT-API-continues" "the loop stopped after $(round_calls) round(s); a transient failure killed the run"
fi
# The loop's OWN report, not the transcript it echoes: "classified agent_api"
# is the driver speaking. "Stopped" with no reason is what made this expensive.
if printf '%s' "${LOOP_ERR}" | grep -qF 'classified agent_api'; then
  pass "AGENT-API-named" "the loop's own verdict names the actual failure class"
else
  fail "AGENT-API-named" "the round's failure class is not named in the loop's report"
fi
if [ "$rc" -eq 0 ] && printf '%s' "${LOOP_OUT}" | grep -qF '"loop_result":"DONE"'; then
  pass "AGENT-API-converges" "the run reaches its own conclusion (DONE) after recovering"
else
  fail "AGENT-API-converges" "the run did not converge (rc=${rc}): ${LOOP_OUT}"
fi

# ---------------------------------------------------------------------------
# 2. STILL TERMINAL — the child's exit status IS the guard's answer.
# ---------------------------------------------------------------------------
reset_stub
export STUB_RC_1=79
export STUB_STDOUT_1='BLOCKED_TERMINAL orchestration_unavailable: depth 3 of max 3 (issue #964/#1326).
This is a POLICY decision, not an infrastructure fault.'
run_loop guard79; rc="${LOOP_RC}"
if [ "$rc" -eq 79 ]; then
  pass "GUARD79-terminal" "exit 79 is still propagated as the loop's own exit code"
else
  fail "GUARD79-terminal" "a real guard refusal became rc=${rc}"
fi
if printf '%s' "${LOOP_ERR}" | grep -qF 'TERMINAL_POLICY_REFUSAL'; then
  pass "GUARD79-escalates" "a real guard refusal is still escalated by name"
else
  fail "GUARD79-escalates" "a real guard refusal was not escalated"
fi
if [ "$(round_calls)" -eq 1 ] && [ "$(health_calls)" -eq 0 ]; then
  pass "GUARD79-no-retry" "the guard is never retried into and costs no model call"
else
  fail "GUARD79-no-retry" "rounds=$(round_calls) evaluator=$(health_calls) after a guard refusal"
fi

# ---------------------------------------------------------------------------
# 3. STILL TERMINAL — the refusal arrives only as the child's classification.
# ---------------------------------------------------------------------------
# A guard that refuses a STEP of the round leaves `recipe-runner-rs` exiting 1,
# not 79, and the runner truncates the step's output in its summary — so the
# uppercase word may never reach the parent's log at all. What always reaches
# it is `amplihack recipe run`'s own classification of the step that ended the
# run. That, not a word count, is what makes this terminal.
reset_stub
export STUB_RC_1=1
export STUB_STDOUT_1='  ✗ step-04-address-blockers: failed
    Output: nested run refused: BLOCKED_TERMI... (truncated)
ERROR: recipe run terminated on attempt 1 — a structural guard refused this run.
amplihack.recipe.failure_class {"schema_version":1,"issue":1267,"class":"work","signal":"blocked_terminal orchestration_unavailable","action":"terminal","attempt":1,"retryable":false,"structural_refusal":true}'
run_loop guardclass; rc="${LOOP_RC}"
if [ "$rc" -eq 79 ]; then
  pass "GUARD-CLASS-terminal" "a classified structural refusal is terminal even when the child exits 1"
else
  fail "GUARD-CLASS-terminal" "a real structural refusal was not detected (rc=${rc})"
fi
if printf '%s' "${LOOP_ERR}" | grep -qF 'structural_refusal=true'; then
  pass "GUARD-CLASS-named" "the refusal names the evidence it was decided on"
else
  fail "GUARD-CLASS-named" "the refusal does not say what made it terminal"
fi
if [ "$(round_calls)" -eq 1 ]; then
  pass "GUARD-CLASS-no-retry" "a classified structural refusal is not retried into"
else
  fail "GUARD-CLASS-no-retry" "a round was retried after a structural refusal"
fi

# ---------------------------------------------------------------------------
# 4. The transcript grep is gone for good.
# ---------------------------------------------------------------------------
# A guard against the fix being re-introduced by someone reaching for the
# obvious one-liner. The terminal test may read the child's exit status and the
# structured classification; it may not scan a transcript for a word.
if ! grep -nE "grep[^\n]*(-q|-c)[^\n]*['\"]?BLOCKED_TERMINAL" "$LOOP" \
     | grep -vE '^[0-9]+:[[:space:]]*#' | grep -q .; then
  pass "NO-TRANSCRIPT-GREP" "the loop never decides terminality by grepping a round transcript"
else
  fail "NO-TRANSCRIPT-GREP" "a transcript grep for BLOCKED_TERMINAL is back in ${LOOP}"
fi

echo "=== ${PASS_COUNT} passed, ${FAIL_COUNT} failed ==="
[ "$FAIL_COUNT" -eq 0 ] || exit 1
exit 0
