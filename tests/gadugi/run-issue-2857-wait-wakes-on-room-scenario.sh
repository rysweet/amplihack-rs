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
# The limit message carries its reset as an epoch five minutes in the past, so
# every wait this scenario drives ends at once and the whole scenario takes
# seconds. The debug-only clock seams of the Rust tests are deliberately not
# used: CI runs this against the release binary it installs, where those seams
# are compiled out. The two ends of a wait that has nothing to wait for are
# then told apart by their reason: `login changed` is the early wake,
# `reset reached` is the wait that ran to its own end.
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

# A reset five minutes ago: the wait has a deadline it has already passed, so
# it starts, ends and resumes without the scenario sleeping.
NOW="$(date +%s)"
LIMIT_EPOCH=$(( NOW - 300 ))

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
# rename, and reports a usage limit on its first $LIMIT_CALLS calls.
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
# Both attempts report the same past reset. The first wait ends `reset
# reached`, which says try now; the retry meets a limit, so the signal was
# wrong. Before #2857 the step ended there with `skipped reason=already_waited`.
# Now it waits again, writes one `reentered` line before the second `start`,
# and the third attempt succeeds.
reset_records
LIMIT_CALLS=2 LEDGER_BODY="" scenario_env config-dir "$BIN" claude --subprocess-safe -- -p step \
  >"$WORK/a.out" 2>"$WORK/a.err" </dev/null && A_RC=0 || A_RC=$?
A_CALLS="$(cat "$RECORDS/count.0" 2>/dev/null || echo 0)"
A_STARTS="$(count_in 'amplihack.recipe.usage_limit_wait start reason=usage_limit' "$WORK/a.err")"
A_REENTERED="$(count_in 'amplihack.recipe.usage_limit_wait reentered met=usage_limit$' "$WORK/a.err")"
A_REFUSED="$(count_in 'already_waited' "$WORK/a.err")"
A_ORDER="$(grep -n 'amplihack.recipe.usage_limit_wait \(resume attempt=2\|reentered \|start \)' "$WORK/a.err" 2>/dev/null \
  | sed 's/^[0-9]*:amplihack.recipe.usage_limit_wait //' | awk '{print $1}' | tr '\n' ' ' || true)"
A_DIAG="rc=$A_RC calls=$A_CALLS a.err tail: $(tail -n 12 "$WORK/a.err" 2>/dev/null | tr '\n' ' ')"

if [ "$A_RC" -eq 0 ] && [ "$A_CALLS" = 3 ]; then
  pass limit_after_a_wait_does_not_end_the_step "the step waited again and succeeded on its third attempt"
else
  fl limit_after_a_wait_does_not_end_the_step "$A_DIAG"
fi
if [ "$A_STARTS" = 2 ] && [ "$A_REFUSED" = 0 ]; then
  pass second_wait_started_not_refused "two waits started and none was refused as already_waited"
else
  fl second_wait_started_not_refused "starts=$A_STARTS already_waited=$A_REFUSED, want 2/0; $A_DIAG"
fi
if [ "$A_REENTERED" = 1 ] && [ "$A_ORDER" = "start resume reentered start " ]; then
  pass reentry_reported_once_before_the_second_start "one reentered met=usage_limit line, between the first resume and the second start"
else
  fl reentry_reported_once_before_the_second_start "reentered=$A_REENTERED order='${A_ORDER}', want 1 and 'start resume reentered start '; $A_DIAG"
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
#   rotator's record -> `end reason=reset reached` (the wait ran to its end)
#   a manual switch with no room bit
#                    -> `end reason=login changed`  (a person chose the login)
#   a manual switch that says no room
#                    -> `end reason=reset reached`
run_swap_case() {
  local name="$1" body="$2" want="$3" other="$4"
  reset_records
  write_initial_ledger
  LIMIT_CALLS=1 LEDGER_BODY="$body" scenario_env home "$BIN" claude --subprocess-safe -- -p step \
    >"$WORK/$name.out" 2>"$WORK/$name.err" </dev/null && rc=0 || rc=$?
  local calls got diag
  calls="$(cat "$RECORDS/count.0" 2>/dev/null || echo 0)"
  got="$(grep -c "amplihack.recipe.usage_limit_wait end reason=$want\$" "$WORK/$name.err" 2>/dev/null || true)"
  [ -n "$got" ] || got=0
  local wrong
  wrong="$(grep -c "amplihack.recipe.usage_limit_wait end reason=$other\$" "$WORK/$name.err" 2>/dev/null || true)"
  [ -n "$wrong" ] || wrong=0
  diag="rc=$rc calls=$calls $name.err tail: $(tail -n 8 "$WORK/$name.err" 2>/dev/null | tr '\n' ' ')"
  if [ "$rc" -eq 0 ] && [ "$calls" = 2 ] && [ "$got" = 1 ] && [ "$wrong" = 0 ]; then
    pass "$name" "the wait ended '$want' and the step retried once"
  else
    fl "$name" "want one 'end reason=$want' and none '$other'; got $got/$wrong; $diag"
  fi
}

T="$NOW"
SWAPPED_ROOM="$(printf '{"version":1,"live":"Y","alert":null,"last_cycle":{"at":%s,"measured_at":%s,"outcome":"swapped","live_room":true}}' "$T" "$T")"
SWAPPED_FULL="$(printf '{"version":1,"live":"Y","alert":null,"last_cycle":{"at":%s,"measured_at":%s,"outcome":"swapped","live_room":false}}' "$T" "$T")"
SWAPPED_OLD="$(printf '{"version":1,"live":"Y","alert":null,"last_cycle":{"at":%s,"measured_at":%s,"outcome":"swapped"}}' "$T" "$T")"
SWITCHED="$(printf '{"version":1,"live":"Y","alert":null,"last_cycle":{"at":%s,"measured_at":%s,"outcome":"switched"}}' "$T" "$T")"
SWITCHED_FULL="$(printf '{"version":1,"live":"Y","alert":null,"last_cycle":{"at":%s,"measured_at":%s,"outcome":"switched","live_room":false}}' "$T" "$T")"
STOPPED="$(printf '{"version":1,"live":"Y","alert":null,"last_cycle":{"at":%s,"outcome":"idle"}}' "$T")"

run_swap_case swap_onto_a_login_with_room_wakes_the_wait "$SWAPPED_ROOM" "login changed" "reset reached"
run_swap_case swap_onto_a_full_login_does_not_wake_the_wait "$SWAPPED_FULL" "reset reached" "login changed"
run_swap_case swap_recorded_by_an_older_rotator_does_not_wake_the_wait "$SWAPPED_OLD" "reset reached" "login changed"
run_swap_case a_stopping_rotators_record_does_not_wake_the_wait "$STOPPED" "reset reached" "login changed"
run_swap_case a_manual_switch_without_a_room_reading_wakes_the_wait "$SWITCHED" "login changed" "reset reached"
run_swap_case a_manual_switch_that_recorded_no_room_does_not_wake_the_wait "$SWITCHED_FULL" "reset reached" "login changed"

if [ "$fail" -eq 0 ]; then
  echo "ALL_CASES_PASSED"
  exit 0
fi
echo "SCENARIO_FAILED"
exit 1
