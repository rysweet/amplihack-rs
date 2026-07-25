#!/usr/bin/env bash
# Unified test runner for the signal-setup skill.
#
# Usage:
#   bash amplifier-bundle/skills/signal-setup/tests/run_all_tests.sh
#
# Exit: 0 = all suites passed AND enough real scenarios ran; non-zero otherwise.
#
# Suites:
#   1. test_skill_structure.sh  — SKILL.md/SECURITY.md contract + script-source
#                                 invariants (the four hard-won facts, gotchas,
#                                 security model, idempotency, --host contract).
#   2. test_script_behavior.sh  — the actual script under a sandboxed HOME with
#                                 mocked signal-cli/qrencode/systemd-run/az/sudo/nc
#                                 (help, validation, injection fail-closed,
#                                 idempotency-renders-no-QR, prereq failures).
#   3. test_readme_and_catalog.sh — the content absorbed from PR #1001: the
#                                 README overview (links resolve, hard-won facts
#                                 preserved) and the docs/skills/SKILL_CATALOG.md
#                                 registration (correct alphabetical slot,
#                                 self-consistent counts, no version-bump files).
#
# Hollow-pass guard: a suite that silently ran zero assertions (or aborted early
# before printing its summary) yet still exited 0 must NOT be reported as green.
# We therefore parse each suite's mandatory "Results: N passed, M failed" line
# and require:
#   * every suite emitted exactly one Results line (ran to completion);
#   * every suite reports passed > 0 and failed == 0;
#   * the grand total of passing assertions is at least MIN_TOTAL_SCENARIOS.
#
# Intentionally omits -e so one failing suite never aborts the runner.
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Minimum number of passing assertions across all suites for the run to be
# considered substantive rather than a hollow pass. Deliberately well below the
# current real count (77) to tolerate churn, but far enough above zero that an
# empty/aborted suite cannot masquerade as success.
MIN_TOTAL_SCENARIOS="${MIN_TOTAL_SCENARIOS:-40}"

SUITES=(
  "test_skill_structure.sh"
  "test_script_behavior.sh"
  "test_readme_and_catalog.sh"
)

TOTAL_FAIL=0
TOTAL_PASSED=0
GUARD_FAIL=0

echo "###########################################################"
echo "#  signal-setup skill — full TDD suite"
echo "###########################################################"
for s in "${SUITES[@]}"; do
  echo ""
  echo ">>> Running $s"

  # Capture output so we can both stream it and parse its Results summary.
  out="$(bash "$SCRIPT_DIR/$s" 2>&1)"
  rc=$?
  echo "$out"

  if [[ "$rc" -eq 0 ]]; then
    echo "<<< $s: exit 0"
  else
    echo "<<< $s: FAILED (exit $rc)"
    TOTAL_FAIL=$((TOTAL_FAIL + 1))
  fi

  # ── Hollow-pass guard: the suite MUST have printed a Results summary. ──
  results_line="$(printf '%s\n' "$out" | grep -E '^Results:[[:space:]]*[0-9]+ passed, [0-9]+ failed' | tail -1)"
  if [[ -z "$results_line" ]]; then
    echo "!!! GUARD: $s printed no 'Results:' summary — treating as a hollow/aborted run"
    GUARD_FAIL=$((GUARD_FAIL + 1))
    continue
  fi

  suite_passed="$(printf '%s\n' "$results_line" | sed -E 's/^Results:[[:space:]]*([0-9]+) passed.*/\1/')"
  suite_failed="$(printf '%s\n' "$results_line" | sed -E 's/.* ([0-9]+) failed.*/\1/')"
  TOTAL_PASSED=$((TOTAL_PASSED + suite_passed))

  if [[ "$suite_passed" -le 0 ]]; then
    echo "!!! GUARD: $s ran 0 passing assertions — hollow pass"
    GUARD_FAIL=$((GUARD_FAIL + 1))
  fi
  if [[ "$suite_failed" -ne 0 && "$rc" -eq 0 ]]; then
    echo "!!! GUARD: $s reported $suite_failed failure(s) but exited 0 — inconsistent"
    GUARD_FAIL=$((GUARD_FAIL + 1))
  fi
done

echo ""
echo "###########################################################"
echo "#  Total passing assertions: $TOTAL_PASSED (floor: $MIN_TOTAL_SCENARIOS)"

if [[ "$TOTAL_PASSED" -lt "$MIN_TOTAL_SCENARIOS" ]]; then
  echo "!!! GUARD: only $TOTAL_PASSED passing assertions (< $MIN_TOTAL_SCENARIOS) — hollow suite"
  GUARD_FAIL=$((GUARD_FAIL + 1))
fi

if [[ "$TOTAL_FAIL" -eq 0 && "$GUARD_FAIL" -eq 0 ]]; then
  echo "#  ALL SUITES PASSED"
else
  [[ "$TOTAL_FAIL" -gt 0 ]] && echo "#  $TOTAL_FAIL SUITE(S) FAILED"
  [[ "$GUARD_FAIL" -gt 0 ]] && echo "#  $GUARD_FAIL HOLLOW-PASS GUARD VIOLATION(S)"
fi
echo "###########################################################"

if [[ "$TOTAL_FAIL" -gt 0 || "$GUARD_FAIL" -gt 0 ]]; then
  exit 1
fi
exit 0
