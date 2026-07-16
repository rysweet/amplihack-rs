#!/usr/bin/env bash
# test-default-workflow-reliability.sh — regression coverage for default-workflow reliability issues.
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
#   4. Default-workflow commit checkpoints resolve the active pre-commit hook
#      with git rev-parse --git-path hooks/pre-commit and scope
#      PRE_COMMIT_ALLOW_NO_CONFIG=1 only to commits where that hook exists and
#      .pre-commit-config.yaml is absent.
#   5. Issue #752: development/code-change workflows fail closed unless terminal
#      evidence proves implementation+verification, publish/PR state, explicit
#      no-op, or an explicit failure. Planning, analysis, design, and worktree
#      prep are not success evidence by themselves.
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
PUBLISH_HELPER="${REPO_ROOT}/amplifier-bundle/tools/workflow_publish_pr.sh"
RUNTIME_ARTIFACT_HELPER="${REPO_ROOT}/amplifier-bundle/tools/workflow_runtime_artifacts.sh"
TDD_RECIPE="${REPO_ROOT}/amplifier-bundle/recipes/workflow-tdd.yaml"
REFACTOR_REVIEW_RECIPE="${REPO_ROOT}/amplifier-bundle/recipes/workflow-refactor-review.yaml"
PR_REVIEW_RECIPE="${REPO_ROOT}/amplifier-bundle/recipes/workflow-pr-review.yaml"
FINALIZE_RECIPE="${REPO_ROOT}/amplifier-bundle/recipes/workflow-finalize.yaml"
TERMINAL_RECIPE="${REPO_ROOT}/amplifier-bundle/recipes/workflow-terminal-state.yaml"
DEFAULT_RECIPE="${REPO_ROOT}/amplifier-bundle/recipes/default-workflow.yaml"
SMART_EXECUTE_RECIPE="${REPO_ROOT}/amplifier-bundle/recipes/smart-execute-routing.yaml"
SMART_ORCHESTRATOR_RECIPE="${REPO_ROOT}/amplifier-bundle/recipes/smart-orchestrator.yaml"
FINAL_STATUS_TOOL="${REPO_ROOT}/amplifier-bundle/tools/workflow_final_status.sh"
PR_SCOPE_HELPER="${REPO_ROOT}/amplifier-bundle/tools/workflow_pr_scope.sh"

if [[ ! -f "${WORKTREE_RECIPE}" ]]; then
    echo "HARNESS-ERROR: ${WORKTREE_RECIPE} not found" >&2
    exit 2
fi
if [[ ! -f "${PUBLISH_RECIPE}" ]]; then
    echo "HARNESS-ERROR: ${PUBLISH_RECIPE} not found" >&2
    exit 2
fi
if [[ ! -f "${PUBLISH_HELPER}" ]]; then
    echo "HARNESS-ERROR: ${PUBLISH_HELPER} not found" >&2
    exit 2
fi
for recipe in "${TDD_RECIPE}" "${REFACTOR_REVIEW_RECIPE}" "${PR_REVIEW_RECIPE}" "${FINALIZE_RECIPE}"; do
    if [[ ! -f "${recipe}" ]]; then
        echo "HARNESS-ERROR: ${recipe} not found" >&2
        exit 2
    fi
done
for path in "${TERMINAL_RECIPE}" "${DEFAULT_RECIPE}" "${SMART_EXECUTE_RECIPE}" "${SMART_ORCHESTRATOR_RECIPE}" "${FINAL_STATUS_TOOL}"; do
    if [[ ! -f "${path}" ]]; then
        echo "HARNESS-ERROR: ${path} not found" >&2
        exit 2
    fi
done
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
STEP_TDD_CHECKPOINT="${WORK}/checkpoint-after-implementation.sh"
STEP_REFACTOR_REVIEW_CHECKPOINT="${WORK}/checkpoint-after-review-feedback.sh"
STEP_PR_REVIEW_FEEDBACK="${WORK}/step-18c-push-feedback-changes.sh"
STEP_FINALIZE_CLEANUP="${WORK}/step-20b-push-cleanup.sh"

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

assert_yaml_recipe_step_present() {
    local label="$1"
    local recipe="$2"
    local step_id="$3"
    local child_recipe="$4"

    awk -v step_id="${step_id}" -v child_recipe="${child_recipe}" '
        $0 ~ "^[[:space:]]*- id: \"" step_id "\"[[:space:]]*$" {
            in_step = 1
            next
        }
        in_step && /^[[:space:]]*- id: / {
            exit
        }
        in_step && $0 ~ "^[[:space:]]*recipe: \"" child_recipe "\"[[:space:]]*$" {
            found = 1
        }
        END { exit found ? 0 : 1 }
    ' "${recipe}" || fail "${label} must invoke recipe '${child_recipe}' via step '${step_id}'"
}

assert_yaml_step_not_fatal_false() {
    local label="$1"
    local recipe="$2"
    local step_id="$3"

    awk -v step_id="${step_id}" '
        $0 ~ "^[[:space:]]*- id: \"" step_id "\"[[:space:]]*$" {
            in_step = 1
            next
        }
        in_step && /^[[:space:]]*- id: / {
            exit
        }
        in_step && /^[[:space:]]*fatal:[[:space:]]*false[[:space:]]*$/ {
            fatal_false = 1
        }
        END { exit fatal_false ? 1 : 0 }
    ' "${recipe}" || fail "${label} must not mark '${step_id}' as fatal: false"
}

assert_runtime_artifact_helper_contracts() {
    [[ -f "${RUNTIME_ARTIFACT_HELPER}" ]] \
        || fail "workflow_runtime_artifacts.sh must exist for narrow runtime artifact cleanup"

    # shellcheck source=/dev/null
    source "${RUNTIME_ARTIFACT_HELPER}"

    command -v cleanup_known_workflow_runtime_artifacts >/dev/null 2>&1 \
        || fail "workflow_runtime_artifacts.sh must define cleanup_known_workflow_runtime_artifacts"
    command -v preflight_known_workflow_runtime_artifacts >/dev/null 2>&1 \
        || fail "workflow_runtime_artifacts.sh must define preflight_known_workflow_runtime_artifacts"

    local case_dir="${WORK}/runtime-artifacts"
    local repo="${case_dir}/repo"
    local nongit="${case_dir}/not-a-git-worktree"

    mkdir -p "${case_dir}"
    git init -b main "${repo}" >/dev/null
    configure_identity "${repo}"
    printf 'base\n' > "${repo}/README.md"
    mkdir -p "${repo}/.claude"
    printf '{"user":"owned"}\n' > "${repo}/.claude/settings.json"
    git -C "${repo}" add README.md .claude/settings.json
    git -C "${repo}" commit -m "base" >/dev/null

    mkdir -p "${repo}/.claude/runtime/logs" "${repo}/worktrees/generated-agent"
    printf 'generated runtime\n' > "${repo}/.claude/runtime/logs/session.log"
    printf 'generated nested worktree\n' > "${repo}/worktrees/generated-agent/trace.log"
    printf 'user-authored untracked file\n' > "${repo}/notes.txt"

    preflight_known_workflow_runtime_artifacts "${repo}" \
        || fail "preflight must remove known workflow runtime artifacts from a git worktree"

    [[ ! -e "${repo}/.claude/runtime" ]] \
        || fail "preflight must remove exactly the generated .claude/runtime directory"
    [[ ! -e "${repo}/worktrees" ]] \
        || fail "preflight must remove workflow-created nested worktrees under the task worktree"
    [[ -f "${repo}/.claude/settings.json" ]] \
        || fail "preflight must preserve user-authored .claude configuration"
    [[ -f "${repo}/notes.txt" ]] \
        || fail "preflight must preserve unrelated untracked user files"

    mkdir -p "${repo}/worktrees/tracked-source"
    printf 'tracked source\n' > "${repo}/worktrees/tracked-source/source.txt"
    git -C "${repo}" add worktrees/tracked-source/source.txt
    git -C "${repo}" commit -m "tracked worktrees source" >/dev/null

    if cleanup_known_workflow_runtime_artifacts "${repo}"; then
        fail "cleanup must fail closed instead of deleting tracked source under worktrees/"
    fi
    [[ -f "${repo}/worktrees/tracked-source/source.txt" ]] \
        || fail "cleanup must not delete tracked worktrees/ source"

    mkdir -p "${nongit}/.claude/runtime"
    if cleanup_known_workflow_runtime_artifacts "${nongit}"; then
        fail "cleanup must reject non-git directories instead of deleting by path shape alone"
    fi
    [[ -d "${nongit}/.claude/runtime" ]] \
        || fail "cleanup must not delete anything when the target is not a git worktree"
}

assert_terminal_recipe_uses_final_status_tool() {
    grep -qF 'workflow_final_status.sh' "${TERMINAL_RECIPE}" \
        || fail "workflow-terminal-state must wrap workflow_final_status.sh as the canonical terminal gate"
}

create_terminal_status_repo() {
    local repo="$1"
    local branch="${2:-feat/issue-752-terminal-state}"

    git init -b main "${repo}" >/dev/null
    configure_identity "${repo}"
    printf 'base\n' > "${repo}/README.md"
    git -C "${repo}" add README.md
    git -C "${repo}" commit -m "base" >/dev/null
    git -C "${repo}" checkout -b "${branch}" >/dev/null
}

assert_terminal_status_case() {
    local label="$1"
    local expected_rc="$2"
    local expected_status="$3"
    local expected_state="$4"
    local expected_text="$5"
    shift 5

    local case_dir="${WORK}/terminal-status-${label}"
    local repo="${case_dir}/repo"
    local stdout_file="${case_dir}/stdout.log"
    local stderr_file="${case_dir}/stderr.log"
    local status=0

    mkdir -p "${case_dir}"
    create_terminal_status_repo "${repo}"

    (
        export REPO_PATH="${repo}"
        export WORKTREE_SETUP_WORKTREE_PATH="${repo}"
        export WORKFLOW_CLASSIFICATION="Development"
        export RECIPE_NAME="default-workflow"
        export TASK_DESCRIPTION="Issue 752 terminal-state ${label}"
        export ISSUE_NUMBER="752"
        export BRANCH_NAME="feat/issue-752-terminal-state"
        export BASE_REF="main"
        unset PR_URL PR_NUMBER PR_PUBLISH_RESULT_PR_URL RECIPE_VAR_pr_publish_result__pr_url
        unset IMPLEMENTATION_COMPLETED VERIFICATION_COMPLETED PUBLISH_STATE_REACHED TERMINAL_NO_OP TERMINAL_FAILURE
        unset TERMINAL_STATE TERMINAL_REASON OBSERVED_PHASES ALLOW_NO_OP
        for assignment in "$@"; do
            export "${assignment}"
        done
        bash "${FINAL_STATUS_TOOL}"
    ) >"${stdout_file}" 2>"${stderr_file}" || status=$?

    if [[ "${status}" != "${expected_rc}" ]]; then
        echo "--- terminal-status ${label} stdout ---" >&2
        cat "${stdout_file}" >&2
        echo "--- terminal-status ${label} stderr ---" >&2
        cat "${stderr_file}" >&2
        fail "workflow_final_status ${label} exited ${status}, expected ${expected_rc}"
    fi

    grep -qF "terminal_success=${expected_status}" "${stdout_file}" \
        || fail "workflow_final_status ${label} must emit terminal_success=${expected_status}"
    grep -qF "terminal_state=${expected_state}" "${stdout_file}" \
        || fail "workflow_final_status ${label} must emit terminal_state=${expected_state}"
    if [[ -n "${expected_text}" ]]; then
        if ! grep -qF "${expected_text}" "${stdout_file}" && ! grep -qF "${expected_text}" "${stderr_file}"; then
            echo "--- terminal-status ${label} stdout ---" >&2
            cat "${stdout_file}" >&2
            echo "--- terminal-status ${label} stderr ---" >&2
            cat "${stderr_file}" >&2
            fail "workflow_final_status ${label} must mention '${expected_text}'"
        fi
    fi
}

assert_commit_guard_static() {
    local label="$1"
    local step_file="$2"
    local allow_count
    local guarded_commit_count

    grep -qF 'git rev-parse --git-path hooks/pre-commit' "${step_file}" \
        || fail "${label} does not resolve hooks/pre-commit with git rev-parse --git-path"
    grep -qF '[ -f "$pre_commit_hook" ]' "${step_file}" \
        || fail "${label} does not test the resolved pre-commit hook path"
    grep -qF '[ ! -f .pre-commit-config.yaml ]' "${step_file}" \
        || fail "${label} does not require .pre-commit-config.yaml to be absent"
    grep -qF 'PRE_COMMIT_ALLOW_NO_CONFIG=1 git commit' "${step_file}" \
        || fail "${label} does not scope PRE_COMMIT_ALLOW_NO_CONFIG=1 inline to git commit"
    if grep -qE '(^|[[:space:]])export[[:space:]]+PRE_COMMIT_ALLOW_NO_CONFIG' "${step_file}"; then
        fail "${label} exports PRE_COMMIT_ALLOW_NO_CONFIG instead of scoping it to one commit"
    fi

    allow_count="$(grep -o 'PRE_COMMIT_ALLOW_NO_CONFIG=1' "${step_file}" | wc -l | tr -d ' ')"
    [[ "${allow_count}" == "1" ]] \
        || fail "${label} should contain exactly one PRE_COMMIT_ALLOW_NO_CONFIG=1 assignment, found ${allow_count}"

    guarded_commit_count="$(grep -c 'commit_with_pre_commit_guard -m' "${step_file}" || true)"
    [[ "${guarded_commit_count}" == "1" ]] \
        || fail "${label} should invoke the guarded commit helper exactly once, found ${guarded_commit_count}"
    if grep -nE '^[[:space:]]*(PRE_COMMIT_ALLOW_NO_CONFIG=1[[:space:]]+)?git[[:space:]]+commit[[:space:]]+-m' "${step_file}" >&2; then
        fail "${label} still has a direct git commit -m path outside the guard helper"
    fi
}

create_commit_guard_repo() {
    local label="$1"
    local repo="$2"
    local origin="$3"

    git init --bare -b main "${origin}" >/dev/null
    git init -b main "${repo}" >/dev/null
    configure_identity "${repo}"
    printf 'base\n' > "${repo}/README.md"
    git -C "${repo}" add README.md
    git -C "${repo}" commit -m "seed ${label}" >/dev/null
    git -C "${repo}" remote add origin "${origin}"
    git -C "${repo}" push -u origin main >/dev/null 2>&1
    git -C "${repo}" checkout -b "feat/issue-573-${label}" >/dev/null
    git -C "${repo}" push -u origin "feat/issue-573-${label}" >/dev/null 2>&1
}

install_pre_commit_probe() {
    local repo="$1"
    local expected="$2"
    local hook_path

    hook_path="$(git -C "${repo}" rev-parse --git-path hooks/pre-commit)"
    mkdir -p "$(dirname "${repo}/${hook_path}")"
    cat > "${repo}/${hook_path}" <<'HOOK'
#!/usr/bin/env bash
set -euo pipefail
actual="${PRE_COMMIT_ALLOW_NO_CONFIG-__UNSET__}"
if [[ "${actual}" != "${EXPECTED_PRE_COMMIT_ALLOW_NO_CONFIG:?}" ]]; then
    echo "expected PRE_COMMIT_ALLOW_NO_CONFIG=${EXPECTED_PRE_COMMIT_ALLOW_NO_CONFIG}, got ${actual}" >&2
    exit 7
fi
HOOK
    chmod +x "${repo}/${hook_path}"
    EXPECTED_PRE_COMMIT_ALLOW_NO_CONFIG="${expected}" git -C "${repo}" rev-parse --verify HEAD >/dev/null
}

create_git_commit_probe() {
    local bin_dir="$1"
    local real_git="$2"

    mkdir -p "${bin_dir}"
    cat > "${bin_dir}/git" <<'SHIM'
#!/usr/bin/env bash
set -euo pipefail
if [[ "${1:-}" == "commit" ]]; then
    printf '%s\n' "${PRE_COMMIT_ALLOW_NO_CONFIG-__UNSET__}" >> "${COMMIT_ENV_LOG:?}"
fi
exec "${REAL_GIT:?}" "$@"
SHIM
    chmod +x "${bin_dir}/git"
    REAL_GIT="${real_git}" "${bin_dir}/git" --version >/dev/null
}

run_commit_guard_case() {
    local label="$1"
    local step_file="$2"
    local hook_state="$3"
    local config_state="$4"
    local expected_env="$5"
    local case_dir="${WORK}/commit-guard-${label}-${hook_state}-${config_state}"
    local repo="${case_dir}/repo"
    local origin="${case_dir}/origin.git"
    local bin_dir="${case_dir}/bin"
    local stdout_file="${case_dir}/stdout.log"
    local stderr_file="${case_dir}/stderr.log"
    local commit_env_log="${case_dir}/commit-env.log"
    local real_git
    local actual_env

    mkdir -p "${case_dir}"
    real_git="$(command -v git)"
    create_commit_guard_repo "${label}-${hook_state}-${config_state}" "${repo}" "${origin}"
    create_git_commit_probe "${bin_dir}" "${real_git}"

    if [[ "${hook_state}" == "hook" ]]; then
        install_pre_commit_probe "${repo}" "${expected_env}"
    fi
    if [[ "${config_state}" == "config" ]]; then
        printf 'repos: []\n' > "${repo}/.pre-commit-config.yaml"
    fi
    printf 'change for %s %s %s\n' "${label}" "${hook_state}" "${config_state}" \
        > "${repo}/change-${hook_state}-${config_state}.txt"
    : > "${commit_env_log}"

    (
        export PATH="${bin_dir}:${PATH}"
        export REAL_GIT="${real_git}"
        export COMMIT_ENV_LOG="${commit_env_log}"
        export WORKTREE_SETUP_WORKTREE_PATH="${repo}"
        export RECIPE_VAR_worktree_setup__worktree_path="${repo}"
        export EXPECTED_PRE_COMMIT_ALLOW_NO_CONFIG="${expected_env}"
        unset PRE_COMMIT_ALLOW_NO_CONFIG
        bash "${step_file}"
    ) >"${stdout_file}" 2>"${stderr_file}" || {
        echo "--- ${label} ${hook_state}/${config_state} stderr ---" >&2
        cat "${stderr_file}" >&2
        echo "--- ${label} ${hook_state}/${config_state} stdout ---" >&2
        cat "${stdout_file}" >&2
        fail "${label} failed for ${hook_state}/${config_state}; expected PRE_COMMIT_ALLOW_NO_CONFIG=${expected_env}"
    }

    if [[ "$(wc -l < "${commit_env_log}" | tr -d ' ')" != "1" ]]; then
        echo "--- ${label} ${hook_state}/${config_state} commit env log ---" >&2
        cat "${commit_env_log}" >&2
        fail "${label} should run exactly one git commit for ${hook_state}/${config_state}"
    fi
    actual_env="$(cat "${commit_env_log}")"
    [[ "${actual_env}" == "${expected_env}" ]] \
        || fail "${label} recorded PRE_COMMIT_ALLOW_NO_CONFIG=${actual_env}, expected ${expected_env} for ${hook_state}/${config_state}"
}

assert_commit_guard_dynamic() {
    local label="$1"
    local step_file="$2"

    run_commit_guard_case "${label}" "${step_file}" "hook" "no-config" "1"
    run_commit_guard_case "${label}" "${step_file}" "no-hook" "no-config" "__UNSET__"
    run_commit_guard_case "${label}" "${step_file}" "hook" "config" "__UNSET__"
}

setup_pr_scope_repo() {
    local repo="$1"

    git init -b main "${repo}" >/dev/null
    configure_identity "${repo}"
    printf 'base\n' > "${repo}/README.md"
    git -C "${repo}" add README.md
    git -C "${repo}" commit -m "base" >/dev/null
    git -C "${repo}" remote add origin "https://github.com/rysweet/amplihack-rs.git"
    git -C "${repo}" checkout -b feat/issue-754-scoped-monitor >/dev/null
    printf 'scoped monitor\n' > "${repo}/issue-754.txt"
    git -C "${repo}" add issue-754.txt
    git -C "${repo}" commit -m "issue 754 scoped monitor" >/dev/null
}

install_pr_scope_fake_gh() {
    local bin_dir="$1"

    mkdir -p "${bin_dir}"
    cat > "${bin_dir}/gh" <<'SHIM'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> "${GH_SCOPE_LOG:?GH_SCOPE_LOG must be set}"

if [[ "${1:-}" == "auth" && "${2:-}" == "status" ]]; then
    exit 0
fi

if [[ "${1:-}" == "pr" && "${2:-}" == "list" ]]; then
    case "${SCOPE_GH_SCENARIO:?SCOPE_GH_SCENARIO must be set}" in
        matching-among-unrelated)
            jq -nc --arg sha "${EXPECTED_HEAD_SHA:?EXPECTED_HEAD_SHA must be set}" '[
              {
                url: "https://github.com/rysweet/amplihack-rs/pull/999",
                number: 999,
                title: "Unrelated newer PR (#999)",
                state: "OPEN",
                createdAt: "2026-06-12T04:10:00Z",
                headRefName: "feat/unrelated",
                baseRefName: "main",
                headRefOid: "9999999999999999999999999999999999999999",
                headRepositoryOwner: {login: "rysweet"},
                headRepository: {name: "amplihack-rs"},
                isCrossRepository: false
              },
              {
                url: "https://github.com/rysweet/amplihack-rs/pull/754",
                number: 754,
                title: "Fix scoped monitor closure (#754)",
                state: "OPEN",
                createdAt: "2026-06-12T04:03:00Z",
                headRefName: "feat/issue-754-scoped-monitor",
                baseRefName: "main",
                headRefOid: $sha,
                headRepositoryOwner: {login: "rysweet"},
                headRepository: {name: "amplihack-rs"},
                isCrossRepository: false
              }
            ]'
            ;;
        none)
            printf '[]\n'
            ;;
        multiple)
            jq -nc --arg sha "${EXPECTED_HEAD_SHA:?EXPECTED_HEAD_SHA must be set}" '[
              {
                url: "https://github.com/rysweet/amplihack-rs/pull/754",
                number: 754,
                title: "Fix scoped monitor closure (#754)",
                state: "OPEN",
                createdAt: "2026-06-12T04:03:00Z",
                headRefName: "feat/issue-754-scoped-monitor",
                baseRefName: "main",
                headRefOid: $sha,
                headRepositoryOwner: {login: "rysweet"},
                headRepository: {name: "amplihack-rs"},
                isCrossRepository: false
              },
              {
                url: "https://github.com/rysweet/amplihack-rs/pull/755",
                number: 755,
                title: "Fix scoped monitor closure follow-up (#754)",
                state: "OPEN",
                createdAt: "2026-06-12T04:04:00Z",
                headRefName: "feat/issue-754-scoped-monitor",
                baseRefName: "main",
                headRefOid: $sha,
                headRepositoryOwner: {login: "rysweet"},
                headRepository: {name: "amplihack-rs"},
                isCrossRepository: false
              }
            ]'
            ;;
        *)
            echo "unexpected SCOPE_GH_SCENARIO=${SCOPE_GH_SCENARIO}" >&2
            exit 98
            ;;
    esac
    exit 0
fi

echo "unexpected gh call: $*" >&2
exit 99
SHIM
    chmod +x "${bin_dir}/gh"
}

run_pr_scope_helper_case() {
    local scenario="$1"
    local repo="$2"
    local stdout_file="$3"
    local stderr_file="$4"
    local head_sha

    head_sha="$(git -C "${repo}" rev-parse HEAD)"
    (
        cd "${repo}"
        export PATH="${WORK}/scope-bin:${PATH}"
        export GH_SCOPE_LOG="${WORK}/gh-scope-${scenario}.log"
        export SCOPE_GH_SCENARIO="${scenario}"
        export EXPECTED_HEAD_SHA="${head_sha}"
        bash "${PR_SCOPE_HELPER}" \
            --repo "rysweet/amplihack-rs" \
            --head "feat/issue-754-scoped-monitor" \
            --base "main" \
            --issue "754" \
            --work-item "754" \
            --expected-pr-title-prefix "Fix scoped monitor closure" \
            --created-after "2026-06-12T04:02:32Z" \
            --head-sha "${head_sha}"
    ) >"${stdout_file}" 2>"${stderr_file}"
}

assert_scoped_pr_helper_contracts() {
    local repo="${WORK}/scope-repo"
    local stdout_file="${WORK}/scope.out"
    local stderr_file="${WORK}/scope.err"
    local selected_number

    [[ -f "${PR_SCOPE_HELPER}" ]] || fail "workflow_pr_scope.sh must exist as the single current-work PR identity helper"
    setup_pr_scope_repo "${repo}"
    install_pr_scope_fake_gh "${WORK}/scope-bin"

    run_pr_scope_helper_case "matching-among-unrelated" "${repo}" "${stdout_file}" "${stderr_file}" || {
        echo "--- workflow_pr_scope.sh stderr ---" >&2
        cat "${stderr_file}" >&2
        echo "--- workflow_pr_scope.sh stdout ---" >&2
        cat "${stdout_file}" >&2
        fail "workflow_pr_scope.sh must find the scoped PR while ignoring unrelated newer PRs"
    }
    selected_number="$(jq -r '.number // empty' "${stdout_file}")"
    [[ "${selected_number}" == "754" ]] || {
        cat "${stdout_file}" >&2
        fail "workflow_pr_scope.sh selected PR #${selected_number:-<none>}; expected the exact scoped PR #754"
    }
    if grep -Eq -- '--author|sort:updated-desc|sort:created-desc' "${WORK}/gh-scope-matching-among-unrelated.log"; then
        cat "${WORK}/gh-scope-matching-among-unrelated.log" >&2
        fail "workflow_pr_scope.sh must not use author/recent PR discovery for current-work identity"
    fi
    grep -q -- '--head' "${WORK}/gh-scope-matching-among-unrelated.log" \
        || fail "workflow_pr_scope.sh must scope gh lookup by head branch"

    if run_pr_scope_helper_case "none" "${repo}" "${WORK}/scope-none.out" "${WORK}/scope-none.err"; then
        cat "${WORK}/scope-none.out" >&2
        fail "workflow_pr_scope.sh must fail closed when zero scoped PR candidates match"
    fi
    grep -Eq '"reason"[[:space:]]*:[[:space:]]*"no_scoped_pr"' "${WORK}/scope-none.out" "${WORK}/scope-none.err" \
        || fail "zero scoped candidates must emit structured reason no_scoped_pr"

    if run_pr_scope_helper_case "multiple" "${repo}" "${WORK}/scope-multiple.out" "${WORK}/scope-multiple.err"; then
        cat "${WORK}/scope-multiple.out" >&2
        fail "workflow_pr_scope.sh must fail closed when multiple scoped PR candidates match"
    fi
    grep -Eq '"reason"[[:space:]]*:[[:space:]]*"multiple_scoped_prs"' "${WORK}/scope-multiple.out" "${WORK}/scope-multiple.err" \
        || fail "multiple scoped candidates must emit structured reason multiple_scoped_prs"
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

# Issue #752 terminal-state contract coverage. These assertions are expected
# to fail before workflow_final_status.sh and workflow-terminal-state.yaml are
# converted from publish-status summaries into the canonical fail-closed gate.
assert_terminal_status_case \
    "worktree-only-missing-evidence" \
    "1" \
    "false" \
    "FAILED_MISSING_TERMINAL_EVIDENCE" \
    "missing_evidence=implementation_completed,verification_completed,publish_state_reached,terminal_no_op" \
    "OBSERVED_PHASES=workflow-prep,workflow-worktree"

assert_terminal_status_case \
    "analysis-design-only-missing-evidence" \
    "1" \
    "false" \
    "FAILED_MISSING_TERMINAL_EVIDENCE" \
    "workflow-design" \
    "OBSERVED_PHASES=workflow-prep,workflow-worktree,workflow-design"

assert_terminal_status_case \
    "implementation-and-verification-success" \
    "0" \
    "true" \
    "IMPLEMENTED_VERIFIED" \
    "terminal_reason=implementation and verification evidence present" \
    "OBSERVED_PHASES=workflow-prep,workflow-worktree,workflow-design,workflow-tdd,workflow-precommit-test" \
    "IMPLEMENTATION_COMPLETED=true" \
    "VERIFICATION_COMPLETED=true" \
    "TERMINAL_STATE=IMPLEMENTED_VERIFIED" \
    "TERMINAL_REASON=implementation and verification evidence present"

assert_terminal_status_case \
    "publish-state-success" \
    "0" \
    "true" \
    "FOLLOWUP_CREATED" \
    "publish_state_reached=true" \
    "OBSERVED_PHASES=workflow-prep,workflow-worktree,workflow-design,workflow-tdd,workflow-precommit-test,workflow-publish" \
    "IMPLEMENTATION_COMPLETED=true" \
    "VERIFICATION_COMPLETED=true" \
    "PUBLISH_STATE_REACHED=true" \
    "TERMINAL_STATE=FOLLOWUP_CREATED" \
    "TERMINAL_REASON=published follow-up pull request" \
    "PR_URL=https://github.com/example/repo/pull/752"

assert_terminal_status_case \
    "explicit-no-op-success" \
    "0" \
    "true" \
    "ALLOW_NO_OP" \
    "terminal_no_op=true" \
    "OBSERVED_PHASES=workflow-prep,workflow-worktree,workflow-design" \
    "ALLOW_NO_OP=true" \
    "TERMINAL_NO_OP=true" \
    "TERMINAL_STATE=ALLOW_NO_OP" \
    "TERMINAL_REASON=allow_no_op was explicitly selected for a non-code-change path"

assert_terminal_status_case \
    "terminal-probe-no-diff-success" \
    "0" \
    "true" \
    "NO_DIFF_SUCCESS" \
    "terminal_no_op=true" \
    "OBSERVED_PHASES=workflow-prep,workflow-worktree,workflow-design,workflow-publish" \
    "TERMINAL_STATE_TERMINAL_SUCCESS=true" \
    "TERMINAL_STATE=NO_DIFF_SUCCESS" \
    "TERMINAL_REASON=branch already has no meaningful diff"

assert_terminal_status_case \
    "malformed-evidence-fails" \
    "2" \
    "false" \
    "FAILED_INVALID_EVIDENCE" \
    "IMPLEMENTATION_COMPLETED" \
    "OBSERVED_PHASES=workflow-prep,workflow-worktree,workflow-tdd" \
    "IMPLEMENTATION_COMPLETED=maybe" \
    "VERIFICATION_COMPLETED=true"

assert_terminal_status_case \
    "terminal-failure-overrides-success-looking-markers" \
    "1" \
    "false" \
    "BLOCKED_CI" \
    "terminal_failure=true" \
    "OBSERVED_PHASES=workflow-prep,workflow-worktree,workflow-design,workflow-tdd,workflow-precommit-test" \
    "IMPLEMENTATION_COMPLETED=true" \
    "VERIFICATION_COMPLETED=true" \
    "TERMINAL_FAILURE=true" \
    "TERMINAL_STATE=BLOCKED_CI" \
    "TERMINAL_REASON=required checks failed"

assert_terminal_recipe_uses_final_status_tool
assert_yaml_step_not_fatal_false "workflow-finalize final status" "${FINALIZE_RECIPE}" "step-22b-final-status"
assert_yaml_recipe_step_present "smart-orchestrator routing" "${SMART_ORCHESTRATOR_RECIPE}" "smart-execute-routing" "smart-execute-routing"
assert_yaml_step_not_fatal_false "smart-execute-routing development path" "${SMART_EXECUTE_RECIPE}" "execute-single-round-1-development"
assert_yaml_step_not_fatal_false "smart-execute-routing adaptive development path" "${SMART_EXECUTE_RECIPE}" "adaptive-execute-development"
assert_runtime_artifact_helper_contracts

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
' "${PUBLISH_RECIPE}" "${PUBLISH_HELPER}"; then
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
git -C "${PR_REPO}" remote set-url origin "https://github.com/example/repo.git"

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
    export REMOTE_HOST_TYPE="github"
    export AMPLIHACK_HOME="${REPO_ROOT}"
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

if [[ "$(grep -c '^pr list' "${GH_LOG}")" -lt 2 ]]; then
    echo "--- gh log ---" >&2
    cat "${GH_LOG}" >&2
    echo "--- step-16 stderr ---" >&2
    cat "${WORK}/step16.err" >&2
    fail "workflow-publish must retry one transient gh pr list failure before continuing"
fi

if [[ "$(grep -c '^pr create' "${GH_LOG}")" -ne 3 ]]; then
    echo "--- gh log ---" >&2
    cat "${GH_LOG}" >&2
    echo "--- step-16 stderr ---" >&2
    cat "${WORK}/step16.err" >&2
    fail "workflow-publish must retry transient gh pr create failures before succeeding"
fi

# Static and dynamic coverage: workflow commit paths must only set
# PRE_COMMIT_ALLOW_NO_CONFIG=1 when a resolved pre-commit hook exists and the
# repository has no .pre-commit-config.yaml.
extract_step_command "${TDD_RECIPE}" "checkpoint-after-implementation" "${STEP_TDD_CHECKPOINT}"
extract_step_command "${REFACTOR_REVIEW_RECIPE}" "checkpoint-after-review-feedback" "${STEP_REFACTOR_REVIEW_CHECKPOINT}"
extract_step_command "${PR_REVIEW_RECIPE}" "step-18c-push-feedback-changes" "${STEP_PR_REVIEW_FEEDBACK}"
extract_step_command "${FINALIZE_RECIPE}" "step-20b-push-cleanup" "${STEP_FINALIZE_CLEANUP}"

assert_commit_guard_static "workflow-tdd checkpoint-after-implementation" "${STEP_TDD_CHECKPOINT}"
assert_commit_guard_static "workflow-refactor-review checkpoint-after-review-feedback" "${STEP_REFACTOR_REVIEW_CHECKPOINT}"
assert_commit_guard_static "workflow-pr-review step-18c review feedback commit" "${STEP_PR_REVIEW_FEEDBACK}"
assert_commit_guard_static "workflow-finalize final cleanup commit" "${STEP_FINALIZE_CLEANUP}"

assert_commit_guard_dynamic "workflow-tdd" "${STEP_TDD_CHECKPOINT}"
assert_commit_guard_dynamic "workflow-refactor-review" "${STEP_REFACTOR_REVIEW_CHECKPOINT}"
assert_commit_guard_dynamic "workflow-pr-review" "${STEP_PR_REVIEW_FEEDBACK}"
assert_commit_guard_dynamic "workflow-finalize" "${STEP_FINALIZE_CLEANUP}"

assert_scoped_pr_helper_contracts

# ---------------------------------------------------------------------------
# Issue #929 regression coverage.
#
# Bug: default-workflow lockfile-sync opened PRs titled
# "Update Cargo.lock with N changed files" whose branches actually carried the
# entire feature diff, and those PRs auto-merged even when the task explicitly
# said "do NOT merge".
#
# Contract under test:
#   R1  workflow_publish_pr.sh must derive the PR-title scope from the first
#       *substantive* changed file, ignoring generated/lockfiles (Cargo.lock,
#       *.lock, package-lock.json, go.sum, ...) whenever any non-generated file
#       is present. Only a genuinely lockfile-only diff may fall back to the
#       lockfile scope. CHANGED_COUNT and the PR body still enumerate all files.
#   R2  workflow-terminal-state.yaml must detect an explicit no-merge directive
#       in TASK_DESCRIPTION ("do not merge", "don't merge", "no admin merge",
#       "no-merge", "leave ... open") and force should_merge="false" on the
#       active emit path, while leaving should_merge="true" when no directive is
#       present (default behavior unchanged).
#
# These assertions SHOULD FAIL before the issue #929 fixes land and MUST PASS
# afterwards.
# ---------------------------------------------------------------------------

install_publish_fake_gh() {
    # Fake gh that answers the calls workflow_publish_pr.sh + workflow_pr_scope.sh
    # make on the "new draft PR" path, and records the --title of pr create so
    # the test can assert on the generated PR title.
    local bin_dir="$1"
    mkdir -p "${bin_dir}"
    cat > "${bin_dir}/gh" <<'SHIM'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> "${PUBLISH_GH_LOG:?PUBLISH_GH_LOG must be set}"

if [[ "${1:-}" == "auth" ]]; then
    exit 0
fi

if [[ "${1:-}" == "pr" && "${2:-}" == "list" ]]; then
    # No scoped current-work PR exists yet -> new draft PR path.
    printf '[]\n'
    exit 0
fi

if [[ "${1:-}" == "pr" && "${2:-}" == "create" ]]; then
    shift 2
    title=""
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --title) title="${2:-}"; shift 2 ;;
            --body) shift 2 ;;
            *) shift ;;
        esac
    done
    printf '%s\n' "${title}" >> "${PUBLISH_PR_TITLE_LOG:?PUBLISH_PR_TITLE_LOG must be set}"
    printf 'https://github.com/example/repo/pull/929\n'
    exit 0
fi

echo "unexpected gh call: $*" >&2
exit 99
SHIM
    chmod +x "${bin_dir}/gh"
}

setup_publish_title_repo() {
    # Creates a repo whose feature branch changes both a root lockfile (which
    # sorts first in `git diff --name-only`) and a substantive recipe file.
    local repo="$1"
    shift
    local origin="${repo}.origin.git"

    git init --bare -b main "${origin}" >/dev/null
    git init -b main "${repo}" >/dev/null
    configure_identity "${repo}"
    printf 'base\n' > "${repo}/README.md"
    git -C "${repo}" add README.md
    git -C "${repo}" commit -m "base" >/dev/null
    git -C "${repo}" remote add origin "${origin}"
    git -C "${repo}" push -u origin main >/dev/null 2>&1
    git -C "${repo}" checkout -b feat/issue-929-title >/dev/null

    # Changed files for this feature branch. The caller passes the relative
    # paths to create/modify; each is committed so the worktree stays clean.
    local rel
    for rel in "$@"; do
        mkdir -p "${repo}/$(dirname "${rel}")"
        printf 'change for %s\n' "${rel}" > "${repo}/${rel}"
    done
    git -C "${repo}" add -A
    git -C "${repo}" commit -m "feature work plus lockfile churn" >/dev/null

    # Present a GitHub identity to the helper while keeping the local
    # origin/main tracking ref that resolve_pr_base_ref needs.
    git -C "${repo}" remote set-url origin "https://github.com/example/repo.git"
}

run_publish_title_case() {
    local repo="$1"
    local title_log="$2"
    local out="$3"
    local err="$4"

    (
        cd "${repo}"
        export PATH="${WORK}/publish-title-bin:${PATH}"
        export PUBLISH_GH_LOG="${WORK}/publish-title-gh.log"
        export PUBLISH_PR_TITLE_LOG="${title_log}"
        export REMOTE_HOST_TYPE="github"
        export ISSUE_NUMBER="929"
        export TASK_DESCRIPTION="Fix issue 929 title selection"
        export AMPLIHACK_HOME="${REPO_ROOT}"
        bash "${PUBLISH_HELPER}"
    ) >"${out}" 2>"${err}"
}

assert_pr_title_ignores_lockfiles() {
    install_publish_fake_gh "${WORK}/publish-title-bin"
    : > "${WORK}/publish-title-gh.log"

    # Case 1 (the bug): a mixed diff where Cargo.lock sorts first but a
    # substantive recipe file is present. The title must reflect the recipe
    # scope, never "Cargo.lock".
    local mixed_repo="${WORK}/publish-title-mixed"
    local mixed_titles="${WORK}/publish-title-mixed.titles"
    : > "${mixed_titles}"
    setup_publish_title_repo "${mixed_repo}" \
        "Cargo.lock" \
        "amplifier-bundle/recipes/example-929.yaml"
    if ! run_publish_title_case "${mixed_repo}" "${mixed_titles}" \
        "${WORK}/publish-title-mixed.out" "${WORK}/publish-title-mixed.err"; then
        echo "--- publish-title mixed stdout ---" >&2
        cat "${WORK}/publish-title-mixed.out" >&2
        echo "--- publish-title mixed stderr ---" >&2
        cat "${WORK}/publish-title-mixed.err" >&2
        fail "workflow_publish_pr.sh must create a draft PR for a mixed lockfile+feature diff"
    fi
    local mixed_title
    mixed_title="$(head -n1 "${mixed_titles}")"
    [ -n "${mixed_title}" ] || fail "workflow_publish_pr.sh did not record a PR title for the mixed diff"
    if printf '%s' "${mixed_title}" | grep -qi 'Cargo\.lock'; then
        echo "PR title was: ${mixed_title}" >&2
        fail "issue #929: PR title must not label a feature diff as a Cargo.lock change"
    fi
    printf '%s' "${mixed_title}" | grep -qF 'workflow recipes' \
        || { echo "PR title was: ${mixed_title}" >&2; fail "issue #929: mixed diff PR title must reflect the substantive (recipe) scope"; }

    # Case 2 (fallback): a genuinely lockfile-only diff may still use the
    # lockfile scope. This locks the fallback so the filter never yields an
    # empty scope.
    local lock_repo="${WORK}/publish-title-lockonly"
    local lock_titles="${WORK}/publish-title-lockonly.titles"
    : > "${lock_titles}"
    setup_publish_title_repo "${lock_repo}" "Cargo.lock"
    if ! run_publish_title_case "${lock_repo}" "${lock_titles}" \
        "${WORK}/publish-title-lockonly.out" "${WORK}/publish-title-lockonly.err"; then
        echo "--- publish-title lockonly stdout ---" >&2
        cat "${WORK}/publish-title-lockonly.out" >&2
        echo "--- publish-title lockonly stderr ---" >&2
        cat "${WORK}/publish-title-lockonly.err" >&2
        fail "workflow_publish_pr.sh must create a draft PR for a lockfile-only diff"
    fi
    local lock_title
    lock_title="$(head -n1 "${lock_titles}")"
    printf '%s' "${lock_title}" | grep -qi 'Cargo\.lock' \
        || { echo "PR title was: ${lock_title}" >&2; fail "issue #929: a genuinely lockfile-only diff should keep the Cargo.lock scope"; }
}

setup_nomerge_probe_repo() {
    local repo="$1"
    local origin="${repo}.origin.git"

    git init --bare -b main "${origin}" >/dev/null
    git init -b main "${repo}" >/dev/null
    configure_identity "${repo}"
    printf 'base\n' > "${repo}/README.md"
    git -C "${repo}" add README.md
    git -C "${repo}" commit -m "base" >/dev/null
    git -C "${repo}" remote add origin "${origin}"
    git -C "${repo}" push -u origin main >/dev/null 2>&1
    git -C "${repo}" checkout -b feat/issue-929-nomerge >/dev/null
    printf 'feature work\n' > "${repo}/feature-929.txt"
    git -C "${repo}" add feature-929.txt
    git -C "${repo}" commit -m "issue 929 feature work" >/dev/null
    git -C "${repo}" remote set-url origin "https://github.com/example/repo.git"
}

install_nomerge_probe_fake_gh() {
    local bin_dir="$1"
    mkdir -p "${bin_dir}"
    cat > "${bin_dir}/gh" <<'SHIM'
#!/usr/bin/env bash
set -euo pipefail
if [[ "${1:-}" == "auth" ]]; then
    exit 0
fi
if [[ "${1:-}" == "pr" && "${2:-}" == "list" ]]; then
    printf '[]\n'
    exit 0
fi
echo "unexpected gh call: $*" >&2
exit 99
SHIM
    chmod +x "${bin_dir}/gh"
}

run_nomerge_probe_case() {
    local repo="$1"
    local task_description="$2"
    local out="$3"
    local err="$4"

    (
        cd "${repo}"
        export PATH="${WORK}/nomerge-probe-bin:${PATH}"
        export REPO_PATH="${repo}"
        export WORKTREE_SETUP_WORKTREE_PATH="${repo}"
        export BRANCH_NAME="feat/issue-929-nomerge"
        export BASE_REF="main"
        export ISSUE_NUMBER="929"
        export AMPLIHACK_HOME="${REPO_ROOT}"
        export TASK_DESCRIPTION="${task_description}"
        unset PR_URL PR_NUMBER GOAL_ALREADY_MET
        bash "${PROBE_TERMINAL_STEP}"
    ) >"${out}" 2>"${err}"
}

assert_no_merge_directive_suppresses_auto_merge() {
    PROBE_TERMINAL_STEP="${WORK}/probe-terminal-state.sh"
    extract_step_command "${TERMINAL_RECIPE}" "probe-terminal-state" "${PROBE_TERMINAL_STEP}"
    install_nomerge_probe_fake_gh "${WORK}/nomerge-probe-bin"

    # Control: no directive -> active state must keep should_merge="true"
    # (default behavior unchanged).
    local ctrl_repo="${WORK}/nomerge-probe-control"
    setup_nomerge_probe_repo "${ctrl_repo}"
    if ! run_nomerge_probe_case "${ctrl_repo}" \
        "Fix issue 929 without any special directive" \
        "${WORK}/nomerge-control.out" "${WORK}/nomerge-control.err"; then
        echo "--- nomerge control stdout ---" >&2
        cat "${WORK}/nomerge-control.out" >&2
        echo "--- nomerge control stderr ---" >&2
        cat "${WORK}/nomerge-control.err" >&2
        fail "workflow-terminal-state probe must succeed on an active branch with a meaningful diff"
    fi
    grep -q '"should_merge":"true"' "${WORK}/nomerge-control.out" \
        || { cat "${WORK}/nomerge-control.out" >&2; fail "issue #929: default (no directive) active state must keep should_merge=true"; }

    # Directive present -> should_merge must be forced to "false".
    local directive
    for directive in \
        "Fix issue 929. Do NOT merge the PR; leave it open." \
        "Please don't merge this, no admin merge." \
        "Implement the change but this is a no-merge task"; do
        local case_repo="${WORK}/nomerge-probe-$(printf '%s' "${directive}" | tr -c 'A-Za-z0-9' '_' | cut -c1-24)"
        setup_nomerge_probe_repo "${case_repo}"
        if ! run_nomerge_probe_case "${case_repo}" "${directive}" \
            "${WORK}/nomerge-dir.out" "${WORK}/nomerge-dir.err"; then
            echo "--- nomerge directive stdout ---" >&2
            cat "${WORK}/nomerge-dir.out" >&2
            echo "--- nomerge directive stderr ---" >&2
            cat "${WORK}/nomerge-dir.err" >&2
            fail "workflow-terminal-state probe must succeed on an active branch when a no-merge directive is present"
        fi
        grep -q '"should_merge":"false"' "${WORK}/nomerge-dir.out" \
            || { echo "directive was: ${directive}" >&2; cat "${WORK}/nomerge-dir.out" >&2; fail "issue #929: no-merge directive must force should_merge=false"; }
    done
}

assert_pr_title_ignores_lockfiles
assert_no_merge_directive_suppresses_auto_merge

echo "PASS: default workflow reliability contracts are covered."
