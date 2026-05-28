#!/usr/bin/env bash
# Tests for code-philosophy skill — Pass 3: PHILOSOPHY SPIRIT
# Validates detection of ruthless simplicity violations, zero-BS naming,
# modular regeneratable bricks, over-abstraction, and sycophancy in comments.

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

echo "═══════════════════════════════════════════════════════"
echo "  Test Suite: code-philosophy — Pass 3 PHILOSOPHY SPIRIT"
echo "═══════════════════════════════════════════════════════"

for f in "$SKILL_FILE" "$REFERENCE_FILE"; do
  if [[ ! -f "$f" ]]; then
    echo "FATAL: $(basename "$f") not found — cannot run tests"
    exit 1
  fi
done

# ─── Test 1: Ruthless simplicity detection ───────────────────────────────────

echo ""
echo "Test 1: Ruthless simplicity violation detection"

assert_in_file \
  "SKILL.md documents ruthless simplicity checks" \
  "ruthless.*simpl|simplicity" \
  "$SKILL_FILE"

assert_in_file \
  "reference.md documents simplicity patterns" \
  "ruthless.*simpl|simplicity|over.engineer|future.proof" \
  "$REFERENCE_FILE"

# ─── Test 2: Zero-BS naming ─────────────────────────────────────────────────

echo ""
echo "Test 2: Zero-BS naming detection"

assert_in_file \
  "SKILL.md documents naming checks" \
  "naming|zero.BS|name.*clarity|clear.*name" \
  "$SKILL_FILE"

assert_in_file \
  "reference.md documents bad naming patterns" \
  "naming|Manager|Helper|Util|Handler|Processor|Base.*class" \
  "$REFERENCE_FILE"

# ─── Test 3: Modular regeneratable bricks ────────────────────────────────────

echo ""
echo "Test 3: Modular regeneratable bricks"

assert_in_file \
  "SKILL.md documents brick/module checks" \
  "brick|modular|regenerat|self.contained" \
  "$SKILL_FILE"

assert_in_file \
  "reference.md documents regeneration assessment" \
  "regenerat|brick|self.contained|module.*boundar" \
  "$REFERENCE_FILE"

# ─── Test 4: Over-abstraction detection ─────────────────────────────────────

echo ""
echo "Test 4: Over-abstraction detection"

assert_in_file \
  "SKILL.md documents over-abstraction check" \
  "over.abstract|unnecessary.*abstract|abstraction.*layer" \
  "$SKILL_FILE"

assert_in_file \
  "reference.md documents abstraction anti-patterns" \
  "over.abstract|single.*impl|wrapper|unnecessary.*layer|abstraction.*layer" \
  "$REFERENCE_FILE"

# ─── Test 5: Sycophancy in comments ─────────────────────────────────────────

echo ""
echo "Test 5: Sycophancy detection in comments"

assert_in_file \
  "SKILL.md documents sycophancy check" \
  "sycophancy|sycophant|praise.*word|flattery|platitude" \
  "$SKILL_FILE"

# Must detect specific sycophantic words
SYCOPHANTIC_WORDS=("Great" "Excellent" "Amazing" "Beautiful" "Brilliant")
FOUND_ANY=false
for word in "${SYCOPHANTIC_WORDS[@]}"; do
  if grep -q "$word" "$REFERENCE_FILE" 2>/dev/null; then
    FOUND_ANY=true
    break
  fi
done

if $FOUND_ANY; then
  pass "reference.md lists specific sycophantic words to detect"
else
  fail "reference.md should list specific sycophantic words (Great, Excellent, Amazing, etc.)"
fi

# ─── Test 6: PHILOSOPHY SPIRIT pass contains all checks ─────────────────────

echo ""
echo "Test 6: PHILOSOPHY SPIRIT pass is comprehensive"

SPIRIT_SECTION=$(sed -n '/PHILOSOPHY SPIRIT/,/^## \|^### Pass [12]\|^### Re.assessment/p' "$SKILL_FILE" 2>/dev/null || true)

if [[ -n "$SPIRIT_SECTION" ]]; then
  for check in "simplicity|simple" "naming|name" "brick|modular" "abstract" "sycophancy|sycophant|flattery"; do
    if echo "$SPIRIT_SECTION" | grep -qiE "$check"; then
      pass "PHILOSOPHY SPIRIT section includes: $check"
    else
      fail "PHILOSOPHY SPIRIT section missing: $check"
    fi
  done
else
  fail "could not extract PHILOSOPHY SPIRIT section from SKILL.md"
fi

# ─── Test 7: Future-proofing detection ──────────────────────────────────────

echo ""
echo "Test 7: Future-proofing anti-pattern detection"

assert_in_file \
  "reference.md flags future-proofing" \
  "future.proof|hypothetical|maybe someday|just in case" \
  "$REFERENCE_FILE"

# ─── Summary ─────────────────────────────────────────────────────────────────

echo ""
echo "═══════════════════════════════"
echo "Results: $PASS passed, $FAIL failed"
echo "═══════════════════════════════"

if [[ "$FAIL" -gt 0 ]]; then
  exit 1
fi
