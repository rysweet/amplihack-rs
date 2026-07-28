#!/usr/bin/env bash
# test-issue-1103-relay-redaction.sh — TDD (RED) contract for the
# `sanitize_cli_output` secret-redaction sed chains in workflow-prep.yaml.
#
# Issues: #1103 (redact Azure DevOps PATs + generic Bearer/Authorization
# secrets) and the shell defense-in-depth layer of the #1096/#1108 hardening.
#
# Contract under test:
#   1. workflow-prep.yaml carries the redaction sed program in TWO places
#      (the `sanitize_cli_output()` helper near line 267 and the inline
#      copy near line 393). BOTH must exist and must be byte-identical so the
#      two chains can never drift out of sync.
#   2. Each chain must redact:
#        - Azure DevOps PATs (52-char base64-ish high-entropy tokens),
#        - generic `Authorization:`/`Bearer` credentials,
#      in ADDITION to the existing GitHub-token and URL-userinfo coverage.
#   3. Existing coverage (GitHub tokens, github_pat_, URL userinfo) must remain
#      intact (strictly-additive: no regression).
#
# This test SHOULD FAIL before the #1103 hardening lands (the current chains
# only cover GitHub tokens + URL userinfo) and MUST PASS once both chains are
# extended with the AzDO-PAT/base64 catch-all and the Bearer/Authorization rule.
#
# Usage: bash amplifier-bundle/recipes/tests/test-issue-1103-relay-redaction.sh
# Exit codes: 0 = pass, 1 = fail, 2 = test harness error.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
PREP_RECIPE="${REPO_ROOT}/amplifier-bundle/recipes/workflow-prep.yaml"

if [[ ! -f "${PREP_RECIPE}" ]]; then
    echo "HARNESS-ERROR: ${PREP_RECIPE} not found" >&2
    exit 2
fi

fail() {
    echo "FAIL: $*" >&2
    exit 1
}

# Representative secrets.
AZDO_PAT="abcd1234efgh5678ijkl9012mnop3456qrst7890uvwx1234yzAB" # 52 chars
BEARER_TOKEN="eyJhbGciOiJIUzI1NiJ9.payloadpayloadpayload.sigsigsig"
GH_TOKEN="ghp_0123456789abcdefghij0123"
URL_WITH_CREDS="https://user:supersecretpw@example.com/repo.git"

# 1. Extract every `sed -E '...'` redaction program from the recipe.
mapfile -t SED_PROGRAMS < <(grep -oE "sed -E '[^']*'" "${PREP_RECIPE}" | sed -E "s/^sed -E '//; s/'\$//")

if [[ "${#SED_PROGRAMS[@]}" -ne 2 ]]; then
    fail "expected exactly 2 sanitize sed chains in workflow-prep.yaml, found ${#SED_PROGRAMS[@]}"
fi

# 1b. The two chains must be byte-identical (kept in sync — #1103 requirement).
if [[ "${SED_PROGRAMS[0]}" != "${SED_PROGRAMS[1]}" ]]; then
    fail "the two sanitize sed chains have drifted out of sync:
  chain-1: ${SED_PROGRAMS[0]}
  chain-2: ${SED_PROGRAMS[1]}"
fi

# Run every chain against an input and assert the secret is gone.
assert_redacted() {
    local label="$1" secret="$2" input="$3"
    local idx=0
    for prog in "${SED_PROGRAMS[@]}"; do
        idx=$((idx + 1))
        local out
        out="$(printf '%s' "${input}" | sed -E "${prog}")"
        if printf '%s' "${out}" | grep -qF -- "${secret}"; then
            fail "[chain ${idx}] ${label}: secret survived redaction
  input:  ${input}
  output: ${out}"
        fi
    done
    echo "PASS: ${label} redacted by both chains"
}

# 2. New coverage (#1103).
assert_redacted "Azure DevOps PAT (env assignment)" "${AZDO_PAT}" "AZURE_DEVOPS_EXT_PAT=${AZDO_PAT}"
assert_redacted "Bearer token" "${BEARER_TOKEN}" "Authorization: Bearer ${BEARER_TOKEN}"

# 3. Existing coverage must remain intact (no regression).
assert_redacted "GitHub token (regression)" "${GH_TOKEN}" "cloned with ${GH_TOKEN} today"
assert_redacted "URL userinfo password (regression)" "supersecretpw" "git clone ${URL_WITH_CREDS}"

echo "ALL PASS: both sanitize_cli_output sed chains redact AzDO PATs + Bearer tokens and preserve existing coverage."
exit 0
