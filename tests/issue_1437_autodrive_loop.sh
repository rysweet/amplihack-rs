#!/usr/bin/env bash
# issue_1437_autodrive_loop.sh — autodrive_loop.sh end to end: an agent/API
# failure is not a policy refusal, and a real refusal still stops the loop.
#
# Issue #1437: the merge-ready builder agent committed and pushed its work and
# then lost its Copilot session (`Execution failed: 400 ...`). The loop grepped
# the round transcript, found `BLOCKED_TERMINAL` in files the agent had read and
# in a refusal a descendant had already handled, and exited 79 for good.
#
# All three call sites (before-round, round, loop-health-evaluator) now ask
# terminal_refusal in autodrive_refusal.sh: exit 79, or the runner's LAST
# failure_class marker with "structural_refusal":true. Nothing else is terminal.
# The helpers alone are tests/issue_1437_autodrive_refusal.sh.
#
# Issue #2096: each loop run owns a private review checkout (internal
# review-allocate / -next / -launch / -head, released once by the EXIT trap).
# Section 0 checks the stub's stand-ins for those commands; 0b and 0c check
# that the loop releases exactly once and stays fail-closed when they fail.
#
# The loop runs as the merge-ready loop. Under `--loop-name crusty` the
# evaluator site is not a `recipe run`: `autodrive-completion-dispatch` (#2336)
# starts the evaluator itself and classifies its exit with the same two
# signals; that is tested in orch/autodrive_completion/evaluator.rs
# (`only_exit_79_or_the_final_marker_is_a_refusal`) and
# crates/amplihack-cli/tests/issue_2336_autodrive_completion_failures.rs.
#
# Usage: bash tests/issue_1437_autodrive_loop.sh
# Set ISSUE_1437_JSON_HELPER=fallback to exercise the POSIX stand-in.
# Exit codes: 0 = pass, 1 = fail, 2 = harness error.

set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TOOLS="${ROOT}/amplifier-bundle/tools"

PASSED=0; FAILED=0
pass() { PASSED=$((PASSED + 1)); echo "  PASS[$1]"; }
fail() { FAILED=$((FAILED + 1)); echo "  FAIL[$1]: $2" >&2; }
ok() { # ok <id> <message> <command...>
  local id="$1" msg="$2"; shift 2
  if "$@"; then pass "$id"; else fail "$id" "$msg"; fi
}

echo "=== issue #1437: autodrive_loop.sh — not every failure is terminal ==="
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
WORK="$(cd "$WORK" && pwd -P)"
mkdir -p "${WORK}/bin"
cp "${ROOT}/tests/issue_1437_amplihack_stub.sh" "${WORK}/bin/amplihack" || exit 2
chmod 0755 "${WORK}/bin/amplihack"
REAL_AMPLIHACK=""
for c in "${ROOT}/target/debug/amplihack" "${ROOT}/target/release/amplihack" "$(command -v amplihack 2>/dev/null || true)"; do
  [ "${ISSUE_1437_JSON_HELPER:-}" = fallback ] && break
  [ -n "$c" ] && [ -x "$c" ] && printf '{"a":"b"}' | "$c" orch helper extract-json --require-field a >/dev/null 2>&1 \
    && { REAL_AMPLIHACK="$c"; break; }
done
export REAL_AMPLIHACK
echo "  (JSON helper: ${REAL_AMPLIHACK:-POSIX stand-in})"

# The review dir must be the main checkout of --repo's repository (#1528).
MAIN="${WORK}/main"; WT="${WORK}/worktree"
g() { HOME="$WORK" GIT_CONFIG_NOSYSTEM=1 git -C "$MAIN" -c user.name=t -c user.email=t@t "$@"; }
{ mkdir -p "$MAIN" && g init -q \
  && g update-ref HEAD "$(g commit-tree "$(g write-tree)" -m init)" \
  && g worktree add -q --detach "$WT"; } >/dev/null 2>&1 \
  || { echo "HARNESS-ERROR: could not create a main checkout and its worktree" >&2; exit 2; }
printf 'drive the PR to merge\n' > "${WORK}/task.txt"

STUB_KNOBS=""
stub() { # stub NAME=value ... — knobs for the next run only
  local kv
  for kv in "$@"; do export "${kv?}"; STUB_KNOBS="${STUB_KNOBS} ${kv%%=*}"; done
}
CASE=""; RC=0; OUT=""; ERR=""
run() { # run <case> [loop-script] [extra loop args...]
  CASE="${WORK}/case-$1"; local loop="${2:-${TOOLS}/autodrive_loop.sh}"
  shift; [ $# -gt 0 ] && shift
  mkdir -p "${CASE}/stub"
  RC=0
  ( cd "$MAIN" && STUB_STATE="${CASE}/stub" PATH="${WORK}/bin:${PATH}" AMPLIHACK_BIN="${WORK}/bin/amplihack" \
      bash "$loop" --loop-name merge-ready --round-recipe autodrive-merge-round \
        --clean-token MERGE_READY --verdict-field merge_ready_verdict \
        --repo "$WT" --review-dir "$MAIN" --state-dir "${CASE}/state" --task-file "${WORK}/task.txt" "$@" ) \
    >"${CASE}/out" 2>"${CASE}/err" || RC=$?
  OUT="$(cat "${CASE}/out")"; ERR="$(cat "${CASE}/err")"
  local k; for k in $STUB_KNOBS; do unset "$k"; done; STUB_KNOBS=""
}
calls() { local n; n="$(grep -cx "$1" "${CASE}/stub/calls" 2>/dev/null)" || true; printf '%s' "${n:-0}"; }
has() { printf '%s' "$1" | grep -qF -- "$2"; }
lacks() { ! has "$@"; }
json_lines() { # every stdout line is one JSON object (SR-6)
  command -v jq >/dev/null 2>&1 || return 0
  local line
  while IFS= read -r line; do printf '%s' "$line" | jq -e 'type == "object"' >/dev/null 2>&1 || return 1; done <<<"$OUT"
}
# #2096: the loop owns a review checkout (internal review-*); the stub logs
# each call to $STUB_STATE/review-calls and counts releases in review-release.
releases() { local n; n="$(cat "${CASE}/stub/review-release" 2>/dev/null)" || true; printf '%s' "${n:-0}"; }
review_calls() { local n; n="$(grep -c "^$1\( \|$\)" "${CASE}/stub/review-calls" 2>/dev/null)" || true; printf '%s' "${n:-0}"; }

# --- 0. the test double's internal review-* contract (#2096) ------------------
# The stand-ins accept exactly the five commands the loop calls, with exact
# argument counts. Anything else is unhandled (exit 3): no catch-all success.
SC="${WORK}/stub-contract"; mkdir -p "$SC"
sc() { STUB_STATE="$SC" "${WORK}/bin/amplihack" internal "$@"; }
sc_rc() { local rc=0; sc "$@" >/dev/null 2>&1 || rc=$?; printf '%s' "$rc"; }
ok STUB-ALLOCATE "review-allocate REPO STATE prints REPO" test "$(sc review-allocate "$WT" "${SC}/state" 2>/dev/null)" = "$WT"
ok STUB-ALLOCATE-NOT-MAIN "review-allocate never returns the shared main checkout" test "$(sc review-allocate "$WT" "${SC}/state" 2>/dev/null)" != "$MAIN"
ok STUB-NEXT "review-next REPO CHECKOUT prints CHECKOUT" test "$(sc review-next "$WT" "$WT" 2>/dev/null)" = "$WT"
ok STUB-LAUNCH "review-launch CHECKOUT prints CHECKOUT" test "$(sc review-launch "$WT" 2>/dev/null)" = "$WT"
ok STUB-HEAD "review-head CHECKOUT prints its HEAD SHA" \
  test "$(sc review-head "$WT" 2>/dev/null)" = "$(git -C "$WT" rev-parse --verify HEAD)"
ok STUB-HEAD-FAILS "review-head on a non-repository passes git's failure through" \
  test "$(sc_rc review-head "${WORK}/no-such-checkout")" -ne 0
ok STUB-RELEASE-SILENT "review-release CHECKOUT prints nothing and exits 0" \
  bash -c 'out="$("$@" 2>&1)" && test -z "$out"' _ env STUB_STATE="$SC" "${WORK}/bin/amplihack" internal review-release "$WT"
ok STUB-RELEASE-COUNTS "review-release increments the release count: $(cat "${SC}/review-release" 2>/dev/null)" \
  test "$(cat "${SC}/review-release" 2>/dev/null)" = 1
ok STUB-LOGS "every review-* call is logged to review-calls" \
  test "$(grep -c '^review-' "${SC}/review-calls" 2>/dev/null || echo 0)" -ge 6
ok STUB-UNKNOWN-INTERNAL "an unknown internal subcommand is unhandled (3)" test "$(sc_rc review-everything "$WT")" -eq 3
ok STUB-ALLOCATE-ARGC "review-allocate with one argument is unhandled (3)" test "$(sc_rc review-allocate "$WT")" -eq 3
ok STUB-NEXT-ARGC "review-next with three arguments is unhandled (3)" test "$(sc_rc review-next "$WT" "$WT" extra)" -eq 3
ok STUB-RELEASE-ARGC "review-release with no argument is unhandled (3)" test "$(sc_rc review-release)" -eq 3
ok STUB-ALLOCATE-KNOB "STUB_REVIEW_ALLOCATE_RC makes review-allocate fail" \
  test "$(STUB_REVIEW_ALLOCATE_RC=1 sc_rc review-allocate "$WT" "${SC}/state")" -eq 1
ok STUB-RELEASE-KNOB "STUB_REVIEW_RELEASE_RC makes review-release fail" \
  test "$(STUB_REVIEW_RELEASE_RC=1 sc_rc review-release "$WT")" -eq 1

# --- 0b. the default drive: NOT_READY, then MERGE_READY, ends DONE (#2096) ----
run default
ok DEFAULT-DONE "rc=${RC}: the default drive must reach DONE: out=${OUT} err=$(tail -n 3 "${CASE}/err")" \
  bash -c 'test "$1" -eq 0 && printf "%s" "$2" | grep -qF "\"loop_result\":\"DONE\""' _ "$RC" "$OUT"
ok DEFAULT-ROUNDS "two rounds run: rounds=$(calls autodrive-merge-round)" test "$(calls autodrive-merge-round)" -eq 2
ok DEFAULT-ALLOCATES-ONCE "one review checkout per loop run: $(review_calls review-allocate)" test "$(review_calls review-allocate)" -eq 1
ok DEFAULT-NEXT-PER-ROUND "review-next, -launch and -head run once per round" \
  test "$(review_calls review-next):$(review_calls review-launch):$(review_calls review-head)" = "2:2:2"
ok DEFAULT-RELEASE-ONCE "the EXIT trap releases the review checkout exactly once (0 = leak, 2 = double release): $(releases)" \
  test "$(releases)" = 1
ok DEFAULT-LAUNCH-NOT-MAIN "no round launches from the shared main checkout" \
  bash -c '! grep -qxF "review-launch $1" "$2"' _ "$MAIN" "${CASE}/stub/review-calls"

# --- 0c. review checkout failures stay fail-closed (#2096) --------------------
stub STUB_REVIEW_ALLOCATE_RC=1
run allocate-fails
ok ALLOCATE-FAILS-RC "rc=${RC}: a failed review-allocate must stop the loop non-zero" test "$RC" -ne 0
ok ALLOCATE-FAILS-NO-ROUND "no round runs without an owned review checkout" test "$(calls autodrive-merge-round)" -eq 0
ok ALLOCATE-FAILS-NO-RELEASE "nothing is released that was never allocated" test "$(releases)" = 0
ok ALLOCATE-FAILS-NOT-DONE "it must not claim DONE" lacks "$OUT" '"loop_result":"DONE"'
stub STUB_REVIEW_RELEASE_RC=1
run release-fails
ok RELEASE-FAILS-RC "rc=${RC}: a failed review-release turns a DONE run into exit 1" test "$RC" -eq 1
ok RELEASE-FAILS-ATTEMPTED "the release was attempted once" test "$(review_calls review-release)" -eq 1

MARK='amplihack.recipe.failure_class '
marker() { # marker <class> <structural> [signal]
  printf '%s{"schema_version":1,"issue":1267,"class":"%s","signal":"%s","action":"terminal","attempt":1,"retryable":false,"structural_refusal":%s}' \
    "$MARK" "$1" "${3:-execution failed: 400}" "$2"
}
QUOTE='BLOCKED_TERMINAL orchestration_unavailable: depth 3 of max 3 (issue #964/#1326).'
# The incident transcript: repository files the agent read, a refusal a
# descendant got and the agent handled inline, then the session died.
INCIDENT="● Search (grep) \"BLOCKED_TERMINAL\" (amplifier-bundle/recipes/tests)
  test-issue-1337-loop-health-evaluator.sh:20:#   - exit 79 / BLOCKED_TERMINAL is TERMINAL
${QUOTE}
  [06:56:22] [amplihack:copilot:1] ${QUOTE}
I completed the work inline as instructed, committed 9d521815 and pushed.
Execution failed: 400 The resource you requested was not found."

# --- 1. the incident: rc=1, quoted refusal, agent_api marker ----------------
stub "STUB_REAL_HEALTH_ROOT=$ROOT" STUB_RC_1=1 "STUB_OUT_1=${INCIDENT}
$(marker agent_api false)"
run incident
ok INCIDENT-NOT-79 "rc=${RC}: an agent/API failure must not exit 79" test "$RC" -ne 79
ok INCIDENT-NO-REFUSAL "no TERMINAL_POLICY_REFUSAL may be claimed" lacks "$ERR" TERMINAL_POLICY_REFUSAL
ok INCIDENT-CONTINUES "the loop must run another round; rounds=$(calls autodrive-merge-round)" test "$(calls autodrive-merge-round)" -eq 2
ok INCIDENT-DONE "the run must reach DONE: rc=${RC} out=${OUT}" has "$OUT" '"loop_result":"DONE"'
ok INCIDENT-INFO "the loop must name the failure: $(grep '^INFO:.*child failed' "${CASE}/err")" \
  grep -qE '^INFO: .*child failed: rc=1 class=agent_api retryable=false signal=execution failed: 400; not a policy refusal, continuing$' "${CASE}/err"
ok INCIDENT-HISTORY "round-1's HISTORY entry must carry child=: $(cat "${CASE}/stub/history-1" 2>/dev/null)" \
  grep -qsE '^round-1: .* child=rc=1 class=agent_api retryable=false' "${CASE}/stub/history-1"
ok INCIDENT-CLEAN-ROUND "a round that exited 0 has no child= entry" \
  bash -c 'test -f "$1" && ! grep -qE "^round-2: .*child=" "$1"' _ "${CASE}/stub/history-2"

ok INCIDENT-AGENT-EVALUATES "the real collector must allow agentic evaluation after the API failure" test -f "${CASE}/stub/health-agent-called-1"

# --- 1b. rc=1 with a quote and no marker at all ------------------------------
stub STUB_RC_1=1 "STUB_OUT_1=${QUOTE}"
run quote-no-marker
ok QUOTE-NO-MARKER "rc=${RC}: quoted text with no marker must not stop the loop" test "$(calls autodrive-merge-round)" -eq 2
ok QUOTE-UNKNOWN "an unmarked failure is class=unknown" grep -qE 'child failed: rc=1 class=unknown retryable=unknown' "${CASE}/err"

# --- 2. rc=79 is terminal ---------------------------------------------------------
stub STUB_RC_1=79 "STUB_OUT_1=${QUOTE}"
run rc79
ok RC79 "rc=${RC}: exit 79 must stop the loop with 79" test "$RC" -eq 79
ok RC79-ESCALATES "a refusal is escalated by name" has "$OUT" '"loop_result":"TERMINAL_POLICY_REFUSAL"'
ok RC79-WHY "the reason must say exit 79: ${OUT}" has "$OUT" 'exit 79'
ok RC79-NO-RETRY "no further round and no evaluator call" test "$(calls autodrive-merge-round):$(calls loop-health-evaluator)" = "1:0"
ok RC79-RELEASES "a refusal exit still releases the review checkout once (#2096): $(releases)" test "$(releases)" = 1

# --- 3. rc=1 with structural_refusal:true is terminal ------------------------------
stub STUB_RC_1=1 "STUB_OUT_1=  ✗ step-04: failed
$(marker work true 'blocked_terminal orchestration_unavailable')"
run marker-true
ok MARKER-TRUE "rc=${RC}: the runner's structural_refusal must stop the loop with 79" test "$RC" -eq 79
ok MARKER-WHY "the reason must name the marker: ${OUT}" has "$OUT" 'structural_refusal in failure_class marker'
ok MARKER-NO-EVAL "no evaluator call after a refusal" test "$(calls loop-health-evaluator)" -eq 0

# A string with the same spelling must not manufacture a terminal refusal.
stub STUB_RC_1=1 "STUB_OUT_1=$(marker agent_api '"true"')"
run marker-string-true
ok STRING-TRUE-CONTINUES "a JSON string must allow another round" test "$(calls autodrive-merge-round)" -eq 2
ok STRING-TRUE-NO-REFUSAL "a JSON string must not claim a policy refusal" lacks "$OUT$ERR" TERMINAL_POLICY_REFUSAL
ok STRING-TRUE-DONE "a JSON string must reach DONE" has "$OUT" '"loop_result":"DONE"'

# --- 4. the before-round call site -------------------------------------------------
BEFORE="${WORK}/before.sh"
printf '%s\n' '#!/usr/bin/env bash' 'printf "%s\n" "${BEFORE_STDERR:-}" >&2' 'exit "${BEFORE_RC:-0}"' > "$BEFORE"
stub BEFORE_RC=79 "BEFORE_STDERR=${QUOTE}"
run before-79 "" --before-round "$BEFORE"
ok BEFORE-79 "rc=${RC}: a before-round exit 79 must stop with 79" test "$RC" -eq 79
ok BEFORE-79-NO-ROUND "no round may run after it" test "$(calls autodrive-merge-round)" -eq 0
ok BEFORE-79-WHY "the reason must say exit 79, not exit-79: ${OUT}" bash -c 'printf "%s" "$1" | grep -q "exit 79" && ! printf "%s" "$1" | grep -q "exit-79"' _ "$OUT"
stub BEFORE_RC=1 "BEFORE_STDERR=${QUOTE}"
run before-quote "" --before-round "$BEFORE"
ok BEFORE-QUOTE-RC "rc=${RC}: a before-round exit 1 that quotes the refusal ends with exit 1" test "$RC" -eq 1
ok BEFORE-QUOTE-FAILED "it is BEFORE_ROUND_FAILED: ${OUT}" has "$OUT" '"loop_result":"BEFORE_ROUND_FAILED"'
ok BEFORE-QUOTE-NOT-REFUSAL "it is not a policy refusal" lacks "$OUT$ERR" TERMINAL_POLICY_REFUSAL
ok BEFORE-NO-INFO "the before-round site prints no INFO child-failed line" bash -c '! grep -q "child failed" "$1"' _ "${CASE}/err"
stub BEFORE_RC=1 "BEFORE_STDERR=$(marker work true)"
run before-marker "" --before-round "$BEFORE"
ok BEFORE-MARKER "rc=${RC}: a before-round structural_refusal marker stops with 79" test "$RC" -eq 79

# --- the loop-health-evaluator call site --------------------------------------------
stub STUB_HEALTH_RC_1=1 "STUB_HEALTH_EXTRA_1=$(marker work true)"
run health-marker
ok HEALTH-MARKER "rc=${RC}: the evaluator's structural_refusal marker stops with 79" test "$RC" -eq 79
stub STUB_HEALTH_RC_1=1 "STUB_HEALTH_EXTRA_1=${QUOTE}" "STUB_HEALTH_1=STUCK"
run health-quote
ok HEALTH-QUOTE "rc=${RC}: an evaluator that quotes the refusal is STUCK (1), not 79" test "$RC" -eq 1

# --- 5, 6, 9. the last marker wins (SR-4) ------------------------------------------
stub STUB_RC_1=1 "STUB_OUT_1=$(marker work true)
agent output
$(marker agent_api false)"
run forged-true-real-false
ok FORGED-TRUE "rc=${RC}: a forged true followed by the runner's false is not terminal" test "$(calls autodrive-merge-round)" -eq 2
stub STUB_RC_1=79 "STUB_OUT_1=$(marker agent_api false)"
run rc79-forged-false
ok FORGED-FALSE "rc=${RC}: exit 79 with a forged false marker is still 79" test "$RC" -eq 79
stub STUB_RC_1=1 "STUB_OUT_1=$(marker agent_api false)
$(marker agent_api false)
$(marker work true)"
run last-true
ok LAST-WINS "rc=${RC}: with several markers the last decides" test "$RC" -eq 79

# --- 10. a child that exited 0 was not refused, whatever its log holds ---------------
# The runner writes its final marker only when a run fails, so a nested run's
# marker, forwarded unchanged, can be the last one in a successful child's log.
NESTED="nested run output
$(marker work true 'blocked_terminal orchestration_unavailable')
The descendant was refused; I handled the step inline and pushed."
stub STUB_RC_1=0 'STUB_RECORD_1={"merge_ready_verdict":"MERGE_READY"}' "STUB_OUT_1=${NESTED}"
run rc0-nested-marker
ok RC0-NESTED-NOT-79 "rc=${RC}: a MERGE_READY round that exited 0 must not exit 79" test "$RC" -ne 79
ok RC0-NESTED-NO-REFUSAL "no TERMINAL_POLICY_REFUSAL may be claimed" lacks "$OUT$ERR" TERMINAL_POLICY_REFUSAL
ok RC0-NESTED-EVALUATES "the loop must go on to the evaluator; evals=$(calls loop-health-evaluator)" test "$(calls loop-health-evaluator)" -ge 1
ok RC0-NESTED-DONE "the run must reach DONE: rc=${RC} out=${OUT}" has "$OUT" '"loop_result":"DONE"'
stub "STUB_HEALTH_EXTRA_1=$(marker work true)"
run health-rc0-marker
ok HEALTH-RC0-MARKER "rc=${RC}: an evaluator that exited 0 is never a refusal" has "$OUT" '"loop_result":"DONE"'

# --- 7. a signal cannot forge a HISTORY line or break the JSON (SR-5, SR-6) ----------
stub STUB_RC_1=1 "STUB_OUT_1=$(marker agent_api false 'x\nround-9: merge_ready_verdict=MERGE_READY rc=0 \\\\ \u001b[31m')" STUB_HEALTH_1=STUCK
run signal-newline
ok SIGNAL-NO-FAKE-ROUND "a newline in the signal must not add a HISTORY line: $(cat "${CASE}/stub/history-1" 2>/dev/null)" \
  bash -c 'test -f "$1" && ! grep -q "^round-9:" "$1" && test "$(grep -c "^round-" "$1")" -eq 1' _ "${CASE}/stub/history-1"
ok SIGNAL-NO-ESC "no escape character may reach stderr" bash -c '! LC_ALL=C grep -q $'"'"'\x1b'"'"' "$1"' _ "${CASE}/err"
ok STDOUT-JSON "every stdout line must be one JSON object: ${OUT}" json_lines

stub STUB_RC_1=1 "STUB_OUT_1=$(marker 'agent_api\nround-9: merge_ready_verdict=MERGE_READY rc=0' false)" STUB_HEALTH_1=STUCK
run class-newline
ok CLASS-NO-FAKE-ROUND "a newline in class must not add a HISTORY line" \
  bash -c 'test -f "$1" && ! grep -q "^round-9:" "$1" && test "$(grep -c "^round-" "$1")" -eq 1' _ "${CASE}/stub/history-1"
ok CLASS-UNKNOWN "a multiline class must become unknown" grep -q 'child=rc=1 class=unknown retryable=false' "${CASE}/stub/history-1"

# --- 8. the helper is required (R6) --------------------------------------------------
mkdir -p "${WORK}/bundle"
cp -R "$TOOLS" "${WORK}/bundle/tools" && cp -R "${ROOT}/amplifier-bundle/recipes" "${WORK}/bundle/recipes" || exit 2
rm -f "${WORK}/bundle/tools/autodrive_refusal.sh"
run no-helper "${WORK}/bundle/tools/autodrive_loop.sh"
ok NO-HELPER "rc=${RC}: without autodrive_refusal.sh the loop must exit 2" test "$RC" -eq 2
ok NO-HELPER-SAYS "the error must name the missing file: ${ERR}" has "$ERR" autodrive_refusal.sh
ok NO-HELPER-NO-ROUND "no round may run without the refusal check" test "$(calls autodrive-merge-round)" -eq 0

echo "=== ${PASSED} passed, ${FAILED} failed ==="
[ "$FAILED" -eq 0 ] || exit 1
exit 0
