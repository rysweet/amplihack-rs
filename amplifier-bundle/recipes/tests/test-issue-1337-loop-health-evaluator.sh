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

[[ -f "${RECIPE}" ]] || { echo "HARNESS-ERROR: recipe not found: ${RECIPE}" >&2; exit 2; }

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

COLLECT="$(extract_step_command "${RECIPE}" "step-01-collect-loop-evidence")"
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
if grep -nEi '(max_iterations|max_iteration|max_rounds|max_attempts|iteration_limit|MAX_LOOPS)' "${RECIPE}" \
   | grep -vi 'not an iteration counter' | grep -q .; then
    fail "NO-CAP" "the recipe introduces a numeric iteration cap — issue #1337 rejects this outright"
else
    pass "NO-CAP" "no numeric iteration cap: the terminator is absence of progress, not attempt count"
fi

if grep -nE '^\s*timeout(_seconds)?:' "${RECIPE}" | grep -q .; then
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

echo ""
echo "--- Summary: ${PASS_COUNT} passed, ${FAIL_COUNT} failed ---"
if [[ ${FAIL_COUNT} -gt 0 ]]; then exit 1; fi
echo "PASS: Issue #1337 — loop-health evaluation stops a stuck loop, fails safe to STUCK on malformed input, and never retries into an exit-79 refusal."
exit 0
