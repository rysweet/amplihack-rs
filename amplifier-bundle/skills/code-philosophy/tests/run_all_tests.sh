#!/usr/bin/env bash
# Run all code-philosophy skill tests
# Usage: bash amplifier-bundle/skills/code-philosophy/tests/run_all_tests.sh
#
# Returns 0 only if ALL test suites pass.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
TOTAL_PASS=0
TOTAL_FAIL=0
SUITE_RESULTS=()

run_suite() {
  local name="$1"
  local script="$2"
  echo ""
  echo "╔═══════════════════════════════════════════════════════╗"
  echo "  Running: $name"
  echo "╚═══════════════════════════════════════════════════════╝"
  echo ""

  if bash "$script" 2>&1; then
    SUITE_RESULTS+=("  ✅ $name")
  else
    SUITE_RESULTS+=("  ❌ $name")
    TOTAL_FAIL=$((TOTAL_FAIL + 1))
  fi
}

run_suite "Skill Structure"        "$SCRIPT_DIR/test_skill_structure.sh"
run_suite "Pass 1: BRICK RULES"    "$SCRIPT_DIR/test_pass1_brick_rules.sh"
run_suite "Pass 2: QUALITY INVARIANTS" "$SCRIPT_DIR/test_pass2_quality_invariants.sh"
run_suite "Pass 3: PHILOSOPHY SPIRIT"  "$SCRIPT_DIR/test_pass3_philosophy_spirit.sh"
run_suite "Workflow & Integration"  "$SCRIPT_DIR/test_workflow_integration.sh"
run_suite "reference.md Content"    "$SCRIPT_DIR/test_reference_content.sh"
run_suite "Recipe Orchestration"    "$SCRIPT_DIR/test_recipe_orchestration.sh"

echo ""
echo "╔═══════════════════════════════════════════════════════╗"
echo "  OVERALL RESULTS"
echo "╠═══════════════════════════════════════════════════════╣"
for result in "${SUITE_RESULTS[@]}"; do
  echo "$result"
done
echo "╚═══════════════════════════════════════════════════════╝"

if [[ "$TOTAL_FAIL" -gt 0 ]]; then
  echo ""
  echo "❌ $TOTAL_FAIL suite(s) FAILED"
  exit 1
else
  echo ""
  echo "✅ All suites passed"
  exit 0
fi
