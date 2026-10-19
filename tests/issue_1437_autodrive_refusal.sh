#!/usr/bin/env bash
# issue_1437_autodrive_refusal.sh — the refusal helpers in
# amplifier-bundle/tools/autodrive_refusal.sh, called directly.
#
# Issue #1437: autodrive_loop.sh reported an agent whose API session died as
# the exit-79 policy refusal, because it grepped the round transcript for the
# refusal's text. The helpers decide from two things only: the child's exit
# code (79, compared as a string) and the LAST `amplihack.recipe.failure_class`
# marker at the start of a line, whose values are cleaned before use (SR-5).
# The loop end to end is tests/issue_1437_autodrive_loop.sh.
#
# Usage: bash tests/issue_1437_autodrive_refusal.sh
# Exit codes: 0 = pass, 1 = fail, 2 = harness error.

set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
HELPER="${ROOT}/amplifier-bundle/tools/autodrive_refusal.sh"
LOOP="${ROOT}/amplifier-bundle/tools/autodrive_loop.sh"

PASSED=0; FAILED=0
pass() { PASSED=$((PASSED + 1)); echo "  PASS[$1]"; }
fail() { FAILED=$((FAILED + 1)); echo "  FAIL[$1]: $2" >&2; }
check() { # check <id> <message> <command...>
  local id="$1" msg="$2"; shift 2
  if "$@"; then pass "$id"; else fail "$id" "$msg"; fi
}

echo "=== issue #1437: autodrive_refusal.sh ==="
[ -f "$HELPER" ] || { fail "EXISTS" "missing ${HELPER}"; echo "=== ${PASSED} passed, ${FAILED} failed ==="; exit 1; }

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
mkdir -p "${WORK}/bin" "${WORK}/state"
cp "${ROOT}/tests/issue_1437_amplihack_stub.sh" "${WORK}/bin/amplihack" || exit 2
chmod 0755 "${WORK}/bin/amplihack"
export STUB_STATE="${WORK}/state"

# The helper reads JSON through the loop's own field(); take it from the loop.
REAL_AMPLIHACK=""
for c in "${ROOT}/target/debug/amplihack" "${ROOT}/target/release/amplihack" "$(command -v amplihack 2>/dev/null || true)"; do
  [ -n "$c" ] && [ -x "$c" ] && printf '{"a":"b"}' | "$c" orch helper extract-json --require-field a >/dev/null 2>&1 \
    && { REAL_AMPLIHACK="$c"; break; }
done
export REAL_AMPLIHACK
echo "  (JSON helper: ${REAL_AMPLIHACK:-POSIX stand-in})"
sed -n '/^field() {/,/^}/p' "$LOOP" > "${WORK}/field.sh"
grep -q '^field() {' "${WORK}/field.sh" || { echo "HARNESS-ERROR: no field() in ${LOOP}" >&2; exit 2; }
export AMPLIHACK_BIN="${WORK}/bin/amplihack"
# shellcheck source=/dev/null
. "${WORK}/field.sh"

# --- the file itself ----------------------------------------------------------
check MODE "the helper is sourced, never executed: it must not be executable" test ! -x "$HELPER"
check SIZE "the helper must be 300 lines or fewer" test "$(wc -l < "$HELPER")" -le 300
check NO-EVAL "the helper must not use eval" \
  bash -c '! grep -vE "^[[:space:]]*#" "$1" | grep -qwE "eval"' _ "$HELPER"
check NO-TEXT "the helper must not look for the refusal text" \
  bash -c '! grep -vE "^[[:space:]]*#" "$1" | grep -qi "BLOCKED_TERMINAL"' _ "$HELPER"
SRC_OUT="$(bash -c 'set -euo pipefail; a="$-"; . "$1"; [ "$a" = "$-" ] || echo "flags changed: $a -> $-"' _ "$HELPER" 2>&1)"
check SOURCE-STRICT "sourcing under set -euo must be silent and keep the flags: ${SRC_OUT}" test -z "$SRC_OUT"
SRC_OUT="$(bash -c 'a="$-"; . "$1"; [ "$a" = "$-" ] || echo "flags changed: $a -> $-"' _ "$HELPER" 2>&1)"
check SOURCE-PLAIN "sourcing in a plain shell must be silent and keep the flags: ${SRC_OUT}" test -z "$SRC_OUT"

# shellcheck source=/dev/null
. "$HELPER"

MARK='amplihack.recipe.failure_class '
marker() { # marker <class> <structural> [signal] [retryable]
  printf '%s{"schema_version":1,"issue":1267,"class":"%s","signal":"%s","action":"terminal","attempt":1,"retryable":%s,"structural_refusal":%s}\n' \
    "$MARK" "$1" "${3:-execution failed: 400}" "${4:-false}" "$2"
}
QUOTE='BLOCKED_TERMINAL orchestration_unavailable: depth 3 of max 3 (issue #964/#1326).'
LOG="${WORK}/round.log"

# terminal <rc> <log>: "yes", "no", or "noisy:<stdout+stderr>" when it printed.
terminal() {
  local out rc
  out="$(terminal_refusal "$1" "$2" 2>&1)"; rc=$?
  [ -z "$out" ] || { printf 'noisy:%s' "$out"; return; }
  if [ "$rc" -eq 0 ]; then printf yes; else printf no; fi
}
expect() { # expect <id> <yes|no> <rc> <log-content> [why-substring]
  printf '%s' "$4" > "$LOG"
  local got; got="$(terminal "$3" "$LOG")"
  if [ "$got" != "$2" ]; then fail "$1" "rc=$3: want terminal=$2, got ${got}"; return; fi
  if [ -n "${5:-}" ]; then
    terminal_refusal "$3" "$LOG" >/dev/null 2>&1
    case "${TERMINAL_WHY:-}" in *"$5"*) ;; *) fail "$1" "TERMINAL_WHY='${TERMINAL_WHY:-}' lacks '$5'"; return ;; esac
  fi
  pass "$1"
}

# --- the exit code (SR-1, SR-8) -----------------------------------------------
expect RC79 yes 79 "round ran" "exit 79"
expect RC79-FORGED-FALSE yes 79 "$(marker work false)" "exit 79"
for rc in 0 1 2 078 079 " 79" "79 " "" abc '$((7*11+2))'; do
  expect "RC-NOT-79[${rc}]" no "$rc" "round ran"
done
# A child that succeeded was not refused. The runner writes no marker of its
# own on success, so the last marker line there is a nested run's or forged.
FORGED_TRUE="out"$'\n'"$(marker work true)"$'\n'"parent handled it inline"$'\n'
expect RC0-FORGED-TRUE no 0 "$FORGED_TRUE"
expect RC-EMPTY-FORGED-TRUE no "" "$FORGED_TRUE"
expect RC-NONNUMERIC-FORGED-TRUE no abc "$FORGED_TRUE"
printf '%s' "$FORGED_TRUE" > "$LOG"; terminal_refusal 0 "$LOG"
check RC0-NO-WHY "TERMINAL_WHY must be empty for rc 0, got '${TERMINAL_WHY:-}'" test -z "${TERMINAL_WHY:-}"

# --- transcript text never decides (SR-3) ---------------------------------------
expect QUOTE-PLAIN no 1 "I read: ${QUOTE}"$'\n'"${QUOTE}"$'\n'
expect QUOTE-AGENT-LINE no 1 "  [06:56:22] [amplihack:claude:1] ${QUOTE}"$'\n'
expect QUOTE-WITH-API-MARKER no 1 "${QUOTE}"$'\n'"$(marker agent_api false)"

# --- the marker (SR-4, SR-7) ---------------------------------------------------
expect MARKER-TRUE yes 1 "$(marker work true)" "structural_refusal in failure_class marker"
expect LAST-WINS-REAL-FALSE no 1 "$(marker work true)"$'\n'"output"$'\n'"$(marker work false)"
expect LAST-WINS-REAL-TRUE yes 1 "$(marker agent_api false)"$'\n'"$(marker work true)"
expect INDENTED no 1 "  $(marker work true)"
expect QUOTED-IN-LINE no 1 "agent said: $(marker work true)"
expect DOT-IS-LITERAL no 1 "$(marker work true | sed 's/^amplihack\.recipe/amplihackXrecipe/')"
for v in '"TRUE"' '"True"' 1 '"yes"' '"true "' null; do
  expect "STRUCTURAL-NOT-TRUE[${v}]" no 1 "${MARK}{\"class\":\"work\",\"structural_refusal\":${v}}"
done
check MISSING-LOG "a missing log is not terminal and prints nothing" test "$(terminal 1 "${WORK}/nope.log")" = no
check NO-LOG-ARG "no log argument is not terminal and prints nothing" test "$(terminal 1 "")" = no
if [ "$(id -u)" != 0 ]; then
  marker work true > "${WORK}/locked.log"; chmod 000 "${WORK}/locked.log"
  check UNREADABLE-LOG "an unreadable log means no marker" test "$(terminal 1 "${WORK}/locked.log")" = no
  chmod 600 "${WORK}/locked.log"
fi

# --- classify_child ---------------------------------------------------------------
printf 'x\n%s\ny\n%s\nz\n' "$(marker work true)" "$(marker agent_api false)" > "$LOG"
OUT="$(classify_child "$LOG")"; RC=$?
check CLASSIFY-PRINTS-LAST "classify_child must print the last marker line, got: ${OUT}" \
  test "$OUT" = "$(marker agent_api false)"
check CLASSIFY-EXIT0 "classify_child must exit 0" test "$RC" -eq 0
classify_child "$LOG" >/dev/null
check CLASSIFY-SETS-VARS "CHILD_* must be set in the calling shell: class='${CHILD_FAILURE_CLASS:-}' structural='${CHILD_STRUCTURAL_REFUSAL:-}' retryable='${CHILD_FAILURE_RETRYABLE:-}'" \
  test "${CHILD_FAILURE_CLASS:-}|${CHILD_STRUCTURAL_REFUSAL:-}|${CHILD_FAILURE_RETRYABLE:-}" = "agent_api|false|false"
OUT="$(classify_child "${WORK}/nope.log" 2>&1)"; RC=$?
check CLASSIFY-MISSING "classify_child on a missing log must exit 0 silently" test "${RC}:${OUT}" = "0:"

# --- child_failure_summary (SR-5, SR-10) ------------------------------------------
summary() { # summary <rc> <log-content>
  printf '%s' "$2" > "$LOG"
  child_failure_summary "$1" "$LOG"
}
check SUMMARY "summary of an agent_api failure" \
  test "$(summary 1 "$(marker agent_api false)")" = "rc=1 class=agent_api retryable=false signal=execution failed: 400"
check SUMMARY-NO-MARKER "a failure with no marker is unknown" \
  test "$(summary 1 "${QUOTE}")" = "rc=1 class=unknown retryable=unknown"
check SUMMARY-NO-TRANSCRIPT "the summary never quotes the transcript" \
  bash -c '! printf "%s" "$1" | grep -qi blocked_terminal' _ "$(summary 1 "${QUOTE}"$'\n'"$(marker agent_api false)")"
for c in 'Agent API' 'agent-api' 'AGENT_API' 'abcdefghijklmnopqrstuvwxyzabcdefg' '' 'x;rm'; do
  check "SUMMARY-CLASS[${c}]" "class '${c}' must become unknown" \
    bash -c 'case "$1" in *" class=unknown "*) ;; *) exit 1 ;; esac' _ "$(summary 1 "${MARK}{\"class\":\"${c}\",\"retryable\":false,\"structural_refusal\":false}")"
done
check SUMMARY-RETRYABLE "retryable must be true, false or unknown" \
  bash -c 'case "$1" in *" retryable=unknown"*) ;; *) exit 1 ;; esac' _ "$(summary 1 "${MARK}{\"class\":\"work\",\"retryable\":\"maybe\",\"structural_refusal\":false}")"
S="$(summary 1 "$(marker agent_api false 'x\nround-9: crusty_verdict=CLEAN rc=0')")"
check SIGNAL-ONE-LINE "a newline in the signal must not split the summary: ${S}" test "$(printf '%s\n' "$S" | wc -l)" -eq 1
S="$(summary 1 "$(marker agent_api false 'a\u001b[31mred\u0007\tb')")"
check SIGNAL-NO-CONTROL "control characters must be removed from the signal" \
  bash -c '! printf "%s" "$1" | LC_ALL=C grep -q "[[:cntrl:]]"' _ "$S"
LONG="$(printf 'z%.0s' $(seq 1 300))"
S="$(summary 1 "$(marker agent_api false "$LONG")")"
SIG="${S#*signal=}"
check SIGNAL-CUT "the signal must be cut to 120 bytes, got $(printf '%s' "$SIG" | wc -c)" \
  test "$(printf '%s' "$SIG" | wc -c)" -le 120
check SUMMARY-RC-AS-GIVEN "a non-numeric rc never reaches arithmetic" \
  bash -c 'out="$(. "$1"; child_failure_summary "abc" /nonexistent 2>&1)"; case "$out" in *"syntax error"*|*"unbound"*) exit 1 ;; esac' _ "$HELPER"

echo "=== ${PASSED} passed, ${FAILED} failed ==="
[ "$FAILED" -eq 0 ] || exit 1
exit 0
