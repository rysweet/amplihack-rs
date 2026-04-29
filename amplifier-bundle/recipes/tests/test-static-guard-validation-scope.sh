#!/usr/bin/env bash
# test-static-guard-validation-scope.sh — verifies that
# scripts/check-recipes-no-python.sh detects any reintroduction of the legacy
# `build_publish_validation_*.py` helper inside the shipped scan roots
# (amplifier-bundle/{recipes,agents,prompts}/).
#
# Contract (from RETCON_DOCS §6):
#   - Pinned regex: build_publish_validation_[A-Za-z0-9_]+\.py
#   - Scoped to amplifier-bundle/{recipes,agents,prompts}/ (NOT .git/, target/,
#     node_modules/, docs/).
#   - Clean tree → exit 0; tree with a fixture violation → exit 1, with the
#     offending path printed.
#
# This test SHOULD FAIL before scripts/check-recipes-no-python.sh is extended
# with the validation-scope guard.
#
# Usage: bash amplifier-bundle/recipes/tests/test-static-guard-validation-scope.sh
# Exit: 0 = pass, 1 = fail, 2 = harness error.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
GUARD="${REPO_ROOT}/scripts/check-recipes-no-python.sh"

if [[ ! -x "${GUARD}" ]]; then
    echo "HARNESS-ERROR: ${GUARD} not found or not executable" >&2
    exit 2
fi

# --- Assertion 1: clean repo (no fixtures) passes the guard.
if ! "${GUARD}" >/dev/null 2>&1; then
    echo "FAIL[1]: guard reports failure on the clean tree" >&2
    "${GUARD}" >&2 || true
    exit 1
fi

# --- Assertion 2: the guard's source MUST mention the validation-scope regex.
if ! grep -qE 'build_publish_validation_\[A-Za-z0-9_\]\+\\\.py|build_publish_validation' \
        "${GUARD}"; then
    echo "FAIL[2]: guard does not implement the build_publish_validation_*.py check" >&2
    exit 1
fi

# --- Assertion 3: planting a fixture in each scan root triggers exit 1.
WORK="$(mktemp -d -t static-guard-XXXXXX)"
trap 'cleanup' EXIT
PLANTED=()
cleanup() {
    for f in "${PLANTED[@]:-}"; do
        [[ -n "$f" && -f "$f" ]] && rm -f "$f"
    done
    rm -rf "${WORK}"
}

# Use a uniquely named fixture so we can confirm the guard names it.
FIXTURE_NAME="z-fixture-$$-validation-scope-test.yaml"
FIXTURE_BODY='# fixture: should be flagged
steps:
  - run: build_publish_validation_scope.py --check'

for root in recipes agents prompts; do
    target_dir="${REPO_ROOT}/amplifier-bundle/${root}"
    [[ -d "${target_dir}" ]] || continue
    fixture_path="${target_dir}/${FIXTURE_NAME}"
    printf '%s\n' "${FIXTURE_BODY}" > "${fixture_path}"
    PLANTED+=("${fixture_path}")

    set +e
    output="$("${GUARD}" 2>&1)"
    rc=$?
    set -e

    if [[ ${rc} -eq 0 ]]; then
        echo "FAIL[3:${root}]: guard did not detect fixture at ${fixture_path}" >&2
        echo "--- guard output ---" >&2
        printf '%s\n' "${output}" >&2
        exit 1
    fi

    if ! printf '%s\n' "${output}" | grep -qF "${FIXTURE_NAME}"; then
        echo "FAIL[3b:${root}]: guard exited non-zero but did not name the fixture" >&2
        printf '%s\n' "${output}" >&2
        exit 1
    fi

    rm -f "${fixture_path}"
    PLANTED=("${PLANTED[@]/$fixture_path}")
done

# --- Assertion 4: docs/ fixture MUST NOT trigger the guard (scope discipline).
docs_fixture="${REPO_ROOT}/docs/${FIXTURE_NAME}"
if [[ -d "${REPO_ROOT}/docs" ]]; then
    printf '%s\n' "${FIXTURE_BODY}" > "${docs_fixture}"
    PLANTED+=("${docs_fixture}")
    set +e
    "${GUARD}" >/dev/null 2>&1
    rc=$?
    set -e
    rm -f "${docs_fixture}"
    PLANTED=("${PLANTED[@]/$docs_fixture}")
    if [[ ${rc} -ne 0 ]]; then
        echo "FAIL[4]: guard incorrectly flagged docs/ (out of scope)" >&2
        exit 1
    fi
fi

echo "PASS: static guard correctly detects build_publish_validation_*.py and respects scope."
exit 0
