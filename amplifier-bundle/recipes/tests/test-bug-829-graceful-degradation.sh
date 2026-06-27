#!/usr/bin/env bash
# test-bug-829-graceful-degradation.sh — regression test for issue #829.
#
# Bug (residual after #818): the runtime-artifact-helper preflight at
# checkpoints that run AFTER durable side effects (implementation commit/push,
# PR creation, push cleanup, final status) does `exit 2` when the helper is
# missing from ALL search paths. This turns already-successful work
# (implementation + verification + branch push + PR) into an apparent failure
# requiring manual reconciliation.
#
# #818 (path-resolution hardening) is NOT re-done here. This fix only softens
# the *missing-helper* case at post-durable-side-effect checkpoints from a hard
# `exit 2` into a WARNING + degraded-continue that names the searched paths and
# the current working directory (cwd=).
#
# Contracts under test:
#   A. Softened sites (post-durable-side-effect) MUST, in the missing-helper
#      branch, emit a WARNING (not ERROR), include `cwd=`, and NOT `exit 2`:
#        - workflow-tdd.yaml      :: checkpoint-after-implementation
#        - workflow-finalize.yaml :: step-20a-artifact-guard
#        - workflow-finalize.yaml :: step-20b-push-cleanup
#        - workflow-finalize.yaml :: step-22b-final-status
#   B. Each softened site MUST still source + run the preflight enrichment in
#      the helper-PRESENT branch (degrade only the missing case).
#   C. Pre-publish GATE sites (run BEFORE durable side effects) MUST retain the
#      hard `exit 2` on missing helper:
#        - workflow-publish.yaml       (2 sites)
#        - workflow-refactor-review.yaml (1 site)
#        - workflow-pr-review.yaml     (1 site)
#      => exactly 4 retained runtime-artifact `exit 2` gates.
#   D. Durable-work guards MUST remain intact at the softened sites:
#        - `amplihack hygiene artifact-guard` call preserved (tdd checkpoint,
#          finalize 20a, finalize 20b).
#        - git-identity helper retains its own `exit 2`.
#        - final-status helper retains its own `exit 1`.
#
# This test SHOULD FAIL before the #829 fix lands (sites still `exit 2`).
# It MUST PASS once the four sites are softened.
#
# Usage: bash amplifier-bundle/recipes/tests/test-bug-829-graceful-degradation.sh
# Exit codes: 0 = pass, 1 = fail, 2 = test harness error.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
RECIPES="${REPO_ROOT}/amplifier-bundle/recipes"

TDD="${RECIPES}/workflow-tdd.yaml"
FINALIZE="${RECIPES}/workflow-finalize.yaml"
PUBLISH="${RECIPES}/workflow-publish.yaml"
REFACTOR_REVIEW="${RECIPES}/workflow-refactor-review.yaml"
PR_REVIEW="${RECIPES}/workflow-pr-review.yaml"

PASS_COUNT=0
FAIL_COUNT=0

pass() { PASS_COUNT=$((PASS_COUNT + 1)); echo "  PASS[$1]: $2"; }
fail() { FAIL_COUNT=$((FAIL_COUNT + 1)); echo "  FAIL[$1]: $2" >&2; }

for f in "${TDD}" "${FINALIZE}" "${PUBLISH}" "${REFACTOR_REVIEW}" "${PR_REVIEW}"; do
    if [[ ! -f "${f}" ]]; then
        echo "HARNESS-ERROR: required recipe not found: ${f}" >&2
        exit 2
    fi
done

# extract_step <file> <step-id>
# Prints the contiguous block from the matching `- id: "<step-id>"` line up to
# (but not including) the next top-level `  - id:` step marker.
extract_step() {
    local file="$1" step_id="$2"
    awk -v target="${step_id}" '
        BEGIN { inblk = 0 }
        /^[[:space:]]*-[[:space:]]+id:[[:space:]]*"/ {
            line = $0
            sub(/^[[:space:]]*-[[:space:]]+id:[[:space:]]*"/, "", line)
            sub(/".*$/, "", line)
            if (line == target) { inblk = 1; print; next }
            else if (inblk) { inblk = 0 }
        }
        inblk { print }
    ' "${file}"
}

echo "=== Bug #829: graceful degradation for missing runtime-artifact helper ==="

# ---------------------------------------------------------------------------
# Contract A + B: softened sites.
# For each (file, step), the helper-not-found line MUST be a WARNING with cwd=,
# MUST NOT carry `exit 2`, AND the present-branch MUST still source + preflight.
# ---------------------------------------------------------------------------
assert_softened() {
    local label="$1" file="$2" step="$3"
    local block
    block="$(extract_step "${file}" "${step}")"

    if [[ -z "${block}" ]]; then
        fail "${label}:block" "could not extract step '${step}' from $(basename "${file}")"
        return
    fi

    # The line that announces the missing helper.
    local helper_line
    helper_line="$(printf '%s\n' "${block}" \
        | grep -E 'workflow runtime artifact helper not found' || true)"

    if [[ -z "${helper_line}" ]]; then
        fail "${label}:present" "step '${step}' no longer references the runtime-artifact helper at all"
        return
    fi

    # A1: WARNING, not ERROR, on the missing-helper line.
    if printf '%s\n' "${helper_line}" | grep -qE 'WARNING' \
       && ! printf '%s\n' "${helper_line}" | grep -qE 'ERROR'; then
        pass "${label}:warn" "missing-helper branch warns (not errors)"
    else
        fail "${label}:warn" "missing-helper branch still ERRORs instead of WARNING: ${helper_line}"
    fi

    # A2: includes cwd= (issue #829 explicit ask: report cwd).
    if printf '%s\n' "${helper_line}" | grep -qE 'cwd='; then
        pass "${label}:cwd" "missing-helper WARNING reports cwd="
    else
        fail "${label}:cwd" "missing-helper WARNING does not report cwd=: ${helper_line}"
    fi

    # A3: names the searched paths (diagnosis aid).
    if printf '%s\n' "${helper_line}" | grep -qE 'searched'; then
        pass "${label}:paths" "missing-helper WARNING names searched paths"
    else
        fail "${label}:paths" "missing-helper WARNING does not name searched paths: ${helper_line}"
    fi

    # A4: the missing-helper branch MUST NOT abort with `exit 2`.
    if printf '%s\n' "${helper_line}" | grep -qE 'exit[[:space:]]+2'; then
        fail "${label}:no-exit2" "missing-helper branch still hard-aborts with exit 2"
    else
        pass "${label}:no-exit2" "missing-helper branch does not exit 2"
    fi

    # B: present branch still sources + runs preflight enrichment.
    if printf '%s\n' "${block}" | grep -qE 'preflight_known_workflow_runtime_artifacts'; then
        pass "${label}:preflight" "present branch still runs preflight enrichment"
    else
        fail "${label}:preflight" "present branch dropped preflight enrichment"
    fi
}

assert_softened "A-tdd"        "${TDD}"      "checkpoint-after-implementation"
assert_softened "A-fin20a"     "${FINALIZE}" "step-20a-artifact-guard"
assert_softened "A-fin20b"     "${FINALIZE}" "step-20b-push-cleanup"
assert_softened "A-fin22b"     "${FINALIZE}" "step-22b-final-status"

# ---------------------------------------------------------------------------
# Contract C: pre-publish GATE sites retain exit 2 on missing helper.
# Count runtime-artifact helper-not-found `exit 2` gates across gate recipes.
# Expected: publish(2) + refactor-review(1) + pr-review(1) = 4.
# ---------------------------------------------------------------------------
count_gate_exit2() {
    # Count lines that both reference the missing helper AND `exit 2`.
    grep -E 'workflow runtime artifact helper not found' "$1" 2>/dev/null \
        | grep -cE 'exit[[:space:]]+2' || true
}

publish_gates=$(count_gate_exit2 "${PUBLISH}")
refactor_gates=$(count_gate_exit2 "${REFACTOR_REVIEW}")
prreview_gates=$(count_gate_exit2 "${PR_REVIEW}")
total_gates=$((publish_gates + refactor_gates + prreview_gates))

if [[ "${publish_gates}" -eq 2 ]]; then
    pass "C-publish" "workflow-publish.yaml retains 2 pre-publish exit-2 gates"
else
    fail "C-publish" "workflow-publish.yaml has ${publish_gates} exit-2 gates (expected 2)"
fi
if [[ "${refactor_gates}" -eq 1 ]]; then
    pass "C-refactor" "workflow-refactor-review.yaml retains 1 pre-publish exit-2 gate"
else
    fail "C-refactor" "workflow-refactor-review.yaml has ${refactor_gates} exit-2 gates (expected 1)"
fi
if [[ "${prreview_gates}" -eq 1 ]]; then
    pass "C-prreview" "workflow-pr-review.yaml retains 1 pre-publish exit-2 gate"
else
    fail "C-prreview" "workflow-pr-review.yaml has ${prreview_gates} exit-2 gates (expected 1)"
fi
if [[ "${total_gates}" -eq 4 ]]; then
    pass "C-total" "exactly 4 pre-publish runtime-artifact exit-2 gates retained"
else
    fail "C-total" "found ${total_gates} pre-publish exit-2 gates (expected 4) — a gate may have been softened by mistake"
fi

# ---------------------------------------------------------------------------
# Contract C2: softened files MUST have ZERO runtime-artifact exit-2 gates left.
# ---------------------------------------------------------------------------
tdd_gates=$(count_gate_exit2 "${TDD}")
fin_gates=$(count_gate_exit2 "${FINALIZE}")
if [[ "${tdd_gates}" -eq 0 ]]; then
    pass "C2-tdd" "workflow-tdd.yaml has no runtime-artifact exit-2 gate"
else
    fail "C2-tdd" "workflow-tdd.yaml still has ${tdd_gates} runtime-artifact exit-2 gate(s)"
fi
if [[ "${fin_gates}" -eq 0 ]]; then
    pass "C2-fin" "workflow-finalize.yaml has no runtime-artifact exit-2 gate"
else
    fail "C2-fin" "workflow-finalize.yaml still has ${fin_gates} runtime-artifact exit-2 gate(s)"
fi

# ---------------------------------------------------------------------------
# Contract D: durable-work guards remain intact at softened sites.
# ---------------------------------------------------------------------------
assert_contains() {
    local label="$1" file="$2" step="$3" pattern="$4" desc="$5"
    local block
    block="$(extract_step "${file}" "${step}")"
    if printf '%s\n' "${block}" | grep -qE "${pattern}"; then
        pass "${label}" "${desc}"
    else
        fail "${label}" "${desc} — MISSING in ${step}"
    fi
}

# D1: artifact-guard call preserved where it was present.
assert_contains "D-guard-tdd"  "${TDD}"      "checkpoint-after-implementation" \
    'amplihack hygiene artifact-guard' "tdd checkpoint keeps artifact-guard call"
assert_contains "D-guard-20a"  "${FINALIZE}" "step-20a-artifact-guard" \
    'amplihack hygiene artifact-guard' "finalize 20a keeps artifact-guard call"
assert_contains "D-guard-20b"  "${FINALIZE}" "step-20b-push-cleanup" \
    'amplihack hygiene artifact-guard' "finalize 20b keeps artifact-guard call"

# D2: git-identity helper retains its own exit 2.
assert_contains "D-gitid-tdd"  "${TDD}"      "checkpoint-after-implementation" \
    'git identity helper not found.*exit[[:space:]]+2' "tdd checkpoint keeps git-identity exit-2"
assert_contains "D-gitid-20b"  "${FINALIZE}" "step-20b-push-cleanup" \
    'git identity helper not found.*exit[[:space:]]+2' "finalize 20b keeps git-identity exit-2"

# D3: final-status helper retains its own exit 1.
assert_contains "D-final-22b"  "${FINALIZE}" "step-22b-final-status" \
    'final-status helper not found.*exit[[:space:]]+1' "finalize 22b keeps final-status exit-1"

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
echo ""
echo "--- Summary: ${PASS_COUNT} passed, ${FAIL_COUNT} failed ---"

if [[ ${FAIL_COUNT} -gt 0 ]]; then
    exit 1
fi

echo "PASS: Bug #829 — missing runtime-artifact helper degrades gracefully at post-side-effect checkpoints."
exit 0
