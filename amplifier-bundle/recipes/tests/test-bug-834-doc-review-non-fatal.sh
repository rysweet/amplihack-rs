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
#   S1. Only fixed trusted framework sourcing; no eval / data execution; no indirect expansion
#       or "@P"-style parameter transformation.
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

# Extract executable command separately; metadata/comments cannot prove linkage.
CHECKPOINT_COMMAND="$(printf '%s\n' "$CHECKPOINT_BLOCK" | awk '
    /command: \|/ { capture=1; next }
    capture && /^    [a-zA-Z_]+:/ { exit }
    capture { sub(/^      /, ""); print }
')"
CHECKPOINT_HELPER="$REPO_ROOT/amplifier-bundle/tools/workflow_doc_review_checkpoint.sh"
if [[ -f "$CHECKPOINT_HELPER" ]] \
   && printf '%s\n' "$CHECKPOINT_COMMAND" | grep -qxF '  CONTEXT_HELPER="$AMPLIHACK_HOME/amplifier-bundle/tools/workflow_context.sh"' \
   && printf '%s\n' "$CHECKPOINT_COMMAND" | grep -qxF 'DOC_CHECKPOINT_HELPER="$(dirname "$CONTEXT_HELPER")/workflow_doc_review_checkpoint.sh"' \
   && printf '%s\n' "$CHECKPOINT_COMMAND" | grep -qxF '  . "$DOC_CHECKPOINT_HELPER"'; then
    pass linkage "canonical command sources the fixed sibling of selected context helper"
else
    fail linkage "canonical command/helper sourcing relationship is broken"
fi
# Preserve metadata assertions and inspect the actual selected implementation.
CHECKPOINT_BLOCK="$CHECKPOINT_BLOCK
$(cat "$CHECKPOINT_HELPER")"

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
    # Selected structured context or legacy DOC_REVIEW_FEEDBACK stays data.
    # It must be piped through printf '%s' before structured parsing, never word-split into a command.
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
# Assertion 6 (security): only fixed framework sources; no eval or data execution and
# no indirect expansion or "@P"-style parameter transformation in the checkpoint step.
# ---------------------------------------------------------------------------
if [[ -n "${CHECKPOINT_BLOCK}" ]]; then
    # Fixed trusted sources are allowed; every other executable source is rejected.
    SECURITY_CODE="$(printf '%s\n' "$CHECKPOINT_COMMAND" "$(cat "$CHECKPOINT_HELPER")" | sed '/^[[:space:]]*#/d')"
    UNTRUSTED_SOURCE="$(printf '%s\n' "$SECURITY_CODE" | grep -E '(^|[;&|[:space:]])(source|\.)[[:space:]]' \
        | grep -vF '. "$CONTEXT_HELPER"' | grep -vF '. "$DOC_CHECKPOINT_HELPER"' || true)"
    if [[ -n "$UNTRUSTED_SOURCE" ]] \
       || printf '%s\n' "$SECURITY_CODE" | grep -qE '(^|[^[:alnum:]_])eval([^[:alnum:]_]|$)|\$\{![A-Za-z_]|@P\}'; then
        fail 6 "checkpoint executes untrusted source/eval/indirect-expansion"
    else
        pass 6 "checkpoint sources only fixed trusted helpers; feedback is never code"
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

# Execute the canonical wrapper with real helper tools, never a parser stub.
RUNTIME="$(mktemp -d "${TMPDIR:-${RUNNER_TEMP:-/tmp}}/doc-checkpoint.XXXXXX")"
trap 'rm -rf "$RUNTIME"' EXIT
mkdir -p "$RUNTIME/bin" "$RUNTIME/empty" "$RUNTIME/home"
if [[ -n "${AMPLIHACK_BIN:-}" ]]; then
    TOOL="$AMPLIHACK_BIN"
else
    # The lint job reaches this gate before sccache setup; build without a wrapper.
    (cd "$REPO_ROOT" && RUSTC_WRAPPER='' cargo build --locked -p amplihack --bin amplihack)
    TOOL="${CARGO_TARGET_DIR:-$REPO_ROOT/target}/debug/amplihack"
fi
[[ -x "$TOOL" ]] || { echo "HARNESS-ERROR: real amplihack binary missing" >&2; exit 2; }
ln -s "$(cd "$(dirname "$TOOL")" && pwd)/$(basename "$TOOL")" "$RUNTIME/bin/amplihack"
for tool in bash jq dirname tr grep; do ln -s "$(command -v "$tool")" "$RUNTIME/bin/$tool"; done
run_checkpoint() {
    local name="$1" feedback="$2" framework="$3" refs="${4:-absent}"
    local -a metadata=()
    if [[ "$refs" = populated ]]; then
        metadata=(BRANCH_NAME=fix/834 PR_NUMBER=834 PR_URL=https://example.test/pr/834
                  COMMIT_SHA=abc123 REVIEW_THREAD_ID=thread834)
    fi
    # env -i removes inherited context, tokens and refs; feedback stays one argument.
    set +e
    (cd "$RUNTIME" && env -i PATH="$RUNTIME/bin" HOME="$RUNTIME/home" TMPDIR="$RUNTIME" \
        AMPLIHACK_HOME="$framework" REPO_PATH="$REPO_ROOT" \
        DOC_REVIEW_FEEDBACK="$feedback" "${metadata[@]}" \
        bash -c "$CHECKPOINT_COMMAND") >"$RUNTIME/$name.out" 2>"$RUNTIME/$name.err"
    CHILD_EXIT=$?
    set -e
}
for scenario in ok degraded absent malformed hostile; do
    case "$scenario" in
        ok) feedback='{"status":"OK","feedback":"review passed"}' ;;
        degraded) feedback='{"status":"NEEDS_ATTENTION","feedback":"repair docs"}' ;;
        absent) feedback='' ;;
        malformed) feedback='not JSON' ;;
        hostile)
            payload=$(cat <<'HOSTILE'
%s%n " ' $(/usr/bin/touch ATTACK_PATH); `/usr/bin/touch ATTACK_PATH`; eval echo HOSTILE_FEEDBACK
HOSTILE
)
            payload=${payload//ATTACK_PATH/$RUNTIME/executed}
            feedback="$(jq -nc --arg text "$payload" '{status:"NEEDS_ATTENTION",feedback:$text}')" ;;

    esac
    run_checkpoint "$scenario" "$feedback" "$REPO_ROOT"
    expected=NEEDS_ATTENTION; [[ "$scenario" = ok ]] && expected=OK
    if [[ "$CHILD_EXIT" = 0 ]] && grep -qxF "DOC_REVIEW_CHECKPOINT: $expected" "$RUNTIME/$scenario.out" \
       && ! grep -qE '    (branch|pr_number|pr_url|commit_sha|review_thread):' "$RUNTIME/$scenario.out" \
       && ! grep -qF HOSTILE_FEEDBACK "$RUNTIME/$scenario.out" \
       && ! grep -qF HOSTILE_FEEDBACK "$RUNTIME/$scenario.err" \
       && [[ ! -e "$RUNTIME/executed" && ! -e "$RUNTIME/marker" ]]; then
        if [[ "$scenario" = ok && ! -s "$RUNTIME/$scenario.err" ]] \
           || { [[ "$scenario" != ok ]] && grep -q WARNING "$RUNTIME/$scenario.err"; }; then
            pass "runtime:$scenario" "exit0, correct summary/warning, no fabricated refs or feedback execution/leak"
        else fail "runtime:$scenario" "incorrect warning channel"; fi
    else fail "runtime:$scenario" "incorrect exit, summary, references or feedback execution/leak"; fi
 done
run_checkpoint populated '{"status":"NEEDS_ATTENTION"}' "$REPO_ROOT" populated
if [[ "$CHILD_EXIT" = 0 ]] && grep -q WARNING "$RUNTIME/populated.err" \
   && grep -qxF 'DOC_REVIEW_CHECKPOINT: NEEDS_ATTENTION' "$RUNTIME/populated.out" \
   && grep -qxF '    branch: fix/834' "$RUNTIME/populated.out" \
   && grep -qxF '    pr_number: 834' "$RUNTIME/populated.out" \
   && grep -qxF '    pr_url: https://example.test/pr/834' "$RUNTIME/populated.out" \
   && grep -qxF '    commit_sha: abc123' "$RUNTIME/populated.out" \
   && grep -qxF '    review_thread: thread834' "$RUNTIME/populated.out"; then
    pass runtime:refs "degraded exit0 retains every populated durable reference"
else fail runtime:refs "degraded checkpoint lost durable references"; fi
# Authoritative wrong roots must not silently use the nearby checkout's helper.
mkdir -p "$RUNTIME/context-only/amplifier-bundle/tools"
cp "$REPO_ROOT/amplifier-bundle/tools/workflow_context.sh" "$RUNTIME/context-only/amplifier-bundle/tools/"
for framework in empty context-only; do
    run_checkpoint "missing-$framework" '{"status":"OK"}' "$RUNTIME/$framework"
    if [[ "$CHILD_EXIT" = 0 ]] && grep -qxF 'DOC_REVIEW_CHECKPOINT: NEEDS_ATTENTION' "$RUNTIME/missing-$framework.out" \
       && grep -q 'WARNING: documentation checkpoint helper is missing' "$RUNTIME/missing-$framework.err"; then
        pass "runtime:$framework" "selected missing helper degrades visibly without checkout fallback"
    else fail "runtime:$framework" "selected missing helper falsely passed or became fatal"; fi
 done

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
