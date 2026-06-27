#!/usr/bin/env bash
# test-bug-834-doc-review-non-fatal.sh — regression test for issue #834.
#
# Bug: a smart-orchestrator/default-workflow run reports overall FAILURE when
# the 'step-06b-documentation-review' agent step exits non-zero, even though
# earlier steps already produced durable, useful output (a pushed hardening
# commit, a merged follow-up PR, and a posted review thread). The generic
# failure obscures the completed work and forces manual reconciliation.
#
# Expected behaviour after the fix (per the issue):
#   1. A doc-review failure that runs AFTER durable side effects MUST NOT leave
#      the parent workflow as a generic hard failure. step-06b therefore carries
#      `continue_on_error: true` so its non-zero exit is non-fatal.
#   2. A follow-on non-fatal `type: bash` checkpoint step records and surfaces
#      the durable artifact references (branch, PR id/url, thread/comment id,
#      commit sha) it knows about, each guarded against unset, so the summary
#      shows them instead of just 'failed'.
#   3. The doc-review failure is non-fatal-but-reported: a `WARNING` line on
#      stderr plus a `NEEDS_ATTENTION` marker in the checkpoint's declared
#      `output`, producing a degraded-success/partial state.
#   4. The failure is NOT silently swallowed — both the WARNING (stderr) and the
#      NEEDS_ATTENTION marker (summary output) must be present.
#
# Security contracts for the new checkpoint bash step (untrusted agent feedback
# is consumed as data, never as code):
#   S1. No eval / source / dynamic command construction; no ${!var} / ${var@P}.
#   S2. Untrusted feedback is read with `printf '%s'` (not `printf "$X"` or a
#       bare `echo $X`) to prevent format-string / word-splitting injection.
#   S3. `set -uo pipefail` WITHOUT `-e`, and the step is structured to exit 0
#       (non-fatal checkpoint).
#   S4. No secret/env dumps (no `env`, `set`, or token printing).
#
# This test SHOULD FAIL before the #834 fix lands and MUST PASS afterwards.
#
# Usage: bash amplifier-bundle/recipes/tests/test-bug-834-doc-review-non-fatal.sh
# Exit codes: 0 = pass, 1 = fail, 2 = test harness error.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"

DESIGN_RECIPE="${REPO_ROOT}/amplifier-bundle/recipes/workflow-design.yaml"

# id of the doc-review step and the new non-fatal checkpoint step.
REVIEW_STEP_ID="step-06b-documentation-review"
CHECKPOINT_STEP_ID="step-06b-checkpoint-doc-review"
CHECKPOINT_OUTPUT="doc_review_checkpoint"

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

if [[ ! -f "${DESIGN_RECIPE}" ]]; then
    echo "HARNESS-ERROR: ${DESIGN_RECIPE} not found" >&2
    exit 2
fi

# extract_step <id> : print the YAML lines of a single `- id: "<id>"` block,
# from its `- id:` line up to (but not including) the next `- id:` line.
extract_step() {
    local target="$1"
    awk -v target="${target}" '
        # Match a step header line:  - id: "<id>"   (any leading indent)
        /^[[:space:]]*-[[:space:]]+id:[[:space:]]*["'\'']?/ {
            line = $0
            # strip to the value after id:
            sub(/^[[:space:]]*-[[:space:]]+id:[[:space:]]*/, "", line)
            gsub(/["'\'']/, "", line)
            sub(/[[:space:]]+#.*$/, "", line)
            sub(/[[:space:]]+$/, "", line)
            if (line == target) { capture = 1; print $0; next }
            if (capture == 1) { capture = 0 }
        }
        capture == 1 { print }
    ' "${DESIGN_RECIPE}"
}

echo "=== Bug #834: documentation-review failure is non-fatal-but-reported ==="

REVIEW_BLOCK="$(extract_step "${REVIEW_STEP_ID}")"
CHECKPOINT_BLOCK="$(extract_step "${CHECKPOINT_STEP_ID}")"

# ---------------------------------------------------------------------------
# Assertion 1: step-06b-documentation-review carries continue_on_error: true
# so a non-zero exit does NOT hard-fail the parent workflow.
# ---------------------------------------------------------------------------
if [[ -z "${REVIEW_BLOCK}" ]]; then
    fail 1 "${REVIEW_STEP_ID} step not found in workflow-design.yaml"
elif printf '%s\n' "${REVIEW_BLOCK}" | grep -qE '^[[:space:]]*continue_on_error:[[:space:]]*true'; then
    pass 1 "${REVIEW_STEP_ID} has continue_on_error: true (non-fatal)"
else
    fail 1 "${REVIEW_STEP_ID} is missing continue_on_error: true — a doc-review failure still hard-fails the workflow"
fi

# ---------------------------------------------------------------------------
# Assertion 2: a dedicated non-fatal checkpoint step exists, is type bash,
# and declares the doc_review_checkpoint output.
# ---------------------------------------------------------------------------
if [[ -z "${CHECKPOINT_BLOCK}" ]]; then
    fail 2a "${CHECKPOINT_STEP_ID} checkpoint step not found in workflow-design.yaml"
else
    pass 2a "${CHECKPOINT_STEP_ID} checkpoint step exists"

    if printf '%s\n' "${CHECKPOINT_BLOCK}" | grep -qE '^[[:space:]]*type:[[:space:]]*["'\'']?bash'; then
        pass 2b "${CHECKPOINT_STEP_ID} is a type: bash step"
    else
        fail 2b "${CHECKPOINT_STEP_ID} is not a type: bash step"
    fi

    if printf '%s\n' "${CHECKPOINT_BLOCK}" | grep -qE "^[[:space:]]*output:[[:space:]]*[\"']?${CHECKPOINT_OUTPUT}"; then
        pass 2c "${CHECKPOINT_STEP_ID} declares output: ${CHECKPOINT_OUTPUT} (propagated to summary)"
    else
        fail 2c "${CHECKPOINT_STEP_ID} does not declare output: ${CHECKPOINT_OUTPUT}"
    fi
fi

# ---------------------------------------------------------------------------
# Assertion 3: the checkpoint surfaces the failure (not swallowed):
#   - a WARNING line on stderr, AND
#   - a NEEDS_ATTENTION marker (machine-consumable, lands in the summary).
# ---------------------------------------------------------------------------
if [[ -n "${CHECKPOINT_BLOCK}" ]]; then
    if printf '%s\n' "${CHECKPOINT_BLOCK}" | grep -qE 'WARNING' \
       && printf '%s\n' "${CHECKPOINT_BLOCK}" | grep -qE '>&2'; then
        pass 3a "checkpoint emits a WARNING line to stderr"
    else
        fail 3a "checkpoint does not emit a WARNING to stderr (failure is hidden)"
    fi

    if printf '%s\n' "${CHECKPOINT_BLOCK}" | grep -qE 'NEEDS_ATTENTION'; then
        pass 3b "checkpoint records a NEEDS_ATTENTION marker in the summary output"
    else
        fail 3b "checkpoint does not record a NEEDS_ATTENTION marker (degraded state not surfaced)"
    fi
fi

# ---------------------------------------------------------------------------
# Assertion 4: durable artifact references are surfaced, each guarded against
# unset so absent refs are omitted rather than erroring. We require that at
# least branch, PR, commit, and review-thread refs are referenced with the
# ${VAR:-...} guard form.
# ---------------------------------------------------------------------------
if [[ -n "${CHECKPOINT_BLOCK}" ]]; then
    ref_misses=0
    for ref_re in \
        'BRANCH|WORKTREE' \
        'PR_URL|PR_NUMBER|PULL_REQUEST' \
        'COMMIT_SHA|HEAD_SHA|COMMIT' \
        'REVIEW_THREAD|REVIEW_COMMENT|THREAD_ID|COMMENT_ID'; do
        if printf '%s\n' "${CHECKPOINT_BLOCK}" | grep -qE "\\\$\\{(${ref_re})[A-Z_]*:-"; then
            :
        else
            fail "4:${ref_re}" "checkpoint does not surface a guarded durable ref matching: ${ref_re}"
            ref_misses=$((ref_misses + 1))
        fi
    done
    if [[ ${ref_misses} -eq 0 ]]; then
        pass 4 "checkpoint surfaces guarded durable artifact refs (branch, PR, commit, review thread)"
    fi
fi

# ---------------------------------------------------------------------------
# Assertion 5 (security): untrusted agent feedback is consumed as DATA.
#   - the checkpoint reads doc_review_feedback via `printf '%s'` (not a bare
#     echo of the raw variable, nor `printf "$VAR"`).
# ---------------------------------------------------------------------------
if [[ -n "${CHECKPOINT_BLOCK}" ]]; then
    # The untrusted value is injected via the templated {{doc_review_feedback}}
    # or an env-flattened DOC_REVIEW_FEEDBACK; either way it must be piped
    # through printf '%s' before grep, never word-split into a command.
    if printf '%s\n' "${CHECKPOINT_BLOCK}" | grep -qE "printf[[:space:]]+'%s'"; then
        pass 5a "checkpoint uses printf '%s' to consume untrusted feedback safely"
    else
        fail 5a "checkpoint does not use printf '%s' for untrusted feedback (format-string risk)"
    fi

    # Must NOT use the format-string-injection-prone `printf "$VAR"` form.
    if printf '%s\n' "${CHECKPOINT_BLOCK}" | grep -qE 'printf[[:space:]]+"\$'; then
        fail 5b "checkpoint uses printf with a variable as the format string (injection risk)"
    else
        pass 5b "checkpoint never uses a variable as a printf format string"
    fi
fi

# ---------------------------------------------------------------------------
# Assertion 6 (security): no eval / source / dynamic command construction and
# no ${!var} / ${var@P} indirection in the checkpoint step.
# ---------------------------------------------------------------------------
if [[ -n "${CHECKPOINT_BLOCK}" ]]; then
    if printf '%s\n' "${CHECKPOINT_BLOCK}" | grep -qE '(^|[^[:alnum:]_])(eval|source)([^[:alnum:]_]|$)' \
       || printf '%s\n' "${CHECKPOINT_BLOCK}" | grep -qE '\$\{![A-Za-z_]' \
       || printf '%s\n' "${CHECKPOINT_BLOCK}" | grep -qE '@P\}'; then
        fail 6 "checkpoint uses eval/source/indirect-expansion (code-injection risk)"
    else
        pass 6 "checkpoint has no eval/source/indirect-expansion"
    fi
fi

# ---------------------------------------------------------------------------
# Assertion 7 (security/robustness): the checkpoint runs with `set -uo pipefail`
# but WITHOUT `-e` (so it never aborts mid-summary) and is structured to be
# non-fatal (it must not `exit 1`).
# ---------------------------------------------------------------------------
if [[ -n "${CHECKPOINT_BLOCK}" ]]; then
    if printf '%s\n' "${CHECKPOINT_BLOCK}" | grep -qE 'set[[:space:]]+-uo[[:space:]]+pipefail' \
       || printf '%s\n' "${CHECKPOINT_BLOCK}" | grep -qE 'set[[:space:]]+-[[:alpha:]]*u[[:alpha:]]*o[[:alpha:]]*[[:space:]]+pipefail'; then
        # ensure -e is not enabled
        if printf '%s\n' "${CHECKPOINT_BLOCK}" | grep -qE 'set[[:space:]]+-[[:alpha:]]*e'; then
            fail 7a "checkpoint enables 'set -e' — a failed sub-command would abort the non-fatal checkpoint"
        else
            pass 7a "checkpoint uses 'set -uo pipefail' without -e"
        fi
    else
        fail 7a "checkpoint does not use 'set -uo pipefail'"
    fi

    if printf '%s\n' "${CHECKPOINT_BLOCK}" | grep -qE 'exit[[:space:]]+1'; then
        fail 7b "checkpoint contains 'exit 1' — the checkpoint must be non-fatal"
    else
        pass 7b "checkpoint never 'exit 1' (structurally non-fatal)"
    fi
fi

# ---------------------------------------------------------------------------
# Assertion 8 (security): no secret/env dumping in the checkpoint.
# ---------------------------------------------------------------------------
if [[ -n "${CHECKPOINT_BLOCK}" ]]; then
    if printf '%s\n' "${CHECKPOINT_BLOCK}" | grep -qE '(^|[^[:alnum:]_/.-])(env|set)[[:space:]]*$' \
       || printf '%s\n' "${CHECKPOINT_BLOCK}" | grep -qiE 'GITHUB_TOKEN|GH_TOKEN|[[:space:]]TOKEN[[:space:]=]'; then
        fail 8 "checkpoint may dump env/secrets/tokens into the summary"
    else
        pass 8 "checkpoint does not dump env or print tokens"
    fi
fi

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
echo ""
echo "--- Summary: ${PASS_COUNT} passed, ${FAIL_COUNT} failed ---"

if [[ ${FAIL_COUNT} -gt 0 ]]; then
    exit 1
fi

echo "PASS: Bug #834 — documentation-review failure is non-fatal-but-reported."
exit 0
