#!/usr/bin/env bash
# Tests for code-philosophy skill — Pass 2: QUALITY INVARIANTS
# Validates that SKILL.md and reference.md document all quality invariant
# checks derived from PHILOSOPHY.md §3 (Zero-BS Implementations).

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

echo "═══════════════════════════════════════════════════════"
echo "  Test Suite: code-philosophy — Pass 2 QUALITY INVARIANTS"
echo "═══════════════════════════════════════════════════════"

for f in "$SKILL_FILE" "$REFERENCE_FILE"; do
  if [[ ! -f "$f" ]]; then
    echo "FATAL: $(basename "$f") not found — cannot run tests"
    exit 1
  fi
done

# ─── Test 1: unwrap/panic detection (Rust-specific) ────────────────────────

echo ""
echo "Test 1: unwrap/panic detection in production code"

assert_in_file \
  "SKILL.md documents unwrap check" \
  "unwrap|\.unwrap\(\)" \
  "$SKILL_FILE"

assert_in_file \
  "SKILL.md documents panic check" \
  "panic" \
  "$SKILL_FILE"

assert_in_file \
  "reference.md documents unwrap detection pattern" \
  "unwrap|\.unwrap\(\)" \
  "$REFERENCE_FILE"

# ─── Test 2: unsafe code detection ──────────────────────────────────────────

echo ""
echo "Test 2: unsafe code detection"

assert_in_file \
  "SKILL.md documents unsafe check" \
  "unsafe" \
  "$SKILL_FILE"

assert_in_file \
  "reference.md documents unsafe detection" \
  "unsafe" \
  "$REFERENCE_FILE"

# ─── Test 3: Rust-specific checks gated by file extension ──────────────────

echo ""
echo "Test 3: Language-specific check gating"

# Must gate Rust checks to .rs files (avoid false positives on other langs)
assert_in_file \
  "reference.md gates Rust checks to .rs files" \
  "\.rs|Rust|rust" \
  "$REFERENCE_FILE"

# ─── Test 4: Error handling — no swallowed exceptions ───────────────────────

echo ""
echo "Test 4: Error handling checks"

assert_in_file \
  "SKILL.md documents error handling check" \
  "error handling|error.*handl|swallowed.*exception|exception.*swallow" \
  "$SKILL_FILE"

assert_in_file \
  "reference.md documents error handling patterns" \
  "error handling|error.*handl|Result|except.*pass|catch.*empty" \
  "$REFERENCE_FILE"

# ─── Test 5: Test-to-prod ratio ─────────────────────────────────────────────

echo ""
echo "Test 5: Test-to-prod ratio checking"

# PHILOSOPHY.md defines specific ratios (1:1 to 15:1 by criticality, >20:1 red flag)
assert_in_file \
  "SKILL.md documents test-to-prod ratio" \
  "test.*ratio|test.*prod|ratio.*test|coverage.*ratio" \
  "$SKILL_FILE"

assert_in_file \
  "reference.md documents ratio thresholds" \
  "ratio|1:1|2:1|15:1|20:1|test.*coverage" \
  "$REFERENCE_FILE"

# ─── Test 6: Install-completeness invariant ─────────────────────────────────

echo ""
echo "Test 6: Install-completeness invariant"

# From PHILOSOPHY.md §3: install must fail loudly, update install + verifier together
assert_in_file \
  "SKILL.md documents install-completeness check" \
  "install.*complete|install.*completeness|install.*verif|staging.*verif" \
  "$SKILL_FILE"

# ─── Test 7: No stubs/TODOs/dead code ──────────────────────────────────────

echo ""
echo "Test 7: Zero-BS — no stubs, TODOs, dead code"

assert_in_file \
  "SKILL.md documents stub/TODO detection" \
  "stub|TODO|todo!|unimplemented|dead code|placeholder" \
  "$SKILL_FILE"

assert_in_file \
  "reference.md documents stub patterns" \
  "todo!|unimplemented!|stub|TODO|FIXME|placeholder" \
  "$REFERENCE_FILE"

# ─── Test 8: QUALITY INVARIANTS pass contains all required checks ────────────

echo ""
echo "Test 8: QUALITY INVARIANTS pass is comprehensive"

QUALITY_SECTION=$(sed -n '/QUALITY INVARIANT/,/^## \|^### Pass [13]/p' "$SKILL_FILE" 2>/dev/null || true)

if [[ -n "$QUALITY_SECTION" ]]; then
  for check in "unwrap" "panic" "unsafe" "error" "test.*ratio|ratio" "install"; do
    if echo "$QUALITY_SECTION" | grep -qiE "$check"; then
      pass "QUALITY INVARIANTS section includes: $check"
    else
      fail "QUALITY INVARIANTS section missing: $check"
    fi
  done
else
  fail "could not extract QUALITY INVARIANTS section from SKILL.md"
fi

# ─── Summary ─────────────────────────────────────────────────────────────────

echo ""
echo "═══════════════════════════════"
echo "Results: $PASS passed, $FAIL failed"
echo "═══════════════════════════════"

if [[ "$FAIL" -gt 0 ]]; then
  exit 1
fi
