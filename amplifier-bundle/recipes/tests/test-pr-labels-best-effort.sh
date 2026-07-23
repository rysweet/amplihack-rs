#!/usr/bin/env bash
# test-pr-labels-best-effort.sh — unit test for apply_pr_labels_best_effort()
# in tools/workflow_publish_pr.sh.
#
# Contract under test:
#   1. When WORKFLOW_PR_LABELS is set, the host is GitHub, a numeric PR number
#      was resolved, and `gh` is present, EACH comma-separated label is applied
#      via `gh pr edit <number> --add-label <label>` (whitespace trimmed).
#   2. When WORKFLOW_PR_LABELS is empty/unset, NO `gh pr edit` runs.
#   3. When the host is not GitHub, NO `gh pr edit` runs.
#   4. When the PR number is not numeric, NO `gh pr edit` runs.
#   5. A failing `gh pr edit` (e.g. label missing in repo) is best-effort: the
#      function still returns 0 and never aborts the publish.
#
# The helper is sourced in isolation via the WORKFLOW_PUBLISH_PR_LIB_ONLY seam,
# so this test needs neither a git remote nor the wider publish machinery.
#
# Usage: bash amplifier-bundle/recipes/tests/test-pr-labels-best-effort.sh
# Exit codes: 0 = pass, 1 = fail, 2 = test harness error.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
PUBLISH_LIB="${REPO_ROOT}/amplifier-bundle/tools/workflow_publish_pr.sh"

if [[ ! -f "${PUBLISH_LIB}" ]]; then
    echo "HARNESS-ERROR: ${PUBLISH_LIB} not found" >&2
    exit 2
fi
if ! command -v jq >/dev/null 2>&1; then
    echo "HARNESS-ERROR: jq required" >&2
    exit 2
fi

WORK="$(mktemp -d -t pr-labels-XXXXXX)"
trap 'rm -rf "${WORK}"' EXIT

mkdir -p "${WORK}/bin"
GH_LOG="${WORK}/gh-invocations.log"
export GH_INVOCATIONS_LOG="${GH_LOG}"

# Fake `gh` shim — records every invocation. Fails `pr edit --add-label` when
# the label is the sentinel "does-not-exist" (to exercise best-effort), else
# succeeds. Never contacts the network.
cat > "${WORK}/bin/gh" <<'SHIM'
#!/usr/bin/env bash
log="${GH_INVOCATIONS_LOG:?GH_INVOCATIONS_LOG must be set}"
printf '%s\n' "$*" >> "$log"
if [ "${1:-}" = "pr" ] && [ "${2:-}" = "edit" ]; then
    for a in "$@"; do
        if [ "$a" = "does-not-exist" ]; then
            echo "could not add label: 'does-not-exist' not found" >&2
            exit 1
        fi
    done
fi
exit 0
SHIM
chmod +x "${WORK}/bin/gh"

fail() { echo "FAIL[$1]: $2" >&2; echo "--- gh-invocations.log ---" >&2; cat "${GH_LOG}" 2>/dev/null >&2 || true; exit 1; }

# Reset the invocation log and run apply_pr_labels_best_effort in a subshell
# with the given environment. Emits the function's return code on stdout.
# Args: HOST_TYPE PR_NUMBER_RESULT WORKFLOW_PR_LABELS with_gh(1|0)
run_case() {
    local host="$1" prnum="$2" labels="$3" with_gh="$4"
    : > "${GH_LOG}"
    local path_prefix=""
    [ "$with_gh" = "1" ] && path_prefix="${WORK}/bin:"
    (
        set -euo pipefail
        export PATH="${path_prefix}/usr/bin:/bin"
        export GH_INVOCATIONS_LOG="${GH_LOG}"
        export WORKFLOW_PUBLISH_PR_LIB_ONLY=1
        export WORKFLOW_PR_LABELS="${labels}"
        # shellcheck source=/dev/null
        . "${PUBLISH_LIB}"
        # shellcheck disable=SC2034  # consumed by the sourced apply_pr_labels_best_effort
        HOST_TYPE="${host}"
        # shellcheck disable=SC2034  # consumed by the sourced apply_pr_labels_best_effort
        PR_NUMBER_RESULT="${prnum}"
        apply_pr_labels_best_effort
        echo "rc=$?"
    )
}

edit_calls() { grep -cE '^pr edit ' "${GH_LOG}" 2>/dev/null || true; }

# --- Case 1: happy path, two labels (one padded with whitespace) -----------
out="$(run_case github 4321 'simard-autonomous, needs-attention ' 1)"
[[ "${out##*rc=}" == "0" ]] || fail 1 "function did not return 0 on happy path (got '${out}')"
grep -qE '^pr edit 4321 --add-label simard-autonomous$' "${GH_LOG}" \
    || fail 1 "expected 'pr edit 4321 --add-label simard-autonomous' not recorded"
grep -qE '^pr edit 4321 --add-label needs-attention$' "${GH_LOG}" \
    || fail 1 "second label not trimmed/applied ('needs-attention' expected)"
[[ "$(edit_calls)" == "2" ]] || fail 1 "expected exactly 2 pr-edit calls, got $(edit_calls)"

# --- Case 2: no labels configured → no edits -------------------------------
run_case github 4321 '' 1 >/dev/null
[[ "$(edit_calls)" == "0" ]] || fail 2 "pr edit invoked despite empty WORKFLOW_PR_LABELS"

# --- Case 3: non-GitHub host → no edits ------------------------------------
run_case azdo 4321 'simard-autonomous' 1 >/dev/null
[[ "$(edit_calls)" == "0" ]] || fail 3 "pr edit invoked on non-GitHub host"

# --- Case 4: non-numeric PR number → no edits ------------------------------
run_case github 'not-a-number' 'simard-autonomous' 1 >/dev/null
[[ "$(edit_calls)" == "0" ]] || fail 4 "pr edit invoked with non-numeric PR number"

# --- Case 4b: empty PR number → no edits -----------------------------------
run_case github '' 'simard-autonomous' 1 >/dev/null
[[ "$(edit_calls)" == "0" ]] || fail 4b "pr edit invoked with empty PR number"

# --- Case 5: failing label edit is best-effort (function still returns 0) ---
out="$(run_case github 4321 'does-not-exist' 1)"
[[ "${out##*rc=}" == "0" ]] || fail 5 "function did not return 0 when gh pr edit failed"
grep -qE '^pr edit 4321 --add-label does-not-exist$' "${GH_LOG}" \
    || fail 5 "failing label edit was not attempted"

echo "PASS: apply_pr_labels_best_effort applies WORKFLOW_PR_LABELS best-effort on GitHub PRs only."
exit 0
