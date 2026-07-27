#!/usr/bin/env bash
# Tests for property-based-testing skill — structural validation
# Run: bash amplifier-bundle/skills/property-based-testing/tests/test_skill_structure.sh
#
# Validates SKILL.md frontmatter, required sections, per-language examples,
# quiz, no-secret-leak, and reciprocal cross-references from sibling skills.
# Follows the code-philosophy test pattern. Self-contained; no deps beyond
# coreutils + grep. These tests define the contract for the skill deliverable
# and MUST fail before the skill is authored (TDD).

set -uo pipefail

PASS=0
FAIL=0

pass() { echo "  PASS: $1"; PASS=$((PASS+1)); }
fail() { echo "  FAIL: $1"; FAIL=$((FAIL+1)); }

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
SKILL_DIR="$(dirname "$SCRIPT_DIR")"
SKILL_FILE="$SKILL_DIR/SKILL.md"
SKILLS_ROOT="$(cd "$SKILL_DIR/.." && pwd)"

echo "═══════════════════════════════════════════════════════"
echo "  Test Suite: property-based-testing skill — Structure"
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
  echo "═══════════════════════════"
  echo "Results: $PASS passed, $FAIL failed"
  echo "═══════════════════════════"
  exit 1
fi

README_FILE="$SKILL_DIR/README.md"
if [[ -f "$README_FILE" ]]; then
  pass "README.md exists"
else
  fail "README.md not found at $README_FILE"
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

DELIM_COUNT=$(grep -c "^---$" "$SKILL_FILE" || echo 0)
if [[ "$DELIM_COUNT" -ge 2 ]]; then
  pass "frontmatter has closing --- delimiter ($DELIM_COUNT found)"
else
  fail "frontmatter missing closing --- delimiter (only $DELIM_COUNT found)"
fi

FRONTMATTER=$(sed -n '/^---$/,/^---$/p' "$SKILL_FILE")

if echo "$FRONTMATTER" | grep -q "^name: property-based-testing"; then
  pass "frontmatter name is 'property-based-testing'"
else
  fail "frontmatter name must be exactly 'property-based-testing'"
fi

if echo "$FRONTMATTER" | grep -q "^version:"; then
  pass "frontmatter has version field"
else
  fail "frontmatter missing version field"
fi

if echo "$FRONTMATTER" | grep -q "^description:"; then
  pass "frontmatter has description field"
else
  fail "frontmatter missing description field"
fi

if echo "$FRONTMATTER" | grep -qE "^auto_activates:"; then
  pass "frontmatter has auto_activates field"
else
  fail "frontmatter missing auto_activates field"
fi

if echo "$FRONTMATTER" | grep -qE "^explicit_triggers:"; then
  pass "frontmatter has explicit_triggers field"
else
  fail "frontmatter missing explicit_triggers field (required by convention)"
fi

# ─── Test 3: Positioning vs the formal-methods triad ────────────────────────

echo ""
echo "Test 3: Positions PBT vs example tests, Gherkin, TLA+"

for term in "example" "Gherkin" "TLA+"; do
  if grep -qi -- "$term" "$SKILL_FILE"; then
    pass "mentions '$term' for positioning"
  else
    fail "should position PBT relative to '$term'"
  fi
done

# ─── Test 4: Per-stack library selection ────────────────────────────────────

echo ""
echo "Test 4: Per-stack library selection table"

for lib in "proptest" "quickcheck" "Hypothesis" "FsCheck" "CsCheck" "fast-check" "jqwik"; do
  if grep -qi -- "$lib" "$SKILL_FILE"; then
    pass "recommends library '$lib'"
  else
    fail "missing library recommendation '$lib'"
  fi
done

# ─── Test 5: Property families are enumerated ───────────────────────────────

echo ""
echo "Test 5: Property families enumerated"

for family in "invariant" "round-trip" "idempoten" "commutativ" "oracle" "differential" "metamorphic"; do
  if grep -qi -- "$family" "$SKILL_FILE"; then
    pass "covers property family matching '$family'"
  else
    fail "missing property family '$family'"
  fi
done

# ─── Test 6: Shrinking, seeding, runner integration ─────────────────────────

echo ""
echo "Test 6: Shrinking, seeding, existing-runner integration"

for concept in "shrink" "seed" "runner"; do
  if grep -qi -- "$concept" "$SKILL_FILE"; then
    pass "discusses '$concept'"
  else
    fail "should discuss '$concept'"
  fi
done

# ─── Test 7: Per-language worked examples present ───────────────────────────

echo ""
echo "Test 7: Worked examples for all five stacks"

for lang in "Rust" "Python" ".NET" "JS/TS" "Java"; do
  if grep -qi -- "$lang" "$SKILL_FILE"; then
    pass "has example section for '$lang'"
  else
    fail "missing example section for '$lang'"
  fi
done

# Concrete property families from the issue must appear as worked examples.
for issue_prop in "redact(redact" "no-secret-leak" "shard_jobs_max" "coverage tally" "telemetry"; do
  if grep -qi -- "$issue_prop" "$SKILL_FILE"; then
    pass "worked example references issue property '$issue_prop'"
  else
    fail "missing issue property example '$issue_prop'"
  fi
done

# Fenced code blocks must exist for examples (need >= 5, one per stack).
FENCE_COUNT=$(grep -c '^```' "$SKILL_FILE" || echo 0)
if [[ "$FENCE_COUNT" -ge 10 ]]; then
  pass "has >=5 fenced code blocks ($((FENCE_COUNT/2)) blocks)"
else
  fail "expected >=5 fenced code blocks, found $((FENCE_COUNT/2))"
fi

# ─── Test 8: Quiz present ────────────────────────────────────────────────────

echo ""
echo "Test 8: Short quiz/tests section"

if grep -qiE "^#{2,3} .*(quiz|check your understanding|self-test)" "$SKILL_FILE"; then
  pass "has a quiz section"
else
  fail "missing quiz/self-test section"
fi

QUIZ_QUESTIONS=$(grep -cE "^[0-9]+\." "$SKILL_FILE" || echo 0)
if [[ "$QUIZ_QUESTIONS" -ge 3 ]]; then
  pass "quiz has >=3 numbered questions ($QUIZ_QUESTIONS numbered items)"
else
  fail "quiz should have >=3 numbered questions (found $QUIZ_QUESTIONS)"
fi

# ─── Test 9: Reciprocal cross-references ────────────────────────────────────

echo ""
echo "Test 9: Cross-references wired both directions"

# This skill references the four sibling skills.
for sib in "tla-plus-expert" "gherkin-expert" "smart-test" "test-gap-analyzer"; do
  if grep -q -- "$sib" "$SKILL_FILE"; then
    pass "SKILL.md references sibling '$sib'"
  else
    fail "SKILL.md should reference sibling '$sib'"
  fi
done

# The four sibling skills reference this skill back.
for sib in "tla-plus-expert" "gherkin-expert" "smart-test" "test-gap-analyzer"; do
  sib_file="$SKILLS_ROOT/$sib/SKILL.md"
  if [[ -f "$sib_file" ]] && grep -q "property-based-testing" "$sib_file"; then
    pass "'$sib' reciprocally references property-based-testing"
  else
    fail "'$sib' must reference property-based-testing"
  fi
done

# ─── Test 10: No leaked secrets ─────────────────────────────────────────────

echo ""
echo "Test 10: No leaked secrets in skill content"

# Guard against real credential-looking tokens. Synthetic placeholders used in
# the redaction example (e.g. a literal 'SECRET-1234' fixture) are fine; real
# AWS-style keys or PEM blocks are not.
if grep -qE "AKIA[0-9A-Z]{16}" "$SKILL_FILE"; then
  fail "contains an AWS-access-key-shaped token"
else
  pass "no AWS-access-key-shaped tokens"
fi

if grep -q "BEGIN RSA PRIVATE KEY" "$SKILL_FILE"; then
  fail "contains a PEM private key block"
else
  pass "no PEM private key blocks"
fi

# ─── Test 11: README consistency with SKILL.md ──────────────────────────────

echo ""
echo "Test 11: README.md stays consistent with SKILL.md"

if [[ -f "$README_FILE" ]]; then
  # README must link to the canonical SKILL.md so readers reach runnable snippets.
  if grep -q "SKILL.md" "$README_FILE"; then
    pass "README links to SKILL.md"
  else
    fail "README should link to SKILL.md"
  fi

  # README must list every stack's library so its table can't drift from SKILL.md.
  for lib in "proptest" "quickcheck" "Hypothesis" "FsCheck" "CsCheck" "fast-check" "jqwik"; do
    if grep -qi -- "$lib" "$README_FILE"; then
      pass "README mentions library '$lib'"
    else
      fail "README missing library '$lib' (drift from SKILL.md)"
    fi
  done

  # README must reciprocally reference all four sibling skills, like SKILL.md.
  for sib in "tla-plus-expert" "gherkin-expert" "smart-test" "test-gap-analyzer"; do
    if grep -q -- "$sib" "$README_FILE"; then
      pass "README references sibling '$sib'"
    else
      fail "README should reference sibling '$sib'"
    fi
  done
else
  fail "README.md not found; cannot check consistency"
fi

# ─── Results ─────────────────────────────────────────────────────────────────

echo ""
echo "═══════════════════════════════════════════════════════"
echo "Results: $PASS passed, $FAIL failed"
echo "═══════════════════════════════════════════════════════"

if [[ "$FAIL" -gt 0 ]]; then
  exit 1
fi
exit 0
