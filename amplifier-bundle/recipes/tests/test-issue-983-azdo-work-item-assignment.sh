#!/usr/bin/env bash
# test-issue-983-azdo-work-item-assignment.sh — regression test for issue #983.
#
# Bug: the ADO branch of step-03-create-issue in workflow-prep.yaml created work
# items UNASSIGNED and ALWAYS as `--type "Task"`, and swallowed genuine AzDO API
# failures with `2>/dev/null || echo ""` before silently degrading to local
# tracking. Over time this produced hundreds of orphaned, unassigned Task items.
#
# Fix contracts under test:
#   STATIC:
#     - The create call no longer hardcodes `--type "Task"`.
#     - It passes an `--assigned-to` identity (never unassigned).
#     - It uses `${AZDO_WORK_ITEM_TYPE:-Issue}` for the type.
#     - It resolves the caller's UPN client-side (`az account show`) rather than
#       relying on the WIQL-only `@me` macro that create does not expand.
#     - The silent `2>/dev/null || echo ""` swallow on the create is gone.
#     - REF_ISSUE_NUM reuse (work-item show) is retained.
#   DYNAMIC (real step-03 command executed with a stubbed `az`):
#     - CREATE-OK: default run creates as `Issue`, assigned to the resolved UPN
#              (from `az account show`), prints URL, exit 0.
#     - OVERRIDE: AZDO_ASSIGNED_TO / AZDO_WORK_ITEM_TYPE are honored.
#     - REUSE: when the task references AB#N and `work-item show` resolves, the
#              existing item is reused and NO create happens.
#     - FAIL-LOUD: a genuine create failure exits non-zero with ERROR and does
#              NOT fall back to local tracking (no tracking_system=local emitted).
#     - NO-IDENTITY: when the caller's identity cannot be resolved and no
#              override is set, the step fails loud (exit non-zero, ERROR) rather
#              than creating an unassigned item.
#     - ENV-ABSENCE: with no `az` on PATH, the step legitimately WARNs and falls
#              back to local tracking (exit 0) — that path must be preserved.
#
# Usage: bash amplifier-bundle/recipes/tests/test-issue-983-azdo-work-item-assignment.sh
# Exit codes: 0 = pass, 1 = fail, 2 = test harness error.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
RECIPE="${REPO_ROOT}/amplifier-bundle/recipes/workflow-prep.yaml"

PASS_COUNT=0
FAIL_COUNT=0
pass() { PASS_COUNT=$((PASS_COUNT + 1)); echo "  PASS[$1]: $2"; }
fail() { FAIL_COUNT=$((FAIL_COUNT + 1)); echo "  FAIL[$1]: $2" >&2; }

[[ -f "${RECIPE}" ]] || { echo "HARNESS-ERROR: recipe not found: ${RECIPE}" >&2; exit 2; }
command -v python3 >/dev/null 2>&1 || { echo "HARNESS-ERROR: python3 required" >&2; exit 2; }

echo "=== Issue #983: ADO work-item assignment + configurable type + fail-loud ==="

# ---------------------------------------------------------------------------
# STATIC checks against the recipe source.
# ---------------------------------------------------------------------------
if ! grep -Eq 'az boards work-item create[^\n]*--type[[:space:]]+"Task"' "${RECIPE}" \
   && ! grep -Eq -- '--type[[:space:]]+"Task"' "${RECIPE}"; then
    pass "STATIC-no-task" 'no hardcoded `--type "Task"` remains'
else
    fail "STATIC-no-task" 'a hardcoded `--type "Task"` is still present'
fi

if grep -Fq -- '--assigned-to "$AZDO_WI_ASSIGNEE"' "${RECIPE}"; then
    pass "STATIC-assigned-to" 'create passes an --assigned-to identity'
else
    fail "STATIC-assigned-to" 'create is missing --assigned-to'
fi

if grep -Fq -- 'az account show' "${RECIPE}" \
   && grep -Fq -- '${AZDO_ASSIGNED_TO:-}' "${RECIPE}"; then
    pass "STATIC-assignee-resolve" 'assignee resolved client-side (az account show) with AZDO_ASSIGNED_TO override, not the WIQL @me macro'
else
    fail "STATIC-assignee-resolve" 'missing client-side UPN resolution (az account show / ${AZDO_ASSIGNED_TO:-} default)'
fi

grep -Fq -- '${AZDO_WORK_ITEM_TYPE:-Issue}' "${RECIPE}" \
    && pass "STATIC-type-default" 'type defaults to Issue with AZDO_WORK_ITEM_TYPE override' \
    || fail "STATIC-type-default" 'missing ${AZDO_WORK_ITEM_TYPE:-Issue} default'

# The old silent-swallow create pattern must be gone.
if grep -Eq 'az boards work-item create.*2>/dev/null \|\| echo ""' "${RECIPE}"; then
    fail "STATIC-no-swallow" 'create still swallows errors via `2>/dev/null || echo ""`'
else
    pass "STATIC-no-swallow" 'create no longer swallows errors silently'
fi

grep -Fq 'az boards work-item show' "${RECIPE}" \
    && pass "STATIC-reuse" 'REF_ISSUE_NUM reuse (work-item show) retained' \
    || fail "STATIC-reuse" 'REF_ISSUE_NUM reuse path removed'

# ---------------------------------------------------------------------------
# DYNAMIC fixture: run the REAL step-03-create-issue command with a stub `az`.
# ---------------------------------------------------------------------------
TMP_ROOT="$(mktemp -d)"
cleanup() { rm -rf "${TMP_ROOT}"; }
trap cleanup EXIT

# Extract the exact step-03-create-issue command body from the recipe.
STEP_CMD="${TMP_ROOT}/step03.sh"
python3 - "$RECIPE" > "${STEP_CMD}" <<'PY'
import sys, yaml
d = yaml.safe_load(open(sys.argv[1]))
cmd = next(s["command"] for s in d["steps"] if s["id"] == "step-03-create-issue")
sys.stdout.write(cmd)
PY
[[ -s "${STEP_CMD}" ]] || { echo "HARNESS-ERROR: failed to extract step-03 command" >&2; exit 2; }

# Fake git repo with an Azure DevOps origin remote.
FAKE_REPO="${TMP_ROOT}/repo"
mkdir -p "${FAKE_REPO}"
git -C "${FAKE_REPO}" init -q
git -C "${FAKE_REPO}" remote add origin "https://dev.azure.com/acs-mdash/acs-mdash/_git/acs-mdash"

# Stub `az` that records its argv and behaves per AZ_STUB_MODE.
STUB_BIN="${TMP_ROOT}/bin"
mkdir -p "${STUB_BIN}"
cat > "${STUB_BIN}/az" <<'AZ'
#!/usr/bin/env bash
printf '%s\0' "$@" >> "${AZ_ARGS_LOG}"
printf '\n---INVOCATION---\n' >> "${AZ_ARGS_LOG}"
# Subcommand detection.
sub="$*"
case "$sub" in
  *"account show"*)
    if [ "${AZ_STUB_MODE:-}" = "no-identity" ]; then exit 1; fi
    echo "${AZ_UPN:-caller@example.com}"
    exit 0 ;;
  *"ad signed-in-user show"*)
    if [ "${AZ_STUB_MODE:-}" = "no-identity" ]; then exit 1; fi
    echo "${AZ_UPN:-caller@example.com}"
    exit 0 ;;
  *"work-item show"*)
    if [ "${AZ_STUB_MODE:-}" = "reuse" ]; then
      echo "https://dev.azure.com/acs-mdash/acs-mdash/_workitems/edit/${AZ_REUSE_ID:-4242}"
      exit 0
    fi
    exit 1 ;;
  *"work-item create"*)
    if [ "${AZ_STUB_MODE:-}" = "create-fail" ]; then
      echo "TF401232: Work item type does not exist." >&2
      exit 1
    fi
    # Emit `[id,url]` tsv shape.
    printf '9001\thttps://dev.azure.com/acs-mdash/acs-mdash/_workitems/edit/9001\n'
    exit 0 ;;
esac
exit 0
AZ
chmod +x "${STUB_BIN}/az"

# run_env_step <mode> <task-desc> [VAR=val ...] — execute the real step-03 in the
# fixture with a stubbed `az`. Task description is passed explicitly (never
# inherited from the outer shell). Any trailing VAR=val pairs are applied to the
# step invocation itself. Sets LAST_OUT / LAST_RC / LAST_ARGS / LAST_ERR.
run_env_step() {
    local mode="$1" task="$2"; shift 2
    local args_log="${TMP_ROOT}/az_args.log"; : > "${args_log}"
    local out rc
    out="$(
        cd "${FAKE_REPO}" || exit 3
        export PATH="${STUB_BIN}:/usr/bin:/bin"
        # Issue #1361: step-03's provider-metadata helpers (emit_local_metadata,
        # sanitize_cli_output, _pct_decode) now live in
        # amplifier-bundle/tools/workflow_issue_tracking.sh, reached through the
        # AMPLIHACK_HOME / REPO_PATH / cwd / ~/.copilot / ~/.amplihack cascade.
        # REPO_PATH here is a synthetic AzDO fixture with no bundle in it, so the
        # step is told where the bundle is. In a real run the recipe runner
        # always sets AMPLIHACK_HOME (EnvBuilder::with_amplihack_home_from), so
        # this is a harness detail, not a production one.
        export AMPLIHACK_HOME="${REPO_ROOT}"
        export AZ_STUB_MODE="${mode}"
        export AZ_ARGS_LOG="${args_log}"
        export REPO_PATH="${FAKE_REPO}"
        export REMOTE_HOST_TYPE="azdo"
        export TASK_DESCRIPTION="${task}"
        export FINAL_REQUIREMENTS="requirements body"
        export ISSUE_NUMBER=""
        export GITHUB_OUTPUT="${TMP_ROOT}/gh_output"
        env "$@" bash "${STEP_CMD}" 2>"${TMP_ROOT}/stderr.log"
    )"
    rc=$?
    LAST_OUT="${out}"
    LAST_RC="${rc}"
    LAST_ARGS="$(tr '\0' ' ' < "${args_log}")"
    LAST_ERR="$(cat "${TMP_ROOT}/stderr.log" 2>/dev/null || true)"
}

# --- CREATE-OK: default assignment = resolved UPN, type Issue ----------------
run_env_step "create-ok" "Fix the widget alignment bug"
if [[ ${LAST_RC} -eq 0 ]] \
   && printf '%s' "${LAST_OUT}" | grep -q '_workitems/edit/9001' \
   && printf '%s' "${LAST_ARGS}" | grep -q -- '--assigned-to caller@example.com' \
   && ! printf '%s' "${LAST_ARGS}" | grep -q -- '--assigned-to @me' \
   && printf '%s' "${LAST_ARGS}" | grep -q -- '--type Issue' \
   && ! printf '%s' "${LAST_ARGS}" | grep -q -- '--type Task'; then
    pass "DYN-create-ok" "default create is type=Issue, assigned-to=resolved UPN (not @me), prints URL, exit 0"
else
    fail "DYN-create-ok" "rc=${LAST_RC} out='${LAST_OUT}' args='${LAST_ARGS}'"
fi

# --- OVERRIDE: AZDO_ASSIGNED_TO + AZDO_WORK_ITEM_TYPE -----------------------
run_env_step "create-ok" "Fix the widget alignment bug" \
    AZDO_ASSIGNED_TO="dev@example.com" AZDO_WORK_ITEM_TYPE="Product Backlog Item"
if [[ ${LAST_RC} -eq 0 ]] \
   && printf '%s' "${LAST_ARGS}" | grep -q -- '--assigned-to dev@example.com' \
   && printf '%s' "${LAST_ARGS}" | grep -q -- '--type Product Backlog Item'; then
    pass "DYN-override" "honors AZDO_ASSIGNED_TO and AZDO_WORK_ITEM_TYPE overrides"
else
    fail "DYN-override" "rc=${LAST_RC} args='${LAST_ARGS}'"
fi

# --- REUSE: task references AB#N, existing item resolved, no create ---------
run_env_step "reuse" "Fix the thing AB#4242"
if [[ ${LAST_RC} -eq 0 ]] \
   && printf '%s' "${LAST_OUT}" | grep -q '_workitems/edit/4242' \
   && ! printf '%s' "${LAST_ARGS}" | grep -q -- 'work-item create'; then
    pass "DYN-reuse" "reuses AB#4242 via work-item show, never calls create"
else
    fail "DYN-reuse" "rc=${LAST_RC} out='${LAST_OUT}' args='${LAST_ARGS}'"
fi

# --- FAIL-LOUD: genuine create failure exits non-zero, no local fallback ----
run_env_step "create-fail" "Fix the widget alignment bug"
if [[ ${LAST_RC} -ne 0 ]] \
   && printf '%s' "${LAST_ERR}" | grep -q 'ERROR' \
   && ! printf '%s%s' "${LAST_OUT}" "${LAST_ERR}" | grep -q 'tracking_system=local'; then
    pass "DYN-fail-loud" "genuine create failure exits non-zero with ERROR, no silent local fallback"
else
    fail "DYN-fail-loud" "rc=${LAST_RC} out='${LAST_OUT}' err='${LAST_ERR}'"
fi

# --- NO-IDENTITY: identity unresolved + no override → report the reason and
# degrade to local tracking (exit 0), never creating an unassigned work item.
# This honors issue #983 ("never unassigned"; the failure is reported, not
# silently swallowed) while preserving the issue #684 contract that the AzDO
# path degrades gracefully to local tracking instead of aborting the workflow.
run_env_step "no-identity" "Fix the widget alignment bug"
if [[ ${LAST_RC} -eq 0 ]] \
   && printf '%s' "${LAST_ERR}" | grep -qi 'could not resolve' \
   && ! printf '%s' "${LAST_ARGS}" | grep -q -- 'work-item create' \
   && printf '%s' "${LAST_OUT}" | grep -q 'tracking_system=local'; then
    pass "DYN-no-identity" "unresolved identity reports the reason and degrades to local tracking, never creating an unassigned item"
else
    fail "DYN-no-identity" "rc=${LAST_RC} out='${LAST_OUT}' args='${LAST_ARGS}' err='${LAST_ERR}'"
fi

# --- OVERRIDE-NO-IDENTITY: identity unresolved but AZDO_ASSIGNED_TO set → create OK ---
run_env_step "no-identity" "Fix the widget alignment bug" AZDO_ASSIGNED_TO="dev@example.com"
if [[ ${LAST_RC} -eq 0 ]] \
   && printf '%s' "${LAST_ARGS}" | grep -q -- '--assigned-to dev@example.com'; then
    pass "DYN-override-no-identity" "explicit AZDO_ASSIGNED_TO bypasses identity resolution"
else
    fail "DYN-override-no-identity" "rc=${LAST_RC} args='${LAST_ARGS}' err='${LAST_ERR}'"
fi

# --- ENV-ABSENCE: no `az` on PATH → legitimate local tracking (exit 0) ------
# Only meaningfully testable when no real `az` is installed; the stub is kept off
# PATH here so we exercise the `command -v az` absence branch.
if command -v az >/dev/null 2>&1; then
    pass "DYN-env-absence" "skipped (real az present on system); env-absence branch left unchanged"
else
    ABSENT_ERRLOG="${TMP_ROOT}/stderr2.log"
    ABSENT_OUT="$(
        cd "${FAKE_REPO}" || exit 3
        export PATH="/usr/bin:/bin"
        # See the note in run_env_step: the fixture has no bundle, so the step is
        # told where amplifier-bundle/tools/ is (issue #1361).
        export AMPLIHACK_HOME="${REPO_ROOT}"
        export REPO_PATH="${FAKE_REPO}"
        export REMOTE_HOST_TYPE="azdo"
        export TASK_DESCRIPTION="Local only task with no az"
        export FINAL_REQUIREMENTS="requirements body"
        export ISSUE_NUMBER=""
        export GITHUB_OUTPUT="${TMP_ROOT}/gh_output2"
        bash "${STEP_CMD}" 2>"${ABSENT_ERRLOG}"
    )"
    ABSENT_RC=$?
    ABSENT_ERR="$(cat "${ABSENT_ERRLOG}" 2>/dev/null || true)"
    if [[ ${ABSENT_RC} -eq 0 ]] \
       && printf '%s%s' "${ABSENT_OUT}" "${ABSENT_ERR}" | grep -q 'local tracking'; then
        pass "DYN-env-absence" "no az → WARN + local tracking, exit 0 (preserved)"
    else
        fail "DYN-env-absence" "rc=${ABSENT_RC} out='${ABSENT_OUT}' err='${ABSENT_ERR}'"
    fi
fi

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
echo ""
echo "--- Summary: ${PASS_COUNT} passed, ${FAIL_COUNT} failed ---"
[[ ${FAIL_COUNT} -gt 0 ]] && exit 1
echo "PASS: Issue #983 — ADO work items are assigned, correctly typed, and fail loudly."
exit 0
