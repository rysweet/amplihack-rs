#!/usr/bin/env bash
# Self-asserting gadugi-test scenario body for issues #815 / #804.
#
# Drives the SHIPPED step-03b-extract-issue-number recipe logic (via
# run-step-03b.sh) through the regression cases and exits non-zero if any
# case regresses. Designed to be launched by the gadugi `execute` action with
# no arguments, then checked with `validate_exit_code`.
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RUN="$SCRIPT_DIR/run-step-03b.sh"
fail=0

check_eq() { # <label> <expected> <actual>
  if [ "$2" = "$3" ]; then
    echo "PASS: $1 => '$3'"
  else
    echo "FAIL: $1 => expected '$2', got '$3'"
    fail=1
  fi
}

# 1. Exact #815/#804 shape: hash-based local reference is propagated verbatim.
out="$("$RUN" $'tracking_system=local\ntracking_reference=local-5d904cff4398\ntracking_issue=local-5d904cff4398\nissue_creation=local-tracking\n' 'update PR !598 guide')"
rc=$?
check_eq "hash local ref accepted+propagated" "local-5d904cff4398" "$out"
check_eq "hash local ref does not abort (exit 0)" "0" "$rc"

# 2. A local fallback must NOT surface a bare embedded number.
out="$("$RUN" $'tracking_system=local\ntracking_reference=local-issue-763\ntracking_issue=local-issue-763\nissue_creation=local-tracking\nissue_number=763\n' 'local fallback')"
check_eq "local fallback propagates reference, not bare number" "local-issue-763" "$out"

# 3. Real GitHub issue numbers still extract numerically.
out="$("$RUN" "https://github.com/example-org/example-repo/issues/901" 'gh follow-up')"
check_eq "github issue still numeric" "901" "$out"

# 4. Malformed local metadata (no reference) must fail closed.
if "$RUN" $'tracking_system=local\nissue_creation=local-tracking\nissue_number=763\n' 'no usable reference' >/dev/null 2>&1; then
  echo "FAIL: malformed local metadata must fail closed but succeeded"
  fail=1
else
  echo "PASS: malformed local metadata fails closed"
fi

if [ "$fail" -eq 0 ]; then
  echo "ALL_CASES_PASSED"
  exit 0
fi
echo "SCENARIO_FAILED"
exit 1
