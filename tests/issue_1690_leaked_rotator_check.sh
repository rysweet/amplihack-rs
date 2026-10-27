#!/usr/bin/env bash
# Issue #1690 — the leaked-rotator detector finds a `profile rotate watch
# --spawned` process that a test run left behind, and nothing else.
#
# The detector (scripts/check-leaked-rotators.sh) reports, and never signals,
# every process of this uid whose argv holds the exact entries `profile`,
# `rotate`, `watch` and `--spawned`, whose executable is under a `target*/`
# directory (even once deleted), and whose HOME is under $TMPDIR or /tmp.
# CI runs it after the test suite with `if: always()`.
#
# Each decoy is a copy of bash, started as
#   <copy> -c '<sleep loop>' profile rotate watch --interval 600 --spawned
# so its cmdline matches without running amplihack. Only decoys this script
# started (their own `$!`) are ever killed; there is no pkill -f here.
#
# Under test:
#   1. a decoy that matches every rule is reported: exit 1, its pid on a
#      line starting `leaked rotator:`;
#   2. once it is gone the detector is clean again (exit 0, if it was clean
#      before this script started anything);
#   3. near misses are not reported: HOME outside the temp dir, a `/tmpevil`
#      HOME, an exe outside `target*/`, no `--spawned`, `--spawned` only as
#      a substring;
#   4. a decoy whose exe was deleted (a rebuilt test binary) is still found;
#   5. control characters in HOME are stripped from the report;
#   6. the detector itself never signals and never traces or evals;
#   7. an exe under $CARGO_TARGET_DIR is found even when that directory is
#      reached through a symlink and its name holds no `target`.
set +e -u

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DETECT="$ROOT/scripts/check-leaked-rotators.sh"

fails=0
pass() { printf '  ok    %s\n' "$1"; }
fail() { printf '  FAIL  %s\n' "$1"; fails=$((fails + 1)); }

if [ ! -f "$DETECT" ]; then
  echo "missing $DETECT"
  exit 1
fi
if [ ! -r /proc/self/cmdline ]; then
  echo "no /proc on this host; the detector is Linux-only, nothing to test"
  exit 0
fi

# --- 6. static rules: the script never signals, traces or evals ------------
if grep -nE '\b(kill|pkill|killall|eval)\b|set -x' "$DETECT" | grep -v '^[0-9]*:[[:space:]]*#'; then
  fail "detector contains kill/pkill/killall/eval/set -x"
else
  pass "detector never signals, evals or traces"
fi

tmp=""
decoys=()
cleanup() {
  local p
  # Signal only decoys still running from $tmp: a reaped PID may be reused.
  for p in "${decoys[@]}"; do
    case "$(readlink "/proc/$p/exe" 2>/dev/null)" in
      "$tmp"/*) kill "$p" 2>/dev/null; wait "$p" 2>/dev/null ;;
    esac
  done
  [ -n "$tmp" ] && [ -d "$tmp" ] && rm -rf -- "$tmp"
}
trap cleanup EXIT

tmp=$(mktemp -d)
mkdir -p "$tmp/target/debug" "$tmp/bin" "$tmp/home" "$tmp/cargo-out/debug"
ln -s "$tmp/cargo-out" "$tmp/out-link"
BASH_BIN=$(command -v bash)
cp "$BASH_BIN" "$tmp/target/debug/amplihack"
cp "$BASH_BIN" "$tmp/target/debug/amplihack-deleted"
cp "$BASH_BIN" "$tmp/bin/amplihack"
cp "$BASH_BIN" "$tmp/cargo-out/debug/amplihack"

LOOP='trap "exit 0" TERM; while :; do sleep 1 & wait $!; done'
ARGV_OK=(profile rotate watch --interval 600 --spawned)

# start_decoy HOME EXE ARGV... -> sets $decoy_pid
decoy_pid=""
start_decoy() {
  local home=$1 exe=$2
  shift 2
  HOME="$home" "$exe" -c "$LOOP" "$@" </dev/null >/dev/null 2>&1 &
  decoy_pid=$!
  decoys+=("$decoy_pid")
  # Wait for the exec to replace the fork's argv (bounded, never forever).
  # Every decoy has `profile`; the near misses may lack `watch`.
  local _
  for _ in 1 2 3 4 5 6 7 8 9 10; do
    if tr '\0' ' ' <"/proc/$decoy_pid/cmdline" 2>/dev/null | grep -q -- "profile"; then
      return 0
    fi
    sleep 0.1
  done
  return 1
}
stop_decoy() {
  kill "$1" 2>/dev/null
  wait "$1" 2>/dev/null
  # Forget the reaped PID so cleanup never signals a process that reused it.
  local keep=() p
  for p in "${decoys[@]}"; do [ "$p" = "$1" ] || keep+=("$p"); done
  decoys=("${keep[@]}")
}

run_detect() {
  out=$(bash "$DETECT" 2>&1)
  rc=$?
}
reported() { printf '%s\n' "$out" | grep -E '^leaked rotator:' | grep -qw -- "$1"; }

# Baseline: on a developer host another worktree's run may be in flight.
run_detect
base_rc=$rc
base_out=$out
if [ "$base_rc" -ne 0 ] && [ "$base_rc" -ne 1 ]; then
  fail "baseline scan exited $base_rc (expected 0 or 1): $base_out"
fi

# --- 1. a matching decoy is reported ----------------------------------------
if ! start_decoy "$tmp/home" "$tmp/target/debug/amplihack" "${ARGV_OK[@]}" \
  || ! kill -0 "$decoy_pid" 2>/dev/null; then
  fail "decoy did not start (noexec temp dir?)"
  exit 1
fi
match_pid=$decoy_pid
run_detect
if [ "$rc" -eq 1 ]; then pass "a leaked rotator makes the detector exit 1"; else fail "exit $rc with a leaked rotator, want 1: $out"; fi
if reported "$match_pid"; then pass "its pid is reported on a 'leaked rotator:' line"; else fail "pid $match_pid not reported: $out"; fi
if printf '%s\n' "$out" | grep -F -- "$tmp/home" | grep -q '^leaked rotator:'; then
  pass "the report names its HOME"
else
  fail "HOME $tmp/home not in the report: $out"
fi
if printf '%s\n' "$out" | grep -qE 'PATH=|LOOP|while :'; then
  fail "the report leaks environ or the full cmdline: $out"
else
  pass "the report holds no environ or full cmdline"
fi

# --- 3. near misses ---------------------------------------------------------
near_miss() {
  local label=$1
  shift
  if ! start_decoy "$@"; then
    fail "$label: decoy did not start"
    return
  fi
  local p=$decoy_pid
  run_detect
  if reported "$p"; then fail "$label: wrongly reported: $out"; else pass "$label: not reported"; fi
  stop_decoy "$p"
}
near_miss "HOME outside the temp dir" /var/empty/amplihack-1690 "$tmp/target/debug/amplihack" "${ARGV_OK[@]}"
near_miss "HOME under /tmpevil, not /tmp" /tmpevil/home "$tmp/target/debug/amplihack" "${ARGV_OK[@]}"
near_miss "exe outside target*/" "$tmp/home" "$tmp/bin/amplihack" "${ARGV_OK[@]}"
near_miss "no --spawned" "$tmp/home" "$tmp/target/debug/amplihack" profile rotate watch --interval 600
near_miss "--spawned only as a substring" "$tmp/home" "$tmp/target/debug/amplihack" profile rotate watch --interval 600 --spawned-not
near_miss "rotate status, not watch" "$tmp/home" "$tmp/target/debug/amplihack" profile rotate status --spawned

# --- 4. a deleted exe is still found ----------------------------------------
if start_decoy "$tmp/home" "$tmp/target/debug/amplihack-deleted" "${ARGV_OK[@]}"; then
  del_pid=$decoy_pid
  rm -f -- "$tmp/target/debug/amplihack-deleted"
  run_detect
  if reported "$del_pid"; then pass "an exe marked (deleted) is still found"; else fail "deleted-exe decoy $del_pid missed: $out"; fi
  if printf '%s\n' "$out" | grep -q '(deleted)'; then
    fail "the ' (deleted)' suffix should be stripped from exe: $out"
  else
    pass "the ' (deleted)' suffix is stripped"
  fi
  stop_decoy "$del_pid"
else
  fail "deleted-exe decoy did not start"
fi

# --- 7. a custom CARGO_TARGET_DIR, reached through a symlink ---------------
# /proc/<pid>/exe holds the resolved path, which need not contain `target`.
if start_decoy "$tmp/home" "$tmp/out-link/debug/amplihack" "${ARGV_OK[@]}"; then
  ctd_pid=$decoy_pid
  run_detect
  if reported "$ctd_pid"; then fail "exe outside any build dir wrongly reported: $out"; else pass "exe in an unknown build dir: not reported"; fi
  out=$(CARGO_TARGET_DIR="$tmp/out-link" bash "$DETECT" 2>&1)
  if reported "$ctd_pid"; then pass "exe under a symlinked CARGO_TARGET_DIR is found"; else fail "CARGO_TARGET_DIR decoy $ctd_pid missed: $out"; fi
  stop_decoy "$ctd_pid"
else
  fail "CARGO_TARGET_DIR decoy did not start"
fi

# --- 5. control characters are stripped -------------------------------------
esc_home="$tmp/home/x$(printf '\033')]0;pwn$(printf '\007')y"
if start_decoy "$esc_home" "$tmp/target/debug/amplihack" "${ARGV_OK[@]}"; then
  esc_pid=$decoy_pid
  run_detect
  if reported "$esc_pid"; then pass "a HOME with control characters is reported"; else fail "control-char decoy missed: $out"; fi
  if printf '%s' "$out" | LC_ALL=C grep -q "$(printf '[\001-\010\013-\037\177]')"; then
    fail "the report contains control characters"
  else
    pass "control characters are stripped from the report"
  fi
  stop_decoy "$esc_pid"
else
  fail "control-char decoy did not start"
fi

# --- 2. clean again once the decoy is gone ----------------------------------
stop_decoy "$match_pid"
run_detect
if reported "$match_pid"; then fail "pid $match_pid still reported after it exited: $out"; else pass "a reaped decoy is no longer reported"; fi
if [ "$base_rc" -eq 0 ]; then
  if [ "$rc" -eq 0 ]; then pass "clean scan exits 0"; else fail "exit $rc after cleanup, want 0: $out"; fi
else
  echo "  skip  clean-exit check: leaks from elsewhere were present before this test"
fi

echo
if [ "$fails" -eq 0 ]; then
  echo "issue #1690 leaked-rotator check: all passed"
  exit 0
fi
echo "issue #1690 leaked-rotator check: $fails failed"
exit 1
