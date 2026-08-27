#!/usr/bin/env bash
# test-issue-1337-loop-health-evaluator.sh — contract test for the reusable
# agentic loop-health evaluator brick (issue #1337).
#
# Motivating evidence (measured, real): a default-workflow run spent 2h47m and
# produced ZERO commits. Seven consecutive steps each reported exactly
# `10m 0s` with no artifacts, one child returned BLOCKED_TERMINAL at depth 4/3
# with exit 79, and the run itself said "The review workflow is still running;
# I'm waiting for its structured findings." Every one of those was a visible
# signal and nothing acted on any of them.
#
# The two paths most likely to be wrong and most costly if they are:
#
#   STUCK path            — the evaluator says stop, and NOTHING proceeds.
#   malformed-verdict path — unparseable / missing input is treated as STUCK,
#                            NEVER as CONTINUE. Failing safe means stopping,
#                            not looping.
#
# Also asserted:
#   - exit 79 / BLOCKED_TERMINAL is TERMINAL: surfaced, and never retried into
#     (the agent evaluation step is skipped entirely).
#   - the worked 2h47m case is detected from its evidence alone.
#   - CONTINUE and DONE still pass through, so a converging loop is not cut off.
#   - there is NO numeric iteration cap anywhere in the recipe.
#   - no seconds-scale / single-digit-minute timeout anywhere in the recipe.
#
# Usage: bash amplifier-bundle/recipes/tests/test-issue-1337-loop-health-evaluator.sh
# Exit codes: 0 = pass, 1 = fail, 2 = test harness error.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
RECIPE="${REPO_ROOT}/amplifier-bundle/recipes/loop-health-evaluator.yaml"
# The deterministic measurement half is its own brick, composed as step-01.
COLLECTOR="${REPO_ROOT}/amplifier-bundle/recipes/loop-evidence-collector.yaml"

for f in "${RECIPE}" "${COLLECTOR}"; do
    [[ -f "${f}" ]] || { echo "HARNESS-ERROR: recipe not found: ${f}" >&2; exit 2; }
done

# `amplihack` must be resolvable — the whole verdict pipeline runs through
# `orch helper`. Prefer a binary built from THIS tree over an older installed
# one that may still be first on PATH.
supports_helper() { "$1" orch helper normalise-loop-verdict </dev/null >/dev/null 2>&1; }
AMPLIHACK_BIN=""
for cand in "${REPO_ROOT}/target/release/amplihack" "${REPO_ROOT}/target/debug/amplihack" \
            "$(command -v amplihack 2>/dev/null || true)"; do
    [[ -n "${cand}" && -x "${cand}" ]] || continue
    if supports_helper "${cand}"; then AMPLIHACK_BIN="${cand}"; break; fi
done
if [[ -z "${AMPLIHACK_BIN}" ]]; then
    echo "HARNESS-ERROR: no 'amplihack' providing 'orch helper normalise-loop-verdict'." >&2
    echo "  The issue #1337 helper is not implemented, or the binary is stale." >&2
    echo "  Build it with: cargo build -p amplihack --bin amplihack" >&2
    exit 2
fi
# The extracted recipe step bodies call bare `amplihack`, so put the chosen
# binary's directory first on PATH for the whole test.
PATH="$(cd "$(dirname "${AMPLIHACK_BIN}")" && pwd):${PATH}"; export PATH

PASS_COUNT=0
FAIL_COUNT=0
pass() { PASS_COUNT=$((PASS_COUNT + 1)); echo "  PASS[$1]: $2"; }
fail() { FAIL_COUNT=$((FAIL_COUNT + 1)); echo "  FAIL[$1]: $2" >&2; }

echo "=== Issue #1337: agentic loop-health evaluator contract ==="

# Extract one step's `command: |` body out of the recipe.
extract_step_command() {
    local recipe="$1" step="$2"
    awk -v step="$step" '
        index($0, "id: \"" step "\"") { instep=1 }
        instep && $0 ~ /^    command: \|/ { incmd=1; next }
        incmd {
            if ($0 ~ /^    [a-zA-Z_]+:/ || $0 ~ /^  - id:/) { exit }
            sub(/^      /, "")
            print
        }
    ' "${recipe}"
}

COLLECT="$(extract_step_command "${COLLECTOR}" "step-01-collect-loop-evidence")"
RESOLVE="$(extract_step_command "${RECIPE}" "step-03-resolve-loop-verdict")"
ENFORCE="$(extract_step_command "${RECIPE}" "step-04-enforce-loop-verdict")"
for pair in "step-01:${COLLECT}" "step-03:${RESOLVE}" "step-04:${ENFORCE}"; do
    name="${pair%%:*}"; body="${pair#*:}"
    [[ -n "${body}" ]] || { echo "HARNESS-ERROR: could not extract ${name} command body" >&2; exit 2; }
done

# run_step <body> — run an extracted step body; env comes from the caller.
run_step() { printf '%s\n' "$1" | bash; }

# resolve_verdict <loop_evidence-json> <raw-assessment> -> canonical token on stdout
resolve_verdict() {
    LOOP_EVIDENCE="$1" LOOP_HEALTH_ASSESSMENT="$2" LOOP_NAME="t" LOOP_ROUND_LABEL="r" \
        run_step "${RESOLVE}" 2>/dev/null \
        | amplihack orch helper extract-json \
        | amplihack orch helper extract-field --field loop_verdict --default MISSING
}

CLEAN_EV='{"terminal_refusal":"false","terminal_reason":"","commits_since_baseline":3,"diff_lines":120,"repeated_output":"false"}'

# ---------------------------------------------------------------------------
# 1. STUCK path — the evaluator says stop, and nothing proceeds.
# ---------------------------------------------------------------------------
V="$(resolve_verdict "${CLEAN_EV}" '{"loop_verdict":"STUCK","not_converging":["same finding for 3 rounds"]}')"
if [[ "${V}" == "STUCK" ]]; then
    pass "STUCK-resolve" "an explicit STUCK verdict resolves to STUCK"
else
    fail "STUCK-resolve" "expected STUCK, got '${V}'"
fi

STUCK_HEALTH='{"loop_verdict":"STUCK","verdict_source":"evaluator","loop_name":"t","not_converging":["zero commits in 7 rounds"]}'
out="$(LOOP_HEALTH="${STUCK_HEALTH}" LOOP_EVIDENCE="${CLEAN_EV}" LOOP_NAME="t" LOOP_ROUND_LABEL="r" run_step "${ENFORCE}" 2>&1)"; rc=$?
if [[ ${rc} -ne 0 ]]; then
    pass "STUCK-enforce" "STUCK exits non-zero — the loop stops, nothing proceeds (rc=${rc})"
else
    fail "STUCK-enforce" "STUCK did NOT stop the loop (rc=0): ${out}"
fi
if printf '%s' "${out}" | grep -qF 'zero commits in 7 rounds'; then
    pass "STUCK-escalates" "STUCK escalates with the specific evidence of what is not converging"
else
    fail "STUCK-escalates" "STUCK did not surface the not_converging evidence: ${out}"
fi

# ---------------------------------------------------------------------------
# 2. Malformed-verdict path — unparseable input is STUCK, NEVER CONTINUE.
#    This is the fail-open bug that would let a dead loop burn the budget.
# ---------------------------------------------------------------------------
while IFS= read -r raw; do
    [[ -z "${raw}" && "${raw}" != "" ]] && continue
    V="$(resolve_verdict "${CLEAN_EV}" "${raw}")"
    label="$(printf '%.44s' "${raw:-<empty>}")"
    if [[ "${V}" == "STUCK" ]]; then
        pass "MALFORMED" "unparseable verdict [${label}] -> STUCK"
    else
        fail "MALFORMED" "unparseable verdict [${label}] -> '${V}' (must be STUCK, never CONTINUE)"
    fi
done <<'MALFORMED'

The review workflow is still running; I'm waiting for its structured findings.
{"loop_verdict":
{"loop_verdict": "MAYBE"}
{"verdict": "CONTINUE"}
{"loop_verdict": "DISCONTINUE"}
{"loop_verdict": "CANNOT_CONTINUE"}
{"loop_verdict": "NOT_DONE"}
{}
null
MALFORMED

# Empty output from the evaluator (the step never ran / produced nothing).
V="$(resolve_verdict "${CLEAN_EV}" "")"
if [[ "${V}" == "STUCK" ]]; then
    pass "MALFORMED-empty" "a missing verdict is STUCK, never CONTINUE"
else
    fail "MALFORMED-empty" "missing verdict resolved to '${V}'"
fi

# Missing evidence entirely — must not be read as "the guard did not fire".
V="$(resolve_verdict "" '{"loop_verdict":"CONTINUE"}')"
if [[ "${V}" == "STUCK" ]]; then
    pass "MALFORMED-noevidence" "unparseable evidence fails safe to STUCK even with a CONTINUE verdict"
else
    fail "MALFORMED-noevidence" "unparseable evidence resolved to '${V}'"
fi

# And the enforcement step must fail safe on garbage too.
for bad in "" "not json at all" '{"loop_verdict":"DISCONTINUE"}'; do
    out="$(LOOP_HEALTH="${bad}" LOOP_NAME="t" run_step "${ENFORCE}" 2>&1)"; rc=$?
    if [[ ${rc} -ne 0 ]]; then
        pass "MALFORMED-enforce" "enforcement on [${bad:-<empty>}] stops the loop (rc=${rc})"
    else
        fail "MALFORMED-enforce" "enforcement on [${bad:-<empty>}] allowed the loop to proceed"
    fi
done

# ---------------------------------------------------------------------------
# 3. CONTINUE / DONE still pass through — a converging loop is not cut off.
# ---------------------------------------------------------------------------
for tok in CONTINUE DONE; do
    V="$(resolve_verdict "${CLEAN_EV}" "{\"loop_verdict\":\"${tok}\"}")"
    if [[ "${V}" == "${tok}" ]]; then
        pass "PASSTHRU" "${tok} resolves to ${tok}"
    else
        fail "PASSTHRU" "${tok} resolved to '${V}'"
    fi
    out="$(LOOP_HEALTH="{\"loop_verdict\":\"${tok}\"}" LOOP_NAME="t" run_step "${ENFORCE}" 2>&1)"; rc=$?
    if [[ ${rc} -eq 0 ]]; then
        pass "PASSTHRU-enforce" "${tok} does not stop the loop (rc=0)"
    else
        fail "PASSTHRU-enforce" "${tok} wrongly stopped the loop (rc=${rc}): ${out}"
    fi
done

# ---------------------------------------------------------------------------
# 4. Exit 79 is terminal: surfaced, forced STUCK, never retried into.
# ---------------------------------------------------------------------------
EV79="$(LOOP_CHILD_EXIT_CODE=79 LOOP_REPO_PATH="${REPO_ROOT}" LOOP_NAME="t" \
    LOOP_LAST_ROUND_OUTPUT="" LOOP_HISTORY="" run_step "${COLLECT}" 2>/dev/null)"
T="$(printf '%s' "${EV79}" | amplihack orch helper extract-json \
    | amplihack orch helper extract-field --field terminal_refusal --default MISSING)"
if [[ "${T}" == "true" ]]; then
    pass "EXIT79-detect" "child exit code 79 is recorded as a terminal policy refusal"
else
    fail "EXIT79-detect" "exit 79 not detected (terminal_refusal='${T}'): ${EV79}"
fi

EVBT="$(LOOP_REPO_PATH="${REPO_ROOT}" LOOP_NAME="t" LOOP_HISTORY="" \
    LOOP_LAST_ROUND_OUTPUT="child returned BLOCKED_TERMINAL at depth 4/3" \
    run_step "${COLLECT}" 2>/dev/null)"
T="$(printf '%s' "${EVBT}" | amplihack orch helper extract-json \
    | amplihack orch helper extract-field --field terminal_refusal --default MISSING)"
if [[ "${T}" == "true" ]]; then
    pass "EXIT79-blocked-terminal" "BLOCKED_TERMINAL in round output is a terminal policy refusal"
else
    fail "EXIT79-blocked-terminal" "BLOCKED_TERMINAL not detected (terminal_refusal='${T}')"
fi

# A terminal refusal forces STUCK even when the model said CONTINUE.
V="$(resolve_verdict "${EV79}" '{"loop_verdict":"CONTINUE"}')"
if [[ "${V}" == "STUCK" ]]; then
    pass "EXIT79-terminal" "exit 79 forces STUCK even against a CONTINUE verdict — never retried into the guard"
else
    fail "EXIT79-terminal" "exit 79 did not force STUCK (got '${V}')"
fi

# The agent evaluation step must be gated off on a terminal refusal, so no
# model call is spent deciding whether to re-enter a sealed guard.
if grep -q "condition: \"loop_evidence.terminal_refusal == 'false'\"" "${RECIPE}"; then
    pass "EXIT79-skips-agent" "the evaluator agent step is skipped on a terminal policy refusal"
else
    fail "EXIT79-skips-agent" "the agent step is not gated on loop_evidence.terminal_refusal"
fi

# ---------------------------------------------------------------------------
# 5. The worked example: 2h47m, zero commits, seven identical `10m 0s` steps,
#    a BLOCKED_TERMINAL child, and "waiting for its structured findings".
#    Every one of those must be VISIBLE in the collected evidence.
# ---------------------------------------------------------------------------
WORKED_HISTORY="$(printf 'step-%d completed in 10m 0s (no artifacts produced)\n' 1 2 3 4 5 6 7)"
WORKED_LAST="The review workflow is still running; I'm waiting for its structured findings."
EVW="$(LOOP_REPO_PATH="${REPO_ROOT}" LOOP_NAME="default-workflow" LOOP_ROUND_LABEL="2h47m" \
    LOOP_HISTORY="${WORKED_HISTORY}" LOOP_LAST_ROUND_OUTPUT="${WORKED_LAST}" \
    LOOP_BASELINE_REF="HEAD" \
    LOOP_FINDINGS_CURRENT=$'F1\nF2' LOOP_FINDINGS_PREVIOUS=$'F1\nF2' \
    LOOP_TEST_SIGNAL="19 passed" LOOP_TEST_SIGNAL_PREVIOUS="19 passed" \
    run_step "${COLLECT}" 2>/dev/null)"

field() {
    printf '%s' "${EVW}" | amplihack orch helper extract-json \
        | amplihack orch helper extract-field --field "$1" --default MISSING
}

check_field() {
    local f="$1" want="$2" desc="$3" got
    got="$(field "${f}")"
    if [[ "${got}" == "${want}" ]]; then
        pass "WORKED" "${desc} (${f}=${got})"
    else
        fail "WORKED" "${desc} — ${f} was '${got}', expected '${want}'. Evidence: ${EVW}"
    fi
}

check_field "repeated_duration_count" "7"     "seven identical '10m 0s' step durations are counted, not ignored"
check_field "repeated_duration_value" "10m 0s" "the repeating duration value itself is surfaced"
check_field "commits_since_baseline" "0"       "zero commits produced by the round is recorded"
check_field "waiting_on_output" "true"         "'waiting for its structured findings' is recorded"
check_field "findings_recurring" "2"           "recurring findings are counted"
check_field "findings_resolved" "0"            "nothing was resolved"
check_field "tests_moved" "false"              "the test signal did not move"

# Same evidence, but with a BLOCKED_TERMINAL child in the history.
EVW79="$(LOOP_REPO_PATH="${REPO_ROOT}" LOOP_NAME="default-workflow" \
    LOOP_HISTORY="${WORKED_HISTORY}
child BLOCKED_TERMINAL at depth 4/3, exit 79" \
    LOOP_LAST_ROUND_OUTPUT="${WORKED_LAST}" run_step "${COLLECT}" 2>/dev/null)"
V="$(resolve_verdict "${EVW79}" "")"
if [[ "${V}" == "STUCK" ]]; then
    pass "WORKED-verdict" "the full 2h47m worked example resolves to STUCK"
else
    fail "WORKED-verdict" "the worked example resolved to '${V}'"
fi

# ---------------------------------------------------------------------------
# 6. Design constraints the issue is explicit about.
# ---------------------------------------------------------------------------
if grep -nEi '(max_iterations|max_iteration|max_rounds|max_attempts|iteration_limit|MAX_LOOPS)' "${RECIPE}" "${COLLECTOR}" \
   | grep -vi 'not an iteration counter' | grep -q .; then
    fail "NO-CAP" "the recipe introduces a numeric iteration cap — issue #1337 rejects this outright"
else
    pass "NO-CAP" "no numeric iteration cap: the terminator is absence of progress, not attempt count"
fi

if grep -nE '^\s*timeout(_seconds)?:' "${RECIPE}" "${COLLECTOR}" | grep -q .; then
    fail "NO-SHORT-TIMEOUT" "the recipe declares a per-step timeout — see issue #439"
else
    pass "NO-SHORT-TIMEOUT" "no per-step timeout anywhere: nothing is bounded at seconds or single-digit-minute scale"
fi

for tok in CONTINUE DONE STUCK; do
    if grep -qF "\"${tok}\"" "${RECIPE}"; then
        pass "THREE-OUTCOMES" "${tok} is part of the documented verdict contract"
    else
        fail "THREE-OUTCOMES" "${tok} missing from the recipe's verdict contract"
    fi
done


# ---------------------------------------------------------------------------
# 7. END-TO-END through the real recipe-runner-rs.
#
# Everything above extracts the step bodies faithfully and then supplies the
# environment BY HAND. That is exactly how issue #1337's first cut shipped a
# recipe that could never work: `loop_evidence` / `loop_health` declare
# parse_json, so the runner stores them as JSON objects and creates only
# `RECIPE_VAR_<name>` — the plain `LOOP_EVIDENCE` / `LOOP_HEALTH` names the
# steps read did not exist at runtime. Every run returned STUCK and fabricated
# an exit-79 policy refusal as the reason. 39 green assertions, an inert brick.
#
# So: run the ACTUAL recipe files through the ACTUAL runner, with only the
# agentic step swapped for a bash step that prints a fixed evaluator output.
# Nothing else is stubbed, and no environment is invented.
# ---------------------------------------------------------------------------
RUNNER="${RECIPE_RUNNER_RS_PATH:-$(command -v recipe-runner-rs 2>/dev/null || true)}"
if [[ -z "${RUNNER}" || ! -x "${RUNNER}" ]]; then
    echo "  SKIP[E2E]: recipe-runner-rs not found (set RECIPE_RUNNER_RS_PATH to run the" >&2
    echo "             end-to-end probe; it is the ONLY check that catches an inert recipe)." >&2
    SKIPPED_E2E=1
else
    SKIPPED_E2E=0
    E2E_DIR="$(mktemp -d)"
    trap 'rm -rf "${E2E_DIR}"' EXIT
    cp "${REPO_ROOT}/amplifier-bundle/recipes/loop-evidence-collector.yaml" "${E2E_DIR}/"

    # Replace ONLY the `step-02-evaluate-loop-health` node with a bash step that
    # cats a fixed evaluator output. Every other byte of the recipe is the
    # shipped file.
    make_stubbed_recipe() {
        local out="$1" payload="$2"
        awk -v payload="${payload}" '
            /^  - id: "step-02-evaluate-loop-health"/ {
                print
                print "    condition: \"loop_evidence.terminal_refusal == '"'"'false'"'"'\""
                print "    type: \"bash\""
                print "    command: |"
                print "      cat " payload
                print "    output: \"loop_health_assessment\""
                skip = 1
                next
            }
            skip && /^  - id: / { skip = 0 }
            !skip { print }
        ' "${RECIPE}" > "${out}"
        grep -q 'step-02-evaluate-loop-health' "${out}" || return 1
        grep -q 'cat ' "${out}" || return 1
    }

    # e2e_run <name> <evaluator-output> [extra -c args...] -> sets E2E_RC/E2E_OUT
    e2e_run() {
        local name="$1" payload_text="$2"; shift 2
        local payload="${E2E_DIR}/${name}.out"
        local stubbed="${E2E_DIR}/${name}-loop-health-evaluator.yaml"
        printf '%s\n' "${payload_text}" > "${payload}"
        make_stubbed_recipe "${stubbed}" "${payload}" || {
            echo "HARNESS-ERROR: could not stub step-02 for ${name}" >&2; exit 2; }
        E2E_OUT="$("${RUNNER}" "${stubbed}" \
            -R "${E2E_DIR}" -C "${REPO_ROOT}" \
            -c loop_name="e2e-${name}" -c loop_repo_path="${REPO_ROOT}" \
            --output-format json "$@" 2>&1)"
        E2E_RC=$?
    }

    # --- 7a. CONTINUE reaches step-04 and exits 0 (the B1 regression) --------
    e2e_run continue '{"loop_verdict":"CONTINUE","moved":["3 commits"]}'
    if [[ ${E2E_RC} -eq 0 ]] && printf '%s' "${E2E_OUT}" | grep -qF 'LOOP_HEALTH: CONTINUE'; then
        pass "E2E-continue" "a CONTINUE verdict survives the real runner and exits 0"
    else
        fail "E2E-continue" "CONTINUE did not reach step-04 (rc=${E2E_RC}):
${E2E_OUT}"
    fi
    # The exact signature of the inert-recipe bug: the reads miss, the
    # `--default true` fail-safe fires, and an exit-79 refusal that never
    # happened is reported as the reason.
    if printf '%s' "${E2E_OUT}" | grep -qF 'terminal_policy_refusal'; then
        fail "E2E-no-fabricated-refusal" "the run fabricated a terminal policy refusal:
${E2E_OUT}"
    else
        pass "E2E-no-fabricated-refusal" "no exit-79 refusal is invented when none occurred"
    fi
    if printf '%s' "${E2E_OUT}" | grep -q '"verdict_source": *"evaluator"'; then
        pass "E2E-verdict-source" "the verdict is attributed to the evaluator, not to a failed read"
    else
        fail "E2E-verdict-source" "verdict_source is not 'evaluator':
${E2E_OUT}"
    fi

    # --- 7b. STUCK stops the loop, end to end -------------------------------
    e2e_run stuck '{"loop_verdict":"STUCK","not_converging":["zero commits in 7 rounds"]}'
    if [[ ${E2E_RC} -ne 0 ]] && printf '%s' "${E2E_OUT}" | grep -qF 'LOOP_HEALTH: STUCK'; then
        pass "E2E-stuck" "a STUCK verdict fails the recipe end to end (rc=${E2E_RC})"
    else
        fail "E2E-stuck" "STUCK did not stop the run (rc=${E2E_RC}):
${E2E_OUT}"
    fi

    # --- 7c. The verdict pipeline is last-object-wins, not first-JSON-wins ---
    # Measured: first-JSON-wins let the sentence that should stop the loop
    # authorise it.
    e2e_run reconsidered '{"plan":"check","loop_verdict":"CONTINUE"}
On reflection nothing moved.
{"loop_verdict":"STUCK","not_converging":["zero commits"]}'
    if [[ ${E2E_RC} -ne 0 ]]; then
        pass "E2E-reconsidered" "a reconsidered STUCK after a draft CONTINUE stops the loop"
    else
        fail "E2E-reconsidered" "the draft CONTINUE won over the reconsidered STUCK (rc=0):
${E2E_OUT}"
    fi

    # The mirror: evidence quoted back inside a ```json fence must not be read
    # as the verdict and kill a converging loop.
    e2e_run quoted 'Here is the evidence I was given:
```json
{"commits_since_baseline": 3, "diff_lines": 120}
```
It moved. Verdict:
{"loop_verdict": "CONTINUE", "moved": ["3 commits"]}'
    if [[ ${E2E_RC} -eq 0 ]] && printf '%s' "${E2E_OUT}" | grep -qF 'LOOP_HEALTH: CONTINUE'; then
        pass "E2E-quoted-evidence" "evidence quoted back in a fence is ignored; the real verdict wins"
    else
        fail "E2E-quoted-evidence" "quoted evidence was read as the verdict (rc=${E2E_RC}):
${E2E_OUT}"
    fi

    # --- 7d. A real exit-79 refusal is still terminal, end to end -----------
    e2e_run terminal '{"loop_verdict":"CONTINUE"}' -c loop_child_exit_code=79
    if [[ ${E2E_RC} -ne 0 ]] && printf '%s' "${E2E_OUT}" | grep -qF 'terminal_policy_refusal'; then
        pass "E2E-terminal" "a genuine exit-79 refusal still forces STUCK and stops the run"
    else
        fail "E2E-terminal" "exit 79 was not terminal end to end (rc=${E2E_RC}):
${E2E_OUT}"
    fi

    # --- 7e. The brick does not poison itself -------------------------------
    # Feed a previous escalation report back in as loop_history. Its own reason
    # string says "exited 79" and would re-match the detector, making the loop
    # permanently terminal on evidence it invented.
    SELF_REPORT="LOOP_HEALTH: STUCK — 'e2e' is not converging (source=terminal_policy_refusal). [loop-health-evaluator]
  Evidence: {\"terminal_refusal\":\"true\",\"terminal_reason\":\"child process exited 79 (policy refusal, #1327/#1332) [loop-health-evaluator]\"}"
    e2e_run selfpoison '{"loop_verdict":"CONTINUE","moved":["3 commits"]}' \
        -c loop_history="${SELF_REPORT}"
    if [[ ${E2E_RC} -eq 0 ]] && printf '%s' "${E2E_OUT}" | grep -qF 'LOOP_HEALTH: CONTINUE'; then
        pass "E2E-self-poison" "this brick's own escalation report does not re-trigger its exit-79 detector"
    else
        fail "E2E-self-poison" "the brick poisoned itself from its own report (rc=${E2E_RC}):
${E2E_OUT}"
    fi
fi

echo ""
echo "--- Summary: ${PASS_COUNT} passed, ${FAIL_COUNT} failed ---"
if [[ ${SKIPPED_E2E:-0} -eq 1 ]]; then
    echo "--- NOTE: the end-to-end runner probe was SKIPPED. The contract above is"
    echo "---       asserted against hand-supplied environment only."
fi
if [[ ${FAIL_COUNT} -gt 0 ]]; then exit 1; fi
echo "PASS: Issue #1337 — loop-health evaluation stops a stuck loop, fails safe to STUCK on malformed input, and never retries into an exit-79 refusal."
exit 0
