#!/usr/bin/env bash
# test-default-workflow-reliability.sh — regression coverage for issue #573.
#
# Contracts under test:
#   1. workflow-worktree chooses a usable remote base without assuming origin/main.
#      It must prefer origin/HEAD, refresh remote HEAD before master/develop
#      fallback, and tolerate repositories whose supported remote base is
#      origin/master or origin/develop.
#   2. workflow-publish does not wrap GitHub CLI publish/PR calls in shell
#      timeout/gtimeout commands.
#   3. workflow-publish keeps explicit gh error handling and treats design_spec /
#      DESIGN_SPEC as optional under set -u while preserving bounded retries for
#      transient gh failures.
#
# This test SHOULD FAIL before the issue #573 reliability fixes land. It MUST
# PASS once workflow-worktree resolves local/remote origin/HEAD before
# origin/master -> origin/develop fallback and workflow-publish removes timeout
# wrappers while making design spec optional.
#
# Usage: bash amplifier-bundle/recipes/tests/test-default-workflow-reliability.sh
# Exit codes: 0 = pass, 1 = fail, 2 = test harness error.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
WORKTREE_RECIPE="${REPO_ROOT}/amplifier-bundle/recipes/workflow-worktree.yaml"
PUBLISH_RECIPE="${REPO_ROOT}/amplifier-bundle/recipes/workflow-publish.yaml"

if [[ ! -f "${WORKTREE_RECIPE}" ]]; then
    echo "HARNESS-ERROR: ${WORKTREE_RECIPE} not found" >&2
    exit 2
fi
if [[ ! -f "${PUBLISH_RECIPE}" ]]; then
    echo "HARNESS-ERROR: ${PUBLISH_RECIPE} not found" >&2
    exit 2
fi
if ! command -v git >/dev/null 2>&1; then
    echo "HARNESS-ERROR: git is required" >&2
    exit 2
fi
if ! command -v python3 >/dev/null 2>&1; then
    echo "HARNESS-ERROR: python3 is required" >&2
    exit 2
fi

WORK="$(mktemp -d -t default-workflow-reliability-XXXXXX)"
trap 'rm -rf "${WORK}"' EXIT
STEP04="${WORK}/step-04-setup-worktree.sh"

fail() {
    echo "FAIL: $*" >&2
    exit 1
}

extract_step_command() {
    local recipe="$1"
    local step_id="$2"
    local output="$3"

    awk -v step_id="${step_id}" '
        $0 ~ "^[[:space:]]*- id: \"" step_id "\"[[:space:]]*$" {
            in_step = 1
            next
        }
        in_step && /^[[:space:]]*- id: / {
            exit
        }
        in_step && /^[[:space:]]*command: \|[[:space:]]*$/ {
            in_command = 1
            next
        }
        in_command {
            if (substr($0, 1, 6) == "      ") {
                print substr($0, 7)
                next
            }
            if ($0 ~ /^[[:space:]]*$/) {
                print ""
                next
            }
            exit
        }
    ' "${recipe}" > "${output}"

    if [[ ! -s "${output}" ]]; then
        echo "HARNESS-ERROR: could not extract ${step_id} command from ${recipe}" >&2
        exit 2
    fi
    chmod +x "${output}"
}

configure_identity() {
    local repo="$1"
    git -C "${repo}" config user.email "test@example.invalid"
    git -C "${repo}" config user.name "Recipe Reliability Test"
}

create_origin_with_branches() {
    local base_branch="$1"
    local origin_dir="$2"
    local seed_dir="$3"
    local add_master="$4"

    git init --bare -b "${base_branch}" "${origin_dir}" >/dev/null
    git init -b "${base_branch}" "${seed_dir}" >/dev/null
    configure_identity "${seed_dir}"
    printf 'base on %s\n' "${base_branch}" > "${seed_dir}/README.md"
    git -C "${seed_dir}" add README.md
    git -C "${seed_dir}" commit -m "seed ${base_branch}" >/dev/null
    git -C "${seed_dir}" remote add origin "${origin_dir}"
    git -C "${seed_dir}" push -u origin "${base_branch}" >/dev/null 2>&1

    if [[ "${add_master}" == "yes" && "${base_branch}" != "master" ]]; then
        git -C "${seed_dir}" checkout -b master >/dev/null
        printf 'master branch\n' > "${seed_dir}/MASTER.md"
        git -C "${seed_dir}" add MASTER.md
        git -C "${seed_dir}" commit -m "seed master" >/dev/null
        git -C "${seed_dir}" push -u origin master >/dev/null 2>&1
        git -C "${seed_dir}" checkout "${base_branch}" >/dev/null
    fi

    git --git-dir="${origin_dir}" symbolic-ref HEAD "refs/heads/${base_branch}"
}

clone_case_repo() {
    local base_branch="$1"
    local clone_dir="$2"
    local remove_origin_head="$3"
    local add_master="${4:-no}"
    local origin_dir="${WORK}/origin-${base_branch}-${remove_origin_head}-${add_master}.git"
    local seed_dir="${WORK}/seed-${base_branch}-${remove_origin_head}-${add_master}"

    create_origin_with_branches "${base_branch}" "${origin_dir}" "${seed_dir}" "${add_master}"
    git clone "${origin_dir}" "${clone_dir}" >/dev/null 2>&1
    git -C "${clone_dir}" remote set-head origin -a >/dev/null 2>&1 || true
    if [[ "${remove_origin_head}" == "yes" ]]; then
        git -C "${clone_dir}" remote set-head origin -d >/dev/null 2>&1 || true
        git -C "${clone_dir}" update-ref -d refs/remotes/origin/HEAD >/dev/null 2>&1 || true
    fi
}

run_worktree_case() {
    local label="$1"
    local expected_ref="$2"
    local remove_origin_head="$3"
    local add_master="${4:-no}"
    local case_dir="${WORK}/repo-${label}"
    local stdout_file="${WORK}/worktree-${label}.out"
    local stderr_file="${WORK}/worktree-${label}.err"
    local expected_sha
    local branch_name
    local worktree_path
    local worktree_sha

    clone_case_repo "${expected_ref#origin/}" "${case_dir}" "${remove_origin_head}" "${add_master}"

    expected_sha="$(git -C "${case_dir}" rev-parse "${expected_ref}")"

    (
        export REPO_PATH="${case_dir}"
        export TASK_DESCRIPTION="reliability base branch ${label}"
        export ISSUE_NUMBER="573"
        export BRANCH_PREFIX="feat"
        unset EXISTING_BRANCH PR_NUMBER
        bash "${STEP04}"
    ) >"${stdout_file}" 2>"${stderr_file}" || {
        echo "--- step-04 stderr (${label}) ---" >&2
        cat "${stderr_file}" >&2
        echo "--- step-04 stdout (${label}) ---" >&2
        cat "${stdout_file}" >&2
        fail "workflow-worktree failed for ${label}; it must not require origin/main"
    }

    branch_name="$(grep -o '"branch_name": "[^"]*"' "${stdout_file}" | sed 's/.*": "\(.*\)"/\1/' | tail -1)"
    [[ -n "${branch_name}" ]] || fail "workflow-worktree did not emit branch_name for ${label}"

    worktree_path="${case_dir}/worktrees/${branch_name}"
    [[ -d "${worktree_path}" ]] || fail "workflow-worktree did not create ${worktree_path}"

    worktree_sha="$(git -C "${worktree_path}" rev-parse HEAD)"
    if [[ "${worktree_sha}" != "${expected_sha}" ]]; then
        fail "workflow-worktree based ${label} on ${worktree_sha}, expected ${expected_ref} (${expected_sha})"
    fi
}

run_worktree_json_escape_case() {
    local label="json-escaped-existing-branch"
    local case_dir="${WORK}/repo-${label}"
    local stdout_file="${WORK}/worktree-${label}.out"
    local stderr_file="${WORK}/worktree-${label}.err"
    local branch_name='feat/issue-573-json-"quote'

    git init -b main "${case_dir}" >/dev/null
    configure_identity "${case_dir}"
    printf 'base\n' > "${case_dir}/README.md"
    git -C "${case_dir}" add README.md
    git -C "${case_dir}" commit -m "base" >/dev/null
    git -C "${case_dir}" branch "${branch_name}"

    (
        export REPO_PATH="${case_dir}"
        export TASK_DESCRIPTION="reliability json escaping"
        export ISSUE_NUMBER="573"
        export BRANCH_PREFIX="feat"
        export EXISTING_BRANCH="${branch_name}"
        unset PR_NUMBER
        bash "${STEP04}"
    ) >"${stdout_file}" 2>"${stderr_file}" || {
        echo "--- step-04 stderr (${label}) ---" >&2
        cat "${stderr_file}" >&2
        echo "--- step-04 stdout (${label}) ---" >&2
        cat "${stdout_file}" >&2
        fail "workflow-worktree failed for JSON escaping case"
    }

    python3 - "${stdout_file}" "${branch_name}" <<'PY'
import json
import sys

with open(sys.argv[1], "r", encoding="utf-8") as fh:
    data = json.load(fh)

required = {"worktree_path", "branch_name", "base_ref", "base_branch", "created"}
missing = required.difference(data)
if missing:
    raise SystemExit(f"missing JSON keys: {sorted(missing)}")
if data["branch_name"] != sys.argv[2]:
    raise SystemExit(f"branch_name mismatch: {data['branch_name']!r}")
if not isinstance(data["created"], bool):
    raise SystemExit("created must be a JSON boolean")
PY
}

# Integration coverage: non-main base branches and fallback behavior.
extract_step_command "${WORKTREE_RECIPE}" "step-04-setup-worktree" "${STEP04}"
run_worktree_case "origin-head-prefers-develop-over-master" "origin/develop" "no" "yes"
run_worktree_case "remote-head-prefers-develop-over-master-without-local-origin-head" "origin/develop" "yes" "yes"
run_worktree_case "fallback-master-without-origin-head" "origin/master" "yes" "no"
run_worktree_case "fallback-develop-without-origin-head" "origin/develop" "yes" "no"
run_worktree_json_escape_case

# Static coverage: workflow-publish must not use shell timeout wrappers for gh PR paths.
if grep -nE '(^|[^[:alnum:]_])(g?timeout)[[:space:]]+[0-9]+[[:space:]]+gh[[:space:]]+pr[[:space:]]+(list|create)' "${PUBLISH_RECIPE}" >&2; then
    fail "workflow-publish still wraps gh pr list/create with timeout/gtimeout"
fi

# Static coverage: timeout removal must not remove explicit gh pr create error handling.
if ! awk '
    /gh pr create/ { seen = 1 }
    seen && /gh pr create failed|return[[:space:]]+"\$status"|exit[[:space:]]+"\$STATUS"|exit[[:space:]]+\$STATUS|if[[:space:]]+![[:space:]]+gh pr create/ {
        handled = 1
    }
    END { exit handled ? 0 : 1 }
' "${PUBLISH_RECIPE}"; then
    fail "workflow-publish must keep explicit gh pr create failure handling"
fi

# Integration coverage: missing DESIGN_SPEC must be optional under set -u.
STEP16="${WORK}/step-16-create-draft-pr.sh"
extract_step_command "${PUBLISH_RECIPE}" "step-16-create-draft-pr" "${STEP16}"

PR_REPO="${WORK}/publish-repo"
PR_ORIGIN="${WORK}/publish-origin.git"
GH_LOG="${WORK}/gh.log"
GH_LIST_ATTEMPTS="${WORK}/gh-list-attempts"
GH_CREATE_ATTEMPTS="${WORK}/gh-create-attempts"
mkdir -p "${WORK}/bin"
: > "${GH_LOG}"
: > "${GH_LIST_ATTEMPTS}"
: > "${GH_CREATE_ATTEMPTS}"

git init --bare -b main "${PR_ORIGIN}" >/dev/null
git init -b main "${PR_REPO}" >/dev/null
configure_identity "${PR_REPO}"
printf 'base\n' > "${PR_REPO}/README.md"
git -C "${PR_REPO}" add README.md
git -C "${PR_REPO}" commit -m "base" >/dev/null
git -C "${PR_REPO}" remote add origin "${PR_ORIGIN}"
git -C "${PR_REPO}" push -u origin main >/dev/null 2>&1
git -C "${PR_REPO}" checkout -b feat/issue-573-pr >/dev/null
printf 'change\n' >> "${PR_REPO}/README.md"
git -C "${PR_REPO}" add README.md
git -C "${PR_REPO}" commit -m "change" >/dev/null

cat > "${WORK}/bin/gh" <<'SHIM'
#!/usr/bin/env bash
set -euo pipefail
log="${GH_INVOCATIONS_LOG:?GH_INVOCATIONS_LOG must be set}"
printf '%s\n' "$*" >> "${log}"
case "${1:-}-${2:-}" in
    pr-list)
        attempt_file="${GH_LIST_ATTEMPT_FILE:?GH_LIST_ATTEMPT_FILE must be set}"
        attempt="$(wc -l < "${attempt_file}")"
        printf 'attempt\n' >> "${attempt_file}"
        if [[ "${attempt}" -lt 1 ]]; then
            echo "HTTP 503 temporary GitHub API failure" >&2
            exit 1
        fi
        printf '\n'
        ;;
    pr-create)
        attempt_file="${GH_CREATE_ATTEMPT_FILE:?GH_CREATE_ATTEMPT_FILE must be set}"
        attempt="$(wc -l < "${attempt_file}")"
        printf 'attempt\n' >> "${attempt_file}"
        if [[ "${attempt}" -lt 2 ]]; then
            echo "HTTP 502 temporary GitHub API failure" >&2
            exit 1
        fi
        printf 'https://github.com/example/repo/pull/573\n'
        ;;
esac
SHIM
chmod +x "${WORK}/bin/gh"

(
    export PATH="${WORK}/bin:${PATH}"
    export GH_INVOCATIONS_LOG="${GH_LOG}"
    export GH_LIST_ATTEMPT_FILE="${GH_LIST_ATTEMPTS}"
    export GH_CREATE_ATTEMPT_FILE="${GH_CREATE_ATTEMPTS}"
    export WORKTREE_SETUP_WORKTREE_PATH="${PR_REPO}"
    export TASK_DESCRIPTION="Fix default workflow reliability"
    export ISSUE_NUMBER="573"
    unset DESIGN_SPEC design_spec RECIPE_VAR_design_spec RECIPE_VAR_DESIGN_SPEC
    bash "${STEP16}"
) >"${WORK}/step16.out" 2>"${WORK}/step16.err" || {
    echo "--- step-16 stderr ---" >&2
    cat "${WORK}/step16.err" >&2
    echo "--- step-16 stdout ---" >&2
    cat "${WORK}/step16.out" >&2
    fail "workflow-publish PR creation must tolerate missing design_spec / DESIGN_SPEC under set -u"
}

if ! grep -q '^pr create' "${GH_LOG}"; then
    echo "--- gh log ---" >&2
    cat "${GH_LOG}" >&2
    fail "workflow-publish did not invoke gh pr create when DESIGN_SPEC was missing"
fi

if [[ "$(grep -c '^pr list' "${GH_LOG}")" -lt 3 ]]; then
    echo "--- gh log ---" >&2
    cat "${GH_LOG}" >&2
    echo "--- step-16 stderr ---" >&2
    cat "${WORK}/step16.err" >&2
    fail "workflow-publish must retry transient gh pr list failures before continuing"
fi

if [[ "$(grep -c '^pr create' "${GH_LOG}")" -ne 3 ]]; then
    echo "--- gh log ---" >&2
    cat "${GH_LOG}" >&2
    echo "--- step-16 stderr ---" >&2
    cat "${WORK}/step16.err" >&2
    fail "workflow-publish must retry transient gh pr create failures before succeeding"
fi

echo "PASS: default workflow reliability contracts are covered."
