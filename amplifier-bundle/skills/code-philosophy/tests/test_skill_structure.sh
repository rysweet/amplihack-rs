#!/usr/bin/env bash
# Tests for code-philosophy skill — structural validation
# Run: bash amplifier-bundle/skills/code-philosophy/tests/test_skill_structure.sh
# Validates SKILL.md structure, frontmatter, required sections, and file layout.
# All tests are self-contained and follow the crusty-old-engineer test pattern.

set -euo pipefail

PASS=0
FAIL=0

pass() { echo "  PASS: $1"; PASS=$((PASS+1)); }
fail() { echo "  FAIL: $1"; FAIL=$((FAIL+1)); }

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
SKILL_DIR="$(dirname "$SCRIPT_DIR")"
SKILL_FILE="$SKILL_DIR/SKILL.md"
REFERENCE_FILE="$SKILL_DIR/reference.md"
REPO_ROOT="$(cd "$SKILL_DIR/../../.." && pwd)"
PHILOSOPHY_FILE="$REPO_ROOT/amplifier-bundle/context/PHILOSOPHY.md"

echo "═══════════════════════════════════════════════════════"
echo "  Test Suite: code-philosophy skill — Structure"
echo "═══════════════════════════════════════════════════════"

# ─── Test 1: Required files exist ────────────────────────────────────────────

echo ""
echo "Test 1: Required files exist"

if [[ -f "$SKILL_FILE" ]]; then
  pass "SKILL.md exists"
else
  fail "SKILL.md not found at $SKILL_FILE"
  echo "  (Cannot run remaining tests without SKILL.md)"
  echo ""
  echo "═══════════════════════════════"
  echo "Results: $PASS passed, $FAIL failed"
  echo "═══════════════════════════════"
  exit 1
fi

if [[ -f "$REFERENCE_FILE" ]]; then
  pass "reference.md exists"
else
  fail "reference.md not found at $REFERENCE_FILE"
fi

# ─── Test 2: YAML frontmatter — required fields ─────────────────────────────

echo ""
echo "Test 2: YAML frontmatter — required fields"

FIRST_LINE=$(head -1 "$SKILL_FILE")
if [[ "$FIRST_LINE" == "---" ]]; then
  pass "frontmatter starts with --- delimiter"
else
  fail "frontmatter: first line should be '---', got '$FIRST_LINE'"
fi

DELIM_COUNT=$(grep -c "^---$" "$SKILL_FILE" || true)
if [[ "$DELIM_COUNT" -ge 2 ]]; then
  pass "frontmatter has closing --- delimiter ($DELIM_COUNT found)"
else
  fail "frontmatter missing closing --- delimiter (only $DELIM_COUNT found)"
fi

FRONTMATTER=$(sed -n '/^---$/,/^---$/p' "$SKILL_FILE")

if echo "$FRONTMATTER" | grep -q "^name: code-philosophy"; then
  pass "frontmatter: name is 'code-philosophy'"
else
  fail "frontmatter: name field missing or incorrect (expected 'code-philosophy')"
fi

if echo "$FRONTMATTER" | grep -q "^version:"; then
  pass "frontmatter: version field present"
else
  fail "frontmatter: version field missing"
fi

if echo "$FRONTMATTER" | grep -q "^description:"; then
  pass "frontmatter: description field present"
else
  fail "frontmatter: description field missing"
fi

# ─── Test 3: Auto-activation triggers — no overlap with existing skills ──────

echo ""
echo "Test 3: Auto-activation triggers"

# Must have auto_activates or auto-activation section
if grep -q "auto_activates\|auto-activation" "$SKILL_FILE"; then
  pass "auto-activation section present"
else
  fail "auto-activation section missing"
fi

# Required triggers from issue spec
REQUIRED_TRIGGERS=("philosophy check" "brick rule" "code philosophy")
for trigger in "${REQUIRED_TRIGGERS[@]}"; do
  if grep -qi "$trigger" "$SKILL_FILE"; then
    pass "auto-activation trigger present: '$trigger'"
  else
    fail "auto-activation trigger missing: '$trigger'"
  fi
done

# Must NOT overlap with philosophy-compliance-workflow triggers
OVERLAP_TRIGGERS=("philosophy review" "check philosophy" "zen review")
EXISTING_SKILL="$REPO_ROOT/amplifier-bundle/skills/philosophy-compliance-workflow/SKILL.md"
for trigger in "${OVERLAP_TRIGGERS[@]}"; do
  if [[ -f "$EXISTING_SKILL" ]] && grep -qi "$trigger" "$EXISTING_SKILL"; then
    # This trigger belongs to the existing skill — ours must NOT have it
    if grep -qi "auto_activates" "$SKILL_FILE" && sed -n '/auto_activates/,/^[a-z]/p' "$SKILL_FILE" | grep -qi "$trigger"; then
      fail "trigger overlap with philosophy-compliance-workflow: '$trigger'"
    else
      pass "no overlap for existing trigger: '$trigger'"
    fi
  fi
done

# ─── Test 4: Three distinct passes documented ───────────────────────────────

echo ""
echo "Test 4: Three distinct audit passes"

if grep -qi "BRICK RULE\|Brick Rule" "$SKILL_FILE"; then
  pass "Pass 1: BRICK RULE pass documented"
else
  fail "Pass 1: BRICK RULE pass not documented"
fi

if grep -qi "QUALITY INVARIANT" "$SKILL_FILE"; then
  pass "Pass 2: QUALITY INVARIANTS pass documented"
else
  fail "Pass 2: QUALITY INVARIANTS pass not documented"
fi

if grep -qi "PHILOSOPHY SPIRIT" "$SKILL_FILE"; then
  pass "Pass 3: PHILOSOPHY SPIRIT pass documented"
else
  fail "Pass 3: PHILOSOPHY SPIRIT pass not documented"
fi

# Count distinct pass sections (should be at least 3)
PASS_COUNT=$(grep -ci "^### Pass [0-9]\|^## Pass [0-9]" "$SKILL_FILE" || true)
if [[ "$PASS_COUNT" -ge 3 ]]; then
  pass "at least 3 distinct pass sections found ($PASS_COUNT)"
else
  fail "expected at least 3 pass sections, found $PASS_COUNT"
fi

# ─── Test 5: Re-assessment pass documented ───────────────────────────────────

echo ""
echo "Test 5: Re-assessment pass"

if grep -qi "re-assessment\|reassessment\|re.assessment" "$SKILL_FILE"; then
  pass "re-assessment pass documented"
else
  fail "re-assessment pass not documented"
fi

# Re-assessment should only apply to changed files
if grep -qi "changed files\|modified files\|files that changed" "$SKILL_FILE"; then
  pass "re-assessment scoped to changed files"
else
  fail "re-assessment should be scoped to changed files only"
fi

# ─── Test 6: Dev-orchestrator delegation ─────────────────────────────────────

echo ""
echo "Test 6: Dev-orchestrator delegation for fixes"

if grep -q "dev-orchestrator" "$SKILL_FILE"; then
  pass "references dev-orchestrator for fix delegation"
else
  fail "must reference dev-orchestrator for delegating fixes"
fi

# Must NOT make direct code changes
if grep -qi "auditor.*not.*fixer\|audit.*only\|does not.*modify\|read.only\|advisory" "$SKILL_FILE"; then
  pass "documents advisory/audit-only role"
else
  fail "must document that skill is an auditor, not a fixer"
fi

# ─── Test 7: Input modes supported ──────────────────────────────────────────

echo ""
echo "Test 7: Input modes — files, directories, git diffs, PR diffs"

INPUT_MODES=("file" "director" "git diff" "PR diff|pull request")
for mode in "${INPUT_MODES[@]}"; do
  if grep -qiE "$mode" "$SKILL_FILE"; then
    pass "input mode documented: '$mode'"
  else
    fail "input mode not documented: '$mode'"
  fi
done

# ─── Test 8: Structured report format ────────────────────────────────────────

echo ""
echo "Test 8: Structured report format"

REPORT_FIELDS=("pass name|Pass Name|pass_name" "file:line|file.*line|location" "severity|Severity" "suggested fix|Suggested Fix|fix|recommendation")
for field in "${REPORT_FIELDS[@]}"; do
  if grep -qiE "$field" "$SKILL_FILE"; then
    pass "report field documented: '$field'"
  else
    fail "report field not documented: '$field'"
  fi
done

# Severity levels
SEVERITY_LEVELS=("critical" "high" "medium" "low")
for level in "${SEVERITY_LEVELS[@]}"; do
  if grep -qi "$level" "$SKILL_FILE"; then
    pass "severity level documented: '$level'"
  else
    fail "severity level not documented: '$level'"
  fi
done

# ─── Test 9: PHILOSOPHY.md referenced by path ───────────────────────────────

echo ""
echo "Test 9: PHILOSOPHY.md referenced by path (not embedded)"

if grep -q "PHILOSOPHY.md" "$SKILL_FILE"; then
  pass "references PHILOSOPHY.md"
else
  fail "must reference PHILOSOPHY.md"
fi

if grep -q "amplifier-bundle/context/PHILOSOPHY.md\|context/PHILOSOPHY.md\|~/.amplihack/.claude/context/PHILOSOPHY.md" "$SKILL_FILE"; then
  pass "references PHILOSOPHY.md by path"
else
  fail "must reference PHILOSOPHY.md by path, not embed contents"
fi

# ─── Test 10: File size — reasonable bounds ──────────────────────────────────

echo ""
echo "Test 10: File size — reasonable bounds"

LINE_COUNT=$(wc -l < "$SKILL_FILE")
if [[ "$LINE_COUNT" -ge 150 ]]; then
  pass "SKILL.md: at least 150 lines ($LINE_COUNT — substantive for 3-pass skill)"
else
  fail "SKILL.md: only $LINE_COUNT lines — likely incomplete (expected >= 150)"
fi

if [[ "$LINE_COUNT" -le 500 ]]; then
  pass "SKILL.md: under 500 lines ($LINE_COUNT — brick-compliant for orchestrated skill)"
else
  fail "SKILL.md: $LINE_COUNT lines — exceeds 500 LOC limit for orchestrated skill"
fi

if [[ -f "$REFERENCE_FILE" ]]; then
  REF_LINES=$(wc -l < "$REFERENCE_FILE")
  if [[ "$REF_LINES" -ge 100 ]]; then
    pass "reference.md: at least 100 lines ($REF_LINES — substantive)"
  else
    fail "reference.md: only $REF_LINES lines — likely incomplete (expected >= 100)"
  fi
fi

# ─── Test 11: No executable code or secrets ──────────────────────────────────

echo ""
echo "Test 11: No executable code or secrets in SKILL.md"

if grep -qE "^(import |from .* import |require\(|const |let |var |use )" "$SKILL_FILE"; then
  fail "SKILL.md contains executable code imports — should be instruction-only"
else
  pass "no executable code imports found"
fi

if grep -qiE "(sk-|ghp_|Bearer |xoxb-|AKIA)" "$SKILL_FILE"; then
  fail "SKILL.md may contain secrets or API keys"
else
  pass "no secrets detected"
fi

# ─── Test 12: Known Failure Points section ───────────────────────────────────

echo ""
echo "Test 12: Known Failure Points documented"

if grep -qi "Known Failure\|Known Edge Case\|Failure Point\|Limitations" "$SKILL_FILE"; then
  pass "Known Failure Points section present"
else
  fail "Known Failure Points section missing (required by spec)"
fi

# ─── Summary ─────────────────────────────────────────────────────────────────

echo ""
echo "═══════════════════════════════"
echo "Results: $PASS passed, $FAIL failed"
echo "═══════════════════════════════"

if [[ "$FAIL" -gt 0 ]]; then
  exit 1
fi
