#!/usr/bin/env bash
# Self-asserting gadugi-test scenario body for issue #2815: a usage limit in
# a recipe agent step whose prompt arrived on stdin ended the whole run.
#
# A recipe agent step is launched as `amplihack claude --subprocess-safe --
# -p` with the prompt written to the launch's stdin, never in argv. The
# supervisor read that as "a rerun would have no prompt" and refused the
# usage-limit wait with `skipped reason=piped_prompt`, so the first limit any
# such step met was terminal. The supervisor now copies the prompt aside as
# it arrives, waits for the reset and runs the step again with the same
# bytes.
#
# Black boxes, all shipped code, run unchanged: the real binary's
# `claude --subprocess-safe` supervision, its limit classification, its
# usage-limit wait and its `amplihack.recipe.usage_limit_wait` log lines.
#
# Stand-ins, in $WORK/tools and named by AMPLIHACK_CLAUDE_BINARY_PATH: a stub
# `claude` that answers `--version`, counts its calls, records the SHA-256 of
# everything on its stdin, and reports a Claude usage limit on its first call
# only. Nothing reaches a real account, a real agent or the network.
#
# The limit message carries its reset as an epoch a few minutes in the past,
# so the wait this scenario drives ends at once and the whole scenario takes
# seconds. The debug-only clock seams of the Rust tests are deliberately not
# used: CI runs this against the release binary it installs, where those
# seams are compiled out, and a past reset is the one form that gives a real
# wait a deadline it has already passed.
#
# Every scenario command runs under `env -i` with an explicit allowlist (see
# scenario_env below): no token, key or setting from the caller's environment
# reaches it, HOME is a scratch folder, and the real ~/.claude and
# ~/.amplihack are never read or written. Everything is created under one
# mktemp folder that the EXIT trap removes.
#
# Where it runs: CI's "Install Smoke Test" job (.github/workflows/ci.yml)
# runs this script directly against the amplihack binary that job installs.
# CI does not run gadugi-test itself; tests/gadugi/scenarios/
# issue-2815-piped-prompt-wait.yaml wraps this script for anyone running it
# through gadugi-test by hand.
#
# Run by hand: bash tests/gadugi/run-issue-2815-piped-prompt-wait-scenario.sh
# It builds amplihack with cargo first. Set AMPLIHACK_2815_QA_BIN to an
# amplihack binary to skip the build. On success it prints ALL_CASES_PASSED
# and exits 0; otherwise SCENARIO_FAILED and exit 1.
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd -P)"

for c in grep sed awk find timeout mkfifo; do
  p="$(command -v "$c" 2>/dev/null)"
  case "$p" in
    /*) ;;
    *) echo "FAIL: $c is required"; echo "SCENARIO_FAILED"; exit 1 ;;
  esac
done
# One checksum tool, named once and used by both the stub and the assertions.
if command -v sha256sum >/dev/null 2>&1; then
  SUM_CMD=sha256sum
elif command -v shasum >/dev/null 2>&1; then
  SUM_CMD="shasum -a 256"
else
  echo "FAIL: neither sha256sum nor shasum is available"; echo "SCENARIO_FAILED"; exit 1
fi

WORK="$(mktemp -d "${TMPDIR:-/tmp}/issue-2815-qa.XXXXXX")" || {
  echo "FAIL: cannot create a temporary directory"; echo "SCENARIO_FAILED"; exit 1
}
WORK="$(cd "$WORK" && pwd -P)" || { echo "FAIL: cannot resolve the temporary directory"; echo "SCENARIO_FAILED"; exit 1; }
trap 'chmod -R -- u+rwx "$WORK" 2>/dev/null; rm -rf -- "$WORK"' EXIT
SYS_PATH="/usr/local/bin:/usr/bin:/bin"

# --- the binary under test --------------------------------------------------
BIN="${AMPLIHACK_2815_QA_BIN:-}"
if [ -z "$BIN" ]; then
  # As run-issue-2778-umask-crusty-scenario.sh does: json-render-diagnostics
  # keeps compiler errors on stderr while stdout carries the artifact records,
  # and one awk reads every record so cargo never writes into a closed pipe
  # (issue #1434). 14 is the length of `"executable":"`; 15 adds the quote.
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
mkdir -p -- "$HOME_DIR" "$CONFIG" "$RECORDS" "$TOOLS" "$RUNDIR" \
  || { echo "FAIL: cannot create the scratch folders"; echo "SCENARIO_FAILED"; exit 1; }
mkdir -m 700 -- "$PROFILES" || { echo "FAIL: cannot create the profile root"; echo "SCENARIO_FAILED"; exit 1; }

# scenario_env <command...>: runs the command with only the variables below.
#   PATH, HOME, TMPDIR, LANG            scratch folders and a fixed locale
#   TZ                                  a fixed zone, so reset times are exact
#   AMPLIHACK_CLAUDE_PROFILE_ROOT       an empty private profile root, never the user's
#   CLAUDE_CONFIG_DIR                   a scratch Claude config, never ~/.claude
#   AMPLIHACK_CLAUDE_BINARY_PATH        the stub agent
#   RECORD_DIR, SUM_CMD, LIMIT_EPOCH,
#   SKIP_STDIN                          the stub's own inputs
#   AMPLIHACK_NONINTERACTIVE            the shape a recipe step runs in
#   AMPLIHACK_NO_FRESHNESS_CHECK,
#   AMPLIHACK_SKIP_AUTO_INSTALL,
#   AMPLIHACK_SKIP_POWER_STEERING,
#   AMPLIHACK_SKIP_REFLECTION           no install, update or reflection work
#   AMPLIHACK_ROTATE_AUTOSTART=0        no detached rotator outlives the scenario
#   AMPLIHACK_GOVERNOR=off              the #1811 capacity governor would hold
#                                       the exhausted reading before the wait
scenario_env() {
  env -i \
    PATH="$TOOLS:$SYS_PATH" \
    HOME="$HOME_DIR" \
    TMPDIR="$WORK/tmp" \
    LANG=C \
    TZ=UTC \
    AMPLIHACK_CLAUDE_PROFILE_ROOT="$PROFILES" \
    CLAUDE_CONFIG_DIR="$CONFIG" \
    AMPLIHACK_CLAUDE_BINARY_PATH="$TOOLS/claude" \
    RECORD_DIR="$RECORDS" \
    SUM_CMD="$SUM_CMD" \
    SKIP_STDIN="${SKIP_STDIN:-}" \
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
mkdir -p -- "$WORK/tmp" || { echo "FAIL: cannot create the scratch TMPDIR"; echo "SCENARIO_FAILED"; exit 1; }

# A reset five minutes ago: the wait has a deadline it has already passed, so
# it starts, ends and resumes without the scenario sleeping.
LIMIT_EPOCH=$(( $(date +%s) - 300 ))

# --- the stub agent ---------------------------------------------------------
cat > "$TOOLS/claude" <<'STUB'
#!/bin/sh
# Stub `claude`: answers --version, counts its calls per $STEP, records the
# SHA-256 of all of its stdin, and reports a usage limit on its first call.
case "$1" in
  --version) echo '2.1.319 (Claude Code)'; exit 0 ;;
esac
step="${STEP:-0}"
n=$(( $(cat "$RECORD_DIR/count.$step" 2>/dev/null || echo 0) + 1 ))
echo "$n" > "$RECORD_DIR/count.$step"
if [ -n "${SKIP_STDIN:-}" ]; then sum=skipped; else sum=$($SUM_CMD | cut -d' ' -f1); fi
{
  printf 'ARGV='
  for a in "$@"; do printf '%s|' "$a"; done
  printf '\n'
  printf 'PWD=%s\n' "$(pwd -P)"
  printf 'HOME=%s\n' "$HOME"
  printf 'CLAUDE_CONFIG_DIR=%s\n' "${CLAUDE_CONFIG_DIR-<unset>}"
  printf 'STDIN=%s\n' "$sum"
} > "$RECORD_DIR/call.$step.$n"
echo "attempt $n output"
if [ "$n" -le "${LIMIT_CALLS:-1}" ]; then
  printf 'Claude AI usage limit reached|%s\n' "$LIMIT_EPOCH" >&2
  exit 1
fi
exit 0
STUB
chmod 755 "$TOOLS/claude" || { echo "FAIL: cannot write the stub agent"; echo "SCENARIO_FAILED"; exit 1; }

# --- the prompt -------------------------------------------------------------
# A NUL byte, bytes that are not valid UTF-8, and far more than a pipe holds,
# so the copy is exercised rather than fitting in the kernel's buffer.
{
  printf 'the piped prompt '
  head -c 1 /dev/zero
  printf '\377\376\n'
  i=0
  while [ "$i" -lt 8000 ]; do
    printf 'line %s of the piped prompt that no argv carries\n' "$i"
    i=$(( i + 1 ))
  done
} > "$WORK/prompt" 2>/dev/null
if [ ! -s "$WORK/prompt" ]; then
  echo "FAIL: could not build the prompt"; echo "SCENARIO_FAILED"; exit 1
fi
WANT_SUM="$($SUM_CMD < "$WORK/prompt" | cut -d' ' -f1)"

# --- case 1: the limit is waited out and the rerun gets the same prompt ------
# `-p` with no prompt word, and stdin a pipe: exactly how a recipe agent step
# is launched. `cat |` keeps stdin a pipe; a redirect would make it a file,
# which was never the failing shape.
cd -- "$RUNDIR" || { echo "FAIL: cannot enter the working folder"; echo "SCENARIO_FAILED"; exit 1; }
cat -- "$WORK/prompt" | scenario_env "$BIN" claude --subprocess-safe -- -p \
  >"$WORK/run.out" 2>"$WORK/run.err"
RUN_RC=${PIPESTATUS[1]}

fail=0
pass() { echo "PASS: $1 - $2"; }
fl() { echo "FAIL: $1 - $2"; fail=1; }
DIAG="rc=$RUN_RC run.err tail: $(tail -n 12 "$WORK/run.err" 2>/dev/null | tr '\n' ' ')"

CALLS="$(cat "$RECORDS/count.0" 2>/dev/null || echo 0)"
SUM1="$(sed -n 's/^STDIN=//p' "$RECORDS/call.0.1" 2>/dev/null)"
SUM2="$(sed -n 's/^STDIN=//p' "$RECORDS/call.0.2" 2>/dev/null)"
# `grep -c` prints 0 and exits 1 when nothing matches, so its status is not
# used; a missing file gives no output at all and counts as 0.
count_in() {
  c="$(grep -c -- "$1" "$2" 2>/dev/null)"
  [ -n "$c" ] || c=0
  printf '%s' "$c"
}
WAIT_LINES="$(count_in 'amplihack.recipe.usage_limit_wait ' "$WORK/run.err")"
STARTS="$(count_in 'amplihack.recipe.usage_limit_wait start reason=usage_limit' "$WORK/run.err")"
ENDS="$(count_in 'amplihack.recipe.usage_limit_wait end reason=' "$WORK/run.err")"
RESUMES="$(count_in 'amplihack.recipe.usage_limit_wait resume attempt=2' "$WORK/run.err")"
SKIPPED="$(count_in 'skipped reason=piped_prompt' "$WORK/run.err")"

# 1. The run succeeded instead of ending on the first limit.
if [ "$RUN_RC" -eq 0 ]; then
  pass step_succeeded_after_the_limit "the supervised step exited 0"
else
  fl step_succeeded_after_the_limit "$DIAG"
fi

# 2. The step ran a second time.
if [ "$CALLS" = 2 ]; then
  pass step_ran_again "the agent was called twice"
else
  fl step_ran_again "the agent was called ${CALLS} time(s), want 2; $DIAG"
fi

# 3. Both calls read the prompt, byte for byte as it was written.
if [ -n "$SUM1" ] && [ "$SUM1" = "$WANT_SUM" ] && [ "$SUM2" = "$WANT_SUM" ]; then
  pass rerun_read_the_same_prompt "both calls read the whole piped prompt, byte for byte"
else
  fl rerun_read_the_same_prompt "call 1 '${SUM1:-none}', call 2 '${SUM2:-none}', want '$WANT_SUM'; $DIAG"
fi

# 4. One wait, reported once, with its reset and its end.
if [ "$STARTS" = 1 ] && [ "$ENDS" = 1 ] && [ "$RESUMES" = 1 ] && [ "$WAIT_LINES" = 3 ] \
   && grep -q 'usage_limit_wait start reason=usage_limit harness=claude reset=' "$WORK/run.err"; then
  pass one_wait_reported_with_its_reset "the run log holds one wait start naming the reset, one end and one resume"
else
  fl one_wait_reported_with_its_reset "start=$STARTS end=$ENDS resume=$RESUMES total=$WAIT_LINES, want 1/1/1/3; $DIAG"
fi

# 5. The old refusal is gone.
if [ "$SKIPPED" = 0 ]; then
  pass no_piped_prompt_refusal "the run never wrote skipped reason=piped_prompt"
else
  fl no_piped_prompt_refusal "the run still refused the wait with piped_prompt; $DIAG"
fi

# 6. The private copy of the prompt is gone once the step has finished.
LEFT="$(find "$HOME_DIR/.amplihack/rerun" -maxdepth 1 -name '*.in' -print 2>/dev/null | tr '\n' ' ')"
if [ -z "$LEFT" ]; then
  pass no_captured_prompt_left_behind "no copy of the prompt was left in the rerun scratch folder"
else
  fl no_captured_prompt_left_behind "left behind: ${LEFT//"$WORK"\//}"
fi

# --- case 2: a held-open pipe with the prompt in argv still does not hang ----
# The regression guard of issue #1618: the prompt is in argv, so nothing is
# copied, and a caller that holds stdin open without ever writing or closing
# it must not stop the step. A FIFO nothing writes to stands in for that
# caller, which never writes and never closes; the agent is told not to read
# its stdin ($SKIP_STDIN), as a real agent given its prompt in argv does not.
# The step is given 120 s, far longer than it needs.
rm -f -- "$RECORDS"/count.* "$RECORDS"/call.* 2>/dev/null
FIFO="$WORK/held.fifo"
if mkfifo -- "$FIFO" 2>/dev/null; then
  # A writer that holds the pipe open for the whole of the step and never
  # writes: the shell keeps the descriptor until the subshell is stopped.
  sleep 120 > "$FIFO" &
  HOLDER=$!
  # `env -i` resolves `timeout` with the scenario's own PATH, so the ceiling
  # is inside the isolated environment rather than outside it.
  SKIP_STDIN=1 scenario_env timeout 120 "$BIN" claude --subprocess-safe -- -p step \
    <"$FIFO" >"$WORK/held.out" 2>"$WORK/held.err"
  HELD_RC=$?
  # Only the holder this scenario started, by the pid it recorded.
  kill "$HOLDER" 2>/dev/null
  wait "$HOLDER" 2>/dev/null
  HELD_CALLS="$(cat "$RECORDS/count.0" 2>/dev/null || echo 0)"
  if [ "$HELD_RC" -eq 0 ] && [ "$HELD_CALLS" = 2 ]; then
    pass held_open_stdin_does_not_hang "an argv prompt with stdin held open still waited and ran again"
  else
    fl held_open_stdin_does_not_hang "rc=$HELD_RC calls=$HELD_CALLS (124 is the 120 s timeout); held.err tail: $(tail -n 8 "$WORK/held.err" 2>/dev/null | tr '\n' ' ')"
  fi
else
  fl held_open_stdin_does_not_hang "could not create a FIFO for the held-open case"
fi

if [ "$fail" -eq 0 ]; then
  echo "ALL_CASES_PASSED"
  exit 0
fi
echo "SCENARIO_FAILED"
exit 1
