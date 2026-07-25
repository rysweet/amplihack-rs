#!/usr/bin/env bash
# TDD behavioral tests for signal-setup.sh — the safe, side-effect-free surface.
#
# Run: bash amplifier-bundle/skills/signal-setup/tests/test_script_behavior.sh
#
# These exercise the ACTUAL script with a sandboxed HOME and mocked binaries
# (signal-cli, qrencode, systemd-run, sudo, az, nc) so nothing ever touches the
# real Signal service, systemd, sudo, or Azure. The not-linked *mint* path IS
# driven end-to-end here, but only through the mocked systemd-run (local) and
# mocked az (remote): the mocks synthesise an sgnl:// URI and a post-mint linked
# account, so no real device-link is ever performed and no Signal/Azure endpoint
# is contacted. Source-level invariants for the mint path are additionally
# pinned by test_skill_structure.sh.
#
# What is verified here:
#   * --help succeeds and prints usage;
#   * --host is required; unknown flags are rejected;
#   * strict input validation FAILS CLOSED for command/arg-injection attempts
#     in --host / --group / --phone / --resource-group;
#   * valid inputs pass validation;
#   * idempotency: an already-linked host is a no-op that NEVER renders a QR
#     (local and remote);
#   * remote mode genuinely drives the az CLI;
#   * the mint path (LOCAL via mocked systemd-run, REMOTE via mocked az) renders
#     the QR with ANSIUTF8i, verifies the freshly-minted linkage, and completes;
#   * a failed account probe aborts BEFORE minting ("refusing to mint");
#   * prerequisite failures (missing signal-cli / qrencode) abort clearly.
#
# Each test asserts the exit status AND a content signal so a wrong-reason pass
# cannot slip through.

set -uo pipefail

PASS=0
FAIL=0
pass() { echo "  PASS: $1"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $1"; FAIL=$((FAIL + 1)); }

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
SKILL_DIR="$(dirname "$SCRIPT_DIR")"
IMPL="$SKILL_DIR/scripts/signal-setup.sh"

if [[ ! -x "$IMPL" ]]; then
  echo "  FAIL: $IMPL is missing or not executable"
  echo "Results: 0 passed, 1 failed"
  exit 1
fi

# --------------------------------------------------------------------------- #
# Sandbox + mock binaries
# --------------------------------------------------------------------------- #
SANDBOX="$(mktemp -d)"
trap 'rm -rf "$SANDBOX"' EXIT
MOCKBIN="$SANDBOX/.local/bin"          # signal-setup.sh prepends $HOME/.local/bin
mkdir -p "$MOCKBIN"
QR_LOG="$SANDBOX/qrencode.calls"       # touched iff qrencode actually runs
AZ_LOG="$SANDBOX/az.calls"

# Mock signal-cli: honours --version, listAccounts (linked iff MOCK_LINKED_NUMBER
# is set), and is a harmless no-op for anything else (e.g. daemon).
cat >"$MOCKBIN/signal-cli" <<'EOF'
#!/usr/bin/env bash
for a in "$@"; do
  [ "$a" = "--version" ] && { echo "signal-cli 0.14.5"; exit 0; }
done
for a in "$@"; do
  if [ "$a" = "listAccounts" ]; then
    if [ -n "${MOCK_LINKED_NUMBER:-}" ]; then
      echo "Number: ${MOCK_LINKED_NUMBER}"
      [ -n "${MOCK_EXTRA_NUMBER:-}" ] && echo "Number: ${MOCK_EXTRA_NUMBER}"
    elif [ -n "${MOCK_LINK_AFTER_MINT_FILE:-}" ] && [ -f "$MOCK_LINK_AFTER_MINT_FILE" ]; then
      echo "Number: +15551234567"
    fi
    exit 0
  fi
done
exit 0
EOF

# Mock qrencode: record every invocation. Its mere execution means a QR was
# rendered — the tests assert it is ABSENT on the idempotent path.
cat >"$MOCKBIN/qrencode" <<EOF
#!/usr/bin/env bash
echo "qrencode \$*" >> "$QR_LOG"
exit 0
EOF

# Mock systemd-run: present-but-inert (idempotent path never mints).
cat >"$MOCKBIN/systemd-run" <<'EOF'
#!/usr/bin/env bash
if [ -n "${MOCK_SYSTEMD_MINT:-}" ]; then
  uri_file="${@: -1}"
  echo "sgnl://mock-local-link" > "$uri_file"
  [ -n "${MOCK_LINK_AFTER_MINT_FILE:-}" ] && touch "$MOCK_LINK_AFTER_MINT_FILE"
fi
exit 0
EOF

# Mock systemctl: reports units inactive.
cat >"$MOCKBIN/systemctl" <<'EOF'
#!/usr/bin/env bash
case "$*" in
  *is-active*) echo "inactive" ;;
esac
exit 0
EOF

# Mock sudo: transparent passthrough (never reached on the tested paths, but
# safe if it were).
cat >"$MOCKBIN/sudo" <<'EOF'
#!/usr/bin/env bash
[ "$1" = "-n" ] && shift
[ "$1" = "true" ] && exit 0
exec "$@"
EOF

# Mock az: emulate `az vm run-command invoke --scripts <script>`. Prereq probe
# returns SIGCLI_OK/SYSTEMD_OK; listAccounts returns the linked number.
cat >"$MOCKBIN/az" <<EOF
#!/usr/bin/env bash
echo "az \$*" >> "$AZ_LOG"
script=""
prev=""
for a in "\$@"; do
  [ "\$prev" = "--scripts" ] && script="\$a"
  prev="\$a"
done
case "\$script" in
  *listAccounts*)
    [ -n "\${MOCK_AZ_FAIL_LIST:-}" ] && { echo "mock az listAccounts failure" >&2; exit 42; }
    [ -n "\${MOCK_AZ_INNER_FAIL_LIST:-}" ] && { echo "__SIGNAL_CLI_FAILED__1"; echo "mock signal-cli listAccounts failure"; exit 0; }
    if [ -n "\${MOCK_LINKED_NUMBER:-}" ]; then
      echo "Number: \${MOCK_LINKED_NUMBER}"
      [ -n "\${MOCK_EXTRA_NUMBER:-}" ] && echo "Number: \${MOCK_EXTRA_NUMBER}"
    elif [ -n "\${MOCK_LINK_AFTER_MINT_FILE:-}" ] && [ -f "\$MOCK_LINK_AFTER_MINT_FILE" ]; then
      echo "Number: +15551234567"
    fi ;;
  *URI_START*)
    if [ -n "\${MOCK_REMOTE_MINT:-}" ]; then
      [ -n "\${MOCK_LINK_AFTER_MINT_FILE:-}" ] && touch "\$MOCK_LINK_AFTER_MINT_FILE"
      echo "URI_START"; echo "sgnl://mock-remote-link"; echo "URI_END"
    fi ;;
  *updateGroup*send*)
    echo "POST_TEST_OK" ;;
  *"test -x"*|*SIGCLI_OK*)
    echo "SIGCLI_OK"; echo "SYSTEMD_OK" ;;
  *) : ;;
esac
exit 0
EOF

# Mock nc / qrencode-adjacent tools present but inert.
cat >"$MOCKBIN/nc" <<'EOF'
#!/usr/bin/env bash
payload="$(cat)"
case "$payload" in
  *updateGroup*) echo '{"jsonrpc":"2.0","result":{"groupId":"mock-group"},"id":1}' ;;
  *send*) echo '{"jsonrpc":"2.0","results":[],"timestamp":123,"id":1}' ;;
esac
exit 0
EOF

chmod +x "$MOCKBIN"/*

# Curated real tools symlinked into MOCKBIN so we can run with PATH=$MOCKBIN
# ONLY. This is what makes the sandbox hermetic: without it, a real system
# qrencode (or signal-cli) on /usr/bin would leak in and defeat the
# missing-prerequisite tests.
for t in bash env grep sed seq sleep date hostname id rm head tail cat timeout tr touch; do
  real="$(command -v "$t" 2>/dev/null)" || continue
  ln -sf "$real" "$MOCKBIN/$t"
done

# Run the script under the sandbox with a hermetic PATH. Extra env/args pass
# through.  Usage: run_ss <args...>  ; captures OUT (stdout+stderr) and RC.
run_ss() {
  OUT="$(env -i \
    HOME="$SANDBOX" \
    PATH="$MOCKBIN" \
    MOCK_LINKED_NUMBER="${MOCK_LINKED_NUMBER:-}" \
    MOCK_EXTRA_NUMBER="${MOCK_EXTRA_NUMBER:-}" \
    MOCK_AZ_FAIL_LIST="${MOCK_AZ_FAIL_LIST:-}" \
    MOCK_AZ_INNER_FAIL_LIST="${MOCK_AZ_INNER_FAIL_LIST:-}" \
    MOCK_SYSTEMD_MINT="${MOCK_SYSTEMD_MINT:-}" \
    MOCK_REMOTE_MINT="${MOCK_REMOTE_MINT:-}" \
    MOCK_LINK_AFTER_MINT_FILE="$SANDBOX/linked-after-mint" \
    SIGNAL_SETUP_TEST_DAEMON_UP="${SIGNAL_SETUP_TEST_DAEMON_UP:-}" \
    SIGNAL_SETUP_DAEMON_TCP="${SIGNAL_SETUP_DAEMON_TCP:-}" \
    SIGNAL_SETUP_DAEMON_WAIT_ATTEMPTS="${SIGNAL_SETUP_DAEMON_WAIT_ATTEMPTS:-}" \
    SIGNAL_SETUP_RPC_TIMEOUT_SECONDS="${SIGNAL_SETUP_RPC_TIMEOUT_SECONDS:-}" \
    /bin/bash "$IMPL" "$@" 2>&1)"
  RC=$?
}

reset_logs() { : >"$QR_LOG"; : >"$AZ_LOG"; }

echo "═══════════════════════════════════════════════════════"
echo "  Test Suite: signal-setup.sh — Behavior (mocked)"
echo "═══════════════════════════════════════════════════════"

# ─── Test 1: --help ─────────────────────────────────────────────────────────
echo ""
echo "Test 1: --help prints usage and exits 0"
reset_logs
MOCK_LINKED_NUMBER="" run_ss --help
if [[ "$RC" -eq 0 ]] && echo "$OUT" | grep -qi "USAGE"; then
  pass "--help exits 0 and prints USAGE"
else
  fail "--help must exit 0 and print USAGE (rc=$RC)"
fi

# ─── Test 2: --host is required ─────────────────────────────────────────────
echo ""
echo "Test 2: missing --host aborts non-zero"
reset_logs
MOCK_LINKED_NUMBER="" run_ss
if [[ "$RC" -ne 0 ]] && echo "$OUT" | grep -qi "host"; then
  pass "missing --host fails with a message about host"
else
  fail "missing --host must abort non-zero with a host message (rc=$RC)"
fi

# ─── Test 3: unknown flag is rejected ───────────────────────────────────────
echo ""
echo "Test 3: unknown argument is rejected"
reset_logs
MOCK_LINKED_NUMBER="" run_ss --host local --bogus-flag
if [[ "$RC" -ne 0 ]] && echo "$OUT" | grep -qi "unknown"; then
  pass "unknown flag rejected non-zero"
else
  fail "unknown flag must be rejected (rc=$RC)"
fi

# ─── Test 4: injection fails closed (the security contract) ─────────────────
echo ""
echo "Test 4: input validation FAILS CLOSED against injection"

# host injection
reset_logs
MOCK_LINKED_NUMBER="" run_ss --host 'x;reboot' -y
if [[ "$RC" -ne 0 ]] && echo "$OUT" | grep -qi "invalid"; then
  pass "malicious --host rejected"
else
  fail "malicious --host must be rejected (rc=$RC)"
fi
if [[ ! -s "$QR_LOG" ]]; then
  pass "no QR rendered for a rejected host"
else
  fail "a rejected host must not reach QR rendering"
fi

# host with command substitution (payload MUST stay literal, not expand)
reset_logs
# shellcheck disable=SC2016
MOCK_LINKED_NUMBER="" run_ss --host '$(touch /tmp/pwn)' -y
if [[ "$RC" -ne 0 ]]; then
  pass "--host with \$(...) rejected"
else
  fail "--host with command substitution must be rejected (rc=$RC)"
fi

# group injection
reset_logs
MOCK_LINKED_NUMBER="" run_ss --host local --group 'a";rm -rf /' -y
if [[ "$RC" -ne 0 ]] && echo "$OUT" | grep -qi "invalid"; then
  pass "malicious --group rejected"
else
  fail "malicious --group must be rejected (rc=$RC)"
fi

# resource-group injection
reset_logs
MOCK_LINKED_NUMBER="" run_ss --host somevm --resource-group 'rg;curl evil' -y
if [[ "$RC" -ne 0 ]] && echo "$OUT" | grep -qi "invalid"; then
  pass "malicious --resource-group rejected"
else
  fail "malicious --resource-group must be rejected (rc=$RC)"
fi

# phone: non-E.164
reset_logs
MOCK_LINKED_NUMBER="" run_ss --host local --phone '15551234567' -y   # missing +
if [[ "$RC" -ne 0 ]] && echo "$OUT" | grep -qi "invalid"; then
  pass "non-E.164 --phone rejected (missing +)"
else
  fail "non-E.164 --phone must be rejected (rc=$RC)"
fi

reset_logs
MOCK_LINKED_NUMBER="" run_ss --host local --phone '+1555;evil' -y
if [[ "$RC" -ne 0 ]]; then
  pass "--phone with metacharacters rejected"
else
  fail "--phone with metacharacters must be rejected (rc=$RC)"
fi

# ─── Test 4b: SIGNAL_SETUP_DAEMON_TCP fails closed (the loopback invariant) ──
# The signal-cli JSON-RPC daemon is UNAUTHENTICATED, so its ONLY security
# boundary is the network binding (SECURITY.md §6/§10.4, threat T5). The
# SIGNAL_SETUP_DAEMON_TCP value flows verbatim into `daemon --tcp`, /dev/tcp
# probes, and nc connections on BOTH the local and remote paths. A regression
# that accepted a routable host or an out-of-range port would expose full
# send/receive control of the linked account to the network while still
# shipping green — this block is the guard that makes such a regression fail.
# Validation runs before any prereq/mint side effect, so a rejected endpoint
# must NEVER render a QR.
echo ""
echo "Test 4b: SIGNAL_SETUP_DAEMON_TCP fails closed (loopback host + port range)"

# host must be loopback: a wildcard bind is the exact off-box exposure T5 warns
# about and must be rejected with a loopback message.
reset_logs
SIGNAL_SETUP_DAEMON_TCP='0.0.0.0:7583' MOCK_LINKED_NUMBER="+15551234567" \
  run_ss --host local --phone +15551234567 --no-daemon -y
if [[ "$RC" -ne 0 ]] && echo "$OUT" | grep -qi "loopback"; then
  pass "wildcard 0.0.0.0 daemon bind rejected (loopback-only)"
else
  fail "0.0.0.0 daemon bind must be rejected as non-loopback (rc=$RC): $OUT"
fi
if [[ ! -s "$QR_LOG" ]]; then
  pass "no QR rendered for a rejected daemon endpoint"
else
  fail "a rejected daemon endpoint must not reach QR rendering"
fi

# a routable unicast host is equally off-box and must be rejected.
reset_logs
SIGNAL_SETUP_DAEMON_TCP='1.2.3.4:7583' MOCK_LINKED_NUMBER="+15551234567" \
  run_ss --host local --phone +15551234567 --no-daemon -y
if [[ "$RC" -ne 0 ]] && echo "$OUT" | grep -qi "loopback"; then
  pass "routable 1.2.3.4 daemon bind rejected (loopback-only)"
else
  fail "routable daemon host must be rejected as non-loopback (rc=$RC): $OUT"
fi

# IPv6 / multi-colon forms are refused: they would also break the single-colon
# host:port parsing daemon_up()/nc rely on, silently mis-targeting the probe.
reset_logs
SIGNAL_SETUP_DAEMON_TCP='[::1]:7583' MOCK_LINKED_NUMBER="+15551234567" \
  run_ss --host local --phone +15551234567 --no-daemon -y
if [[ "$RC" -ne 0 ]] && echo "$OUT" | grep -qi "host:port"; then
  pass "IPv6/multi-colon daemon endpoint rejected (single-colon invariant)"
else
  fail "IPv6/multi-colon daemon endpoint must be rejected (rc=$RC): $OUT"
fi

# port above 65535 must be rejected even on a loopback host.
reset_logs
SIGNAL_SETUP_DAEMON_TCP='127.0.0.1:99999' MOCK_LINKED_NUMBER="+15551234567" \
  run_ss --host local --phone +15551234567 --no-daemon -y
if [[ "$RC" -ne 0 ]] && echo "$OUT" | grep -qi "out of range"; then
  pass "out-of-range daemon port (99999) rejected"
else
  fail "out-of-range daemon port must be rejected (rc=$RC): $OUT"
fi

# port 0 is not a valid TCP port and must be rejected by the charset rule.
reset_logs
SIGNAL_SETUP_DAEMON_TCP='127.0.0.1:0' MOCK_LINKED_NUMBER="+15551234567" \
  run_ss --host local --phone +15551234567 --no-daemon -y
if [[ "$RC" -ne 0 ]] && echo "$OUT" | grep -qi "invalid"; then
  pass "daemon port 0 rejected"
else
  fail "daemon port 0 must be rejected (rc=$RC): $OUT"
fi

# a non-numeric port must be rejected (no injection into the port field).
reset_logs
SIGNAL_SETUP_DAEMON_TCP='127.0.0.1:7a' MOCK_LINKED_NUMBER="+15551234567" \
  run_ss --host local --phone +15551234567 --no-daemon -y
if [[ "$RC" -ne 0 ]] && echo "$OUT" | grep -qi "invalid"; then
  pass "non-numeric daemon port rejected"
else
  fail "non-numeric daemon port must be rejected (rc=$RC): $OUT"
fi

# POSITIVE: a valid loopback endpoint with a non-default port passes validation.
# Paired with an already-linked host + --no-daemon so we exit cleanly (proving
# the value was accepted, not merely that the daemon step was skipped).
reset_logs
SIGNAL_SETUP_DAEMON_TCP='localhost:7600' MOCK_LINKED_NUMBER="+15551234567" \
  run_ss --host local --phone +15551234567 --no-daemon -y
if [[ "$RC" -eq 0 ]] && echo "$OUT" | grep -qi "already linked"; then
  pass "valid loopback endpoint (localhost:7600) accepted"
else
  fail "a valid loopback daemon endpoint must be accepted (rc=$RC): $OUT"
fi

# ─── Test 4c: numeric env tunables fail closed on non-integer values ────────
# DAEMON_WAIT_ATTEMPTS / RPC_TIMEOUT_SECONDS are interpolated UNQUOTED into the
# root-executed remote payload (`seq 1 $DAEMON_WAIT_ATTEMPTS`, `timeout
# $RPC_TIMEOUT_SECONDS nc`). A non-numeric value is an injection vector into that
# payload, so validation must reject it before any mint/daemon side effect.
echo ""
echo "Test 4c: numeric env tunables fail closed (self-injection guard)"

reset_logs
SIGNAL_SETUP_DAEMON_WAIT_ATTEMPTS='5; reboot' MOCK_LINKED_NUMBER="+15551234567" \
  run_ss --host local --phone +15551234567 --no-daemon -y
if [[ "$RC" -ne 0 ]] && echo "$OUT" | grep -qi "invalid"; then
  pass "non-numeric SIGNAL_SETUP_DAEMON_WAIT_ATTEMPTS rejected"
else
  fail "non-numeric DAEMON_WAIT_ATTEMPTS must be rejected (rc=$RC): $OUT"
fi
if [[ ! -s "$QR_LOG" ]]; then
  pass "no QR rendered for a rejected wait-attempts value"
else
  fail "a rejected wait-attempts value must not reach QR rendering"
fi

reset_logs
# shellcheck disable=SC2016  # literal $(...) is the injection payload under test — must NOT expand
SIGNAL_SETUP_RPC_TIMEOUT_SECONDS='$(touch /tmp/pwn)' MOCK_LINKED_NUMBER="+15551234567" \
  run_ss --host local --phone +15551234567 --no-daemon -y
if [[ "$RC" -ne 0 ]] && echo "$OUT" | grep -qi "invalid"; then
  pass "non-numeric SIGNAL_SETUP_RPC_TIMEOUT_SECONDS rejected"
else
  fail "non-numeric RPC_TIMEOUT_SECONDS must be rejected (rc=$RC): $OUT"
fi

# POSITIVE: valid integer tunables pass validation (paired with already-linked
# + --no-daemon so we exit cleanly, proving acceptance not a skipped step).
reset_logs
SIGNAL_SETUP_DAEMON_WAIT_ATTEMPTS='30' SIGNAL_SETUP_RPC_TIMEOUT_SECONDS='20' \
  MOCK_LINKED_NUMBER="+15551234567" \
  run_ss --host local --phone +15551234567 --no-daemon -y
if [[ "$RC" -eq 0 ]] && echo "$OUT" | grep -qi "already linked"; then
  pass "valid integer env tunables accepted"
else
  fail "valid integer env tunables must be accepted (rc=$RC): $OUT"
fi

# ─── Test 5: valid inputs pass validation (reach prereqs/idempotency) ───────
echo ""
echo "Test 5: valid inputs pass validation"

# A valid group name WITH a space is allowed by the documented charset; paired
# with an already-linked host so we exit cleanly without minting.
reset_logs
MOCK_LINKED_NUMBER="+15551234567" run_ss \
  --host local --phone +15551234567 --group "amplihack test" --no-daemon -y
if [[ "$RC" -eq 0 ]]; then
  pass "valid --group (with space) + valid --phone accepted"
else
  fail "valid inputs must pass validation (rc=$RC): $OUT"
fi

# ─── Test 6: idempotency (LOCAL) — already linked is a no-op, NO QR ─────────
echo ""
echo "Test 6: LOCAL idempotency — already-linked host renders NO QR"
reset_logs
MOCK_LINKED_NUMBER="+15551234567" run_ss \
  --host local --phone +15551234567 --no-daemon -y
if [[ "$RC" -eq 0 ]] && echo "$OUT" | grep -qi "already linked"; then
  pass "already-linked local host reports 'already linked' and exits 0"
else
  fail "already-linked local host must be a clean no-op (rc=$RC): $OUT"
fi
if [[ ! -s "$QR_LOG" ]]; then
  pass "no QR rendered when already linked (idempotent)"
else
  fail "an already-linked host must NOT render a QR (found: $(cat "$QR_LOG"))"
fi

reset_logs
MOCK_LINKED_NUMBER="+15551234567" MOCK_EXTRA_NUMBER="+15557654321" run_ss \
  --host local --no-daemon -y
if [[ "$RC" -ne 0 ]] && echo "$OUT" | grep -qi "Multiple Signal accounts"; then
  pass "multiple linked accounts without --phone abort instead of choosing one silently"
else
  fail "multiple accounts without --phone must require explicit selection (rc=$RC): $OUT"
fi

# ─── Test 6b: idempotency must NOT match on a phone-number PREFIX ────────────
# Regression: a host linked to +155512345678 must NOT be treated as "already
# linked" when the caller passes the prefix --phone +15551234567. The old
# substring test (*"Number: $PHONE"*) matched prefixes, skipped minting, and
# reported/used the wrong (unlinked) number for the daemon/send steps.
echo ""
echo "Test 6b: prefix phone number must NOT be mistaken for an exact match"
reset_logs
MOCK_LINKED_NUMBER="+155512345678" run_ss \
  --host local --phone +15551234567 --no-daemon -y
if echo "$OUT" | grep -qi "already linked"; then
  fail "prefix --phone +15551234567 must NOT match linked +155512345678 (rc=$RC): $OUT"
else
  pass "prefix phone number is not mistaken for an already-linked exact match"
fi

# ─── Test 6c: exact phone match still reports already-linked ─────────────────
echo ""
echo "Test 6c: exact phone match is still recognised as already-linked"
reset_logs
MOCK_LINKED_NUMBER="+155512345678" run_ss \
  --host local --phone +155512345678 --no-daemon -y
if [[ "$RC" -eq 0 ]] && echo "$OUT" | grep -qi "already linked"; then
  pass "exact --phone match reports already linked (no false negative from the fix)"
else
  fail "exact --phone match must still be recognised as already linked (rc=$RC): $OUT"
fi

# ─── Test 7: idempotency (REMOTE) — uses az, renders NO QR ──────────────────
echo ""
echo "Test 7: REMOTE idempotency — drives az CLI, renders NO QR"
reset_logs
MOCK_LINKED_NUMBER="+15551234567" run_ss \
  --host devvm --phone +15551234567 --resource-group rysweet-linux-vm-pool --no-daemon -y
if [[ "$RC" -eq 0 ]] && echo "$OUT" | grep -qi "already linked"; then
  pass "already-linked remote host reports 'already linked' and exits 0"
else
  fail "already-linked remote host must be a clean no-op (rc=$RC): $OUT"
fi
if grep -q 'run-command' "$AZ_LOG" 2>/dev/null; then
  pass "remote path invoked 'az vm run-command'"
else
  fail "remote path must invoke az vm run-command (az.calls: $(cat "$AZ_LOG" 2>/dev/null))"
fi
if [[ ! -s "$QR_LOG" ]]; then
  pass "no QR rendered for an already-linked remote host"
else
  fail "already-linked remote host must NOT render a QR"
fi

reset_logs
MOCK_LINKED_NUMBER="+15551234567" run_ss \
  --host devvm --phone +15551234567 --resource-group rysweet-linux-vm-pool -y
if [[ "$RC" -eq 0 ]] && echo "$OUT" | grep -qi "Remote daemon reachable; post-test OK"; then
  pass "remote already-linked path runs daemon/self-group/post-test"
else
  fail "remote daemon/self-group/post-test must run when daemon step is enabled (rc=$RC): $OUT"
fi

reset_logs
SIGNAL_SETUP_TEST_DAEMON_UP=1 MOCK_LINKED_NUMBER="+15551234567" run_ss \
  --host local --phone +15551234567 -y
if [[ "$RC" -eq 0 ]] && echo "$OUT" | grep -qi "Post-test OK"; then
  pass "local already-linked path runs daemon/self-group/post-test"
else
  fail "local daemon/self-group/post-test must run when daemon is reachable (rc=$RC): $OUT"
fi

reset_logs
MOCK_AZ_FAIL_LIST=1 MOCK_LINKED_NUMBER="+15551234567" run_ss \
  --host devvm --phone +15551234567 --resource-group rysweet-linux-vm-pool --no-daemon -y
if [[ "$RC" -ne 0 ]] && echo "$OUT" | grep -qi "refusing to mint"; then
  pass "remote account-probe failure aborts instead of minting a fresh link"
else
  fail "remote listAccounts failure must abort before minting (rc=$RC): $OUT"
fi
if [[ ! -s "$QR_LOG" ]]; then
  pass "no QR rendered after remote account-probe failure"
else
  fail "remote account-probe failure must not render a QR"
fi

reset_logs
MOCK_AZ_INNER_FAIL_LIST=1 MOCK_LINKED_NUMBER="+15551234567" run_ss \
  --host devvm --phone +15551234567 --resource-group rysweet-linux-vm-pool --no-daemon -y
if [[ "$RC" -ne 0 ]] && echo "$OUT" | grep -qi "refusing to mint"; then
  pass "remote signal-cli listAccounts failure aborts even when az exits 0"
else
  fail "remote inner signal-cli failure must abort before minting (rc=$RC): $OUT"
fi
if [[ ! -s "$QR_LOG" ]]; then
  pass "no QR rendered after remote signal-cli account-probe failure"
else
  fail "remote signal-cli account-probe failure must not render a QR"
fi

# Regression (Finding 1): --phone set + host NOT yet linked must NOT be
# mistaken for a probe failure. already_linked() previously leaked exit 1 from
# a non-matching [[ ]] test, so "$(already_linked)" || die fired the
# "refusing to mint" abort before the QR could ever be minted. A successful
# probe that finds no account is SUCCESS with empty output; the run must
# proceed toward minting (and, in this hermetic sandbox with an inert
# systemd-run, fail later at "Failed to obtain a link URI"), never at the
# account probe.
reset_logs
MOCK_LINKED_NUMBER="" run_ss \
  --host local --phone +15551234567 --no-daemon -y
if echo "$OUT" | grep -qi "refusing to mint"; then
  fail "phone set + not-linked must not false-abort at the account probe (rc=$RC): $OUT"
else
  pass "phone set + not-linked proceeds past probe (no spurious 'refusing to mint')"
fi

reset_logs
rm -f "$SANDBOX/linked-after-mint"
MOCK_SYSTEMD_MINT=1 MOCK_LINKED_NUMBER="" run_ss \
  --host local --phone +15551234567 --no-daemon -y
if [[ "$RC" -eq 0 ]] && echo "$OUT" | grep -qi "signal-setup complete for local"; then
  pass "local mint path renders QR, verifies linkage, and completes"
else
  fail "local mint path must complete with mocked systemd-run/linkage (rc=$RC): $OUT"
fi
if grep -q 'ANSIUTF8i' "$QR_LOG" 2>/dev/null; then
  pass "local mint path renders the QR with ANSIUTF8i"
else
  fail "local mint path must render ANSIUTF8i QR (qr.calls: $(cat "$QR_LOG" 2>/dev/null))"
fi

reset_logs
rm -f "$SANDBOX/linked-after-mint"
MOCK_REMOTE_MINT=1 MOCK_LINKED_NUMBER="" run_ss \
  --host devvm --phone +15551234567 --resource-group rysweet-linux-vm-pool --no-daemon -y
if [[ "$RC" -eq 0 ]] && echo "$OUT" | grep -qi "signal-setup complete for devvm"; then
  pass "remote mint path extracts URI, renders QR, verifies linkage, and completes"
else
  fail "remote mint path must complete with mocked az/linkage (rc=$RC): $OUT"
fi
if grep -q 'ANSIUTF8i' "$QR_LOG" 2>/dev/null; then
  pass "remote mint path renders the QR with ANSIUTF8i"
else
  fail "remote mint path must render ANSIUTF8i QR (qr.calls: $(cat "$QR_LOG" 2>/dev/null))"
fi

# ─── Test 8: mode auto-detection ────────────────────────────────────────────
echo ""
echo "Test 8: --host local => local mode (no az); named host => remote (az)"

# local: az must NOT be touched at all
reset_logs
MOCK_LINKED_NUMBER="+15551234567" run_ss --host local --phone +15551234567 --no-daemon -y
if [[ ! -s "$AZ_LOG" ]]; then
  pass "local mode never invokes az"
else
  fail "local mode must not invoke az (az.calls: $(cat "$AZ_LOG"))"
fi

# ─── Test 9: prerequisite failures abort clearly ────────────────────────────
echo ""
echo "Test 9: missing prerequisites abort with a clear message"

# Remove qrencode from the sandbox -> local prereq check must fail.
mv "$MOCKBIN/qrencode" "$SANDBOX/qrencode.bak"
reset_logs
MOCK_LINKED_NUMBER="+15551234567" run_ss --host local --phone +15551234567 --no-daemon -y
if [[ "$RC" -ne 0 ]] && echo "$OUT" | grep -qi "qrencode"; then
  pass "missing qrencode aborts with a qrencode message"
else
  fail "missing qrencode must abort clearly (rc=$RC): $OUT"
fi
mv "$SANDBOX/qrencode.bak" "$MOCKBIN/qrencode"

# Remove signal-cli -> local prereq check must fail.
mv "$MOCKBIN/signal-cli" "$SANDBOX/signal-cli.bak"
reset_logs
MOCK_LINKED_NUMBER="+15551234567" run_ss --host local --phone +15551234567 --no-daemon -y
if [[ "$RC" -ne 0 ]] && echo "$OUT" | grep -qi "signal-cli"; then
  pass "missing signal-cli aborts with a signal-cli message"
else
  fail "missing signal-cli must abort clearly (rc=$RC): $OUT"
fi
mv "$SANDBOX/signal-cli.bak" "$MOCKBIN/signal-cli"

# ─── Summary ─────────────────────────────────────────────────────────────────
echo ""
echo "═══════════════════════════════"
echo "Results: $PASS passed, $FAIL failed"
echo "═══════════════════════════════"

[[ "$FAIL" -gt 0 ]] && exit 1
exit 0
