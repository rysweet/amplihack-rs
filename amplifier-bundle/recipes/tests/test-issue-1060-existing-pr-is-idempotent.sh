#!/usr/bin/env bash
# test-issue-1060-existing-pr-is-idempotent.sh — regression lock for issue
# #1060: step-16-create-draft-pr must be IDEMPOTENT. A workflow run whose branch
# already has a matching OPEN pull request (opened in an EARLIER round) must
# conclude GREEN by reusing that PR — never hard-fail the recipe with a false
# FAILED_PR_CREATE / no_scoped_pr.
#
# #1060 was resolved by the primary-key scope refactor (PR #1017/#1022):
# workflow_pr_scope.sh keys on (headRefName, baseRefName, same-repo,
# non-cross-repository) — GitHub's real uniqueness constraint for an open PR —
# and treats createdAt / title-prefix / headRefOid as tie-breakers, NOT hard
# rejections. So the run adopts an earlier-round PR even when its title was
# renamed by a reviewer, its createdAt predates this run, and its head-sha has
# been left behind by newer local commits. This test locks that behavior so it
# cannot silently regress.
#
# Contract under test (exercising the REAL step-16 + workflow_publish_pr.sh
# against a fake `gh`, mirroring how GitHub actually answers `gh pr list`):
#   1. Branch already has an OPEN PR for (head, base) from an earlier round
#      (renamed title, old createdAt, stale head-sha) => step-16 exits 0 with
#      terminal_status "success", state "existing-open-pr", the existing PR
#      number reused, and `gh pr create` is NEVER invoked (idempotent adopt).
#   2. No PR exists for the branch AND `gh pr create` genuinely fails =>
#      step-16 still exits non-zero with FAILED_PR_CREATE, so a real create
#      failure is never masked as success.
#
# Usage: bash amplifier-bundle/recipes/tests/test-issue-1060-existing-pr-is-idempotent.sh
# Exit codes: 0 = pass, 1 = fail, 2 = test harness error.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
PUBLISH_RECIPE="${REPO_ROOT}/amplifier-bundle/recipes/workflow-publish.yaml"

for tool in git jq; do
    command -v "$tool" >/dev/null 2>&1 || { echo "HARNESS-ERROR: ${tool} is required" >&2; exit 2; }
done
[[ -f "${PUBLISH_RECIPE}" ]] || { echo "HARNESS-ERROR: ${PUBLISH_RECIPE} not found" >&2; exit 2; }

WORK="$(mktemp -d -t issue-1060-XXXXXX)"
trap 'rm -rf "${WORK}"' EXIT

PASS_COUNT=0
FAIL_COUNT=0
pass() { PASS_COUNT=$((PASS_COUNT + 1)); echo "  PASS[$1]: $2"; }
fail() { FAIL_COUNT=$((FAIL_COUNT + 1)); echo "  FAIL[$1]: $2" >&2; }

echo "=== Issue #1060: an existing open PR for (head, base) makes step-16 idempotent ==="

# Extract the step-16 command body verbatim from the recipe so we exercise the
# real publish path (which sources/executes workflow_publish_pr.sh), not a mirror.
extract_step_command() {
    local recipe="$1" step="$2"
    awk -v step="$step" '
        index($0, "id: \"" step "\"") { instep=1 }
        instep && $0 ~ /command: \|/ { incmd=1; next }
        incmd {
            if ($0 ~ /^    [a-zA-Z_]+:/ || $0 ~ /^  - id:/) { exit }
            sub(/^      /, "")
            print
        }
    ' "${recipe}"
}

STEP16="${WORK}/step-16.sh"
extract_step_command "${PUBLISH_RECIPE}" "step-16-create-draft-pr" > "${STEP16}"
[[ -s "${STEP16}" ]] || { echo "HARNESS-ERROR: failed to extract step-16 command" >&2; exit 2; }

# Fake `gh`: `pr list` echoes the caller-controlled candidate set (this is the
# scope helper's `--head ... --state all` lookup — exactly what GitHub would
# return); `pr create` ALWAYS fails so a run that reaches create at all is a
# non-idempotent path we can detect.
install_fake_gh() {
    local bindir="$1"
    mkdir -p "${bindir}"
    cat > "${bindir}/gh" <<'SHIM'
#!/usr/bin/env bash
log="${GH_INVOCATIONS_LOG:?GH_INVOCATIONS_LOG must be set}"
printf '%s\n' "$*" >> "${log}"
case "${1:-}-${2:-}" in
    pr-create)
        echo 'a pull request for branch "feat/issue-1060-x" into branch "main" already exists' >&2
        exit 1
        ;;
    pr-list)
        cat "${GH_PR_LIST_RESULT:?GH_PR_LIST_RESULT must be set}"
        ;;
    pr-view)
        echo "{}"
        ;;
esac
exit 0
SHIM
    chmod +x "${bindir}/gh"
}

# Build a fresh feature-branch worktree with a real diff against origin/main.
make_worktree() {
    local dir="$1" origin="$2"
    git init --bare -b main "${origin}" >/dev/null
    git init -b main "${dir}" >/dev/null
    git -C "${dir}" config user.email "test@example.com"
    git -C "${dir}" config user.name "Test User"
    printf 'base\n' > "${dir}/README.md"
    git -C "${dir}" add README.md
    git -C "${dir}" commit -m "base" >/dev/null
    git -C "${dir}" remote add origin "${origin}"
    git -C "${dir}" push -u origin main >/dev/null 2>&1
    git -C "${dir}" checkout -b feat/issue-1060-x >/dev/null
    printf 'change\n' >> "${dir}/README.md"
    git -C "${dir}" add README.md
    git -C "${dir}" commit -m "change" >/dev/null
    git -C "${dir}" remote set-url origin "https://github.com/example/repo.git"
}

run_step16() {
    local worktree="$1" pr_list_result="$2" outfile="$3" errfile="$4"
    (
        export PATH="${WORK}/bin:${PATH}"
        export GH_INVOCATIONS_LOG="${WORK}/gh.log"
        export GH_PR_LIST_RESULT="${pr_list_result}"
        export WORKTREE_SETUP_WORKTREE_PATH="${worktree}"
        export REMOTE_HOST_TYPE="github"
        export ISSUE_NUMBER="1060"
        export AMPLIHACK_HOME="${REPO_ROOT}"
        # Far-future run start: proves the (head, base) primary key adopts the
        # earlier-round PR even though its createdAt is out of the run window.
        export WORKFLOW_STARTED_AT="2999-01-01T00:00:00Z"
        unset DESIGN_SPEC design_spec RECIPE_VAR_design_spec RECIPE_VAR_DESIGN_SPEC WORKFLOW_PR_LABELS
        bash "${STEP16}"
    ) >"${outfile}" 2>"${errfile}"
}

install_fake_gh "${WORK}/bin"

# ---------------------------------------------------------------------------
# Case 1: existing OPEN PR for (head, base) from an earlier round -> adopt green
# The candidate has a REVIEWER-RENAMED title, an OLD createdAt, and a STALE
# head-sha — every secondary discriminator is "wrong" — yet the primary key
# (head, base, same-repo, non-cross) makes it authoritatively ours.
# ---------------------------------------------------------------------------
: > "${WORK}/gh.log"
make_worktree "${WORK}/repo1" "${WORK}/origin1.git"
printf '%s\n' '[{"url":"https://github.com/example/repo/pull/1058","number":1058,"state":"OPEN","mergedAt":null,"headRefName":"feat/issue-1060-x","baseRefName":"main","headRefOid":"deadbeefdeadbeefdeadbeefdeadbeefdeadbeef","isCrossRepository":false,"headRepositoryOwner":{"login":"example"},"headRepository":{"name":"repo"},"title":"Reviewer renamed this PR","body":"","createdAt":"2020-01-01T00:00:00Z"}]' \
    > "${WORK}/pr-present.json"

if run_step16 "${WORK}/repo1" "${WORK}/pr-present.json" "${WORK}/c1.out" "${WORK}/c1.err"; then
    result="$(cat "${WORK}/c1.out")"
    term="$(printf '%s' "${result}" | jq -r '.terminal_status // ""')"
    state="$(printf '%s' "${result}" | jq -r '.state // ""')"
    number="$(printf '%s' "${result}" | jq -r '.pr_number // ""')"
    create_calls="$(grep -c '^pr create' "${WORK}/gh.log" || true)"
    if [ "${term}" = "success" ] && [ "${state}" = "existing-open-pr" ] && [ "${number}" = "1058" ]; then
        pass "1a" "earlier-round open PR #1058 is reused (terminal_status=success, state=existing-open-pr)"
    else
        fail "1a" "existing PR not adopted: terminal_status='${term}' state='${state}' pr_number='${number}'"
        printf '  result: %s\n' "${result}" >&2
    fi
    if [ "${create_calls}" = "0" ]; then
        pass "1b" "gh pr create was NOT invoked when a matching open PR already exists (idempotent)"
    else
        fail "1b" "gh pr create was invoked ${create_calls} time(s) despite an existing open PR"
        echo "--- gh log ---" >&2; cat "${WORK}/gh.log" >&2
    fi
else
    fail "1a" "step-16 hard-failed instead of reusing the existing open PR (issue #1060 regressed)"
    echo "--- stdout ---" >&2; cat "${WORK}/c1.out" >&2
    echo "--- stderr ---" >&2; cat "${WORK}/c1.err" >&2
fi

# ---------------------------------------------------------------------------
# Case 2: no PR exists AND create genuinely fails -> still FAILED_PR_CREATE.
# Guards that idempotent adoption never masks a real create failure.
# ---------------------------------------------------------------------------
: > "${WORK}/gh.log"
make_worktree "${WORK}/repo2" "${WORK}/origin2.git"
printf '%s\n' '[]' > "${WORK}/pr-absent.json"

if run_step16 "${WORK}/repo2" "${WORK}/pr-absent.json" "${WORK}/c2.out" "${WORK}/c2.err"; then
    fail "2" "step-16 unexpectedly succeeded when no PR exists and create failed (masks real failure)"
    echo "--- stdout ---" >&2; cat "${WORK}/c2.out" >&2
else
    result="$(cat "${WORK}/c2.out")"
    state="$(printf '%s' "${result}" | jq -r '.state // ""')"
    create_calls="$(grep -c '^pr create' "${WORK}/gh.log" || true)"
    if [ "${state}" = "FAILED_PR_CREATE" ] && [ "${create_calls}" -ge 1 ]; then
        pass "2" "genuine create failure with no existing PR stays FAILED_PR_CREATE (create was attempted)"
    else
        fail "2" "expected FAILED_PR_CREATE after an attempted create, got state='${state}' create_calls='${create_calls}'"
        printf '  result: %s\n' "${result}" >&2
    fi
fi

echo "=== ${PASS_COUNT} passed, ${FAIL_COUNT} failed ==="
[ "${FAIL_COUNT}" -eq 0 ] || exit 1
echo "PASS: step-16 reuses an existing open PR idempotently and never masks a real create failure."
exit 0
