#!/usr/bin/env bash
# Issue #1167 — `amplihack:migrate` must refuse to migrate a session onto the
# host it is already running on.
#
# Observed: `/amplihack:migrate deva3` was run from a session whose hostname was
# deva3, running inside tmux on deva3. The skill proceeded to "migrate" the
# session to itself — re-bootstrapping tooling, shipping a tarball back, and
# extracting session-state over the live session's own `events.jsonl` and
# `workspace.yaml` while the process held them open. A duplicate
# `copilot --resume` was avoided only because `tmux has-session` matched the
# very session doing the migrating.
#
# Two checks are tested here, because either alone leaves a gap:
#   * by name  — cheap, no network, catches the common mistyped case;
#   * by machine ID — catches an alias or address that routes back here under
#     a different name, where the name check cannot help.

set -uo pipefail

SCRIPT="amplifier-bundle/skills/migrate/scripts/migrate.sh"
[ -f "$SCRIPT" ] || { echo "missing $SCRIPT (run from repo root)"; exit 1; }

fails=0
pass() { printf '  ok    %s\n' "$1"; }
fail() { printf '  FAIL  %s\n' "$1"; fails=$((fails + 1)); }

# Library mode defines the helpers and returns without parsing args or running.
# shellcheck disable=SC1090
AMPLIHACK_MIGRATE_LIB=1 . "$SCRIPT"

if ! declare -F is_same_host_by_name >/dev/null; then
  echo "  FAIL  is_same_host_by_name not defined — the guard is absent"
  exit 1
fi

SHORT="$(hostname -s 2>/dev/null || hostname)"
LONG="$(hostname -f 2>/dev/null || hostname)"

# --- 1. this host, in the forms a user would actually type ------------------
for name in "$SHORT" "$LONG" "${SHORT^^}" localhost 127.0.0.1; do
  if is_same_host_by_name "$name"; then
    pass "refuses '$name' as the same host"
  else
    fail "did NOT recognise '$name' as this host"
  fi
done

# --- 2. a genuinely different host must still be allowed --------------------
# The costly failure is a guard so eager it blocks real migrations.
for name in some-other-host deva99.example.com "${SHORT}-2" "not${SHORT}"; do
  if is_same_host_by_name "$name"; then
    fail "wrongly blocked '$name', which is a different host"
  else
    pass "allows '$name'"
  fi
done

# Empty input must not read as "same host".
is_same_host_by_name "" && fail "empty destination treated as same host" \
  || pass "empty destination is not treated as same host"

# --- 3. machine-id comparison ----------------------------------------------
if ! declare -F is_same_machine_id >/dev/null; then
  fail "is_same_machine_id not defined"
else
  is_same_machine_id "abc123" "abc123" && pass "identical machine IDs match" \
    || fail "identical machine IDs did not match"
  is_same_machine_id "abc123" "def456" && fail "different machine IDs matched" \
    || pass "different machine IDs do not match"
  # Unknown must never read as "same", or a host with no /etc/machine-id would
  # be unable to receive a real migration.
  is_same_machine_id "" "" && fail "two empty IDs treated as the same machine" \
    || pass "unknown machine ID is not treated as a match"
  is_same_machine_id "abc123" "" && fail "empty remote ID treated as a match" \
    || pass "missing remote machine ID is not treated as a match"
fi

# --- 4. behaviour: the real script aborts, before doing anything ------------
# Must exit non-zero without needing azlin, i.e. during argument handling.
set +e
out=$(bash "$SCRIPT" "$SHORT" --dry-run 2>&1); rc=$?
set -e
if [ $rc -ne 0 ] && grep -q 'already running on' <<<"$out"; then
  pass "the script itself refuses this host (exit $rc) with a clear message"
else
  fail "script did not refuse this host: exit $rc: ${out:0:200}"
fi
# It must refuse before warning about shipping credentials — that ordering is
# the difference between a wasted keystroke and a leaked secret.
if grep -q 'copy credentials' <<<"$out"; then
  fail "credential warning was printed before the same-host refusal"
else
  pass "refuses before reaching the credential warning"
fi

# --- 5. the guard runs before any remote command ---------------------------
# first_line_matching <fixed-string> <file> — the 1-based line number of the
# first line containing <fixed-string>, or "" if there is none.
#
# One awk that reads the whole file. The `head`-terminated pipeline this
# replaces left `grep` writing into a closed pipe: under `pipefail` that is a
# non-zero pipeline whose substitution collapses to "", turning a real ordering
# check into a vacuous one — and whether it fires is a race on the pipe buffer,
# so it cannot be ruled out by running the test (issue #1434).
first_line_matching() {
  awk -v needle="$1" 'n == 0 && index($0, needle) { n = FNR } END { if (n) print n }' "$2"
}

guard_line=$(first_line_matching 'already running on' "$SCRIPT")
first_remote=$(first_line_matching 'azlin connect' "$SCRIPT")
if [ -z "$guard_line" ] || [ -z "$first_remote" ]; then
  fail "could not locate guard or first remote call — ordering check is vacuous"
elif [ "$guard_line" -lt "$first_remote" ]; then
  pass "name guard (line $guard_line) precedes the first remote call (line $first_remote)"
else
  fail "guard at $guard_line runs after the first remote call at $first_remote"
fi

echo
if [ "$fails" -gt 0 ]; then
  echo "issue-1167: $fails check(s) failed"
  exit 1
fi
echo "issue-1167: all checks passed"
