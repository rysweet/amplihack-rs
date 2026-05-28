#!/usr/bin/env bash
# Tests for code-philosophy skill — Recipe orchestration layer
# Validates the code-philosophy-audit recipe structure, 5-layer architecture,
# SKILL.md orchestration docs, and reference.md recipe documentation.
#
# Run: bash amplifier-bundle/skills/code-philosophy/tests/test_recipe_orchestration.sh
# Exit: 0 = pass, 1 = fail

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
RECIPE_FILE="$REPO_ROOT/amplifier-bundle/recipes/code-philosophy-audit.yaml"
MANIFEST_FILE="$REPO_ROOT/amplifier-bundle/recipes/_recipe_manifest.json"

echo "═══════════════════════════════════════════════════════"
echo "  Test Suite: code-philosophy — Recipe Orchestration"
echo "═══════════════════════════════════════════════════════"

# ─── Guard: all required files must exist ────────────────────────────────────

echo ""
echo "Test 0: Required files exist"

for f in "$SKILL_FILE" "$REFERENCE_FILE" "$RECIPE_FILE" "$MANIFEST_FILE"; do
  if [[ -f "$f" ]]; then
    pass "$(basename "$f") exists"
  else
    fail "$(basename "$f") not found at $f"
    echo "  FATAL: cannot run remaining tests without $(basename "$f")"
    echo ""
    echo "═══════════════════════════════"
    echo "Results: $PASS passed, $FAIL failed"
    echo "═══════════════════════════════"
    exit 1
  fi
done

# ─── Test 1: Recipe file is valid YAML ───────────────────────────────────────

echo ""
echo "Test 1: Recipe is valid YAML"

if python3 -c "import yaml; yaml.safe_load(open('$RECIPE_FILE'))" 2>/dev/null; then
  pass "code-philosophy-audit.yaml parses as valid YAML"
else
  fail "code-philosophy-audit.yaml is not valid YAML"
fi

# ─── Test 2: Recipe has correct metadata ─────────────────────────────────────

echo ""
echo "Test 2: Recipe metadata"

assert_in_file "recipe name is code-philosophy-audit" \
  '^name:.*code-philosophy-audit' "$RECIPE_FILE"

assert_in_file "recipe has version" \
  '^version:' "$RECIPE_FILE"

assert_in_file "recipe has description" \
  '^description:' "$RECIPE_FILE"

assert_in_file "recipe has author" \
  '^author:' "$RECIPE_FILE"

assert_in_file "recipe has tags" \
  '^tags:' "$RECIPE_FILE"

# ─── Test 3: Recipe has exactly 5 layer steps ────────────────────────────────

echo ""
echo "Test 3: Recipe has 5 layer steps"

STEP_COUNT=$(grep -c '^\s*- id:' "$RECIPE_FILE" || true)
if [[ "$STEP_COUNT" -eq 5 ]]; then
  pass "recipe has exactly 5 steps ($STEP_COUNT)"
else
  fail "recipe must have exactly 5 steps (layers); found $STEP_COUNT"
fi

# ─── Test 4: All 5 layer step IDs present and ordered ───────────────────────

echo ""
echo "Test 4: Layer step IDs present and ordered"

EXPECTED_STEPS=(
  "layer-1-anti-patterns"
  "layer-2-architecture"
  "layer-3-three-pass-audit"
  "layer-4-consolidation"
  "layer-5-reassessment"
)

ACTUAL_STEPS=()
while IFS= read -r line; do
  ACTUAL_STEPS+=("$line")
done < <(grep '^\s*- id:' "$RECIPE_FILE" | sed 's/.*- id: *"\([^"]*\)".*/\1/')

for i in "${!EXPECTED_STEPS[@]}"; do
  expected="${EXPECTED_STEPS[$i]}"
  if [[ $i -lt ${#ACTUAL_STEPS[@]} ]]; then
    actual="${ACTUAL_STEPS[$i]}"
    if [[ "$actual" == "$expected" ]]; then
      pass "step $((i+1)) ID is '$expected'"
    else
      fail "step $((i+1)) ID: expected '$expected', got '$actual'"
    fi
  else
    fail "step $((i+1)) ID '$expected' missing — only ${#ACTUAL_STEPS[@]} steps found"
  fi
done

# ─── Test 5: Layer 1 invokes code-smell-detector ────────────────────────────

echo ""
echo "Test 5: Layer 1 invokes code-smell-detector"

L1_SECTION=$(sed -n '/id: "layer-1-anti-patterns"/,/id: "layer-2/p' "$RECIPE_FILE" 2>/dev/null || true)
if [[ -z "$L1_SECTION" ]]; then
  fail "could not extract Layer 1 section from recipe"
else
  if echo "$L1_SECTION" | grep -qi "code-smell-detector"; then
    pass "Layer 1 references code-smell-detector skill"
  else
    fail "Layer 1 must reference code-smell-detector skill"
  fi

  # Must detect these anti-pattern categories
  for category in "over-abstraction" "inheritance" "large.function|50.*line" "tight.coupling" "missing.*export|__all__"; do
    if echo "$L1_SECTION" | grep -qiE "$category"; then
      pass "Layer 1 covers category: $category"
    else
      fail "Layer 1 missing category: $category"
    fi
  done

  # Output must be stored as layer_1_findings
  if echo "$L1_SECTION" | grep -q 'output:.*layer_1_findings'; then
    pass "Layer 1 outputs to layer_1_findings"
  else
    fail "Layer 1 must output to layer_1_findings"
  fi
fi

# ─── Test 6: Layer 2 invokes philosophy-compliance-workflow ──────────────────

echo ""
echo "Test 6: Layer 2 invokes philosophy-compliance-workflow"

L2_SECTION=$(sed -n '/id: "layer-2-architecture"/,/id: "layer-3/p' "$RECIPE_FILE" 2>/dev/null || true)
if [[ -z "$L2_SECTION" ]]; then
  fail "could not extract Layer 2 section from recipe"
else
  if echo "$L2_SECTION" | grep -qi "philosophy-compliance-workflow"; then
    pass "Layer 2 references philosophy-compliance-workflow skill"
  else
    fail "Layer 2 must reference philosophy-compliance-workflow skill"
  fi

  # Must receive Layer 1 findings for dedup
  if echo "$L2_SECTION" | grep -qi "layer_1_findings"; then
    pass "Layer 2 receives Layer 1 findings for dedup"
  else
    fail "Layer 2 must receive Layer 1 findings for deduplication"
  fi

  # Must cover architecture alignment categories
  for category in "ruthless.*simpl" "brick" "zen.*minimal|minimalism" "composition"; do
    if echo "$L2_SECTION" | grep -qiE "$category"; then
      pass "Layer 2 covers: $category"
    else
      fail "Layer 2 missing: $category"
    fi
  done

  if echo "$L2_SECTION" | grep -q 'output:.*layer_2_findings'; then
    pass "Layer 2 outputs to layer_2_findings"
  else
    fail "Layer 2 must output to layer_2_findings"
  fi
fi

# ─── Test 7: Layer 3 performs 3-pass audit ───────────────────────────────────

echo ""
echo "Test 7: Layer 3 performs 3-pass audit"

L3_SECTION=$(sed -n '/id: "layer-3-three-pass-audit"/,/id: "layer-4/p' "$RECIPE_FILE" 2>/dev/null || true)
if [[ -z "$L3_SECTION" ]]; then
  fail "could not extract Layer 3 section from recipe"
else
  # Must reference all 3 passes
  for pass_name in "BRICK RULE|brick.rule|Pass 1" "QUALITY INVARIANT|quality.invariant|Pass 2" "PHILOSOPHY SPIRIT|philosophy.spirit|Pass 3"; do
    if echo "$L3_SECTION" | grep -qiE "$pass_name"; then
      pass "Layer 3 includes: $pass_name"
    else
      fail "Layer 3 missing: $pass_name"
    fi
  done

  # Must reference Phase 0 file classification
  if echo "$L3_SECTION" | grep -qi "Phase 0\|file.*classif\|classif.*file"; then
    pass "Layer 3 includes Phase 0 file classification"
  else
    fail "Layer 3 must include Phase 0 file classification"
  fi

  # Must receive both prior layer findings for dedup
  if echo "$L3_SECTION" | grep -qi "layer_1_findings" && echo "$L3_SECTION" | grep -qi "layer_2_findings"; then
    pass "Layer 3 receives Layer 1 + Layer 2 findings for dedup"
  else
    fail "Layer 3 must receive both Layer 1 and Layer 2 findings"
  fi

  if echo "$L3_SECTION" | grep -q 'output:.*layer_3_findings'; then
    pass "Layer 3 outputs to layer_3_findings"
  else
    fail "Layer 3 must output to layer_3_findings"
  fi
fi

# ─── Test 8: Layer 4 consolidation ──────────────────────────────────────────

echo ""
echo "Test 8: Layer 4 consolidation"

L4_SECTION=$(sed -n '/id: "layer-4-consolidation"/,/id: "layer-5/p' "$RECIPE_FILE" 2>/dev/null || true)
if [[ -z "$L4_SECTION" ]]; then
  fail "could not extract Layer 4 section from recipe"
else
  # Must merge findings from all 3 layers
  for var in "layer_1_findings" "layer_2_findings" "layer_3_findings"; do
    if echo "$L4_SECTION" | grep -qi "$var"; then
      pass "Layer 4 receives $var"
    else
      fail "Layer 4 must receive $var for consolidation"
    fi
  done

  # Must deduplicate
  if echo "$L4_SECTION" | grep -qiE "dedup|de-dup|deduplic"; then
    pass "Layer 4 performs deduplication"
  else
    fail "Layer 4 must deduplicate findings"
  fi

  # Must sort by severity
  if echo "$L4_SECTION" | grep -qiE "severity.*sort|sort.*severity"; then
    pass "Layer 4 sorts by severity"
  else
    fail "Layer 4 must sort findings by severity"
  fi

  # Must produce a verdict
  if echo "$L4_SECTION" | grep -qiE "verdict|PASS|FAIL"; then
    pass "Layer 4 produces a verdict"
  else
    fail "Layer 4 must produce a verdict (PASS/FAIL/PASS-WITH-WARNINGS)"
  fi

  if echo "$L4_SECTION" | grep -q 'output:.*consolidation_report'; then
    pass "Layer 4 outputs to consolidation_report"
  else
    fail "Layer 4 must output to consolidation_report"
  fi

  # Must have a condition (should run only if layers produced findings)
  if echo "$L4_SECTION" | grep -qi 'condition:'; then
    pass "Layer 4 has a condition gate"
  else
    fail "Layer 4 should have a condition gate"
  fi
fi

# ─── Test 9: Layer 5 re-assessment is conditional ───────────────────────────

echo ""
echo "Test 9: Layer 5 re-assessment"

L5_SECTION=$(sed -n '/id: "layer-5-reassessment"/,$ p' "$RECIPE_FILE" 2>/dev/null || true)
if [[ -z "$L5_SECTION" ]]; then
  fail "could not extract Layer 5 section from recipe"
else
  # Must be conditional on fix_results
  if echo "$L5_SECTION" | grep -qi 'condition:.*fix_results'; then
    pass "Layer 5 is conditional on fix_results"
  else
    fail "Layer 5 must be conditional on fix_results"
  fi

  # Must scope to changed files only
  if echo "$L5_SECTION" | grep -qiE "changed.*file|only.*changed|files.*changed"; then
    pass "Layer 5 scoped to changed files"
  else
    fail "Layer 5 must scope re-assessment to changed files only"
  fi

  # Must limit recursion (no infinite re-assessment)
  if echo "$L5_SECTION" | grep -qiE "max.*1|single.*re.assessment|final.*layer|no.*recurs|NOT.*trigger.*another"; then
    pass "Layer 5 limits re-assessment (no infinite loop)"
  else
    fail "Layer 5 must explicitly limit re-assessment passes"
  fi

  if echo "$L5_SECTION" | grep -q 'output:.*reassessment_report'; then
    pass "Layer 5 outputs to reassessment_report"
  else
    fail "Layer 5 must output to reassessment_report"
  fi
fi

# ─── Test 10: Recipe brick rule — ≤400 lines ────────────────────────────────

echo ""
echo "Test 10: Recipe brick rule — line count"

RECIPE_LINES=$(wc -l < "$RECIPE_FILE")
if [[ "$RECIPE_LINES" -le 400 ]]; then
  pass "recipe is $RECIPE_LINES lines (≤400 brick limit)"
else
  fail "recipe is $RECIPE_LINES lines — exceeds 400 LOC brick limit"
fi

if [[ "$RECIPE_LINES" -ge 100 ]]; then
  pass "recipe is at least 100 lines ($RECIPE_LINES — substantive)"
else
  fail "recipe is only $RECIPE_LINES lines — likely incomplete"
fi

# ─── Test 11: Recipe uses reviewer agent ─────────────────────────────────────

echo ""
echo "Test 11: Recipe uses read-only reviewer agent"

AGENT_REFS=$(grep -c 'agent:.*reviewer' "$RECIPE_FILE" || true)
if [[ "$AGENT_REFS" -ge 5 ]]; then
  pass "all 5 steps use reviewer agent ($AGENT_REFS references)"
else
  fail "all steps must use reviewer agent for read-only enforcement; found $AGENT_REFS"
fi

# No write agent references
assert_not_in_file "recipe does not use builder agent" \
  'agent:.*builder' "$RECIPE_FILE"

# ─── Test 12: Recipe has recursion guards ────────────────────────────────────

echo ""
echo "Test 12: Recursion guards"

assert_in_file "recipe defines max_depth" \
  'max_depth:' "$RECIPE_FILE"

assert_in_file "recipe defines max_total_steps" \
  'max_total_steps:' "$RECIPE_FILE"

# ─── Test 13: Recipe context variables ───────────────────────────────────────

echo ""
echo "Test 13: Recipe context variables"

for ctx_var in "repo_path" "target_path" "task_description" \
  "layer_1_findings" "layer_2_findings" "layer_3_findings" \
  "consolidation_report" "fix_results" "reassessment_report"; do
  if grep -q "$ctx_var" "$RECIPE_FILE"; then
    pass "context variable defined: $ctx_var"
  else
    fail "context variable missing: $ctx_var"
  fi
done

# ─── Test 14: Manifest registration ─────────────────────────────────────────

echo ""
echo "Test 14: Recipe registered in manifest"

if python3 -c "
import json, sys
manifest = json.load(open('$MANIFEST_FILE'))
if 'code-philosophy-audit' in manifest:
    val = manifest['code-philosophy-audit']
    if val and len(val) > 0:
        print('registered')
        sys.exit(0)
    else:
        print('empty value')
        sys.exit(1)
else:
    print('missing')
    sys.exit(1)
" 2>/dev/null; then
  pass "code-philosophy-audit registered in _recipe_manifest.json"
else
  fail "code-philosophy-audit not registered in _recipe_manifest.json"
fi

# Manifest must be valid JSON
if python3 -c "import json; json.load(open('$MANIFEST_FILE'))" 2>/dev/null; then
  pass "manifest is valid JSON"
else
  fail "manifest is not valid JSON"
fi

# ─── Test 15: SKILL.md references recipe ─────────────────────────────────────

echo ""
echo "Test 15: SKILL.md references the recipe"

assert_in_file "SKILL.md references code-philosophy-audit recipe" \
  "code-philosophy-audit" "$SKILL_FILE"

assert_in_file "SKILL.md has recipe: frontmatter field" \
  "^recipe:.*code-philosophy-audit" "$SKILL_FILE"

assert_in_file "SKILL.md has recipe run command example" \
  "amplihack recipe run code-philosophy-audit" "$SKILL_FILE"

# ─── Test 16: SKILL.md has mermaid diagram ───────────────────────────────────

echo ""
echo "Test 16: SKILL.md has mermaid architecture diagram"

MERMAID_COUNT=$(grep -c '```mermaid' "$SKILL_FILE" || true)
if [[ "$MERMAID_COUNT" -ge 1 ]]; then
  pass "SKILL.md has mermaid diagram ($MERMAID_COUNT blocks)"
else
  fail "SKILL.md must have a mermaid architecture diagram"
fi

# Mermaid diagram must show all 5 layers
MERMAID_SECTION=$(sed -n '/```mermaid/,/```/p' "$SKILL_FILE" 2>/dev/null || true)
if [[ -n "$MERMAID_SECTION" ]]; then
  for layer_label in "Layer 1" "Layer 2" "Layer 3" "Layer 4" "Layer 5"; do
    if echo "$MERMAID_SECTION" | grep -qi "$layer_label"; then
      pass "mermaid diagram shows: $layer_label"
    else
      fail "mermaid diagram missing: $layer_label"
    fi
  done

  # Diagram should show the re-assessment loop / conditional path
  if echo "$MERMAID_SECTION" | grep -qiE "re.assessment|reassess|conditional|Fixes"; then
    pass "mermaid diagram shows re-assessment/fix path"
  else
    fail "mermaid diagram must show re-assessment path"
  fi

  # Diagram should show verdict decision point
  if echo "$MERMAID_SECTION" | grep -qiE "verdict|PASS|FAIL"; then
    pass "mermaid diagram shows verdict decision"
  else
    fail "mermaid diagram must show verdict decision point"
  fi

  # Diagram should show consolidation
  if echo "$MERMAID_SECTION" | grep -qiE "consolid|merge|dedup"; then
    pass "mermaid diagram shows consolidation"
  else
    fail "mermaid diagram must show consolidation step"
  fi

  # Diagram should show dev-orchestrator as external/dashed
  if echo "$MERMAID_SECTION" | grep -qiE "dev-orchestrator|external|dashed|External"; then
    pass "mermaid diagram shows dev-orchestrator delegation as external"
  else
    fail "mermaid diagram must show dev-orchestrator delegation as external step"
  fi
else
  fail "could not extract mermaid diagram from SKILL.md"
fi

# ─── Test 17: SKILL.md documents Layer 3 framing ────────────────────────────

echo ""
echo "Test 17: SKILL.md frames passes as Layer 3"

assert_in_file "SKILL.md mentions Layer 3 framing" \
  "Layer 3|layer 3|layer.3" "$SKILL_FILE"

# The existing passes should be documented within a Layer 3 context
LAYER3_SECTION=$(sed -n '/Layer 3/,/^## [A-Z]/p' "$SKILL_FILE" | head -60)
if [[ -n "$LAYER3_SECTION" ]]; then
  if echo "$LAYER3_SECTION" | grep -qiE "BRICK RULE|Pass 1"; then
    pass "Layer 3 section includes Pass 1 reference"
  else
    fail "Layer 3 section should reference Pass 1 (BRICK RULE)"
  fi
fi

# ─── Test 18: SKILL.md documents all 3 composed skills ──────────────────────

echo ""
echo "Test 18: SKILL.md documents skill composition"

assert_in_file "SKILL.md references code-smell-detector" \
  "code-smell-detector" "$SKILL_FILE"

assert_in_file "SKILL.md references philosophy-compliance-workflow" \
  "philosophy-compliance-workflow" "$SKILL_FILE"

assert_in_file "SKILL.md explains its role as Layer 3 + orchestrator" \
  "orchestrat|trigger|activation|Layer 3.*trigger|trigger.*Layer" "$SKILL_FILE"

# ─── Test 19: reference.md documents recipe architecture ────────────────────

echo ""
echo "Test 19: reference.md documents recipe architecture"

assert_in_file "reference.md has recipe architecture section" \
  "Recipe Architecture|recipe architecture" "$REFERENCE_FILE"

assert_in_file "reference.md documents layer interactions" \
  "Layer.*Interaction|layer.*interact|findings.*forward|passed.*forward" "$REFERENCE_FILE"

assert_in_file "reference.md documents deduplication logic" \
  "dedup|de-dup|deduplic" "$REFERENCE_FILE"

assert_in_file "reference.md documents individual vs full audit" \
  "individual.*layer|Layer.*only|full.*audit|individual.*skill" "$REFERENCE_FILE"

# ─── Test 20: Dedup overlap mapping documented ──────────────────────────────

echo ""
echo "Test 20: Deduplication overlap mapping"

# reference.md must document known category overlaps
for overlap in "over-abstraction" "large-function|function-loc" "inheritance" "ruthless-simplicity|simplicity" "brick-modularity|brick.*modular"; do
  if grep -qiE "$overlap" "$REFERENCE_FILE"; then
    pass "dedup overlap documented: $overlap"
  else
    fail "dedup overlap not documented: $overlap"
  fi
done

# ─── Test 21: Recipe JSON output format ──────────────────────────────────────

echo ""
echo "Test 21: Structured JSON output format in recipe"

# Each layer should specify JSON output format
for layer_num in 1 2 3 4 5; do
  if grep -qiE "layer.*$layer_num|Layer $layer_num" "$RECIPE_FILE" | head -1 && \
     grep -qi "json" "$RECIPE_FILE"; then
    pass "recipe specifies JSON output format"
    break
  fi
done

# Recipe should reference finding ID patterns
for pattern in "L1-" "L2-" "L3-" "C-" "R-"; do
  if grep -q "$pattern" "$RECIPE_FILE"; then
    pass "recipe uses finding ID pattern: ${pattern}xxx"
  else
    fail "recipe missing finding ID pattern: ${pattern}xxx"
  fi
done

# ─── Test 22: No modification of composed skills ────────────────────────────

echo ""
echo "Test 22: Composed skills are not modified"

# Verify that code-smell-detector and philosophy-compliance-workflow
# SKILL.md files do NOT reference code-philosophy-audit
SMELL_SKILL="$REPO_ROOT/amplifier-bundle/skills/code-smell-detector/SKILL.md"
COMPLIANCE_SKILL="$REPO_ROOT/amplifier-bundle/skills/philosophy-compliance-workflow/SKILL.md"

if [[ -f "$SMELL_SKILL" ]]; then
  assert_not_in_file "code-smell-detector not modified to reference audit recipe" \
    "code-philosophy-audit" "$SMELL_SKILL"
fi

if [[ -f "$COMPLIANCE_SKILL" ]]; then
  assert_not_in_file "philosophy-compliance-workflow not modified to reference audit recipe" \
    "code-philosophy-audit" "$COMPLIANCE_SKILL"
fi

# ─── Test 23: Recipe dedup rules in Layer 4 ──────────────────────────────────

echo ""
echo "Test 23: Layer 4 dedup rules"

L4_FULL=$(sed -n '/id: "layer-4-consolidation"/,/id: "layer-5/p' "$RECIPE_FILE" 2>/dev/null || true)
if [[ -n "$L4_FULL" ]]; then
  # Must specify file+line matching tolerance
  if echo "$L4_FULL" | grep -qiE "file.*line|line.*tolerance|\±3|±3"; then
    pass "Layer 4 specifies file:line matching"
  else
    fail "Layer 4 must specify file:line matching for dedup"
  fi

  # Must specify severity resolution (keep highest)
  if echo "$L4_FULL" | grep -qiE "higher.*severity|severity.*high|keep.*high"; then
    pass "Layer 4 uses highest-severity resolution"
  else
    fail "Layer 4 must use highest-severity resolution for conflicts"
  fi

  # Must specify severity escalation rules
  if echo "$L4_FULL" | grep -qiE "escalat"; then
    pass "Layer 4 has severity escalation rules"
  else
    fail "Layer 4 must include severity escalation rules"
  fi
fi

# ─── Test 24: SKILL.md still under brick limit ──────────────────────────────

echo ""
echo "Test 24: SKILL.md still within brick limit"

SKILL_LINES=$(wc -l < "$SKILL_FILE")
if [[ "$SKILL_LINES" -le 500 ]]; then
  pass "SKILL.md is $SKILL_LINES lines (reasonable for orchestrated skill)"
else
  fail "SKILL.md is $SKILL_LINES lines — too large for a skill definition"
fi

# ─── Test 25: reference.md updated with recipe docs ──────────────────────────

echo ""
echo "Test 25: reference.md has recipe-specific content"

REF_LINES=$(wc -l < "$REFERENCE_FILE")
if [[ "$REF_LINES" -ge 300 ]]; then
  pass "reference.md is $REF_LINES lines (includes recipe docs)"
else
  fail "reference.md is $REF_LINES lines — should be ≥300 with recipe documentation"
fi

# Must document context variables
assert_in_file "reference.md documents repo_path variable" \
  "repo_path" "$REFERENCE_FILE"

assert_in_file "reference.md documents target_path variable" \
  "target_path" "$REFERENCE_FILE"

assert_in_file "reference.md documents fix_results variable" \
  "fix_results" "$REFERENCE_FILE"

# ─── Test 26: Read-only enforcement documented ──────────────────────────────

echo ""
echo "Test 26: Read-only enforcement documented"

assert_in_file "SKILL.md documents read-only enforcement" \
  "read.only.*enforce|read.only.*constraint|reviewer.*agent|core:reviewer" "$SKILL_FILE"

assert_in_file "SKILL.md documents two-level enforcement" \
  "agent.*definition|recipe.*design|two.*level|tool.*restrict" "$SKILL_FILE"

# ─── Summary ─────────────────────────────────────────────────────────────────

echo ""
echo "═══════════════════════════════"
echo "Results: $PASS passed, $FAIL failed"
echo "═══════════════════════════════"

if [[ "$FAIL" -gt 0 ]]; then
  exit 1
fi
