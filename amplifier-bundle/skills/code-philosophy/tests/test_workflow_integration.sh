#!/usr/bin/env bash
# Tests for code-philosophy skill — Workflow & integration
# Validates the 3-pass + re-assessment workflow, cross-document consistency,
# dev-orchestrator delegation, and edge cases.

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

assert_not_in_file() {
  local label="$1"; local pattern="$2"; local file="$3"
  if [[ ! -f "$file" ]]; then
    fail "$label — file not found: $file"
    return
  fi
  if grep -qiE "$pattern" "$file" 2>/dev/null; then
    fail "$label — forbidden pattern found in $(basename "$file")"
  else
    pass "$label"
  fi
}

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
SKILL_DIR="$(dirname "$SCRIPT_DIR")"
SKILL_FILE="$SKILL_DIR/SKILL.md"
REFERENCE_FILE="$SKILL_DIR/reference.md"
REPO_ROOT="$(cd "$SKILL_DIR/../../.." && pwd)"

echo "═══════════════════════════════════════════════════════"
echo "  Test Suite: code-philosophy — Workflow & Integration"
echo "═══════════════════════════════════════════════════════"

for f in "$SKILL_FILE" "$REFERENCE_FILE"; do
  if [[ ! -f "$f" ]]; then
    echo "FATAL: $(basename "$f") not found — cannot run tests"
    exit 1
  fi
done

# ─── Test 1: Workflow order — passes must be sequential ─────────────────────

echo ""
echo "Test 1: Pass ordering — 1→2→3→re-assessment"

PASS1_LINE=$(grep -n "Pass 1\|BRICK RULE" "$SKILL_FILE" | head -1 | cut -d: -f1)
PASS2_LINE=$(grep -n "Pass 2\|QUALITY INVARIANT" "$SKILL_FILE" | head -1 | cut -d: -f1)
PASS3_LINE=$(grep -n "Pass 3\|PHILOSOPHY SPIRIT" "$SKILL_FILE" | head -1 | cut -d: -f1)
REASSESS_LINE=$(grep -ni "re.assessment\|reassessment" "$SKILL_FILE" | head -1 | cut -d: -f1)

if [[ -n "$PASS1_LINE" && -n "$PASS2_LINE" && "$PASS1_LINE" -lt "$PASS2_LINE" ]]; then
  pass "Pass 1 (BRICK RULE) appears before Pass 2 (QUALITY INVARIANTS)"
else
  fail "Pass 1 must appear before Pass 2 in SKILL.md"
fi

if [[ -n "$PASS2_LINE" && -n "$PASS3_LINE" && "$PASS2_LINE" -lt "$PASS3_LINE" ]]; then
  pass "Pass 2 (QUALITY INVARIANTS) appears before Pass 3 (PHILOSOPHY SPIRIT)"
else
  fail "Pass 2 must appear before Pass 3 in SKILL.md"
fi

if [[ -n "$PASS3_LINE" && -n "$REASSESS_LINE" && "$PASS3_LINE" -lt "$REASSESS_LINE" ]]; then
  pass "Pass 3 appears before Re-assessment"
else
  fail "Re-assessment must appear after all 3 passes"
fi

# ─── Test 2: Re-assessment is conditional ───────────────────────────────────

echo ""
echo "Test 2: Re-assessment is conditional on changes"

# Re-assessment should only happen if changes were proposed/made
assert_in_file \
  "SKILL.md: re-assessment is conditional" \
  "if.*change|when.*change|only.*if|conditional|changes.*made|changes.*proposed" \
  "$SKILL_FILE"

# Must not recurse infinitely — single re-assessment
assert_in_file \
  "SKILL.md: limits re-assessment passes" \
  "single.*re.assessment|one.*re.assessment|max.*re.assessment|no.*recurs|1.*re.assessment|2.*re.assessment" \
  "$SKILL_FILE"

# ─── Test 3: Dev-orchestrator delegation — not direct edits ─────────────────

echo ""
echo "Test 3: Fix delegation to dev-orchestrator"

assert_in_file \
  "SKILL.md delegates fixes to dev-orchestrator" \
  "dev-orchestrator" \
  "$SKILL_FILE"

# Must formulate fix description for dev-orchestrator
assert_in_file \
  "SKILL.md describes fix formulation" \
  "formulate|description|invoke.*dev|delegate.*fix|fix.*description" \
  "$SKILL_FILE"

# Must NOT include write/edit tools in allowed-tools (if present)
TOOLS_LINE=$(grep "allowed.tools:" "$SKILL_FILE" 2>/dev/null || true)
if [[ -n "$TOOLS_LINE" ]]; then
  for tool in "Write" "Edit" "Create"; do
    if echo "$TOOLS_LINE" | grep -q "\"$tool\""; then
      fail "allowed-tools includes write tool '$tool' — skill is advisory-only"
    else
      pass "allowed-tools correctly excludes: $tool"
    fi
  done
else
  pass "no allowed-tools line (delegation model does not need write tools)"
fi

# ─── Test 4: Cross-document consistency ─────────────────────────────────────

echo ""
echo "Test 4: Cross-document consistency (SKILL.md ↔ reference.md)"

# Both files should reference the same 3 pass names
for pass_name in "BRICK RULE" "QUALITY INVARIANT" "PHILOSOPHY SPIRIT"; do
  SKILL_HAS=$(grep -ci "$pass_name" "$SKILL_FILE" || true)
  REF_HAS=$(grep -ci "$pass_name" "$REFERENCE_FILE" || true)
  if [[ "$SKILL_HAS" -gt 0 && "$REF_HAS" -gt 0 ]]; then
    pass "both files reference: $pass_name"
  else
    fail "pass name '$pass_name' missing from $([ "$SKILL_HAS" -eq 0 ] && echo "SKILL.md" || echo "reference.md")"
  fi
done

# Both files should reference the same severity levels
for level in "critical" "high" "medium" "low"; do
  SKILL_HAS=$(grep -ci "$level" "$SKILL_FILE" || true)
  REF_HAS=$(grep -ci "$level" "$REFERENCE_FILE" || true)
  if [[ "$SKILL_HAS" -gt 0 && "$REF_HAS" -gt 0 ]]; then
    pass "both files use severity: $level"
  else
    fail "severity '$level' missing from $([ "$SKILL_HAS" -eq 0 ] && echo "SKILL.md" || echo "reference.md")"
  fi
done

# ─── Test 5: PHILOSOPHY.md is referenced, not embedded ──────────────────────

echo ""
echo "Test 5: PHILOSOPHY.md referenced by path, not embedded"

assert_in_file \
  "SKILL.md references PHILOSOPHY.md by path" \
  "PHILOSOPHY\.md" \
  "$SKILL_FILE"

# Should not embed large chunks of PHILOSOPHY.md content
# (heuristic: PHILOSOPHY.md's "Wabi-sabi" or full "Zen of Simple Code" section
# should not be copied verbatim)
assert_not_in_file \
  "SKILL.md does not embed PHILOSOPHY.md 'Wabi-sabi philosophy'" \
  "Wabi-sabi philosophy.*Embracing simplicity" \
  "$SKILL_FILE"

# ─── Test 6: Edge cases documented ──────────────────────────────────────────

echo ""
echo "Test 6: Edge cases / Known Failure Points"

assert_in_file \
  "SKILL.md documents edge cases or known failures" \
  "Known Failure|edge case|limitation|false positive|caveat" \
  "$SKILL_FILE"

# Should mention generated/vendored code
assert_in_file \
  "handles generated or vendored code" \
  "generat|vendored|auto.generated|macro|codegen" \
  "$SKILL_FILE"

# Should mention test files (unwrap in tests is OK)
assert_in_file \
  "handles test file exceptions" \
  "test.*file|test.*util|test.*exception|test.*allowed|tests.*unwrap" \
  "$SKILL_FILE"

# ─── Test 7: Skill does NOT duplicate code-smell-detector ───────────────────

echo ""
echo "Test 7: Differentiation from code-smell-detector"

# code-philosophy is a 3-pass auditor; code-smell-detector is a pattern detector.
# They should have different purposes.
assert_in_file \
  "SKILL.md documents its unique purpose (audit/compliance)" \
  "audit|compliance|philosophy.*check|three.*pass|3.pass|multi.pass" \
  "$SKILL_FILE"

# ─── Test 8: Token budget or scope management ──────────────────────────────

echo ""
echo "Test 8: Resource management"

# Should have token_budget or scope management documented
if grep -qiE "token_budget|token.budget|scope|budget" "$SKILL_FILE"; then
  pass "resource management documented (token budget or scope)"
else
  fail "should document token budget or scope management"
fi

# ─── Test 9: Report format — structured output ─────────────────────────────

echo ""
echo "Test 9: Report format specification"

# reference.md should include a report template or format specification
assert_in_file \
  "reference.md includes report format/template" \
  "report|Report|template|format|output" \
  "$REFERENCE_FILE"

# Report should include summary with counts
assert_in_file \
  "reference.md specifies finding counts in report" \
  "total|count|summary|finding.*count|violation.*count" \
  "$REFERENCE_FILE"

# Report should include per-pass breakdown
assert_in_file \
  "reference.md specifies per-pass breakdown" \
  "per.pass|by pass|Pass 1.*Pass 2|breakdown|per.*pass" \
  "$REFERENCE_FILE"

# ─── Test 10: Security — no code execution of audited content ───────────────

echo ""
echo "Test 10: Security — audit-only, no execution"

# Skill must analyze structurally, never execute audited code
assert_in_file \
  "documents structural analysis approach" \
  "structur|grep|view|read.only|analyz|scan|inspect" \
  "$SKILL_FILE"

assert_not_in_file \
  "does not instruct to execute audited code" \
  "run the.*code|execute.*target|eval.*target|exec.*audited" \
  "$SKILL_FILE"

# ─── Test 11: Skill name consistency ────────────────────────────────────────

echo ""
echo "Test 11: Skill name consistency"

# Frontmatter name must be 'code-philosophy'
if head -10 "$SKILL_FILE" | grep -q "^name: code-philosophy$"; then
  pass "frontmatter name is exactly 'code-philosophy'"
else
  fail "frontmatter name must be exactly 'code-philosophy'"
fi

# Directory name must match
DIRNAME=$(basename "$SKILL_DIR")
if [[ "$DIRNAME" == "code-philosophy" ]]; then
  pass "directory name matches skill name"
else
  fail "directory name '$DIRNAME' does not match skill name 'code-philosophy'"
fi

# ─── Summary ─────────────────────────────────────────────────────────────────

echo ""
echo "═══════════════════════════════"
echo "Results: $PASS passed, $FAIL failed"
echo "═══════════════════════════════"

if [[ "$FAIL" -gt 0 ]]; then
  exit 1
fi
