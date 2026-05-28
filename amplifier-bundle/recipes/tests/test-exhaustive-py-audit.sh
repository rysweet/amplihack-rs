#!/usr/bin/env bash
# test-exhaustive-py-audit.sh — exhaustive audit for remaining .py invocations
# in the amplifier-bundle/ tree.
#
# Background: The codebase is 100% Rust now. No Python scripts should be
# INVOKED (run as commands) from recipes, skills, agents, or behaviors.
#
# Contracts under test:
#   1. No YAML/shell/markdown file in amplifier-bundle/{recipes,skills,agents,behaviors}/
#      may contain a .py invocation pattern UNLESS it is:
#      a. Inside oxidizer-workflow.yaml (Python-to-Rust migration docs — legitimate)
#      b. Inside dynamic-debugger (debugpy is a real Python tool)
#      c. Inside code-atlas test scripts that use python3 for YAML parsing
#      d. A mention in migration/historical context (not an invocation)
#      e. Inside test fixtures (test-static-guard-validation-scope.sh)
#   2. Specifically: orch_helper.py, session_tree.py, ci_status.py,
#      github_issue.py, build_publish_validation_scope.py MUST NOT appear as
#      invocation targets in any active skill/recipe/agent/behavior file.
#
# Usage: bash amplifier-bundle/recipes/tests/test-exhaustive-py-audit.sh
# Exit codes: 0 = pass, 1 = fail, 2 = test harness error.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"

BUNDLE="${REPO_ROOT}/amplifier-bundle"

if [[ ! -d "${BUNDLE}" ]]; then
    echo "HARNESS-ERROR: ${BUNDLE} not found" >&2
    exit 2
fi

PASS_COUNT=0
FAIL_COUNT=0

pass() {
    PASS_COUNT=$((PASS_COUNT + 1))
    echo "  PASS[$1]: $2"
}

fail() {
    FAIL_COUNT=$((FAIL_COUNT + 1))
    echo "  FAIL[$1]: $2" >&2
}

echo "=== Exhaustive .py invocation audit ==="

# Performance: single grep pass over the bundle tree, cached for all assertions.
# Replaces 8+ redundant directory traversals with one.
PY_CACHE=$(mktemp)
trap 'rm -f "${PY_CACHE}"' EXIT
grep -rnE '\.py\b' \
    "${BUNDLE}/recipes/" \
    "${BUNDLE}/skills/" \
    "${BUNDLE}/agents/" \
    "${BUNDLE}/behaviors/" \
    --include='*.yaml' --include='*.yml' --include='*.sh' --include='*.md' \
    2>/dev/null > "${PY_CACHE}" || true

# ---------------------------------------------------------------------------
# Assertion 1: Known legacy Python scripts MUST NOT be invoked in active
# bundle files (outside of allowed exceptions).
# ---------------------------------------------------------------------------
LEGACY_SCRIPTS=(
    "orch_helper\\.py"
    "session_tree\\.py"
    "ci_status\\.py"
    "github_issue\\.py"
    "build_publish_validation_scope\\.py"
    "check_imports\\.py"
)

# Exclusion patterns (files where Python refs are legitimate)
is_excluded() {
    local filepath="$1"
    local basename
    basename="$(basename "${filepath}")"

    # oxidizer-workflow: Python-to-Rust migration, Python refs are the subject matter
    [[ "${basename}" == "oxidizer-workflow.yaml" ]] && return 0
    # dynamic-debugger: debugpy is a real Python tool
    [[ "${filepath}" == *"dynamic-debugger"* ]] && return 0
    # code-atlas test scripts: python3 for YAML parsing
    [[ "${filepath}" == *"code-atlas"* && "${basename}" == *.sh ]] && return 0
    # test fixtures: these intentionally contain Python refs for testing
    [[ "${basename}" == "test-static-guard-validation-scope.sh" ]] && return 0
    # This very audit test
    [[ "${basename}" == "test-exhaustive-py-audit.sh" ]] && return 0
    # Other bug-fix tests that check for legacy refs
    [[ "${basename}" == "test-bug-666-stale-python-refs.sh" ]] && return 0
    [[ "${basename}" == "test-bug-667-validation-scope-graceful.sh" ]] && return 0
    [[ "${basename}" == "test-pr-always-opens.sh" ]] && return 0

    return 1
}

# Check if the context around a match indicates it's an invocation (not just a
# mention in a "Note:" or "replaced by" context).
is_invocation() {
    local line="$1"
    local lower="${line,,}"
    # Bash regex avoids subshell forks (echo|grep) on every match line
    local deprecation_re='note.*replaced|replaced by|legacy|deprecated|replaces|migration|was replaced|note [(]may'
    local invoke_re='run:|python3?[[:space:]]|bash[[:space:]]|\./|command -v|which[[:space:]]|invoke|execute'
    local list_prefix_re='^[[:space:]]*-[[:space:]]|^[[:space:]]*[|]'
    local legacy_word_re='legacy|replaced|old|was|deprecated'

    if [[ "${lower}" =~ ${deprecation_re} ]]; then
        return 1
    fi
    if [[ "${line}" =~ ${invoke_re} ]]; then
        return 0
    fi
    if [[ "${line}" =~ ${list_prefix_re} ]] && ! [[ "${lower}" =~ ${legacy_word_re} ]]; then
        return 0
    fi
    return 1
}

legacy_violations=0
for pattern in "${LEGACY_SCRIPTS[@]}"; do
    while IFS=: read -r filepath lineno line; do
        [[ -z "${filepath}" ]] && continue
        if is_excluded "${filepath}"; then
            continue
        fi
        if is_invocation "${line}"; then
            fail "1:$(basename "${filepath}"):L${lineno}" \
                 "invocation of legacy script matching ${pattern}: ${line}"
            legacy_violations=$((legacy_violations + 1))
        fi
    done < <(grep -E "${pattern}" "${PY_CACHE}" || true)
done

if [[ ${legacy_violations} -eq 0 ]]; then
    pass 1 "No legacy Python script invocations found in active bundle files"
fi

# ---------------------------------------------------------------------------
# Assertion 2: Broad .py invocation scan — find legacy amplihack Python
# scripts being invoked in recipe/agent YAML steps.
# ---------------------------------------------------------------------------
# Scope: Only YAML recipe steps and shell scripts that form the amplihack
# infrastructure pipeline. Skill documentation (examples.md, patterns.md,
# reference.md, DEPENDENCIES.md, example_usage.md) showing how to use
# external Python tools (docx, pdf, agent SDKs, watchmedo, etc.) is
# legitimate and excluded.

is_infra_file() {
    local filepath="$1"
    local basename
    basename="$(basename "${filepath}")"
    # Recipe YAML and shell scripts are infrastructure
    [[ "${filepath}" == *"/recipes/"* && ( "${basename}" == *.yaml || "${basename}" == *.yml || "${basename}" == *.sh ) ]] && return 0
    # Agent prompt YAML is infrastructure
    [[ "${filepath}" == *"/agents/"* && ( "${basename}" == *.yaml || "${basename}" == *.yml ) ]] && return 0
    # Behavior YAML is infrastructure
    [[ "${filepath}" == *"/behaviors/"* && ( "${basename}" == *.yaml || "${basename}" == *.yml ) ]] && return 0
    # SKILL.md is infrastructure (it tells agents what tools to use)
    [[ "${basename}" == "SKILL.md" ]] && return 0
    # Everything else (examples.md, patterns.md, reference.md, DEPENDENCIES.md,
    # README.md, example_usage.md) is documentation, not infrastructure
    return 1
}

broad_violations=0
invoke_py_re='python3?[[:space:]]+[^[:space:]]+\.py|run:[[:space:]]+[^[:space:]]+\.py|\./[^[:space:]]+\.py|bash[[:space:]]+[^[:space:]]+\.py'
skip_note_re='[Nn]ote|replaced|legacy|deprecated|migration|was[[:space:]]'
while IFS=: read -r filepath lineno line; do
    [[ -z "${filepath}" ]] && continue
    if is_excluded "${filepath}"; then
        continue
    fi
    if ! is_infra_file "${filepath}"; then
        continue
    fi
    if [[ "${line}" =~ ${invoke_py_re} ]]; then
        if [[ "${line}" =~ ${skip_note_re} ]]; then
            continue
        fi
        fail "2:$(basename "${filepath}"):L${lineno}" \
             "broad .py invocation detected: ${line}"
        broad_violations=$((broad_violations + 1))
    fi
done < "${PY_CACHE}"

if [[ ${broad_violations} -eq 0 ]]; then
    pass 2 "No broad .py invocation patterns found in infrastructure files"
fi

# ---------------------------------------------------------------------------
# Assertion 3: The exact grep from the issue requirement should produce
# only acceptable results (historical mentions with deprecation notes,
# excluded files, or Python language discussion).
# ---------------------------------------------------------------------------
echo ""
echo "  --- Informational: All .py references in bundle (for review) ---"
match_count=0
while IFS= read -r line; do
    # Skip excluded files
    filepath="${line%%:*}"
    if is_excluded "${filepath}"; then
        continue
    fi
    echo "    ${line}"
    match_count=$((match_count + 1))
done < "${PY_CACHE}"

echo "  --- ${match_count} non-excluded .py references found ---"
echo ""

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
total_violations=$((legacy_violations + broad_violations))
echo "--- Summary: ${PASS_COUNT} passed, ${FAIL_COUNT} failed (${total_violations} violations) ---"

if [[ ${FAIL_COUNT} -gt 0 ]]; then
    exit 1
fi

echo "PASS: Exhaustive .py audit — no stale Python invocations in amplifier-bundle."
exit 0
