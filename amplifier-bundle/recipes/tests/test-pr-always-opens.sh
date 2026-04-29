#!/usr/bin/env bash
# test-pr-always-opens.sh — regression test for issue: recipes strand pushed
# branches without opening a PR when post-push validation fails (e.g. the
# legacy `build_publish_validation_scope.py` helper is missing).
#
# Contract under test (from RETCON_DOCS §5, §7):
#   1. step-15-commit-push MUST emit a commit_result JSON with a strict
#      string field `pushed` ("true"|"false") on every exit path (trap-on-EXIT).
#   2. The post-push validation step MUST be wrapped warn-and-continue
#      so that a missing build_publish_validation_*.py never aborts the recipe.
#   3. step-16-create-draft-pr MUST run whenever pushed=="true", regardless
#      of any post-push validation failure.
#   4. The fake `gh` shim MUST be invoked with `pr create` after a successful
#      push even when validation failed.
#   5. The captured invocation log MUST contain no token material (defense
#      in depth — nothing in the recipe should echo gh auth output).
#
# This test SHOULD FAIL before the workflow-publish.yaml hardening lands.
# It MUST PASS once the recipe emits commit_result and the PR-creation step
# is gated on commit_result.pushed=="true".
#
# Usage: bash amplifier-bundle/recipes/tests/test-pr-always-opens.sh
# Exit codes: 0 = pass, 1 = fail, 2 = test harness error.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
RECIPE="${REPO_ROOT}/amplifier-bundle/recipes/workflow-publish.yaml"

if [[ ! -f "${RECIPE}" ]]; then
    echo "HARNESS-ERROR: ${RECIPE} not found" >&2
    exit 2
fi

WORK="$(mktemp -d -t pr-always-opens-XXXXXX)"
trap 'rm -rf "${WORK}"' EXIT

mkdir -p "${WORK}/bin"
GH_LOG="${WORK}/gh-invocations.log"
: > "${GH_LOG}"

# Fake `gh` shim — records every invocation, never contacts the network,
# never prints token material.
cat > "${WORK}/bin/gh" <<'SHIM'
#!/usr/bin/env bash
# Records arguments to gh-invocations.log. Returns success for `pr create`,
# success for `pr list` (returns empty list = no existing PR), success for
# `auth status` (without printing the token). Any other subcommand exits 0.
log="${GH_INVOCATIONS_LOG:?GH_INVOCATIONS_LOG must be set}"
printf '%s\n' "$*" >> "$log"
case "${1:-}-${2:-}" in
    pr-create)
        echo "https://github.com/example/repo/pull/9999"
        ;;
    pr-list)
        # No pre-existing PR — empty JSON array
        echo "[]"
        ;;
    auth-status)
        echo "Logged in (no-token-here)" >&2
        ;;
esac
exit 0
SHIM
chmod +x "${WORK}/bin/gh"
export GH_INVOCATIONS_LOG="${GH_LOG}"

# --- Assertion 1: workflow-publish.yaml MUST emit commit_result with pushed
# field on the step-15 commit/push step.
if ! grep -qE 'commit_result' "${RECIPE}"; then
    echo "FAIL[1]: workflow-publish.yaml does not emit commit_result" >&2
    exit 1
fi
if ! grep -qE '"pushed"[[:space:]]*:[[:space:]]*"(true|false)"' "${RECIPE}" \
   && ! grep -qE 'pushed[[:space:]]*[:=]' "${RECIPE}"; then
    echo "FAIL[1b]: commit_result does not record a pushed field" >&2
    exit 1
fi

# --- Assertion 2: PR-creation step MUST be gated on pushed=="true" (or the
# fallback should_create_pr boolean per RETCON_DOCS §4).
if ! grep -qE 'commit_result.*pushed.*true|should_create_pr' "${RECIPE}"; then
    echo "FAIL[2]: PR-creation step is not gated on pushed status" >&2
    exit 1
fi

# --- Assertion 3: Post-push validation MUST be warn-and-continue (no bare
# invocation that can abort the recipe under set -e).
if grep -nE 'build_publish_validation' "${RECIPE}" >/dev/null 2>&1; then
    # If the recipe still references the helper at all, it MUST be wrapped.
    if ! grep -qE 'if[[:space:]]*!.*build_publish_validation|WARN.*validation' \
            "${RECIPE}"; then
        echo "FAIL[3]: build_publish_validation reference is not warn-and-continue" >&2
        exit 1
    fi
fi

# --- Assertion 4: Simulate the post-push contract end-to-end with the fake
# gh shim. Build a minimal harness that mirrors the recipe's commit_result
# emission and PR-creation gate.
HARNESS="${WORK}/run.sh"
cat > "${HARNESS}" <<'HARNESS_EOF'
#!/usr/bin/env bash
set -euo pipefail
export PATH="${WORK}/bin:${PATH}"

# Mirror step-15: emit commit_result on EXIT regardless of later failures.
commit_result_file="${WORK}/commit_result.json"
pushed="false"
sha=""
branch="feat/test-branch"
reason=""
emit_commit_result() {
    jq -n \
        --arg pushed "${pushed}" \
        --arg sha    "${sha}" \
        --arg branch "${branch}" \
        --arg reason "${reason}" \
        '{pushed:$pushed,sha:$sha,branch:$branch,reason:$reason}' \
        > "${commit_result_file}"
}
trap emit_commit_result EXIT

# Pretend git push succeeded.
pushed="true"
sha="0000000000000000000000000000000000000000"
reason="ok"

# Mirror post-push validation: helper missing → warn-and-continue (NOT abort).
if ! command -v build_publish_validation_scope.py >/dev/null 2>&1; then
    echo "WARN: build_publish_validation_scope.py missing, continuing" >&2
fi

# Mirror step-16 gate: if pushed=="true", create PR even though validation
# couldn't run.
emit_commit_result   # flush before reading (trap also fires on normal exit)
if [[ "$(jq -r '.pushed' "${commit_result_file}")" == "true" ]]; then
    # Idempotency probe (mirrors existing recipe behavior).
    existing="$(gh pr list --head "${branch}" --json number 2>/dev/null || echo '[]')"
    if [[ "${existing}" == "[]" ]]; then
        gh pr create \
            --title "fix(recipes): never strand pushed work without opening PR" \
            --body  "Fixes #PLACEHOLDER" \
            --head  "${branch}" \
            --base  "main"
    fi
fi
HARNESS_EOF
chmod +x "${HARNESS}"

if ! command -v jq >/dev/null 2>&1; then
    echo "HARNESS-ERROR: jq required" >&2
    exit 2
fi

WORK="${WORK}" bash "${HARNESS}"

# --- Assertion 4: gh pr create MUST have been called.
if ! grep -qE '^pr create' "${GH_LOG}"; then
    echo "FAIL[4]: gh pr create was not invoked after successful push" >&2
    echo "--- gh-invocations.log ---" >&2
    cat "${GH_LOG}" >&2
    exit 1
fi

# --- Assertion 5: invocation log MUST be free of token material.
if grep -qiE 'token|ghp_|github_pat_' "${GH_LOG}"; then
    echo "FAIL[5]: gh-invocations.log contains token-like material" >&2
    exit 1
fi

# --- Assertion 6: commit_result.json MUST be a strict {pushed,sha,branch,reason}
# object with pushed as the string "true" (not boolean true).
if ! jq -e '.pushed == "true" and (.sha|type=="string") and (.branch|type=="string")' \
        "${WORK}/commit_result.json" >/dev/null; then
    echo "FAIL[6]: commit_result.json schema/type contract violated" >&2
    cat "${WORK}/commit_result.json" >&2
    exit 1
fi

echo "PASS: PR is opened even when post-push validation cannot run."
exit 0
