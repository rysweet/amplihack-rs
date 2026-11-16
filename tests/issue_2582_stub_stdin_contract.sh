#!/usr/bin/env bash
# issue_2582_stub_stdin_contract.sh — who may read stdin in the #1437 harness.
#
# Usage: bash tests/issue_2582_stub_stdin_contract.sh
#
# Issue #2582 moved the recipe context off the runner's argv and onto a file.
# The #1437 stub then started being called in modes it has no stand-in for
# (`orch helper autodrive-completion-seal`), and because it ran `BLOB="$(cat)"`
# *before* dispatching on the mode, it drained — and on an open stdin, blocked
# on — a channel it never uses. tests/issue_1437_autodrive_loop.sh reaches that
# call at its AMPLIHACK-BIN-RECORD-SEAL check and hung there. No cargo test
# covers these shell harnesses, so nothing caught it.
#
# Two separate contracts are specified here, because two separate defects were
# available and fixing either one alone hides the other:
#
#   W1  tests/issue_1437_amplihack_stub.sh must dispatch on the mode BEFORE it
#       opens stdin. A mode with no JSON blob answers without reading. The five
#       blob modes still drain, so a pipeline into them cannot see EPIPE.
#   W2  the `ab()` helper in tests/issue_1437_autodrive_loop.sh must hand each
#       bundled tool a stdin that is already at end of file, so no tool it
#       launches can block on the caller's terminal or pipe.
#
# How "did it read stdin?" is decided, with no timer and no risk of this test
# itself hanging: stdin is a REGULAR FILE holding a sentinel. `<&3` hands the
# child a duplicate of that open file, so parent and child share one file
# offset. After the child exits, the parent reads what is left. Nothing left
# means the child consumed stdin; the sentinel still there means it never
# looked. Because the file is at EOF from the start, even a reading mode
# terminates, so a failure here is a reported failure and never a hang.
# No errexit, and none is switched on later: a failing assertion must be counted
# and reported by ok(), never abort the run before the tally is printed.
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
STUB="${ROOT}/tests/issue_1437_amplihack_stub.sh"
LOOP="${ROOT}/tests/issue_1437_autodrive_loop.sh"
RECORD_TOOL="${ROOT}/amplifier-bundle/tools/autodrive_crusty_record.sh"
for f in "$STUB" "$LOOP" "$RECORD_TOOL"; do
  [ -f "$f" ] || { echo "ERROR: missing $f" >&2; exit 1; }
done

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
SENTINEL='STDIN-LEFTOVER-2582'

# The stub is mode 0644 in the repository and the alternate binary `exec`s it,
# so it is staged executable here just as the loop driver stages it (lines
# 48-50 there). Staging also keeps this test off the checked-in file's bit.
mkdir -p "${WORK}/bin"
cp "$STUB" "${WORK}/bin/amplihack" || exit 2
chmod 0755 "${WORK}/bin/amplihack"
STUB_EXE="${WORK}/bin/amplihack"

PASSED=0; FAILED=0
ok() { # ok <name> <detail> <cmd...>
  local name="$1" detail="$2"; shift 2
  if "$@"; then PASSED=$((PASSED + 1)); printf 'ok   %s\n' "$name"
  else FAILED=$((FAILED + 1)); printf 'FAIL %s — %s\n' "$name" "$detail"; fi
}

# run_mode <mode> [extra args...]
# Runs the stub in <mode> with a regular-file stdin holding $SENTINEL.
# Sets: RC, ERR, LEFT (what remained on stdin after the stub exited).
run_mode() {
  local mode="$1"; shift
  local d="${WORK}/m-$$-${RANDOM}"; mkdir -p "$d"
  printf '%s' "$SENTINEL" > "${d}/payload"
  exec 3<"${d}/payload"
  STUB_STATE="$d" bash "$STUB" orch helper "$mode" "$@" <&3 >/dev/null 2>"${d}/err"
  RC=$?
  LEFT="$(cat <&3)"
  exec 3<&-
  ERR="$(cat "${d}/err")"
}

echo "=== W1: a mode with no JSON blob answers without reading stdin ==="

# Every `orch helper` mode the tree actually calls that is NOT one of the five
# blob modes. autodrive-completion-seal is the one that hung the loop test; the
# rest share its shape, so the contract is stated once for all of them. The
# four *-workstreams / workflow / task modes are called with their own
# regular-file redirect, so not reading an inherited stdin is correct for them
# too. no-such-mode-2582 pins the answer for a mode nobody has written yet.
for mode in \
  autodrive-completion-seal \
  autodrive-completion-write \
  autodrive-completion-prepare \
  autodrive-completion-dispatch \
  step-recovery \
  modes \
  workflow-log-inventory \
  select-workflow \
  build-workstreams-config \
  count-workstreams \
  reclassify-task-type \
  no-such-mode-2582
do
  run_mode "$mode" --private-state "${WORK}/state-${mode}"
  ok "W1-UNREAD-${mode}" \
    "the stub consumed stdin for a mode it has no stand-in for (left='${LEFT}', rc=${RC})" \
    test "$LEFT" = "$SENTINEL"
  ok "W1-RC0-${mode}" "exit status was ${RC}, not 0 (stderr: ${ERR})" test "$RC" -eq 0
done

# `set -u` is on in the stub, and the gate exits before `$BLOB` is ever
# assigned. Pinned so that a dispatch which falls through to the stand-ins
# instead of exiting cannot reach them with the blob variable unset.
run_mode autodrive-completion-seal --private-state "${WORK}/unbound-probe"
ok W1-SET-U-CLEAN "a mode with no stand-in tripped set -u: ${ERR}" \
  bash -c '! printf "%s" "$1" | grep -qi "unbound variable"' _ "$ERR"

echo "=== W1: the five blob modes still drain stdin and still answer ==="

# normalise-loop-verdict, extract-json and extract-field have stand-ins, so
# their answers are pinned as a regression check.
d="${WORK}/blob"; mkdir -p "$d"
ok W1-BLOB-normalise-loop-verdict-known \
  "a known verdict was not passed through" \
  bash -c 'printf CONTINUE | STUB_STATE="$1" bash "$2" orch helper normalise-loop-verdict | grep -qxF CONTINUE' \
  _ "$d" "$STUB"
ok W1-BLOB-normalise-loop-verdict-unknown \
  "an unreadable verdict did not become STUCK" \
  bash -c 'printf unreadable | STUB_STATE="$1" bash "$2" orch helper normalise-loop-verdict | grep -qxF STUCK' \
  _ "$d" "$STUB"
ok W1-BLOB-extract-json \
  "a blob naming the required field was not passed through" \
  bash -c 'printf "%s" "{\"a\":\"b\"}" | STUB_STATE="$1" bash "$2" orch helper extract-json --require-field a | grep -qF "\"a\""' \
  _ "$d" "$STUB"
ok W1-BLOB-extract-json-missing \
  "a blob missing the required field did not become {}" \
  bash -c 'printf "%s" "{\"z\":\"b\"}" | STUB_STATE="$1" bash "$2" orch helper extract-json --require-field a | grep -qxF "{}"' \
  _ "$d" "$STUB"
ok W1-BLOB-extract-field \
  "a string field was not extracted" \
  bash -c 'printf "%s" "{\"a\":\"b\"}" | STUB_STATE="$1" bash "$2" orch helper extract-field --field a --default X | grep -qxF b' \
  _ "$d" "$STUB"
ok W1-BLOB-extract-field-default \
  "a missing field did not fall back to the default" \
  bash -c 'printf "%s" "{\"z\":\"b\"}" | STUB_STATE="$1" bash "$2" orch helper extract-field --field a --default X | grep -qxF X' \
  _ "$d" "$STUB"

# The blob modes must CONSUME stdin, not merely answer. A mode that is piped
# into and does not read leaves the writer with EPIPE.
for mode in extract-json extract-field normalise-loop-verdict; do
  run_mode "$mode" --field a --require-field a --default X
  ok "W1-DRAIN-${mode}" "a blob mode left stdin unread (left='${LEFT}')" test -z "$LEFT"
done

# normalise-verdict and normalise-type have no stand-in, but the tree pipes
# into both, so they must still drain. Dropping them from the drain list turns
# a clean pipeline into exit 141 under `set -o pipefail`.
#
# The pipefail check writes a payload LARGER THAN THE PIPE BUFFER (64 KiB on
# Linux, 16 KiB on macOS). With a few bytes the writer's single write() fits
# in the kernel buffer and returns before the reader is ever scheduled, so the
# pipeline ends 0 whether or not the mode reads — the assertion could not fail.
# Past the buffer the writer blocks, and a mode that does not drain leaves it
# to take SIGPIPE (exit 141) when the mode exits.
BIGBLOB="${WORK}/bigblob"
: > "$BIGBLOB"
for _ in $(seq 1 1024); do printf '%01024d' 0; done >> "$BIGBLOB"   # 1 MiB
for mode in normalise-verdict normalise-type; do
  run_mode "$mode"
  ok "W1-DRAIN-${mode}" \
    "a piped-into mode left stdin unread (left='${LEFT}'); its writer will take SIGPIPE" \
    test -z "$LEFT"
  ok "W1-PIPEFAIL-${mode}" "a pipeline into the mode did not end 0" \
    bash -c 'set -o pipefail; cat "$4" | STUB_STATE="$1" bash "$2" orch helper "$3" >/dev/null' \
    _ "$d" "$STUB" "$mode" "$BIGBLOB"
done

echo "=== W1: the REAL_AMPLIHACK passthrough still wins ==="

# The passthrough must stay ABOVE the new dispatch. If the mode case were
# checked first, a real binary would never be reached for a non-blob mode and
# STUB_REAL_HEALTH_ROOT mode would silently stop working.
PT="${WORK}/passthrough"; mkdir -p "$PT"
printf '%s\n' '#!/usr/bin/env bash' 'printf "REAL %s\n" "$*" >> "$PT_LOG"' 'exit 0' > "${PT}/real"
chmod 0755 "${PT}/real"
: > "${PT}/log"
PT_LOG="${PT}/log" REAL_AMPLIHACK="${PT}/real" STUB_STATE="$PT" \
  bash "$STUB" orch helper autodrive-completion-seal --private-state "${PT}/s" </dev/null >/dev/null 2>&1 || true
ok W1-PASSTHROUGH-NON-BLOB \
  "a non-blob mode did not reach REAL_AMPLIHACK: $(tr '\n' ';' < "${PT}/log")" \
  grep -qF 'REAL orch helper autodrive-completion-seal' "${PT}/log"
: > "${PT}/log"
PT_LOG="${PT}/log" REAL_AMPLIHACK="${PT}/real" STUB_STATE="$PT" \
  bash "$STUB" orch helper extract-json --require-field a </dev/null >/dev/null 2>&1 || true
ok W1-PASSTHROUGH-BLOB \
  "a blob mode did not reach REAL_AMPLIHACK" \
  grep -qF 'REAL orch helper extract-json' "${PT}/log"

echo "=== W2: ab() hands each bundled tool a stdin already at end of file ==="

# The real ab() definition is lifted out of the loop driver and run here, so
# this is the shipped code path and not a copy of it. A probe standing in for a
# bundled tool reports whether its stdin was at EOF.
AB_SRC="$(sed -n '/^ab() {/,/^}/p' "$LOOP")"
ok W2-AB-EXTRACTED "ab() could not be read out of the loop driver" \
  bash -c 'printf "%s" "$1" | grep -q "bash \"\${TOOLS}/\$1\""' _ "$AB_SRC"

W2="${WORK}/w2"; mkdir -p "${W2}/tools" "${W2}/ab/path" "${W2}/ab/state" "${W2}/work/bin" "${W2}/work/home"
# The probe records that it RAN before it records what it read. Without the
# first line, a probe that never starts is indistinguishable from a probe that
# read nothing, and `test ! -s probe-saw` below would pass on a missing file —
# i.e. the assertion could not fail. W2-PROBE-RAN is that positive control.
printf '%s\n' '#!/usr/bin/env bash' 'printf ran > "$PROBE_RAN"' 'cat > "$PROBE_OUT"' \
  > "${W2}/tools/probe_stdin.sh"
printf '%s' "$SENTINEL" > "${W2}/payload"
# Pre-seed the output with the sentinel, so "empty" can only be the probe
# having truncated it on an EOF stdin, never the file being absent.
printf '%s' "$SENTINEL" > "${W2}/probe-saw"
(
  set +u
  WORK="${W2}/work"; AB="${W2}/ab"; TOOLS="${W2}/tools"
  eval "$AB_SRC"
  exec 3<"${W2}/payload"
  PROBE_RAN="${W2}/probe-ran" PROBE_OUT="${W2}/probe-saw" ab probe_stdin.sh <&3
  printf '%s' "$?" > "${W2}/probe-rc"
  printf '%s' "$(cat <&3)" > "${W2}/probe-left"
  exec 3<&-
)
ok W2-PROBE-RAN \
  "ab() never launched the probe, so the stdin assertions below prove nothing" \
  test -s "${W2}/probe-ran"
ok W2-PROBE-RC0 \
  "ab() returned $(cat "${W2}/probe-rc" 2>/dev/null) for the probe" \
  test "$(cat "${W2}/probe-rc" 2>/dev/null)" = 0
ok W2-PROBE-SAW-EOF \
  "a tool launched by ab() inherited the caller's stdin and read '$(cat "${W2}/probe-saw" 2>/dev/null)' from it" \
  test ! -s "${W2}/probe-saw"
ok W2-CALLER-STDIN-INTACT \
  "ab() consumed the caller's stdin (left='$(cat "${W2}/probe-left" 2>/dev/null)')" \
  grep -qxF "$SENTINEL" "${W2}/probe-left"

echo "=== integration: the real record tool seals without touching stdin ==="

# The shipped amplifier-bundle/tools/autodrive_crusty_record.sh, against the
# stub, with a stdin that holds a sentinel. This is the call the loop test hung
# on, exercised through the real tool rather than a re-statement of it.
IT="${WORK}/it"; mkdir -p "${IT}/state"
printf '%s\n' '#!/usr/bin/env bash' 'printf "BIN %s\n" "$*" >> "$AB_LOG"' \
  'case "${1:-} ${2:-}" in' \
  '  "orch helper") STUB_STATE="$AB_STATE" exec "$AB_STUB" "$@" ;;' \
  '  "internal review-bind-record") cat -- "$4"; exit 0 ;;' \
  'esac' 'exit 0' > "${IT}/alt"
chmod 0755 "${IT}/alt"
: > "${IT}/log"
printf '%s' "$SENTINEL" > "${IT}/payload"
SHA40="$(printf 'a%.0s' $(seq 40))"
exec 3<"${IT}/payload"
AB_LOG="${IT}/log" AB_STATE="${IT}/state" AB_STUB="$STUB_EXE" REAL_AMPLIHACK="" \
  AMPLIHACK_BIN="${IT}/alt" \
  AUTODRIVE_ROUND_RECORD="${IT}/record.json" REVIEW_DIR="${IT}/gen" AUTODRIVE_ROUND_LABEL=round-1 \
  RECIPE_VAR_crusty_verdict='{"crusty_verdict":"CLEAN","concern_count":0}' \
  RECIPE_VAR_crusty_round_context="{\"head_sha\":\"${SHA40}\"}" \
  RECIPE_VAR_crusty_fix_evidence='{"commits":0}' \
  AUTODRIVE_COMPLETION_STATE="${IT}/completion" \
  bash "$RECORD_TOOL" <&3 >/dev/null 2>"${IT}/err"
IT_RC=$?
IT_LEFT="$(cat <&3)"
exec 3<&-
ok INT-RECORD-RC0 "the record tool exited ${IT_RC} (stderr: $(tail -n 2 "${IT}/err"))" \
  test "$IT_RC" -eq 0
ok INT-RECORD-SEAL "the completion seal did not reach \$AMPLIHACK_BIN: $(tr '\n' ';' < "${IT}/log")" \
  grep -qxF "BIN orch helper autodrive-completion-seal --private-state ${IT}/completion" "${IT}/log"
ok INT-RECORD-STDIN-INTACT \
  "the record tool's run consumed its caller's stdin (left='${IT_LEFT}'); on an open stdin it blocks instead" \
  test "$IT_LEFT" = "$SENTINEL"
ok INT-RECORD-WRITTEN "no round record was written" test -s "${IT}/record.json"

echo "=== ${PASSED} passed, ${FAILED} failed ==="
[ "$FAILED" -eq 0 ] || exit 1
exit 0
