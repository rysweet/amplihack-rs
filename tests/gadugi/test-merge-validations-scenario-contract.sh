#!/usr/bin/env bash
# TDD spec (Problem 3 / issue #1110): the issue-820 gadugi scenario driver is
# STALE and must be updated to assert the SHIPPED merge-validations contract:
#
#   1. Diagnostic wording drift: shipped emits
#        "validator <label> output unparseable; counting zero votes from it"
#      (driver still greps the old "no parseable JSON object").
#   2. All-unparseable is now FATAL, not graceful: shipped prints
#        "FATAL: all validators produced unparseable output; cannot merge any verdicts"
#      and exits 1 (writes no output JSON, preserving validator_*_raw.txt).
#
# Two layers:
#   PART A — pins the shipped run-merge-validations.sh contract by RUNNING it
#            (source of truth the driver must match). Guards against code regressions.
#   PART B — asserts the driver SOURCE has been retconned to that contract, and
#            that running the driver yields ALL_CASES_PASSED / exit 0.
#
# FAILING-FIRST: Part B fails against the current stale driver; passes once the
# driver's Case-1 grep + Case-3 assertions are updated to the FATAL contract.
# Run:  bash tests/gadugi/test-merge-validations-scenario-contract.sh
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RUN="$SCRIPT_DIR/run-merge-validations.sh"
DRIVER="$SCRIPT_DIR/run-merge-validations-scenario.sh"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

fail=0
pass() { echo "PASS: $1"; }
fl()   { echo "FAIL: $1"; fail=1; }

[ -f "$RUN" ]    || { fl "missing harness: $RUN"; echo "SCENARIO_FAILED"; exit 1; }
[ -f "$DRIVER" ] || { fl "missing driver: $DRIVER"; echo "SCENARIO_FAILED"; exit 1; }

# ---------------------------------------------------------------------------
# PART A — shipped contract (source of truth). These pin current behavior.
# ---------------------------------------------------------------------------

# A1: mixed output emits the "output unparseable" WARNING for the garbage validator.
cat > "$WORK/m1.txt" <<'EOF'
```json
{"validated":[{"finding_id":1,"verdict":"confirmed","new_severity":"high"}]}
```
EOF
printf '%s\n' '{"validated":[{"finding_id":1,"verdict":"confirmed","new_severity":"low"}]}' > "$WORK/m2.txt"
printf '%s\n' 'ERROR: agent timed out, no structured output' > "$WORK/m3.txt"
"$RUN" "$WORK/m1.txt" "$WORK/m2.txt" "$WORK/m3.txt" 2 1 "$WORK/mout" >/dev/null 2>"$WORK/merr.txt" || true
if grep -q "output unparseable" "$WORK/merr.txt"; then
  pass "shipped: mixed output emits 'output unparseable' WARNING"
else
  fl "shipped: expected 'output unparseable' WARNING not found (contract changed?)"
fi

# A2: all-unparseable is FATAL — exit 1, FATAL message, no output JSON,
#     and preserved validator_*_raw.txt artifacts.
printf '%s\n' 'no json here, just a log line' > "$WORK/g1.txt"
printf '%s\n' 'another { stray brace only'    > "$WORK/g2.txt"
printf '%s\n' 'timed out'                      > "$WORK/g3.txt"
gout="$("$RUN" "$WORK/g1.txt" "$WORK/g2.txt" "$WORK/g3.txt" 2 1 "$WORK/gdir" 2>"$WORK/gerr.txt")"
grc=$?
[ "$grc" = "1" ] \
  && pass "shipped: all-unparseable exits 1 (FATAL, not graceful)" \
  || fl  "shipped: all-unparseable exited $grc, expected 1"
if grep -q "FATAL: all validators produced unparseable output" "$WORK/gerr.txt"; then
  pass "shipped: all-unparseable prints the FATAL diagnostic"
else
  fl "shipped: FATAL diagnostic not found for all-unparseable"
fi
[ -z "$gout" ] \
  && pass "shipped: all-unparseable writes no merged JSON to stdout" \
  || fl  "shipped: all-unparseable unexpectedly emitted JSON: '$gout'"
if ls "$WORK/gdir"/cycle_*/validator_*_raw.txt >/dev/null 2>&1; then
  pass "shipped: all-unparseable preserves validator_*_raw.txt artifacts"
else
  fl "shipped: validator_*_raw.txt artifacts not preserved"
fi

# ---------------------------------------------------------------------------
# PART B — driver must be retconned to the shipped contract (FAILS on stale driver).
# ---------------------------------------------------------------------------

# B1: Case-1 greps the CURRENT wording, not the stale "no parseable JSON object".
grep -q "output unparseable" "$DRIVER" \
  && pass "driver greps current 'output unparseable' wording" \
  || fl  "driver does not grep 'output unparseable' (stale Case-1 diagnostic)"
grep -q "no parseable JSON object" "$DRIVER" \
  && fl  "driver still greps stale 'no parseable JSON object' string" \
  || pass "driver no longer greps stale 'no parseable JSON object'"

# B2: Case-3 asserts the FATAL exit-1 contract, not graceful exit 0 / confirmed_count=0.
grep -q "confirmed_count expected 0 for garbage" "$DRIVER" \
  && fl  "driver still asserts graceful 'confirmed_count=0 for garbage' (stale Case-3)" \
  || pass "driver dropped the stale graceful-degrade Case-3 assertion"
grep -q "FATAL: all validators produced unparseable output" "$DRIVER" \
  && pass "driver asserts the shipped FATAL message in Case-3" \
  || fl  "driver does not assert the FATAL message for all-unparseable"
grep -Eq "validator_.*_raw\.txt" "$DRIVER" \
  && pass "driver asserts preserved validator_*_raw.txt artifacts in Case-3" \
  || fl  "driver does not assert preserved validator_*_raw.txt artifacts"

# B3 (the acceptance gate): running the driver passes cleanly.
if bash "$DRIVER" >"$WORK/driver.out" 2>&1; then drc=0; else drc=$?; fi
if grep -q "ALL_CASES_PASSED" "$WORK/driver.out" && [ "$drc" = "0" ]; then
  pass "driver run: ALL_CASES_PASSED and exit 0"
else
  fl "driver run failed (exit $drc); see output below:"
  sed 's/^/    driver| /' "$WORK/driver.out"
fi

if [ "$fail" -eq 0 ]; then echo "ALL_CASES_PASSED"; exit 0; fi
echo "SCENARIO_FAILED"; exit 1
