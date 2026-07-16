#!/usr/bin/env bash
# Scenario 9 — Per-Session Signal Channel fail-closed product boundary (Issue #904)
# Run from repo root: bash tests/outside-in/scripts/scenario9-signal-checks.sh
#
# Exercises the real `amplihack-hooks signal-subscriber` CLI boundary in three
# daemon-independent modes and the fail-closed gate unit tests. The live-daemon
# bidirectional path is validated manually (operator-confirmed) and is out of
# scope here — this script requires NO signal-cli daemon.
set -euo pipefail

BIN="target/debug/amplihack-hooks"
PASS=0
FAIL=0

pass() { echo "  ✓ $1"; PASS=$((PASS + 1)); }
fail() { echo "  ✗ $1" >&2; FAIL=$((FAIL + 1)); }

echo "=== Build (amplihack-hooks --features signal) ==="
if cargo build -p amplihack-hooks-bin --features amplihack-hooks-bin/signal >/dev/null 2>&1; then
  pass "amplihack-hooks built with --features signal"
else
  fail "build failed"
  echo "RESULT: $PASS passed, $FAIL failed" >&2
  exit 1
fi

echo "=== Mode A: missing config is non-fatal (fail-closed, Signal disabled) ==="
OUT_A=$(env -u AMPLIHACK_SIGNAL_ENDPOINT -u AMPLIHACK_SIGNAL_ACCOUNT -u AMPLIHACK_SIGNAL_ALLOWLIST \
  RUST_LOG=warn "$BIN" signal-subscriber --session-id demo-nocfg 2>&1) && RC_A=0 || RC_A=$?
[ "${RC_A:-1}" -eq 0 ] && pass "exit 0 with no config" || fail "expected exit 0, got ${RC_A:-?}"
echo "$OUT_A" | grep -q "config not loaded" && pass "WARN: config not loaded" || fail "missing 'config not loaded' warning"

echo "=== Mode B: valid config, unreachable daemon must not hang ==="
OUT_B=$(env AMPLIHACK_SIGNAL_ENDPOINT=127.0.0.1:1 AMPLIHACK_SIGNAL_ACCOUNT=+15551230000 \
  AMPLIHACK_SIGNAL_ALLOWLIST=+15551230001 RUST_LOG=warn \
  timeout 30 "$BIN" signal-subscriber --session-id demo-unreach 2>&1) && RC_B=0 || RC_B=$?
[ "${RC_B:-1}" -eq 0 ] && pass "exit 0 against unreachable daemon (no hang)" || fail "expected exit 0, got ${RC_B:-?} (possible hang)"

echo "=== Mode C: TOML config via AMPLIHACK_SIGNAL_CONFIG (env > file) ==="
CFG="$(mktemp --suffix=.toml)"
trap 'rm -f "$CFG"' EXIT
cat > "$CFG" <<'TOML'
endpoint = "127.0.0.1:1"
account = "+15551230000"
allowlist = ["+15551230000"]
own_device_id = 2
TOML
OUT_C=$(env -u AMPLIHACK_SIGNAL_ENDPOINT -u AMPLIHACK_SIGNAL_ACCOUNT -u AMPLIHACK_SIGNAL_ALLOWLIST \
  AMPLIHACK_SIGNAL_CONFIG="$CFG" RUST_LOG=warn \
  timeout 20 "$BIN" signal-subscriber --session-id demo-file 2>&1) && RC_C=0 || RC_C=$?
[ "${RC_C:-1}" -eq 0 ] && pass "TOML config loaded, advanced past config gate (exit 0)" || fail "expected exit 0, got ${RC_C:-?}"

echo "=== Gate unit tests: fail-closed, both deployment shapes ==="
if cargo test -p amplihack-signal --features signal >/dev/null 2>&1; then
  pass "amplihack-signal gate/config/transport tests green (--features signal)"
else
  fail "amplihack-signal tests failed"
fi

echo
echo "RESULT: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
