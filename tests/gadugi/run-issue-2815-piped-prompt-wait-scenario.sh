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
# The limit message carries its reset as an epoch a few minutes in the past.
# Before issue #2911 that was the one form that gave a real wait a deadline it
# had already passed, so the wait ended at once and the whole scenario took
# seconds. Since #2911 a passed reset adds no deadline candidate at all: it
# starts a capacity wait, `reason=passed_reset`, bounded by `max_wait_secs`
# alone. The scenario therefore lowers that to MAX_WAIT_SECS, the 60-second
# floor, so each case spends about a minute in its wait and the whole scenario
# takes a few minutes. The debug-only clock seams of the Rust tests are
# deliberately not used: CI runs this against the release binary it installs,
# where those seams are compiled out.
#
# Every scenario command runs under `env -i` with an explicit allowlist (see
# scenario_env below): no token, key or setting from the caller's environment
# reaches it, HOME is a scratch folder, and the real ~/.claude and
# ~/.amplihack are never read or written. Everything is created under one
# mktemp folder that the EXIT trap removes.
#
# The cases:
#   1. `cat prompt | amplihack claude --subprocess-safe -- -p`, the supervisor
#      driven directly, which is the smallest shape that reproduces the bug.
#   2. a held-open stdin pipe with the prompt in argv: the #1618 guard, which
#      copies nothing.
#  2b. a held-open stdin pipe with a bare `-p`, which the supervisor does
#      copy: the same guard on the capture path (issues #2816, #3036, review
#      of PR #2992). The step must not wait on a pipe nobody closes. It
#      waits out its limit and runs the retry at once, from the copy already
#      held, with the scenario still holding the pipe until the step ends.
#   3. a one-step recipe through `amplihack recipe run` and the pinned recipe
#      runner, which is the shape the issue's Check names. The runner builds
#      its own argv (`--system-prompt-file F -p --add-dir . --model M`) and
#      writes the task to the launch's stdin on a thread of its own; nothing
#      in this script constructs that. It needs recipe-runner-rs and git;
#      without either it prints SKIP, or fails under
#      AMPLIHACK_2815_QA_REQUIRE_ALL=1, as the issue #1619 scenario does.
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
#   AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS the outer bound of one wait, lowered to the
#                                       60-second floor (issue #2911)
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
    AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS="$MAX_WAIT_SECS" \
    "$@"
}
mkdir -p -- "$WORK/tmp" || { echo "FAIL: cannot create the scratch TMPDIR"; echo "SCENARIO_FAILED"; exit 1; }

# A reset five minutes ago. Issue #2911: a reset the clock has already passed
# adds no deadline candidate, so the wait it starts runs for MAX_WAIT_SECS and
# is named `passed_reset` rather than ending the moment it begins.
LIMIT_EPOCH=$(( $(date +%s) - 300 ))

# The outer bound of one capacity wait (issue #2911: a bound on one wait, not
# a per-step budget), lowered to plan::MIN_WAIT_SECS. A smaller value would be raised to the floor
# anyway and would print a warning, so 60 is the shortest honest setting.
MAX_WAIT_SECS=60
# A hang detector and nothing else: one wait plus generous room for the step's
# own work. No check below reads the clock. Without it, case 1 hung CI's
# "Install Smoke Test" job for hours when the wait stopped being instant,
# with nothing naming the case that hung.
CEILING=$(( MAX_WAIT_SECS + 120 ))
# Case 3's own ceiling. It starts the recipe runner, discovers a checkout and
# runs a whole one-step recipe around the same single wait, so it is given more
# room than the three direct launches.
RECIPE_CEILING=$(( MAX_WAIT_SECS + 240 ))

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
if [ -n "${SKIP_STDIN:-}" ]; then
  sum=skipped
else
  cat > "$RECORD_DIR/stdin.$step.$n"
  sum=$($SUM_CMD < "$RECORD_DIR/stdin.$step.$n" | cut -d' ' -f1)
fi
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
# The ceiling is a hang detector, as in cases 2 and 2b: the step waits one
# MAX_WAIT_SECS and then succeeds, so anything near the ceiling is a hang.
cat -- "$WORK/prompt" | scenario_env timeout "$CEILING" "$BIN" claude --subprocess-safe -- -p \
  >"$WORK/run.out" 2>"$WORK/run.err"
RUN_RC=${PIPESTATUS[1]}

fail=0
pass() { echo "PASS: $1 - $2"; }
fl() { echo "FAIL: $1 - $2"; fail=1; }
DIAG="rc=$RUN_RC (124 is the ${CEILING} s ceiling) run.err tail: $(tail -n 12 "$WORK/run.err" 2>/dev/null | tr '\n' ' ')"

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
# Issue #2911: a passed reset is a capacity wait, announced by one
# `waiting_for_capacity` line before the `start`.
CAPACITY="$(count_in 'waiting_for_capacity reason=passed_reset' "$WORK/run.err")"
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

# 4. One wait, reported once: the capacity line that names why, the start
#    naming the reset, the end and the resume. Four lines, no more.
#    Issue #2911 also put `wait=` on the start line, between the limit's
#    `reason=` and the harness, so the shape is pinned here: `reason=` names
#    the limit and `wait=` names what the step is waiting for.
if [ "$CAPACITY" = 1 ] && [ "$STARTS" = 1 ] && [ "$ENDS" = 1 ] && [ "$RESUMES" = 1 ] \
   && [ "$WAIT_LINES" = 4 ] \
   && grep -q 'usage_limit_wait start reason=usage_limit wait=capacity harness=claude reset=' "$WORK/run.err"; then
  pass one_wait_reported_with_its_reset "the run log holds one passed_reset capacity line, one wait start naming the kind and the reset, one end and one resume"
else
  fl one_wait_reported_with_its_reset "capacity=$CAPACITY start=$STARTS end=$ENDS resume=$RESUMES total=$WAIT_LINES, want 1/1/1/1/4, and one start line shaped 'reason=usage_limit wait=capacity harness=claude reset='; $DIAG"
fi

# 5. The old refusal is gone.
if [ "$SKIPPED" = 0 ]; then
  pass no_piped_prompt_refusal "the run never wrote skipped reason=piped_prompt"
else
  fl no_piped_prompt_refusal "the run still refused the wait with piped_prompt; $DIAG"
fi

# 6. No copy of the prompt is left on disk. Since the review of PR #2992 the
#    copy is kept in memory and none is written; the check stays so that a
#    change which wrote one again would have to remove it.
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
rm -f -- "$RECORDS"/count.* "$RECORDS"/call.* "$RECORDS"/stdin.* 2>/dev/null
FIFO="$WORK/held.fifo"
# The scenario holds the write end itself, for exactly as long as the step
# runs and not a second longer or shorter: no holder process, and no clock
# deciding how long "as long as the step runs" is. `<>` opens the FIFO
# without waiting for a reader, which a plain `>` would do; a `>` that found
# no reader, or a reader that found no writer, would wait on the other.
if mkfifo -- "$FIFO" 2>/dev/null && exec 9<>"$FIFO"; then
  # The ceiling is a hang detector and nothing else: no check below reads the
  # clock, and the step needs one MAX_WAIT_SECS plus a moment's work. It is
  # deliberately far above anything load can cause, as
  # docs/bugfixes/bugfix-937-recipe-runner-timeout-test-flake.md settled on for
  # the same problem; without it a step that hung would hang the scenario with
  # nothing naming the case that hung.
  # `env -i` resolves `timeout` with the scenario's own PATH, so the ceiling
  # is inside the isolated environment rather than outside it.
  SKIP_STDIN=1 scenario_env timeout "$CEILING" "$BIN" claude --subprocess-safe -- -p step \
    <"$FIFO" >"$WORK/held.out" 2>"$WORK/held.err"
  HELD_RC=$?
  exec 9>&-
  HELD_CALLS="$(cat "$RECORDS/count.0" 2>/dev/null || echo 0)"
  if [ "$HELD_RC" -eq 0 ] && [ "$HELD_CALLS" = 2 ]; then
    pass held_open_stdin_does_not_hang "an argv prompt with stdin held open still waited and ran again"
  else
    fl held_open_stdin_does_not_hang "rc=$HELD_RC calls=$HELD_CALLS (124 is the ${CEILING} s ceiling); held.err tail: $(tail -n 8 "$WORK/held.err" 2>/dev/null | tr '\n' ' ')"
  fi
else
  fl held_open_stdin_does_not_hang "could not create or hold open a FIFO for the held-open case"
fi

# --- case 2b: a held-open pipe the supervisor DID copy does not hang ------
# Issues #2816 and #3036. Case 2 keeps the prompt in argv, so nothing is
# copied and the copier never runs. This is the other half: a bare `-p`, so
# the supervisor copies stdin, and a caller that holds the pipe open and
# writes nothing to it. The agent is told not to read its stdin
# ($SKIP_STDIN), which is what makes the shape reachable: a launch whose
# prompt this version's option table misread, or any caller that leaves an
# idle pipe on fd 0.
#
# The copier ends only when that caller closes the pipe, and nothing
# promises it ever will. Before #2816 the supervisor joined the copier after
# its wait and blocked for ever, deaf to SIGTERM and with nothing in the log
# saying what it was waiting for. #2816 refused the retry by name instead,
# with `skipped reason=piped_prompt`, which ended the step on a limit for a
# prompt that was merely late. The round-4 fix for #3036 waited for the
# caller to close the pipe, with a `waiting_for_prompt` line, and this case
# then hung at the ceiling (Actions run 45059566162). Since the review of
# PR #2992 the retry starts at once from the copy already held and is fed
# the rest as it arrives, so nothing waits on the caller's pipe.
#
# The scenario holds its end of the pipe until the step has exited, so a
# wait of any kind for it hangs this case at the ceiling rather than
# passing it.
rm -f -- "$RECORDS"/count.* "$RECORDS"/call.* "$RECORDS"/stdin.* 2>/dev/null
FIFO2="$WORK/held-copied.fifo"
if mkfifo -- "$FIFO2" 2>/dev/null && exec 8<>"$FIFO2"; then
  # The ceiling is a hang detector and nothing else, as in case 2. `8>&-`
  # keeps the scenario's own end of the FIFO out of the step, so the only
  # writer of the step's stdin is the scenario, which closes it below, after
  # the step has ended.
  SKIP_STDIN=1 scenario_env timeout "$CEILING" "$BIN" claude --subprocess-safe -- -p \
    <"$FIFO2" >"$WORK/held2.out" 2>"$WORK/held2.err" 8>&-
  HELD2_RC=$?
  exec 8>&-
  HELD2_CALLS="$(cat "$RECORDS/count.0" 2>/dev/null || echo 0)"
  HELD2_WAITING="$(count_in 'usage_limit_wait waiting_for_prompt' "$WORK/held2.err")"
  HELD2_RESUME="$(count_in 'usage_limit_wait resume attempt=2' "$WORK/held2.err")"
  HELD2_SKIP="$(count_in 'skipped reason=piped_prompt' "$WORK/held2.err")"
  HELD2_LOST="$(count_in 'prompt_cut_short' "$WORK/held2.err")"
  if [ "$HELD2_RC" -eq 0 ] && [ "$HELD2_CALLS" = 2 ] && [ "$HELD2_WAITING" = 0 ] \
     && [ "$HELD2_RESUME" = 1 ] && [ "$HELD2_SKIP" = 0 ] && [ "$HELD2_LOST" = 0 ]; then
    pass held_open_copied_pipe_does_not_hang \
      "a captured pipe held open for the whole step did not hold it: the retry ran at once from the copy held"
  else
    fl held_open_copied_pipe_does_not_hang \
      "rc=$HELD2_RC calls=$HELD2_CALLS waiting=$HELD2_WAITING resume=$HELD2_RESUME skipped=$HELD2_SKIP lost=$HELD2_LOST, want 0/2/0/1/0/0 (124 is the ${CEILING} s ceiling); held2.err tail: $(tail -n 8 "$WORK/held2.err" 2>/dev/null | tr '\n' ' ')"
  fi
else
  fl held_open_copied_pipe_does_not_hang "could not create or hold open a FIFO for the captured held-open case"
fi

# --- case 3: the same limit in a real recipe step, through the runner ------
# The issue's Check in full: "a recipe step whose fake Claude reports a usage
# limit once, with the prompt piped on stdin, must wait, rerun with the same
# prompt and finish the run."
#
# Nothing here imitates the runner. `amplihack recipe run` starts the pinned
# recipe-runner-rs, which resolves the agent step, builds the launch argv
# itself and writes the step's task to the launch's stdin on a thread of its
# own, then closes it. The launcher it starts is the binary under test
# (AMPLIHACK_LAUNCHER_BINARY), so the supervisor under test is in the chain
# exactly as it is on a real host.
#
# The wait is the only thing that can produce a second call: the profile root
# is an empty private directory, so auto-switch has no candidate to rerun the
# step under, and the assertions below check that both calls ran in the same
# Claude configuration directory, which an auto-switch rerun would have
# changed. The agent's own stderr, which carries the
# `amplihack.recipe.usage_limit_wait` lines, is redirected by the runner to a
# log file it removes when the step succeeds, so those lines are case 1's to
# assert and not this case's.
RECIPE_SENTINEL='PROMPT-SENTINEL-2815-ON-STDIN'
RUNNER="${RECIPE_RUNNER_RS_PATH:-$(command -v recipe-runner-rs 2>/dev/null || true)}"
GIT_BIN="$(command -v git 2>/dev/null || true)"

recipe_skip() {
  if [ "${AMPLIHACK_2815_QA_REQUIRE_ALL:-0}" = 1 ]; then
    fl recipe_step_waited_and_reran "could not run ($1) and AMPLIHACK_2815_QA_REQUIRE_ALL=1"
  else
    echo "SKIP: recipe_step_waited_and_reran ($1)"
  fi
}

run_recipe_case() {
  CASE="$WORK/recipe"
  mkdir -p -- "$CASE/home/.amplihack" "$CASE/project" "$CASE/work" "$CASE/tmp" \
    || return 1
  # No -p: `$CASE/home` exists already, and -m with -p would set the mode
  # on the deepest directory only (shellcheck SC2174).
  mkdir -m 700 -- "$CASE/home/profiles" || return 1
  chmod -R go-rwx -- "$CASE" || return 1

  cat > "$WORK/piped-prompt-qa.yaml" <<RECIPE
name: "issue-2815-piped-prompt-qa"
description: "one agent step whose prompt is delivered on stdin (issue 2815 QA)"
version: "1.0.0"
context: {}
steps:
  - id: "step-01-implement"
    type: "agent"
    prompt: |
      $RECIPE_SENTINEL
      Reply with the single word done and stop. Change nothing.
RECIPE

  # A git project with one commit: checkout discovery needs a repository.
  qa_git() {
    env -i HOME="$CASE/home" PATH="$SYS_PATH" GIT_CONFIG_NOSYSTEM=1 \
      GIT_CONFIG_GLOBAL=/dev/null GIT_TERMINAL_PROMPT=0 \
      "$GIT_BIN" -C "$CASE/project" \
      -c user.name=qa -c user.email=qa@example.invalid -c commit.gpgsign=false "$@" \
      </dev/null >/dev/null 2>&1
  }
  qa_git init -q || return 1
  printf 'issue 2815 QA\n' > "$CASE/project/README.md"
  qa_git add README.md || return 1
  qa_git commit -q -m init || return 1

  # No CLAUDE_CONFIG_DIR: a recipe agent step runs on $HOME/.claude, and
  # HOME is this case's own scratch home.
  ( cd "$CASE/project" && exec env -i \
      PATH="$TOOLS:$SYS_PATH" \
      HOME="$CASE/home" \
      TMPDIR="$CASE/tmp" \
      LANG=C TZ=UTC TERM=dumb \
      AMPLIHACK_CLAUDE_PROFILE_ROOT="$CASE/home/profiles" \
      AMPLIHACK_CLAUDE_BINARY_PATH="$TOOLS/claude" \
      RECORD_DIR="$RECORDS" \
      SUM_CMD="$SUM_CMD" \
      LIMIT_EPOCH="$LIMIT_EPOCH" \
      AMPLIHACK_NONINTERACTIVE=1 \
      AMPLIHACK_NO_FRESHNESS_CHECK=1 \
      AMPLIHACK_SKIP_AUTO_INSTALL=1 \
      AMPLIHACK_SKIP_POWER_STEERING=1 \
      AMPLIHACK_SKIP_REFLECTION=1 \
      AMPLIHACK_ROTATE_AUTOSTART=0 \
      AMPLIHACK_GOVERNOR=off \
      AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS="$MAX_WAIT_SECS" \
      AMPLIHACK_CLEANUP_ON_EXIT=0 \
      AMPLIHACK_WORK_ROOT="$CASE/work" \
      AMPLIHACK_LAUNCHER_BINARY="$BIN" \
      AMPLIHACK_AGENT_BINARY=claude \
      RECIPE_RUNNER_RS_PATH="$RUNNER" \
      timeout "$RECIPE_CEILING" \
      "$BIN" recipe run "$WORK/piped-prompt-qa.yaml" -w "$CASE/project" --no-detach ) \
    </dev/null >"$WORK/recipe.out" 2>"$WORK/recipe.err"
}

rm -f -- "$RECORDS"/count.* "$RECORDS"/call.* "$RECORDS"/stdin.* 2>/dev/null
if [ -z "$RUNNER" ] || [ ! -x "$RUNNER" ]; then
  recipe_skip "no recipe-runner-rs"
elif [ -z "$GIT_BIN" ]; then
  recipe_skip "no git"
else
  run_recipe_case
  RECIPE_RC=$?
  R_CALLS="$(cat "$RECORDS/count.0" 2>/dev/null || echo 0)"
  R_SUM1="$(sed -n 's/^STDIN=//p' "$RECORDS/call.0.1" 2>/dev/null)"
  R_SUM2="$(sed -n 's/^STDIN=//p' "$RECORDS/call.0.2" 2>/dev/null)"
  R_ARGV1="$(sed -n 's/^ARGV=//p' "$RECORDS/call.0.1" 2>/dev/null)"
  R_ARGV2="$(sed -n 's/^ARGV=//p' "$RECORDS/call.0.2" 2>/dev/null)"
  R_CFG1="$(sed -n 's/^CLAUDE_CONFIG_DIR=//p' "$RECORDS/call.0.1" 2>/dev/null)"
  R_CFG2="$(sed -n 's/^CLAUDE_CONFIG_DIR=//p' "$RECORDS/call.0.2" 2>/dev/null)"
  RDIAG="rc=$RECIPE_RC (124 is the ${RECIPE_CEILING} s ceiling) calls=$R_CALLS recipe.err tail: $(tail -n 12 "$WORK/recipe.err" 2>/dev/null | tr '\n' ' ')"
  problems=""
  add() { problems="$problems $1;"; }

  [ "$RECIPE_RC" -eq 0 ] || add "the recipe run exited $RECIPE_RC"
  [ "$R_CALLS" = 2 ] || add "the agent was called ${R_CALLS} time(s), want 2"
  # The prompt arrived on stdin, whole and identical, on both calls.
  if [ -z "$R_SUM1" ] || [ "$R_SUM1" = skipped ] || [ "$R_SUM1" != "$R_SUM2" ]; then
    add "stdin differed between the calls ('${R_SUM1:-none}' vs '${R_SUM2:-none}')"
  fi
  for n in 1 2; do
    if ! grep -q -- "$RECIPE_SENTINEL" "$RECORDS/stdin.0.$n" 2>/dev/null; then
      add "call $n did not read the recipe's prompt on stdin"
    fi
  done
  # ... and not in argv, which is the assumption the bug rested on.
  case "$R_ARGV1$R_ARGV2" in
    *"$RECIPE_SENTINEL"*) add "the prompt reached the agent in argv" ;;
  esac
  # The runner's real argv shape, built by the runner and not by this script.
  for a in "$R_ARGV1" "$R_ARGV2"; do
    case "$a" in
      *"|--system-prompt-file|"*) ;;
      *) add "no --system-prompt-file in argv '${a}'" ;;
    esac
    case "$a" in
      *"|-p|"*) ;;
      *) add "no bare -p in argv '${a}'" ;;
    esac
  done
  # The same login both times, so the rerun was the wait's, not a profile swap.
  [ "$R_CFG1" = "$R_CFG2" ] || add "the rerun changed CLAUDE_CONFIG_DIR ($R_CFG1 -> $R_CFG2)"
  # Nothing of the prompt is left on disk once the run is over.
  R_LEFT="$(find "$CASE/home/.amplihack/rerun" "$CASE/home/profiles" -name '*.in' -print 2>/dev/null | tr '\n' ' ')"
  [ -z "$R_LEFT" ] || add "a copy of the prompt was left behind: $R_LEFT"

  if [ -z "$problems" ]; then
    pass recipe_step_waited_and_reran "a recipe agent step met a usage limit, waited, reran and finished with the same stdin prompt"
  else
    fl recipe_step_waited_and_reran "${problems}; $RDIAG"
  fi
fi

if [ "$fail" -eq 0 ]; then
  echo "ALL_CASES_PASSED"
  exit 0
fi
echo "SCENARIO_FAILED"
exit 1
