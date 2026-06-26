#!/usr/bin/env bash
# Self-asserting gadugi-test scenario body for issue #820.
#
# Drives the SHIPPED `merge-validations` recipe logic (via run-merge-validations.sh)
# through mixed validator-output cases and exits non-zero if any case regresses.
# Designed to be launched by the gadugi `execute` action with no arguments, then
# checked with `validate_exit_code` + an `ALL_CASES_PASSED` output assertion.
#
# Before the #820 fix, mixed validator output (JSON inside a ```json fence, or
# with log preamble) crashed `jq --slurpfile` with "Bad JSON" and aborted the
# whole audit cycle. After the fix, output is normalized via the tolerant
# `extract-json` helper, unparseable output yields a targeted diagnostic, and the
# merge always produces valid JSON.
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RUN="$SCRIPT_DIR/run-merge-validations.sh"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
fail=0

pass() { echo "PASS: $1"; }
fl()   { echo "FAIL: $1"; fail=1; }

confirmed_count() { printf '%s' "$1" | jq -r '.confirmed_count' 2>/dev/null; }
first_verdict()   { printf '%s' "$1" | jq -r '.validated[0].verdict' 2>/dev/null; }

# ---------------------------------------------------------------------------
# Case 1: mixed output — prose+```json fence, bare JSON, and log-only garbage.
# This is the exact shape that used to crash jq with "Bad JSON".
# ---------------------------------------------------------------------------
cat > "$WORK/v1.txt" <<'EOF'
Let me validate the findings now.
I read src/foo.rs:42 — this is a genuine silent fallback.

```json
{"validator":"agent-1","cycle":1,"validated":[{"finding_id":1,"verdict":"confirmed","new_severity":"high","reasoning":"swallows error"}]}
```
EOF
cat > "$WORK/v2.txt" <<'EOF'
{"validator":"agent-2","cycle":1,"validated":[{"finding_id":1,"verdict":"confirmed","new_severity":"medium","reasoning":"agrees"}]}
EOF
cat > "$WORK/v3.txt" <<'EOF'
ERROR: agent timed out before producing structured output
[trace] partial log line with a stray { brace but no closing object
done.
EOF

out="$("$RUN" "$WORK/v1.txt" "$WORK/v2.txt" "$WORK/v3.txt" 2 1 "$WORK/out1" 2>"$WORK/err1.txt")"
rc=$?
[ "$rc" = "0" ] && pass "mixed output does not crash (exit 0)" || fl "mixed output exited $rc"
if grep -q "Bad JSON" "$WORK/err1.txt"; then
  fl "jq 'Bad JSON' leaked on mixed output"
else
  pass "no jq 'Bad JSON' on mixed output"
fi
cc="$(confirmed_count "$out")"
[ "$cc" = "1" ] && pass "two well-formed validators confirm the finding (confirmed_count=1)" \
  || fl "confirmed_count expected 1, got '$cc'"
v="$(first_verdict "$out")"
[ "$v" = "confirmed" ] && pass "finding verdict is confirmed" || fl "verdict expected confirmed, got '$v'"
if grep -q "no parseable JSON object" "$WORK/err1.txt"; then
  pass "log-only validator triggers targeted diagnostic"
else
  fl "log-only validator did not trigger a diagnostic"
fi

# ---------------------------------------------------------------------------
# Case 2: all three validators emit bare JSON objects (no fences) — must merge.
# ---------------------------------------------------------------------------
printf '%s\n' '{"validated":[{"finding_id":7,"verdict":"confirmed","new_severity":"high"}]}' > "$WORK/p1.txt"
printf '%s\n' '{"validated":[{"finding_id":7,"verdict":"confirmed","new_severity":"low"}]}'  > "$WORK/p2.txt"
printf '%s\n' '{"validated":[{"finding_id":7,"verdict":"false_positive"}]}'                  > "$WORK/p3.txt"
out2="$("$RUN" "$WORK/p1.txt" "$WORK/p2.txt" "$WORK/p3.txt" 2 1 "$WORK/out2" 2>"$WORK/err2.txt")"
rc2=$?
[ "$rc2" = "0" ] && pass "bare-JSON output does not crash (exit 0)" || fl "bare-JSON output exited $rc2"
cc2="$(confirmed_count "$out2")"
[ "$cc2" = "1" ] && pass "majority confirms finding 7 (confirmed_count=1)" \
  || fl "confirmed_count expected 1 for finding 7, got '$cc2'"

# ---------------------------------------------------------------------------
# Case 3: every validator produced unparseable output — degrade, do not crash.
# ---------------------------------------------------------------------------
printf '%s\n' 'no json here, just a log line' > "$WORK/g1.txt"
printf '%s\n' 'another { stray brace only'    > "$WORK/g2.txt"
printf '%s\n' 'timed out'                      > "$WORK/g3.txt"
out3="$("$RUN" "$WORK/g1.txt" "$WORK/g2.txt" "$WORK/g3.txt" 2 1 "$WORK/out3" 2>"$WORK/err3.txt")"
rc3=$?
[ "$rc3" = "0" ] && pass "all-garbage output degrades gracefully (exit 0)" || fl "all-garbage output exited $rc3"
if grep -q "Bad JSON" "$WORK/err3.txt"; then fl "jq 'Bad JSON' leaked on all-garbage output"; else pass "no jq 'Bad JSON' on all-garbage output"; fi
cc3="$(confirmed_count "$out3")"
[ "$cc3" = "0" ] && pass "no confirmed findings from garbage (confirmed_count=0)" \
  || fl "confirmed_count expected 0 for garbage, got '$cc3'"

if [ "$fail" -eq 0 ]; then
  echo "ALL_CASES_PASSED"
  exit 0
fi
echo "SCENARIO_FAILED"
exit 1
