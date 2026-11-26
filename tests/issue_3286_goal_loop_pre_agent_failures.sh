#!/usr/bin/env bash
# issue_3286_goal_loop_pre_agent_failures.sh: the goal loop ends a run whose
# turns keep failing, with the same error, before any agent starts.
#
# Issue #3286: every turn of three build runs failed the trusted-bundle check
# before an agent started. The loop started the next turn at once, more than a
# thousand times per run, and wrote nothing to the run's own log. Now three
# such turns in a row end the run with a non-zero exit and the error, a turn in
# which an agent ran starts the count again, and each failed turn writes one
# line, with its number and the error, to the run's log
# (AMPLIHACK_RECIPE_LOG).
#
# "No agent started" is read only from the runner's failure-class marker when
# it lists no completed and no failed step. A turn with no marker, or with one
# that does not list both, is unknown: an agent may have run and been killed
# before the runner wrote a marker. Such a turn never counts and starts the
# count again (crusty review of PR #3293).
#
# The cases run the REAL driver from a copy of this tree's bundle, with the
# #1437 stub as `amplihack` and a fixture goal tool that never calls gh. The
# fixture goal's `final` reading ends every run after eight turns, so a loop
# that does not end on its own is a reported failure, never a hang. Each check
# reads exit codes, counts and the fixture's own strings, never the loop's
# wording.
#
# Usage: bash tests/issue_3286_goal_loop_pre_agent_failures.sh
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

echo "=== issue #3286: the goal loop ends on a repeated failure before any agent starts ==="
command -v jq >/dev/null 2>&1 || { echo "HARNESS-ERROR: jq is required (the loop driver needs it)" >&2; exit 2; }
WORK="$(mktemp -d)" || exit 2
trap 'rm -rf "$WORK"' EXIT
WORK="$(cd "$WORK" && pwd -P)"
mkdir -p "${WORK}/bin" "${WORK}/home"
cp "${ROOT}/tests/issue_1437_amplihack_stub.sh" "${WORK}/bin/amplihack" || exit 2
chmod 0755 "${WORK}/bin/amplihack"
# gh is never needed; one first on PATH refuses, so nothing reaches GitHub.
printf '%s\n' '#!/usr/bin/env bash' 'echo "issue_3286: gh must not be called: $*" >&2' 'exit 97' > "${WORK}/bin/gh"
chmod 0755 "${WORK}/bin/gh"
REAL_AMPLIHACK=""
for c in "${ROOT}/target/debug/amplihack" "${ROOT}/target/release/amplihack" "$(command -v amplihack 2>/dev/null || true)"; do
  [ "${ISSUE_1437_JSON_HELPER:-}" = fallback ] && break
  [ -n "$c" ] && [ -x "$c" ] && printf '{"a":"b"}' | "$c" orch helper extract-json --require-field a >/dev/null 2>&1 \
    && { REAL_AMPLIHACK="$c"; break; }
done
export REAL_AMPLIHACK
echo "  (JSON helper: ${REAL_AMPLIHACK:-POSIX stand-in})"

MAIN="${WORK}/main"; WT="${WORK}/worktree"
g() { HOME="$WORK" GIT_CONFIG_NOSYSTEM=1 git -C "$MAIN" -c user.name=t -c user.email=t@t "$@"; }
{ mkdir -p "$MAIN" && g init -q \
  && g update-ref HEAD "$(g commit-tree "$(g write-tree)" -m init)" \
  && g worktree add -q --detach "$WT"; } >/dev/null 2>&1 \
  || { echo "HARNESS-ERROR: could not create a main checkout and its worktree" >&2; exit 2; }
printf 'build the change\n' > "${WORK}/task.txt"

# A copy of this tree's bundle plus the fixture goal. `final` ends the run
# once FIXTURE_END_AFTER turns (default 8) have run.
BUNDLE="${WORK}/bundle"
{ mkdir -p "$BUNDLE" && cp -R "$TOOLS" "${BUNDLE}/tools" && cp -R "${ROOT}/amplifier-bundle/recipes" "${BUNDLE}/recipes"; } \
  || { echo "HARNESS-ERROR: could not copy the bundle" >&2; exit 2; }
printf 'The #3286 fixture goal.\n' > "${BUNDLE}/goal.md"
cat > "${BUNDLE}/tools/issue_3286_goal.sh" <<'GOAL'
#!/usr/bin/env bash
# The #3286 fixture goal (goal contract v1): one action, no gh.
[ "${AUTODRIVE_GOAL_CONTRACT:-}" = 1 ] || exit 2
case "${1:-}" in
  actions) echo poke ;;
  final) n="$(cat "${STUB_STATE:?}/rounds" 2>/dev/null || echo 0)"
         [ "$n" -lt "${FIXTURE_END_AFTER:-8}" ] || echo fixture-end ;;
  read) printf '{"schema_version":1,"summary":"the #3286 fixture goal"}\n' > "$2/goal.json" ;;
  act) printf '{"outcome":"poked","summary":"the #3286 fixture action"}\n' > "$3/result.json" ;;
  publish) : ;;
  *) exit 2 ;;
esac
GOAL

STUB_KNOBS=""
stub() { # stub NAME=value ... : knobs for the next run only
  local kv
  for kv in "$@"; do export "${kv?}"; STUB_KNOBS="${STUB_KNOBS} ${kv%%=*}"; done
}
turn_fails() { # turn_fails <n> <output>: turn n exits 1 and prints <output>
  stub "STUB_RC_$1=1" "STUB_OUT_$1=$2"
}
CASE=""; RC=0; OUT=""; LOG=""
run() { # run <case>: the loop, with no --once, and a run log of its own
  CASE="${WORK}/case-$1"; LOG="${CASE}/recipe.log"
  mkdir -p "${CASE}/stub"
  ( umask 077 && : > "$LOG" )
  RC=0
  ( cd "$MAIN" && HOME="${WORK}/home" GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 STUB_STATE="${CASE}/stub" \
      PATH="${WORK}/bin:${PATH}" AMPLIHACK_BIN="${WORK}/bin/amplihack" AMPLIHACK_RECIPE_LOG="$LOG" \
      bash "${BUNDLE}/tools/autodrive_goal_loop.sh" --goal-tool issue_3286_goal.sh --goal-file goal.md \
        --repo "$WT" --state-dir "${CASE}/state" --task-file "${WORK}/task.txt" </dev/null ) \
    >"${CASE}/out" 2>"${CASE}/err" || RC=$?
  OUT="$(cat "${CASE}/out")"
  local k; for k in $STUB_KNOBS; do unset "$k"; done; STUB_KNOBS=""
}
turns() { local n; n="$(grep -cx autodrive-goal-turn "${CASE}/stub/calls" 2>/dev/null)" || true; printf '%s' "${n:-0}"; }
log_lines() { awk 'END { print NR }' "$LOG"; }
last_out() { printf '%s\n' "$OUT" | awk 'NF { last = $0 } END { print last }'; }
out_field() { last_out | jq -r "$1" 2>/dev/null; }
end_word() { jq -r '.end // empty' "${CASE}/state/loop-end.json" 2>/dev/null; }
# every line of the run log, in order, names its turn's number and <err>
log_names_turns() { # log_names_turns <err> <first turn> ...
  local err="$1" i=1 n; shift
  [ "$(log_lines)" -eq $# ] || return 1
  for n in "$@"; do
    awk -v i="$i" 'NR == i' "$LOG" | grep -qF -- "$err" || return 1
    awk -v i="$i" 'NR == i' "$LOG" | grep -qw -- "$n" || return 1
    i=$((i + 1))
  done
}
contains() { case "$1" in *"$2"*) return 0 ;; *) return 1 ;; esac; }

ERR_X="Error: trusted bundle has unsafe ownership or modes: ${WORK}/clone-x/amplifier-bundle"
ERR_Y="Error: trusted bundle has unsafe ownership or modes: ${WORK}/clone-y/amplifier-bundle"
MARK='amplihack.recipe.failure_class '
# The marker the runner now writes for a failed bundle check: no step ran.
BUNDLE_MARKER="${MARK}"'{"action":"terminal","attempt":1,"class":"environmental","completed_steps":[],"failed_steps":[],"issue":1267,"retryable":false,"schema_version":1,"signal":"untrusted bundle","structural_refusal":false}'
# A turn whose agent step ran and then failed.
AGENT_MARKER="${MARK}"'{"action":"terminal","attempt":1,"class":"agent_api","completed_steps":["read"],"failed_steps":["decide"],"issue":1267,"retryable":false,"schema_version":1,"signal":"execution failed: 400","structural_refusal":false}'
AGENT_OUT="  step decide: failed
Execution failed: 400 the session ended
${AGENT_MARKER}"
# The runner's marker, then the error: what a turn now prints for a failed
# bundle check (crates/amplihack-cli/tests/issue_3286_untrusted_bundle_marker.rs
# holds the real runner to this).
BX="${BUNDLE_MARKER}
${ERR_X}"
BY="${BUNDLE_MARKER}
${ERR_Y}"
# A turn in which an agent ran and the runner was killed before it wrote a
# marker: the same last line every time, and no marker.
KILLED="Killed: the recipe runner was stopped"
AGENT_KILLED_OUT="  step read: completed
  step decide: running
${KILLED}"

# --- 1. every turn fails with the same error and no marker: unknown, never counted
for t in 1 2 3 4 5 6 7 8; do turn_fails "$t" "$ERR_X"; done
run no-marker
ok A-EXIT "a marker-less failure must not end the run; the goal's word does: rc=${RC}" test "$RC" -eq 0
ok A-END "the goal's final word ends it: $(end_word)" test "$(end_word)" = fixture-end
ok A-EIGHT-TURNS "every turn ran: turns=$(turns)" test "$(turns)" -eq 8
ok A-LOG "one run-log line per failed turn, with its number and the error: $(cat "$LOG")" log_names_turns "$ERR_X" 1 2 3 4 5 6 7 8

# --- 1b. an agent ran and the runner was killed before its marker, every turn ---
for t in 1 2 3 4 5 6 7 8; do turn_fails "$t" "$AGENT_KILLED_OUT"; done
run agent-killed
ok A2-EXIT "a turn whose agent ran must not count as pre-agent: rc=${RC}" test "$RC" -eq 0
ok A2-EIGHT-TURNS "every turn ran: turns=$(turns)" test "$(turns)" -eq 8
ok A2-LOG "one run-log line per failed turn: $(cat "$LOG")" log_names_turns "$KILLED" 1 2 3 4 5 6 7 8

# --- 1c. a marker that does not list the step fields is unknown too -------------
for t in 1 2 3 4 5 6 7 8; do turn_fails "$t" "${MARK}"'{"action":"terminal","class":"environmental","retryable":false}'"
${ERR_X}"; done
run marker-without-steps
ok A3-EXIT "a marker without step lists must not end the run: rc=${RC}" test "$RC" -eq 0
ok A3-EIGHT-TURNS "every turn ran: turns=$(turns)" test "$(turns)" -eq 8

# --- 2. every turn fails the bundle check, with the marker the runner writes ----
for t in 1 2 3 4 5 6 7 8; do turn_fails "$t" "$BX"; done
run bundle-marker
ok B-EXIT "the run must end with a non-zero exit: rc=${RC}" test "$RC" -ne 0
ok B-THREE-TURNS "the run must end after three turns: turns=$(turns)" test "$(turns)" -eq 3
ok B-ERROR-LAST "the last stdout line must carry the error: $(last_out)" contains "$(out_field '.detail // .error // empty')" "$ERR_X"
ok B-EXIT-FIELD "the last stdout line must carry the exit: $(last_out)" test "$(out_field '.exit')" = "$RC"
ok B-END "the end record must say failed: $(end_word)" test "$(end_word)" = failed
ok B-LOG "one run-log line per failed turn, with its number and the error: $(cat "$LOG")" log_names_turns "$ERR_X" 1 2 3
ok B-NOT-RETRYABLE "the turn record must read the marker as not retryable: $(jq -c .failure "${CASE}/state/turns/001/result.json" 2>/dev/null)" \
  test "$(jq -r .failure.retryable "${CASE}/state/turns/001/result.json" 2>/dev/null)" = false

# --- 3. an agent turn fails once and then succeeds: the run goes on as today ----
turn_fails 1 "$AGENT_OUT"
stub FIXTURE_END_AFTER=2
run agent-fails-once
ok C-EXIT "the run must end at the goal's own word: rc=${RC}" test "$RC" -eq 0
ok C-END "the goal's final word ends it: $(end_word)" test "$(end_word)" = fixture-end
ok C-TWO-TURNS "both turns ran: turns=$(turns)" test "$(turns)" -eq 2
ok C-LOG "one run-log line for the one failed turn: $(cat "$LOG")" test "$(log_lines)" -eq 1

# --- 4. a turn in which an agent ran starts the count again -----------------------
turn_fails 1 "$BX"; turn_fails 2 "$BX"; turn_fails 3 "$AGENT_OUT"
for t in 4 5 6 7 8; do turn_fails "$t" "$BX"; done
run agent-resets
ok D-EXIT "the run must end with a non-zero exit: rc=${RC}" test "$RC" -ne 0
ok D-SIX-TURNS "the run must end after three new turns in a row, at turn 6: turns=$(turns)" test "$(turns)" -eq 6
ok D-LOG "one run-log line per failed turn: $(log_lines) lines" test "$(log_lines)" -eq 6

# --- 5. a different error starts the count again ------------------------------------
turn_fails 1 "$BX"; turn_fails 2 "$BX"; turn_fails 3 "$BY"; turn_fails 4 "$BY"
stub FIXTURE_END_AFTER=4
run different-error
ok E-EXIT "two errors, two turns each, must not end the run: rc=${RC}" test "$RC" -eq 0
ok E-END "the goal's final word ends it: $(end_word)" test "$(end_word)" = fixture-end
ok E-FOUR-TURNS "all four turns ran: turns=$(turns)" test "$(turns)" -eq 4

# --- 6. a successful turn starts the count again --------------------------------------
turn_fails 1 "$BX"; turn_fails 2 "$BX"
for t in 4 5 6 7 8; do turn_fails "$t" "$BX"; done
run success-resets
ok F-SIX-TURNS "the run must end after three new turns in a row, at turn 6: turns=$(turns)" test "$(turns)" -eq 6
ok F-EXIT "the run must end with a non-zero exit: rc=${RC}" test "$RC" -ne 0

# --- 7. no run log: the loop behaves the same and writes none ---------------------------
for t in 1 2 3 4 5 6 7 8; do turn_fails "$t" "$BX"; done
CASE="${WORK}/case-no-log"; mkdir -p "${CASE}/stub"; RC=0
( cd "$MAIN" && HOME="${WORK}/home" GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 STUB_STATE="${CASE}/stub" \
    PATH="${WORK}/bin:${PATH}" AMPLIHACK_BIN="${WORK}/bin/amplihack" AMPLIHACK_RECIPE_LOG="relative.log" \
    bash "${BUNDLE}/tools/autodrive_goal_loop.sh" --goal-tool issue_3286_goal.sh --goal-file goal.md \
      --repo "$WT" --state-dir "${CASE}/state" --task-file "${WORK}/task.txt" </dev/null ) \
  >"${CASE}/out" 2>"${CASE}/err" || RC=$?
for k in $STUB_KNOBS; do unset "$k"; done; STUB_KNOBS=""
ok G-EXIT "the run must end the same way without a run log: rc=${RC}" test "$RC" -ne 0
ok G-THREE-TURNS "the run must end after three turns: turns=$(turns)" test "$(turns)" -eq 3
ok G-NO-RELATIVE "a relative run-log path is never written" test ! -e "${MAIN}/relative.log"

echo "=== ${PASSED} passed, ${FAILED} failed ==="
[ "$FAILED" -eq 0 ] || exit 1
exit 0
