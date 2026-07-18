#!/bin/bash
# .claude/skills/code-atlas/tests/test_graph_backend.sh
#
# TDD tests for the BACKEND-AGNOSTIC graph representation.
#
# Asserts (documentation-driven — runs without kuzu/python/an atlas build):
#   - The portable OpenCypher graph is the mandatory, always-emitted deliverable.
#   - Live-DB ingestion is a pluggable, selectable backend
#     (kuzu | lbug | neo4j | portable-cypher-only), never a hard kuzu dependency.
#   - The selected backend is ALWAYS recorded (index.md + staleness-map.yaml) — fail-visible.
#   - Cross-layer link relationships are first-class in EVERY backend adapter.
#   - The compile-deps analyzer is language-pluggable (no hard Python requirement).
#   - No dangling "Kuzu is required" / "fail loudly" mandatory-kuzu wording remains.
#
# Usage: bash .claude/skills/code-atlas/tests/test_graph_backend.sh
# Exit:  0 = all tests passed, non-zero = failures

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SKILL_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

PASS=0
FAIL=0

SKILL_MD="${SKILL_DIR}/SKILL.md"
REF_MD="${SKILL_DIR}/reference.md"
API_MD="${SKILL_DIR}/API-CONTRACTS.md"
README_MD="${SKILL_DIR}/README.md"
EXAMPLES_MD="${SKILL_DIR}/examples.md"
SEC_MD="${SKILL_DIR}/SECURITY.md"

assert_file_contains() {
    local label="$1"; local pattern="$2"; local file="$3"
    if [[ ! -f "$file" ]]; then
        echo "FAIL: $label — file not found: $file"; FAIL=$((FAIL + 1)); return
    fi
    if grep -Eq "$pattern" "$file" 2>/dev/null; then
        echo "PASS: $label"; PASS=$((PASS + 1))
    else
        echo "FAIL: $label — pattern '$pattern' not found in ${file##*/}"; FAIL=$((FAIL + 1))
    fi
}

assert_not_in_file() {
    local label="$1"; local pattern="$2"; local file="$3"
    if [[ ! -f "$file" ]]; then
        echo "FAIL: $label — file not found: $file"; FAIL=$((FAIL + 1)); return
    fi
    if grep -Eqi "$pattern" "$file" 2>/dev/null; then
        echo "FAIL: $label — forbidden pattern '$pattern' found in ${file##*/}"; FAIL=$((FAIL + 1))
    else
        echo "PASS: $label"; PASS=$((PASS + 1))
    fi
}

echo ""
echo "=== Test Suite: Backend-Agnostic Graph Representation ==="
echo ""

# ---------------------------------------------------------------------------
# SECTION 1: No dangling mandatory-kuzu wording
# ---------------------------------------------------------------------------
echo "--- No 'Kuzu is required' mandatory wording remains ---"

assert_not_in_file \
    "SKILL.md no longer says 'Kuzu is required'" \
    "kuzu is required" \
    "$SKILL_MD"

assert_not_in_file \
    "SKILL.md no longer mandates 'Kuzu ingestion is required, not optional'" \
    "kuzu ingestion is \*?\*?required" \
    "$SKILL_MD"

assert_not_in_file \
    "SKILL.md does not tell the build to fail loudly when kuzu is unavailable" \
    "if kuzu is unavailable, fail" \
    "$SKILL_MD"

assert_not_in_file \
    "reference.md no longer has a kuzu-only 'Kuzu Ingestion Schema' heading" \
    "^#+ *kuzu ingestion schema" \
    "$REF_MD"

# ---------------------------------------------------------------------------
# SECTION 2: Portable graph is the mandatory, always-emitted deliverable
# ---------------------------------------------------------------------------
echo ""
echo "--- Portable cypher graph is always emitted ---"

assert_file_contains \
    "SKILL.md states portable graph is ALWAYS emitted" \
    "portable .*graph is always emitted|ALWAYS emit the portable" \
    "$SKILL_MD"

assert_file_contains \
    "SKILL.md keeps docs/atlas/cypher/ output" \
    "docs/atlas/cypher/|cypher/" \
    "$SKILL_MD"

assert_file_contains \
    "SKILL.md requires atlas-relationships (inter-layer links) artifact" \
    "atlas-relationships" \
    "$SKILL_MD"

assert_file_contains \
    "API-CONTRACTS.md marks cypher/ as ALWAYS emitted" \
    "cypher/.*ALWAYS emitted|ALWAYS emitted.*graph" \
    "$API_MD"

# ---------------------------------------------------------------------------
# SECTION 3: Backend is pluggable and selectable
# ---------------------------------------------------------------------------
echo ""
echo "--- Live backend is pluggable (kuzu | lbug | neo4j | portable-cypher-only) ---"

for backend in kuzu lbug neo4j portable-cypher-only; do
    assert_file_contains \
        "SKILL.md documents backend '$backend'" \
        "$backend" \
        "$SKILL_MD"
done

assert_file_contains \
    "SKILL.md documents auto-detect order kuzu -> lbug -> neo4j -> portable-cypher-only" \
    "kuzu.*lbug.*neo4j.*portable-cypher-only" \
    "$SKILL_MD"

assert_file_contains \
    "SKILL.md states absence of kuzu must not hard-fail" \
    "not hard-fail|never hard-fails|MUST NOT hard-fail" \
    "$SKILL_MD"

assert_file_contains \
    "API-CONTRACTS.md defines Backend type with all four backends" \
    "\"kuzu\".*\"lbug\".*\"neo4j\".*\"portable-cypher-only\"" \
    "$API_MD"

assert_file_contains \
    "API-CONTRACTS.md exposes graph_backend invocation option" \
    "graph_backend" \
    "$API_MD"

# ---------------------------------------------------------------------------
# SECTION 4: Backend selection is ALWAYS recorded (fail-visible, no silent skip)
# ---------------------------------------------------------------------------
echo ""
echo "--- Backend selection always recorded (anti-silent-degradation) ---"

assert_file_contains \
    "SKILL.md records graph_backend in index.md and staleness-map.yaml" \
    "index.md.*staleness-map.yaml|staleness-map.yaml" \
    "$SKILL_MD"

assert_file_contains \
    "SKILL.md states portable-cypher-only is recorded, not a silent skip" \
    "recorded outcome, never a silent skip|never a silent skip" \
    "$SKILL_MD"

assert_file_contains \
    "API-CONTRACTS.md records graph_backend in staleness-map.yaml" \
    "graph_backend:.*portable-cypher-only|graph_backend: <" \
    "$API_MD"

# ---------------------------------------------------------------------------
# SECTION 5: Cross-layer links are first-class in EVERY backend adapter
# ---------------------------------------------------------------------------
echo ""
echo "--- Cross-layer link relationships present in all backend adapters ---"

for rel in DEPENDS_ON CALLS EXPOSES USES_DTO REFERENCES READS_FROM WRITES_TO USES_ENV TRAVERSES; do
    assert_file_contains \
        "reference.md canonical model defines relationship $rel" \
        "$rel" \
        "$REF_MD"
done

assert_file_contains \
    "reference.md has an engine-neutral canonical graph model" \
    "Canonical Graph Model \(Engine-Neutral\)|engine-neutral" \
    "$REF_MD"

for adapter in "kuzu" "lbug" "neo4j"; do
    assert_file_contains \
        "reference.md provides a schema emission adapter for '$adapter'" \
        "Adapter.*$adapter|adapter.*$adapter|$adapter.*adapter" \
        "$REF_MD"
done

assert_file_contains \
    "reference.md kuzu adapter uses CREATE NODE/REL TABLE DDL" \
    "CREATE (NODE|REL) TABLE" \
    "$REF_MD"

assert_file_contains \
    "reference.md lbug adapter is OpenCypher-compatible (labelled nodes)" \
    "lbug|ladybug" \
    "$REF_MD"

assert_file_contains \
    "examples.md shows the same links across kuzu/lbug/neo4j/portable adapters" \
    "portable-cypher-only" \
    "$EXAMPLES_MD"

# ---------------------------------------------------------------------------
# SECTION 6: Compile-deps analyzer is language-pluggable (no hard Python)
# ---------------------------------------------------------------------------
echo ""
echo "--- Analyzer is language-pluggable; Python not required for non-Python repos ---"

for mode in python-ast rust-cargo-metadata static-approximation; do
    assert_file_contains \
        "SKILL.md documents analyzer mode '$mode'" \
        "$mode" \
        "$SKILL_MD"
done

assert_file_contains \
    "SKILL.md states Python is not required for non-Python repos" \
    "Python is never required|not require Python|never required for non-Python" \
    "$SKILL_MD"

assert_file_contains \
    "SKILL.md marks code-visualizer as a Python-only optional adapter" \
    "Python repos ONLY|Python repos only|optional adapter" \
    "$SKILL_MD"

assert_file_contains \
    "API-CONTRACTS.md documents analyzer_mode label" \
    "analyzer_mode" \
    "$API_MD"

assert_file_contains \
    "reference.md documents Rust cargo metadata analyzer" \
    "rust-cargo-metadata|cargo metadata" \
    "$REF_MD"

# ---------------------------------------------------------------------------
# SECTION 7: Native-Rust / Simard guidance (lbug, no kuzu/python)
# ---------------------------------------------------------------------------
echo ""
echo "--- Native-Rust / Simard guidance ---"

assert_file_contains \
    "SKILL.md gives explicit Simard/native-Rust guidance (backend = lbug)" \
    "Simard.*lbug|native-Rust.*lbug|lbug.*Simard" \
    "$SKILL_MD"

assert_file_contains \
    "SKILL.md notes kuzu and Python are NOT used on native-Rust projects" \
    "kuzu and Python are NOT used|NOT used there|hard NO-kuzu" \
    "$SKILL_MD"

# ---------------------------------------------------------------------------
# SECTION 8: Preserved non-negotiables (regression guard)
# ---------------------------------------------------------------------------
echo ""
echo "--- Preserved non-negotiables ---"

assert_file_contains \
    "SKILL.md still enforces three-pass bug hunt" \
    "Three-pass bug hunt|three-pass" \
    "$SKILL_MD"

assert_file_contains \
    "SKILL.md still routes bugs to issues, never the atlas" \
    "Bugs go to issues, never the atlas|never stored in the atlas" \
    "$SKILL_MD"

assert_file_contains \
    "SKILL.md still requires ast-lsp-bindings mode on line 1" \
    "Mode is always visible|mode on line 1" \
    "$SKILL_MD"

assert_file_contains \
    "SKILL.md still forbids silent diagram-to-table substitution" \
    "No silent diagram-to-table substitution" \
    "$SKILL_MD"

# ---------------------------------------------------------------------------
# Results
# ---------------------------------------------------------------------------
echo ""
echo "=================================="
echo "Results: ${PASS} passed, ${FAIL} failed"
echo "=================================="

[[ $FAIL -eq 0 ]] && exit 0 || exit 1
