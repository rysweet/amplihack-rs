#!/usr/bin/env bash
# Tests for code-philosophy skill — Pass 1: BRICK RULE compliance
# Validates that SKILL.md and reference.md document all BRICK RULE checks
# derived from PHILOSOPHY.md thresholds.

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
    fail "$label — pattern not found in $(basename "$file")"
  fi
}

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
SKILL_DIR="$(dirname "$SCRIPT_DIR")"
SKILL_FILE="$SKILL_DIR/SKILL.md"
REFERENCE_FILE="$SKILL_DIR/reference.md"
REPO_ROOT="$(cd "$SKILL_DIR/../../.." && pwd)"

echo "═══════════════════════════════════════════════════════"
echo "  Test Suite: code-philosophy — Pass 1 BRICK RULES"
echo "═══════════════════════════════════════════════════════"

# Guard: both files must exist
for f in "$SKILL_FILE" "$REFERENCE_FILE"; do
  if [[ ! -f "$f" ]]; then
    echo "FATAL: $(basename "$f") not found — cannot run tests"
    exit 1
  fi
done

# ─── Test 1: File LOC limit ≤400 ────────────────────────────────────────────

echo ""
echo "Test 1: File LOC limit (≤400 lines per file)"

assert_in_file \
  "SKILL.md documents 400 LOC limit" \
  "400.*LOC|400.*lines|≤\s*400|<=\s*400|max.*400" \
  "$SKILL_FILE"

assert_in_file \
  "reference.md documents 400 LOC limit" \
  "400.*LOC|400.*lines|≤\s*400|<=\s*400|max.*400" \
  "$REFERENCE_FILE"

# ─── Test 2: Function LOC limit ≤50 ─────────────────────────────────────────

echo ""
echo "Test 2: Function LOC limit (≤50 lines per function)"

assert_in_file \
  "SKILL.md documents 50-line function limit" \
  "50.*line|50.*LOC|≤\s*50|<=\s*50|function.*50|fn.*50" \
  "$SKILL_FILE"

assert_in_file \
  "reference.md documents function size limit" \
  "50.*line|50.*LOC|function.*50|fn.*50" \
  "$REFERENCE_FILE"

# ─── Test 3: God object detection ────────────────────────────────────────────

echo ""
echo "Test 3: God object detection"

assert_in_file \
  "SKILL.md documents god object check" \
  "god object|God Object|GOD.OBJECT|multiple.*responsibilit" \
  "$SKILL_FILE"

# Should define a threshold (e.g., >10 fields/methods)
assert_in_file \
  "reference.md defines god object threshold" \
  "god object|>.*10.*field|>.*10.*method|multiple.*responsibilit" \
  "$REFERENCE_FILE"

# ─── Test 4: Deep inheritance check ──────────────────────────────────────────

echo ""
echo "Test 4: Deep inheritance detection"

assert_in_file \
  "SKILL.md documents inheritance depth check" \
  "deep.*inherit|inheritance.*depth|inherit.*level|>.*2.*level|inheritance.*>.*2" \
  "$SKILL_FILE"

assert_in_file \
  "reference.md documents inheritance threshold" \
  "inherit.*depth|>.*2.*level|inheritance.*>.*2|deep.*inherit" \
  "$REFERENCE_FILE"

# ─── Test 5: BRICK RULE pass includes all 4 checks ──────────────────────────

echo ""
echo "Test 5: BRICK RULE pass covers all 4 required checks"

# All 4 checks must appear in context of Pass 1 / BRICK RULE
BRICK_SECTION=$(sed -n '/BRICK RULE/,/^## \|^### Pass [23]/p' "$SKILL_FILE" 2>/dev/null || true)

if [[ -n "$BRICK_SECTION" ]]; then
  if echo "$BRICK_SECTION" | grep -qiE "400|file.*LOC|LOC.*file"; then
    pass "BRICK RULE section includes file LOC check"
  else
    fail "BRICK RULE section missing file LOC check"
  fi

  if echo "$BRICK_SECTION" | grep -qiE "50|function.*line|fn.*line"; then
    pass "BRICK RULE section includes function LOC check"
  else
    fail "BRICK RULE section missing function LOC check"
  fi

  if echo "$BRICK_SECTION" | grep -qiE "god.*object|multiple.*responsib"; then
    pass "BRICK RULE section includes god object check"
  else
    fail "BRICK RULE section missing god object check"
  fi

  if echo "$BRICK_SECTION" | grep -qiE "inherit|depth.*>.*2"; then
    pass "BRICK RULE section includes inheritance depth check"
  else
    fail "BRICK RULE section missing inheritance depth check"
  fi
else
  fail "could not extract BRICK RULE section from SKILL.md"
fi

# ─── Test 6: Severity levels assigned in Pass 1 ─────────────────────────────

echo ""
echo "Test 6: Severity levels defined for BRICK RULE violations"

# reference.md should map brick-rule violations to severities
assert_in_file \
  "reference.md assigns severity to file >400 LOC" \
  "400.*critical|400.*high|file.*LOC.*(critical|high)" \
  "$REFERENCE_FILE"

assert_in_file \
  "reference.md assigns severity to function >50 LOC" \
  "50.*(critical|high|medium)|function.*(critical|high|medium)" \
  "$REFERENCE_FILE"

# ─── Summary ─────────────────────────────────────────────────────────────────

echo ""
echo "═══════════════════════════════"
echo "Results: $PASS passed, $FAIL failed"
echo "═══════════════════════════════"

if [[ "$FAIL" -gt 0 ]]; then
  exit 1
fi
