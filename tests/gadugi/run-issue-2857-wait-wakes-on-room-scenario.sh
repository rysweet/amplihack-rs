#!/usr/bin/env bash
# Self-asserting gadugi-test scenario body for issue #2857: a usage-limit wait
# woke on a login that was already full, and a limit on the first attempt after
# a wait ended the run.
#
# On a shared-login host the rotator swaps logins into ~/.claude, and each swap
# woke every waiting run. The login it swapped onto could be at 98-100%
# session, so the retry's first call returned a 429 that the runner retried
# six times before the run ended indeterminate. Two things are asserted here
# from outside, against the shipped binary:
#
#   A. A limit on the retry a wait earned is waited for again, and reported
#      once. A wait that says "try now" and is contradicted is evidence the
#      signal was wrong, not a reason to end the run.
#   B. A wait wakes on a login change only when the rotator's last cycle says
#      the login now live has room (`last_cycle.live_room`). A swap onto a
#      login without room, a ledger from a rotator that records no room, and
#      the records a stopping rotator leaves, wake nothing; the same swap with
#      room still does. A person's own `profile switch` (outcome `switched`)
#      records no room reading, because nothing measured the login they chose;
#      it still wakes a wait unless it says `live_room: false`.
#
# Black boxes, all shipped code, run unchanged: the real binary's
# `claude --subprocess-safe` supervision, its limit classification, its
# usage-limit wait, its reading of the rotate ledger and its
# `amplihack.recipe.usage_limit_wait` log lines.
#
# Stand-ins, in $WORK/tools and named by AMPLIHACK_CLAUDE_BINARY_PATH: a stub
# `claude` that answers `--version`, counts its calls, reports a Claude usage
# limit, and on its first call replaces the rotate ledger the way the rotator
# does (one private rename). Nothing reaches a real account, a real agent, a
# real rotator or the network.
#
# A first limit message carries its reset as an epoch five minutes in the
# past. The debug-only clock seams of the Rust tests are deliberately not
# used: CI runs this against the release binary it installs, where those seams
# are compiled out.
#
# A second limit repeats the first one's reset, already behind the clock, as a
# harness does when it repeats the window that has just turned over. A limit
# after a wait may not end the step, so the binary waits again.
#
# Issue #2911 shapes every wait here. A reset already behind the clock is a
# wait for capacity with no deadline: it ends on a login with room or a login
# change, and on no timer (`until=none bound=none`). So the scenario makes
# those events itself, as a person or a rotator would, and makes each one only
# once the wait is under way: at its first `progress` line, a minute after it
# starts. A wait that is still running then shows that nothing woke it and
# nothing timed ended it. Case A ends each wait with a login change in the
# step's own config directory; case B's held-back cases end theirs with a
# fresh rotator record that has room (`rotator capacity`), which tells them
# apart from the woken cases (`login changed`, before the wait begins). The
# step timeout plays no part (issue #3037).
#
# The cost is wall clock: a minute for each wait the scenario has to end.
# Case A has two and case B four, so the scenario takes about six minutes.
#
# Every scenario command runs under `env -i` with an explicit allowlist (see
# scenario_env below): no token, key or setting from the caller's environment
# reaches it, HOME is a scratch folder, and the real ~/.claude and
# ~/.amplihack are never read or written. Everything is created under one
# mktemp folder that the EXIT trap removes.
#
# Where it runs: CI's "Install Smoke Test" job (.github/workflows/ci.yml) runs
# this script directly against the amplihack binary that job installs. CI does not run
# gadugi-test itself; tests/gadugi/scenarios/issue-2857-wait-wakes-on-room.yaml
# wraps this script for anyone running it through gadugi-test by hand. It is
# not run on a developer host as part of the change that adds it.
#
# Run by hand: bash tests/gadugi/run-issue-2857-wait-wakes-on-room-scenario.sh
# It builds amplihack with cargo first. Set AMPLIHACK_2857_QA_BIN to an
# amplihack binary to skip the build. On success it prints ALL_CASES_PASSED
# and exits 0; otherwise SCENARIO_FAILED and exit 1.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd -P)"

for c in grep sed awk find date printf; do
  p="$(command -v "$c" 2>/dev/null || true)"
  case "$p" in
    /*) ;;
    *) if [ "$c" != printf ]; then echo "FAIL: $c is required"; echo "SCENARIO_FAILED"; exit 1; fi ;;
  esac
done

WORK="$(mktemp -d "${TMPDIR:-/tmp}/issue-2857-qa.XXXXXX")" || {
  echo "FAIL: cannot create a temporary directory"; echo "SCENARIO_FAILED"; exit 1
}
WORK="$(cd "$WORK" && pwd -P)" || { echo "FAIL: cannot resolve the temporary directory"; echo "SCENARIO_FAILED"; exit 1; }
trap 'chmod -R -- u+rwx "$WORK" 2>/dev/null || true; rm -rf -- "$WORK"' EXIT
SYS_PATH="/usr/local/bin:/usr/bin:/bin"

# --- the binary under test --------------------------------------------------
BIN="${AMPLIHACK_2857_QA_BIN:-}"
if [ -z "$BIN" ]; then
  # json-render-diagnostics keeps compiler errors on stderr while stdout
  # carries the artifact records, and one awk reads every record so cargo
  # never writes into a closed pipe (issue #1434). 14 is the length of
  # `"executable":"`; 15 adds the quote.
  if ! BIN="$(cd "$REPO_ROOT" && cargo build -p amplihack --bin amplihack --locked \
      --message-format=json-render-diagnostics 2>"$WORK/build.log" \
      | awk 'match($0, /"executable":"[^"]*\/amplihack"/) { bin = substr($0, RSTART + 14, RLENGTH - 15) } END { print bin }')"; then
    echo "FAIL: cargo build -p amplihack --bin amplihack failed"
    tail -n 40 "$WORK/build.log"
    echo "SCENARIO_FAILED"; exit 1
  fi
fi
if [ -z "$BIN" ] || [ ! -x "$BIN" ]; then
  echo "FAIL: no executable amplihack binary (got '${BIN}')"
  echo "SCENARIO_FAILED"; exit 1
fi
case "$BIN" in
  /*) ;;
  *) BIN="$PWD/$BIN" ;;
esac
echo "binary under test: $BIN"

HOME_DIR="$WORK/home"; CONFIG="$WORK/claude-config"; RECORDS="$WORK/records"
TOOLS="$WORK/tools"; RUNDIR="$WORK/work"; PROFILES="$WORK/profiles"
LEDGER_DIR="$PROFILES/.auto-switch.d/rotate"
mkdir -p -- "$HOME_DIR/.claude" "$CONFIG" "$RECORDS" "$TOOLS" "$RUNDIR" "$WORK/tmp" \
  || { echo "FAIL: cannot create the scratch folders"; echo "SCENARIO_FAILED"; exit 1; }
mkdir -m 700 -- "$PROFILES" "$PROFILES/.auto-switch.d" "$LEDGER_DIR" \
  || { echo "FAIL: cannot create the profile root"; echo "SCENARIO_FAILED"; exit 1; }

# A reset five minutes ago: the limit states a reset the clock has passed, so
# the step waits for capacity rather than for a time that has been and gone.
NOW="$(date +%s)"
LIMIT_EPOCH=$(( NOW - 300 ))
# How long the scenario gives one wait to write its first progress line (a
# minute after it starts) or one step to finish, before it calls the case
# failed and stops the step.
WAIT_LIMIT_SECS=150

# scenario_env <config-dir|home> <command...>: runs the command with only the
# variables below.
#   PATH, HOME, TMPDIR, LANG            scratch folders and a fixed locale
#   TZ                                  a fixed zone
#   AMPLIHACK_CLAUDE_PROFILE_ROOT       a private scratch profile root, never the user's
#   CLAUDE_CONFIG_DIR                   a scratch Claude config for `config-dir`;
#                                       absent for `home`, which is a $HOME/.claude step
#   AMPLIHACK_CLAUDE_BINARY_PATH        the stub agent
#   RECORD_DIR, LEDGER_DIR, LEDGER_BODY,
#   LIMIT_CALLS, LIMIT_EPOCH            the stub's own inputs
#   AMPLIHACK_NONINTERACTIVE            the shape a recipe step runs in
#   AMPLIHACK_NO_FRESHNESS_CHECK,
#   AMPLIHACK_SKIP_AUTO_INSTALL,
#   AMPLIHACK_SKIP_POWER_STEERING,
#   AMPLIHACK_SKIP_REFLECTION           no install, update or reflection work
#   AMPLIHACK_ROTATE_AUTOSTART=0        no detached rotator outlives the scenario
#   AMPLIHACK_GOVERNOR=off              the #1811 capacity governor would hold
#                                       the exhausted reading before the wait
scenario_env() {
  local kind="$1"; shift
  local cfg=()
  if [ "$kind" = config-dir ]; then cfg=(CLAUDE_CONFIG_DIR="$CONFIG"); fi
  env -i \
    PATH="$TOOLS:$SYS_PATH" \
    HOME="$HOME_DIR" \
    TMPDIR="$WORK/tmp" \
    LANG=C \
    TZ=UTC \
    AMPLIHACK_CLAUDE_PROFILE_ROOT="$PROFILES" \
    ${cfg[@]+"${cfg[@]}"} \
    AMPLIHACK_CLAUDE_BINARY_PATH="$TOOLS/claude" \
    RECORD_DIR="$RECORDS" \
    LEDGER_DIR="$LEDGER_DIR" \
    LEDGER_BODY="${LEDGER_BODY:-}" \
    LIMIT_CALLS="${LIMIT_CALLS:-1}" \
    LIMIT_EPOCH="$LIMIT_EPOCH" \
    AMPLIHACK_NONINTERACTIVE=1 \
    AMPLIHACK_NO_FRESHNESS_CHECK=1 \
    AMPLIHACK_SKIP_AUTO_INSTALL=1 \
    AMPLIHACK_SKIP_POWER_STEERING=1 \
    AMPLIHACK_SKIP_REFLECTION=1 \
    AMPLIHACK_ROTATE_AUTOSTART=0 \
    AMPLIHACK_GOVERNOR=off \
    "$@"
}

# --- the stub agent ---------------------------------------------------------
cat > "$TOOLS/claude" <<'STUB'
#!/bin/sh
# Stub `claude`: answers --version, counts its calls per $STEP, on its first
# call replaces the rotate ledger with $LEDGER_BODY (when set) in one private
# rename, and reports a usage limit on its first $LIMIT_CALLS calls, each one
# stating $LIMIT_EPOCH, a reset already past.
case "$1" in
  --version) echo '2.1.319 (Claude Code)'; exit 0 ;;
esac
step="${STEP:-0}"
n=$(( $(cat "$RECORD_DIR/count.$step" 2>/dev/null || echo 0) + 1 ))
echo "$n" > "$RECORD_DIR/count.$step"
echo "attempt $n output"
if [ "$n" = 1 ] && [ -n "${LEDGER_BODY:-}" ]; then
  umask 077
  printf '%s' "$LEDGER_BODY" > "$LEDGER_DIR/ledger.json.new"
  chmod 600 "$LEDGER_DIR/ledger.json.new"
  mv "$LEDGER_DIR/ledger.json.new" "$LEDGER_DIR/ledger.json"
fi
if [ "$n" -le "${LIMIT_CALLS:-1}" ]; then
  printf 'Claude AI usage limit reached|%s\n' "$LIMIT_EPOCH" >&2
  exit 1
fi
exit 0
STUB
chmod 755 "$TOOLS/claude" || { echo "FAIL: cannot write the stub agent"; echo "SCENARIO_FAILED"; exit 1; }

fail=0
pass() { echo "PASS: $1 - $2"; }
fl() { echo "FAIL: $1 - $2"; fail=1; }

# `grep -c` prints 0 and exits 1 when nothing matches, so its status is not
# used; a missing file gives no output at all and counts as 0.
count_in() {
  local c
  c="$(grep -c -- "$1" "$2" 2>/dev/null || true)"
  [ -n "$c" ] || c=0
  printf '%s' "$c"
}

reset_records() { rm -f -- "$RECORDS"/count.* "$RECORDS"/call.* 2>/dev/null || true; }

# await_lines <file> <pattern> <n> <pid>: waits until <file> holds at least
# <n> lines matching <pattern>, or the process <pid> has exited, or
# WAIT_LIMIT_SECS pass. Succeeds only when the lines are there. It acts on
# the step's own log lines, read once a second; it ends no wait itself.
await_lines() {
  local file="$1" pat="$2" n="$3" pid="$4" i=0
  while [ "$i" -lt "$WAIT_LIMIT_SECS" ]; do
    [ "$(count_in "$pat" "$file")" -ge "$n" ] && return 0
    kill -0 "$pid" 2>/dev/null || { [ "$(count_in "$pat" "$file")" -ge "$n" ]; return; }
    sleep 1
    i=$(( i + 1 ))
  done
  return 1
}

# finish <pid>: sets FINISH_RC to the step's exit code once it has exited,
# or to 124 after WAIT_LIMIT_SECS, when the step is stopped. Not run in a
# subshell, which could not wait for the step.
FINISH_RC=0
finish() {
  local pid="$1" i=0
  while kill -0 "$pid" 2>/dev/null && [ "$i" -lt "$WAIT_LIMIT_SECS" ]; do
    sleep 1
    i=$(( i + 1 ))
  done
  if kill -0 "$pid" 2>/dev/null; then
    kill -TERM "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
    FINISH_RC=124
    return
  fi
  FINISH_RC=0
  wait "$pid" || FINISH_RC=$?
}

# A login change in the step's own config directory, as the wait's
# fingerprint sees one: the first makes a credentials file, the next names an
# account in `.claude.json`, and each later one renames that account.
LOGIN_CHANGES=0
change_login() {
  LOGIN_CHANGES=$(( LOGIN_CHANGES + 1 ))
  if [ "$LOGIN_CHANGES" = 1 ]; then
    ( umask 077; printf '{}' > "$CONFIG/.credentials.json" )
  else
    ( umask 077
      printf '{"oauthAccount":{"accountUuid":"uuid-qa-%s"}}' "$LOGIN_CHANGES" > "$CONFIG/.claude.json.new"
      mv -- "$CONFIG/.claude.json.new" "$CONFIG/.claude.json" )
  fi
}

PROGRESS='amplihack.recipe.usage_limit_wait progress remaining=none$'

# The rotate ledger before the run: X live, a cycle from before the step that
# recorded room, as a rotator leaves it.
write_initial_ledger() {
  local body
  body="$(printf '{"version":1,"live":"X","alert":null,"last_cycle":{"at":%s,"measured_at":%s,"outcome":"stayed","live_room":true}}' \
    "$(( NOW - 3600 ))" "$(( NOW - 3600 ))")"
  printf '%s' "$body" > "$LEDGER_DIR/ledger.json.init"
  chmod 600 "$LEDGER_DIR/ledger.json.init"
  mv -- "$LEDGER_DIR/ledger.json.init" "$LEDGER_DIR/ledger.json"
}

cd -- "$RUNDIR" || { echo "FAIL: cannot enter the working folder"; echo "SCENARIO_FAILED"; exit 1; }

# --- case A: a limit on the retry a wait earned is waited for again ---------
# Both attempts report the same reset, already past. That reset is one the step
# cannot use, so each limit becomes a wait for capacity with no deadline: the
# wait announces itself as `waiting_for_capacity reason=passed_reset ...
# until=none` and runs until an event. The scenario changes the login in the
# step's config directory at each wait's first progress line, the wait ends
# `login changed` and the step retries. The retry meets a limit, so the wait
# that had just ended was not the end of the matter.
#
# Before #2857 the step ended on the second limit with
# `skipped reason=already_waited`, because it had already waited on this login.
# #2857 made it wait again but left the refusal in place for a third, and
# #2911 removed the refusal and the per-step budget entirely: two waits start,
# nothing is ever refused, no `failed` line is written, and the third attempt
# succeeds.
#
# One `reentered` line belongs here. The first wait ended on a login change,
# which says the retry should work, and the limit the retry met contradicts
# that, so the second wait reports it once before its own lines
# (`Resumed::AfterASignal`, from `End::said_try_now`). The Rust test
# `a_usage_limit_after_a_wait_waits_again_and_never_refuses` pins the same
# sequence.
#
# Each wait lasts until its first progress line, so this case takes about two
# minutes. The stub ran once before the first event: a minute of waiting
# relaunched nothing.
reset_records
LIMIT_CALLS=2 LEDGER_BODY="" scenario_env config-dir "$BIN" claude --subprocess-safe -- -p step \
  >"$WORK/a.out" 2>"$WORK/a.err" </dev/null &
A_PID=$!
A_EVENTS=0
A_CALLS_AT_FIRST=""
for n in 1 2; do
  if await_lines "$WORK/a.err" "$PROGRESS" "$n" "$A_PID"; then
    [ "$n" = 1 ] && A_CALLS_AT_FIRST="$(cat "$RECORDS/count.0" 2>/dev/null || echo 0)"
    change_login
    A_EVENTS=$(( A_EVENTS + 1 ))
  fi
done
finish "$A_PID"
A_RC="$FINISH_RC"
A_CALLS="$(cat "$RECORDS/count.0" 2>/dev/null || echo 0)"
A_STARTS="$(count_in 'amplihack.recipe.usage_limit_wait start reason=usage_limit .* until=none bound=none$' "$WORK/a.err")"
A_REENTERED="$(count_in 'amplihack.recipe.usage_limit_wait reentered met=usage_limit$' "$WORK/a.err")"
A_REFUSED="$(count_in 'already_waited' "$WORK/a.err")"
# Issue #2911: the operator-visible line a wait with no reset to end on
# writes, with its reason from the closed set and no deadline. Two waits, so
# two lines.
A_WAITING="$(count_in 'amplihack.recipe.usage_limit_wait waiting_for_capacity reason=passed_reset .*until=none$' "$WORK/a.err")"
A_CHANGED="$(count_in 'amplihack.recipe.usage_limit_wait end reason=login changed$' "$WORK/a.err")"
# Nothing a limit does ends the step any more. Only a stop signal or the end
# of its session writes a closing line, and neither happens here.
A_FAILED="$(count_in 'amplihack.recipe.usage_limit_wait failed reason=' "$WORK/a.err")"
A_SKIPPED="$(count_in 'amplihack.recipe.usage_limit_wait skipped reason=' "$WORK/a.err")"
A_ORDER="$(grep -n 'amplihack.recipe.usage_limit_wait \(resume attempt=2\|reentered \|start \)' "$WORK/a.err" 2>/dev/null \
  | sed 's/^[0-9]*:amplihack.recipe.usage_limit_wait //' | awk '{print $1}' | tr '\n' ' ' || true)"
A_DIAG="rc=$A_RC calls=$A_CALLS events=$A_EVENTS a.err tail: $(tail -n 12 "$WORK/a.err" 2>/dev/null | tr '\n' ' ')"

if [ "$A_RC" -eq 0 ] && [ "$A_CALLS" = 3 ]; then
  pass limit_after_a_wait_does_not_end_the_step "the step waited again and succeeded on its third attempt"
else
  fl limit_after_a_wait_does_not_end_the_step "$A_DIAG"
fi
if [ "$A_STARTS" = 2 ] && [ "$A_REFUSED" = 0 ]; then
  pass second_wait_started_not_refused "two waits with no deadline started and none was refused as already_waited"
else
  fl second_wait_started_not_refused "starts=$A_STARTS already_waited=$A_REFUSED, want 2/0; $A_DIAG"
fi
if [ "$A_CALLS_AT_FIRST" = 1 ] && [ "$A_CHANGED" = 2 ]; then
  pass no_timer_ends_a_wait "a minute into the first wait the stub had run once, and each wait ended on the login change"
else
  fl no_timer_ends_a_wait "calls at the first progress line=${A_CALLS_AT_FIRST:-none} login changed=$A_CHANGED, want 1 and 2; $A_DIAG"
fi
if [ "$A_REENTERED" = 1 ] && [ "$A_ORDER" = "start resume reentered start " ]; then
  pass a_contradicted_wake_is_reported_once "two waits in order, and one reentered line before the second"
else
  fl a_contradicted_wake_is_reported_once "reentered=$A_REENTERED order='${A_ORDER}', want 1 and 'start resume reentered start '; $A_DIAG"
fi
if [ "$A_WAITING" = 2 ]; then
  pass each_wait_says_it_is_waiting_for_capacity "two waiting_for_capacity reason=passed_reset lines with until=none, one per wait"
else
  fl each_wait_says_it_is_waiting_for_capacity "waiting_for_capacity=$A_WAITING, want 2; $A_DIAG"
fi
if [ "$A_FAILED" = 0 ] && [ "$A_SKIPPED" = 0 ]; then
  pass no_limit_ended_the_step "no failed and no skipped line: a limit only delays a step (#2911)"
else
  fl no_limit_ended_the_step "failed=$A_FAILED skipped=$A_SKIPPED, want 0/0; $A_DIAG"
fi

# --- case B: a wait wakes on a login change only when the login has room -----
# A $HOME/.claude step (no CLAUDE_CONFIG_DIR). The stub swaps the rotate
# ledger to login Y while its first attempt fails, as the rotator does on a
# shared-login host; the step then reads the ledger and decides whether the
# login moved to one it should retry on.
#
#   with room      -> `end reason=login changed`  (the early wake, as #2731)
#   without room,
#   or no room bit,
#   or a stopping
#   rotator's record -> no `login changed` at all: the change was held back
#   a manual switch with no room bit
#                    -> `end reason=login changed`  (a person chose the login)
#   a manual switch that says no room
#                    -> no `login changed`
#
# The two groups are told apart by how the wait ends. A woken wait ends
# `login changed` before it begins, and the retry runs. A held-back change
# wakes nothing: the limit's reset is already behind the clock, so the wait is
# a capacity wait with no deadline, and it is still waiting at its first
# progress line, a minute in. The scenario then writes the record a rotator
# writes when it measures room on the live login, and the wait ends
# `rotator capacity`. Either way the step succeeds on its second attempt; only
# the reason differs.
run_swap_case() {
  local name="$1" body="$2" wakes="$3"
  local want other want_calls=2
  if [ "$wakes" = wakes ]; then
    want="login changed"; other="rotator capacity"
  else
    want="rotator capacity"; other="login changed"
  fi
  reset_records
  write_initial_ledger
  LIMIT_CALLS=1 LEDGER_BODY="$body" \
    scenario_env home "$BIN" claude --subprocess-safe -- -p step \
    >"$WORK/$name.out" 2>"$WORK/$name.err" </dev/null &
  local pid=$!
  if [ "$wakes" != wakes ] && await_lines "$WORK/$name.err" "$PROGRESS" 1 "$pid"; then
    local at
    at="$(( $(date +%s) + 1 ))"
    printf '{"version":1,"live":"Y","alert":null,"last_cycle":{"at":%s,"measured_at":%s,"outcome":"swapped","live_room":true}}' \
      "$at" "$at" > "$LEDGER_DIR/ledger.json.room"
    chmod 600 "$LEDGER_DIR/ledger.json.room"
    mv -- "$LEDGER_DIR/ledger.json.room" "$LEDGER_DIR/ledger.json"
  fi
  local rc calls got wrong progress diag
  finish "$pid"
  rc="$FINISH_RC"
  calls="$(cat "$RECORDS/count.0" 2>/dev/null || echo 0)"
  got="$(count_in "amplihack.recipe.usage_limit_wait end reason=$want\$" "$WORK/$name.err")"
  wrong="$(count_in "amplihack.recipe.usage_limit_wait end reason=$other\$" "$WORK/$name.err")"
  progress="$(count_in "$PROGRESS" "$WORK/$name.err")"
  diag="rc=$rc calls=$calls progress=$progress $name.err tail: $(tail -n 8 "$WORK/$name.err" 2>/dev/null | tr '\n' ' ')"
  if [ "$rc" -eq 0 ] && [ "$calls" = "$want_calls" ] && [ "$got" = 1 ] && [ "$wrong" = 0 ]; then
    pass "$name" "the wait ended '$want' after $calls attempt(s)"
  else
    fl "$name" "want one 'end reason=$want', none '$other', rc=0 calls=$want_calls; got $got/$wrong; $diag"
  fi
}

T="$NOW"
SWAPPED_ROOM="$(printf '{"version":1,"live":"Y","alert":null,"last_cycle":{"at":%s,"measured_at":%s,"outcome":"swapped","live_room":true}}' "$T" "$T")"
SWAPPED_FULL="$(printf '{"version":1,"live":"Y","alert":null,"last_cycle":{"at":%s,"measured_at":%s,"outcome":"swapped","live_room":false}}' "$T" "$T")"
SWAPPED_OLD="$(printf '{"version":1,"live":"Y","alert":null,"last_cycle":{"at":%s,"measured_at":%s,"outcome":"swapped"}}' "$T" "$T")"
SWITCHED="$(printf '{"version":1,"live":"Y","alert":null,"last_cycle":{"at":%s,"measured_at":%s,"outcome":"switched"}}' "$T" "$T")"
SWITCHED_FULL="$(printf '{"version":1,"live":"Y","alert":null,"last_cycle":{"at":%s,"measured_at":%s,"outcome":"switched","live_room":false}}' "$T" "$T")"
STOPPED="$(printf '{"version":1,"live":"Y","alert":null,"last_cycle":{"at":%s,"outcome":"idle"}}' "$T")"

run_swap_case swap_onto_a_login_with_room_wakes_the_wait "$SWAPPED_ROOM" wakes
run_swap_case swap_onto_a_full_login_does_not_wake_the_wait "$SWAPPED_FULL" held-back
run_swap_case swap_recorded_by_an_older_rotator_does_not_wake_the_wait "$SWAPPED_OLD" held-back
run_swap_case a_stopping_rotators_record_does_not_wake_the_wait "$STOPPED" held-back
run_swap_case a_manual_switch_without_a_room_reading_wakes_the_wait "$SWITCHED" wakes
run_swap_case a_manual_switch_that_recorded_no_room_does_not_wake_the_wait "$SWITCHED_FULL" held-back

if [ "$fail" -eq 0 ]; then
  echo "ALL_CASES_PASSED"
  exit 0
fi
echo "SCENARIO_FAILED"
exit 1
