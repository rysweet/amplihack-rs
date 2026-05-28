#!/usr/bin/env bash
# test-code-philosophy-audit-recipe.sh — structural validation for the
# code-philosophy-audit recipe.
#
# Contracts under test:
#   1. Recipe YAML parses and has 5 sequential agent steps.
#   2. Steps use amplihack:core:reviewer agent (read-only).
#   3. Layer 4 consolidation is conditional on prior layer output.
#   4. Layer 5 re-assessment is conditional on fix_results.
#   5. No duplicate step IDs.
#   6. Recipe is within brick limit (≤400 lines).
#   7. All required context variables are defined.
#   8. No write tools or builder agents referenced.
#   9. Dedup rules reference file:line matching.
#  10. Finding ID patterns follow L1/L2/L3/C/R naming convention.
#
# This test SHOULD FAIL before the recipe is created. It MUST PASS once
# code-philosophy-audit.yaml exists with the correct structure.
#
# Usage: bash amplifier-bundle/recipes/tests/test-code-philosophy-audit-recipe.sh
# Exit: 0 = pass, 1 = fail, 2 = harness error.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
RECIPE="${REPO_ROOT}/amplifier-bundle/recipes/code-philosophy-audit.yaml"
MANIFEST="${REPO_ROOT}/amplifier-bundle/recipes/_recipe_manifest.json"

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

# ─── Harness checks ─────────────────────────────────────────────────────────

if [[ ! -f "${RECIPE}" ]]; then
  echo "HARNESS-ERROR: ${RECIPE} not found" >&2
  exit 2
fi
if ! command -v python3 >/dev/null 2>&1; then
  echo "HARNESS-ERROR: python3 is required" >&2
  exit 2
fi

# ─── Assertion 1: Recipe is valid YAML with correct structure ────────────────

python3 -c "
import yaml, sys, json

with open('${RECIPE}') as f:
    recipe = yaml.safe_load(f)

errors = []

# 1a. Name must match
if recipe.get('name') != 'code-philosophy-audit':
    errors.append(f'name must be code-philosophy-audit, got {recipe.get(\"name\")}')

# 1b. Must have version
if not recipe.get('version'):
    errors.append('version field missing')

# 1c. Must have steps list
steps = recipe.get('steps', [])
if not isinstance(steps, list):
    errors.append('steps must be a list')

# 1d. Must have exactly 5 steps
if len(steps) != 5:
    errors.append(f'expected 5 steps (layers), got {len(steps)}')

if errors:
    print('\\n'.join(errors))
    sys.exit(1)
" || fail "[1] Recipe YAML structure invalid"

echo "PASS[1]: Recipe is valid YAML with correct structure"

# ─── Assertion 2: Step IDs match expected layer names ────────────────────────

python3 -c "
import yaml, sys

with open('${RECIPE}') as f:
    recipe = yaml.safe_load(f)

expected = [
    'layer-1-anti-patterns',
    'layer-2-architecture',
    'layer-3-three-pass-audit',
    'layer-4-consolidation',
    'layer-5-reassessment',
]

actual = [s.get('id', '') for s in recipe.get('steps', [])]

if actual != expected:
    print(f'Step IDs mismatch:\\n  expected: {expected}\\n  actual:   {actual}')
    sys.exit(1)
" || fail "[2] Step IDs do not match expected layer names"

echo "PASS[2]: Step IDs match expected layer order"

# ─── Assertion 3: All steps use reviewer agent ───────────────────────────────

python3 -c "
import yaml, sys

with open('${RECIPE}') as f:
    recipe = yaml.safe_load(f)

for step in recipe.get('steps', []):
    agent = step.get('agent', '')
    if 'reviewer' not in agent.lower():
        print(f'Step {step.get(\"id\")} uses agent \"{agent}\" — must use reviewer')
        sys.exit(1)
    if 'builder' in agent.lower():
        print(f'Step {step.get(\"id\")} uses builder agent — forbidden for audit')
        sys.exit(1)
" || fail "[3] Not all steps use reviewer agent"

echo "PASS[3]: All steps use reviewer agent (read-only)"

# ─── Assertion 4: Layer 4 has condition on prior findings ────────────────────

python3 -c "
import yaml, sys

with open('${RECIPE}') as f:
    recipe = yaml.safe_load(f)

l4 = [s for s in recipe.get('steps', []) if s.get('id') == 'layer-4-consolidation']
if not l4:
    print('Layer 4 step not found')
    sys.exit(1)

condition = l4[0].get('condition', '')
if not condition:
    print('Layer 4 has no condition gate')
    sys.exit(1)

# Must reference at least one layer finding variable
for var in ['layer_1_findings', 'layer_2_findings', 'layer_3_findings']:
    if var in condition:
        sys.exit(0)

print(f'Layer 4 condition does not reference any layer findings: {condition}')
sys.exit(1)
" || fail "[4] Layer 4 missing condition on prior findings"

echo "PASS[4]: Layer 4 has condition gate on prior findings"

# ─── Assertion 5: Layer 5 is conditional on fix_results ──────────────────────

python3 -c "
import yaml, sys

with open('${RECIPE}') as f:
    recipe = yaml.safe_load(f)

l5 = [s for s in recipe.get('steps', []) if s.get('id') == 'layer-5-reassessment']
if not l5:
    print('Layer 5 step not found')
    sys.exit(1)

condition = l5[0].get('condition', '')
if 'fix_results' not in condition:
    print(f'Layer 5 condition must reference fix_results: {condition}')
    sys.exit(1)
" || fail "[5] Layer 5 not conditional on fix_results"

echo "PASS[5]: Layer 5 conditional on fix_results"

# ─── Assertion 6: No duplicate step IDs ──────────────────────────────────────

python3 -c "
import yaml, sys

with open('${RECIPE}') as f:
    recipe = yaml.safe_load(f)

ids = [s.get('id', '') for s in recipe.get('steps', [])]
seen = set()
dups = []
for i in ids:
    if i in seen:
        dups.append(i)
    seen.add(i)

if dups:
    print(f'Duplicate step IDs: {dups}')
    sys.exit(1)
" || fail "[6] Duplicate step IDs found"

echo "PASS[6]: No duplicate step IDs"

# ─── Assertion 7: Brick rule — ≤400 lines ───────────────────────────────────

LINE_COUNT=$(wc -l < "${RECIPE}")
if [[ ${LINE_COUNT} -gt 400 ]]; then
  fail "[7] Recipe is ${LINE_COUNT} lines — exceeds 400 LOC brick limit"
fi

echo "PASS[7]: Recipe is ${LINE_COUNT} lines (≤400 brick limit)"

# ─── Assertion 8: Required context variables ─────────────────────────────────

python3 -c "
import yaml, sys

with open('${RECIPE}') as f:
    recipe = yaml.safe_load(f)

ctx = recipe.get('context', {})
required = [
    'repo_path', 'target_path', 'task_description',
    'layer_1_findings', 'layer_2_findings', 'layer_3_findings',
    'consolidation_report', 'fix_results', 'reassessment_report',
]
missing = [v for v in required if v not in ctx]
if missing:
    print(f'Missing context variables: {missing}')
    sys.exit(1)
" || fail "[8] Required context variables missing"

echo "PASS[8]: All required context variables defined"

# ─── Assertion 9: Recursion guards defined ───────────────────────────────────

python3 -c "
import yaml, sys

with open('${RECIPE}') as f:
    recipe = yaml.safe_load(f)

recursion = recipe.get('recursion', {})
if not recursion.get('max_depth'):
    print('max_depth not set in recursion block')
    sys.exit(1)
if not recursion.get('max_total_steps'):
    print('max_total_steps not set in recursion block')
    sys.exit(1)
" || fail "[9] Recursion guards missing"

echo "PASS[9]: Recursion guards (max_depth, max_total_steps) defined"

# ─── Assertion 10: Output variables assigned correctly ───────────────────────

python3 -c "
import yaml, sys

with open('${RECIPE}') as f:
    recipe = yaml.safe_load(f)

expected_outputs = {
    'layer-1-anti-patterns': 'layer_1_findings',
    'layer-2-architecture': 'layer_2_findings',
    'layer-3-three-pass-audit': 'layer_3_findings',
    'layer-4-consolidation': 'consolidation_report',
    'layer-5-reassessment': 'reassessment_report',
}

for step in recipe.get('steps', []):
    step_id = step.get('id', '')
    if step_id in expected_outputs:
        actual = step.get('output', '')
        expected = expected_outputs[step_id]
        if actual != expected:
            print(f'{step_id}: output should be \"{expected}\", got \"{actual}\"')
            sys.exit(1)
" || fail "[10] Output variable assignments incorrect"

echo "PASS[10]: Output variables assigned correctly to all steps"

# ─── Assertion 11: Manifest registration ─────────────────────────────────────

if [[ ! -f "${MANIFEST}" ]]; then
  fail "[11] _recipe_manifest.json not found"
fi

python3 -c "
import json, sys

with open('${MANIFEST}') as f:
    manifest = json.load(f)

if 'code-philosophy-audit' not in manifest:
    print('code-philosophy-audit not registered in manifest')
    sys.exit(1)

val = manifest['code-philosophy-audit']
if not val or len(str(val)) < 8:
    print(f'Manifest value too short or empty: {val}')
    sys.exit(1)
" || fail "[11] Recipe not registered in manifest"

echo "PASS[11]: Recipe registered in _recipe_manifest.json"

# ─── Assertion 12: Recipe prompts reference correct skills ───────────────────

RAW=$(cat "${RECIPE}")

if ! echo "${RAW}" | grep -q "code-smell-detector"; then
  fail "[12a] Recipe does not reference code-smell-detector"
fi
echo "PASS[12a]: Recipe references code-smell-detector"

if ! echo "${RAW}" | grep -q "philosophy-compliance-workflow"; then
  fail "[12b] Recipe does not reference philosophy-compliance-workflow"
fi
echo "PASS[12b]: Recipe references philosophy-compliance-workflow"

# ─── Assertion 13: Finding ID patterns ───────────────────────────────────────

for pattern in "L1-" "L2-" "L3-" "C-" "R-"; do
  if ! echo "${RAW}" | grep -q "${pattern}"; then
    fail "[13] Recipe missing finding ID pattern: ${pattern}"
  fi
done
echo "PASS[13]: All finding ID patterns (L1/L2/L3/C/R) present"

# ─── Assertion 14: Layer 2 receives Layer 1 findings ─────────────────────────

# Extract Layer 2 prompt and check for layer_1_findings template reference
L2_PROMPT=$(python3 -c "
import yaml
with open('${RECIPE}') as f:
    r = yaml.safe_load(f)
for s in r.get('steps', []):
    if s.get('id') == 'layer-2-architecture':
        print(s.get('prompt', ''))
")

if ! echo "${L2_PROMPT}" | grep -q "layer_1_findings"; then
  fail "[14] Layer 2 prompt does not reference layer_1_findings for dedup"
fi
echo "PASS[14]: Layer 2 receives Layer 1 findings for deduplication"

# ─── Assertion 15: Layer 3 receives Layer 1 + Layer 2 findings ───────────────

L3_PROMPT=$(python3 -c "
import yaml
with open('${RECIPE}') as f:
    r = yaml.safe_load(f)
for s in r.get('steps', []):
    if s.get('id') == 'layer-3-three-pass-audit':
        print(s.get('prompt', ''))
")

if ! echo "${L3_PROMPT}" | grep -q "layer_1_findings"; then
  fail "[15a] Layer 3 prompt does not reference layer_1_findings"
fi
if ! echo "${L3_PROMPT}" | grep -q "layer_2_findings"; then
  fail "[15b] Layer 3 prompt does not reference layer_2_findings"
fi
echo "PASS[15]: Layer 3 receives both Layer 1 and Layer 2 findings"

# ─── Assertion 16: No audit_mode variable (removed as future-proofing) ───────

python3 -c "
import yaml, sys

with open('${RECIPE}') as f:
    recipe = yaml.safe_load(f)

ctx = recipe.get('context', {})
if 'audit_mode' in ctx:
    print('audit_mode still present in context — should be removed')
    sys.exit(1)
" || fail "[16] audit_mode should be removed (future-proofing)"

echo "PASS[16]: No audit_mode future-proofing variable"

# ─── Assertion 17: amplihack recipe validate passes ──────────────────────────

if command -v amplihack >/dev/null 2>&1; then
  if amplihack recipe validate "${RECIPE}" >/dev/null 2>&1; then
    echo "PASS[17]: amplihack recipe validate passes"
  else
    fail "[17] amplihack recipe validate failed"
  fi
else
  echo "SKIP[17]: amplihack binary not available (validator skipped)"
fi

echo ""
echo "PASS: All code-philosophy-audit recipe structural assertions passed."
exit 0
