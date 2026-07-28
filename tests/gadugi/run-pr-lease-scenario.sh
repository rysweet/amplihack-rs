#!/usr/bin/env bash
# Self-asserting gadugi-test scenario body for the PR-ownership lease (issue #1051, PR #1058).
#
# Exercises the SHIPPED pr-lease behavioral contract end-to-end as a black box:
#   - the lease integration suite (crates/amplihack-orchestration/tests/pr_lease_behavior.rs)
#   - the CLI merge-gate tests (crates/amplihack-cli, pr::tests)
# and asserts the observable contract that #1051 requires: a second session cannot
# acquire a held lease, expired/corrupt leases are reclaimable, the TTL cannot
# overflow into a permanent lease, drop releases the lease, and the merge gate
# vetoes a merge unless the lease is owned.
#
# Launched by the gadugi `execute` action with no arguments, then checked with
# `validate_exit_code` + an `ALL_CASES_PASSED` output assertion.
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$REPO_ROOT"

fail=0
pass() { echo "PASS: $1"; }
fl()   { echo "FAIL: $1"; fail=1; }

LOG="$(mktemp)"
trap 'rm -f "$LOG"' EXIT

# ---------------------------------------------------------------------------
# Case 1: the shipped lease behavior contract (orchestration integration tests).
# These treat the on-disk lease store as a black box via its public API and
# assert acquire/stand-down/expiry/renew/release/CAS/traversal/corrupt/overflow.
# ---------------------------------------------------------------------------
if cargo test -p amplihack-orchestration --test pr_lease_behavior -- --nocapture >"$LOG" 2>&1; then
  pass "pr_lease_behavior integration contract passes"
else
  fl "pr_lease_behavior integration contract FAILED"
  tail -40 "$LOG"
fi

# Assert the specific safety-critical behaviors are actually present and green,
# not merely that some tests ran.
for t in \
  cas_rejects_concurrent_second_writer \
  file_slug_is_traversal_safe \
  corrupt_record_is_reclaimable \
  lease_record_expiry_does_not_overflow \
  drop_releases_on_session_end \
  contended_acquire_stands_down \
  ; do
  if grep -Eq "test .*${t} .* ok" "$LOG"; then
    pass "safety behavior present and green: ${t}"
  else
    fl "safety behavior missing or not green: ${t}"
  fi
done

# ---------------------------------------------------------------------------
# Case 2: the CLI merge-gate contract — a merge is vetoed unless the lease is
# owned, and the lease is released after a successful merge.
# ---------------------------------------------------------------------------
if cargo test -p amplihack-cli pr::tests -- --nocapture >"$LOG" 2>&1; then
  pass "CLI merge-gate tests pass"
else
  fl "CLI merge-gate tests FAILED"
  tail -40 "$LOG"
fi
for t in \
  test_gated_merge_vetoes_when_lease_not_owned \
  test_gated_merge_asserts_then_releases_on_success \
  ; do
  if grep -Eq "test .*${t} .* ok" "$LOG"; then
    pass "gate behavior present and green: ${t}"
  else
    fl "gate behavior missing or not green: ${t}"
  fi
done

if [ "$fail" -eq 0 ]; then
  echo "ALL_CASES_PASSED"
  exit 0
fi
echo "SCENARIO_FAILED"
exit 1
