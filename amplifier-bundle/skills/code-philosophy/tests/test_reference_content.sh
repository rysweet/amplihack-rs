#!/usr/bin/env bash
# Tests for code-philosophy skill — reference.md content completeness
# Validates that reference.md contains detailed detection criteria,
# code examples (good/bad), severity escalation, and report format.

set -euo pipefail

PASS=0
FAIL=0

pass() { echo "  PASS: $1"; PASS=$((PASS+1)); }
fail() { echo "  FAIL: $1"; FAIL=$((FAIL+1)); }

assert_in_file() {
  local label="$1"; local pattern="$2"; local file="$3"
  if [[ ! -f "$file" ]]; then
    fail "$label — file not found: $file"
    return
  fi
  if grep -qiE "$pattern" "$file" 2>/dev/null; then
    pass "$label"
  else
    fail "$label — pattern not found"
  fi
}

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
SKILL_DIR="$(dirname "$SCRIPT_DIR")"
REFERENCE_FILE="$SKILL_DIR/reference.md"

echo "═══════════════════════════════════════════════════════"
echo "  Test Suite: code-philosophy — reference.md Content"
echo "═══════════════════════════════════════════════════════"

if [[ ! -f "$REFERENCE_FILE" ]]; then
  echo "FATAL: reference.md not found — cannot run tests"
  exit 1
fi

# ─── Test 1: All 3 passes documented in reference ──────────────────────────

echo ""
echo "Test 1: All 3 passes documented"

assert_in_file "Pass 1 in reference.md" "BRICK RULE|Pass 1" "$REFERENCE_FILE"
assert_in_file "Pass 2 in reference.md" "QUALITY INVARIANT|Pass 2" "$REFERENCE_FILE"
assert_in_file "Pass 3 in reference.md" "PHILOSOPHY SPIRIT|Pass 3" "$REFERENCE_FILE"

# ─── Test 2: Code examples — good and bad patterns ─────────────────────────

echo ""
echo "Test 2: Code examples (good and bad patterns)"

# Must have code blocks
CODE_BLOCK_COUNT=$(grep -c '```' "$REFERENCE_FILE" || true)
if [[ "$CODE_BLOCK_COUNT" -ge 4 ]]; then
  pass "at least 4 code blocks found ($CODE_BLOCK_COUNT — includes good/bad examples)"
else
  fail "expected at least 4 code blocks (good/bad examples), found $CODE_BLOCK_COUNT"
fi

# Should have explicit BAD/GOOD or violation/correct labeling
assert_in_file \
  "reference.md labels bad patterns" \
  "BAD|bad|violation|VIOLATION|anti.pattern|SMELL|❌" \
  "$REFERENCE_FILE"

assert_in_file \
  "reference.md labels good patterns" \
  "GOOD|good|correct|CORRECT|compliant|✅|FIXED" \
  "$REFERENCE_FILE"

# ─── Test 3: Severity escalation rules ──────────────────────────────────────

echo ""
echo "Test 3: Severity escalation rules"

assert_in_file \
  "defines when severity escalates" \
  "escalat|severity.*rule|when.*critical|threshold.*severity|bump|promote" \
  "$REFERENCE_FILE"

# All 4 severity levels must be defined
for level in "critical" "high" "medium" "low"; do
  assert_in_file "severity level defined: $level" "$level" "$REFERENCE_FILE"
done

# ─── Test 4: Per-check detection patterns ───────────────────────────────────

echo ""
echo "Test 4: Detection patterns per check"

# BRICK RULE checks
assert_in_file "LOC counting pattern" "wc -l|line.*count|LOC|lines of code" "$REFERENCE_FILE"
assert_in_file "inheritance depth pattern" "inherit|class.*chain|depth" "$REFERENCE_FILE"

# QUALITY INVARIANT checks
assert_in_file "unwrap detection pattern" "unwrap|\.unwrap" "$REFERENCE_FILE"
assert_in_file "panic detection pattern" "panic" "$REFERENCE_FILE"
assert_in_file "unsafe detection pattern" "unsafe" "$REFERENCE_FILE"

# PHILOSOPHY SPIRIT checks
assert_in_file "over-abstraction detection" "abstract|layer|wrapper" "$REFERENCE_FILE"
assert_in_file "sycophancy detection" "sycophancy|sycophant|flattery|praise" "$REFERENCE_FILE"
assert_in_file "naming anti-patterns" "Manager|Helper|Util|Handler|Base" "$REFERENCE_FILE"

# ─── Test 5: Report format specification ────────────────────────────────────

echo ""
echo "Test 5: Report format specification"

# Must define the output format
assert_in_file \
  "report format section exists" \
  "Report Format|report format|Output Format|output format|Report Template" \
  "$REFERENCE_FILE"

# Report must include these columns/fields
assert_in_file "report includes pass name" "pass.*name|Pass.*Name|pass_name" "$REFERENCE_FILE"
assert_in_file "report includes location" "file.*line|location|Location|file:line" "$REFERENCE_FILE"
assert_in_file "report includes severity" "severity|Severity" "$REFERENCE_FILE"
assert_in_file "report includes fix suggestion" "fix|suggestion|recommend|Fix|Suggestion" "$REFERENCE_FILE"

# ─── Test 6: Language-specific patterns ─────────────────────────────────────

echo ""
echo "Test 6: Language-specific detection patterns"

# Must handle at least Rust and one other language
assert_in_file "Rust-specific patterns" "Rust|\.rs|rust" "$REFERENCE_FILE"

# Should mention at least one other language
OTHER_LANG_COUNT=0
for lang in "Python" "JavaScript" "TypeScript" "Go" "Shell" "Bash"; do
  if grep -qi "$lang" "$REFERENCE_FILE" 2>/dev/null; then
    OTHER_LANG_COUNT=$((OTHER_LANG_COUNT + 1))
  fi
done

if [[ "$OTHER_LANG_COUNT" -ge 1 ]]; then
  pass "at least one non-Rust language documented ($OTHER_LANG_COUNT found)"
else
  fail "should document patterns for at least one non-Rust language"
fi

# ─── Test 7: Proportionality principle ──────────────────────────────────────

echo ""
echo "Test 7: Proportionality principle from PHILOSOPHY.md"

assert_in_file \
  "reference.md documents proportionality" \
  "proportional|Proportional|ratio|test.*ratio|effort.*match" \
  "$REFERENCE_FILE"

# ─── Test 8: reference.md file size ─────────────────────────────────────────

echo ""
echo "Test 8: reference.md file size"

REF_LINES=$(wc -l < "$REFERENCE_FILE")
if [[ "$REF_LINES" -ge 200 ]]; then
  pass "reference.md: at least 200 lines ($REF_LINES — substantive reference)"
else
  fail "reference.md: only $REF_LINES lines — likely incomplete for 3-pass reference (expected >= 200)"
fi

if [[ "$REF_LINES" -le 900 ]]; then
  pass "reference.md: under 900 lines ($REF_LINES — includes recipe architecture docs)"
else
  fail "reference.md: $REF_LINES lines — may be overly verbose (expected <= 900)"
fi

# ─── Summary ─────────────────────────────────────────────────────────────────

echo ""
echo "═══════════════════════════════"
echo "Results: $PASS passed, $FAIL failed"
echo "═══════════════════════════════"

if [[ "$FAIL" -gt 0 ]]; then
  exit 1
fi
