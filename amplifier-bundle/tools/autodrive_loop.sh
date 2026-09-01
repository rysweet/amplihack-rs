#!/usr/bin/env bash
# autodrive_loop.sh — the agentic loop driver for auto-drive-to-merge.
#
# Runs one round recipe over and over until an AGENTIC evaluator says to stop.
# There is no iteration cap in this file — not a `max_rounds`, not a backstop
# integer, not a wall-clock budget. `ROUND` below is a LABEL that appears in
# reports; it is never compared against a limit and no branch reads it.
#
# The terminator is `loop-health-evaluator` (issue #1337): after every round it
# looks at measured evidence — what the round actually produced, whether the
# same findings keep recurring, whether test/CI signals moved — and answers
# CONTINUE / DONE / STUCK. Absence of progress stops the loop; number of
# attempts does not.
#
# Host safety is NOT this file's job and never was. It is enforced structurally
# one layer down by #1327 (sealed recursion ceiling) and #1332 (width cap +
# free-memory floor), both of which refuse with exit code 79 before a child
# runs. Rounds here are SEQUENTIAL AT CONSTANT DEPTH — each child inherits the
# same AMPLIHACK_SESSION_DEPTH — so a long loop never walks toward that
# ceiling and the ceiling is never raised to accommodate one.
#
#   Exit 0  — the loop converged (DONE) and the round itself verified clean.
#   Exit 1  — STUCK, a malformed/missing verdict, or an inconsistent round.
#             Nothing advances. The escalation names what is not converging.
#   Exit 79 — a child returned the terminal policy refusal. Surfaced, final,
#             and NEVER retried into.
#
# NOT EVERY FAILURE IS TERMINAL (issue #1437). Exit 79 belongs to a structural
# guard that DECLINED to run the work and will decline again. An agent whose API
# session dies, a failing check, a missing binary — those are ordinary round
# failures with their own classes, and the evaluator decides what to do about
# them. The two are told apart by the child's exit status and by the structured
# `amplihack.recipe.failure_class` classification of the step that ended the
# run — never by grepping a round transcript for a word, because a transcript
# contains every file the agents read and every refusal they already handled.
#
# Policy: this workflow NEVER passes a hook-skipping commit flag or any
# branch-protection bypass. See
# docs/reference/auto-drive-to-merge.md#two-absolute-prohibitions.

set -uo pipefail

AUTODRIVE_EXIT_POLICY_REFUSAL=79

LOOP_NAME=""; ROUND_RECIPE=""; CLEAN_TOKEN=""; VERDICT_FIELD=""
REPO="."; STATE_DIR=""; ROUND_CTX=()

while [ $# -gt 0 ]; do
  case "$1" in
    --loop-name)     LOOP_NAME="${2:-}"; shift 2 ;;
    --round-recipe)  ROUND_RECIPE="${2:-}"; shift 2 ;;
    --clean-token)   CLEAN_TOKEN="${2:-}"; shift 2 ;;
    --verdict-field) VERDICT_FIELD="${2:-}"; shift 2 ;;
    --repo)          REPO="${2:-}"; shift 2 ;;
    --state-dir)     STATE_DIR="${2:-}"; shift 2 ;;
    --context)       ROUND_CTX+=("-c" "${2:-}"); shift 2 ;;
    *) echo "ERROR: autodrive_loop.sh: unknown argument '$1'" >&2; exit 2 ;;
  esac
done
for req in LOOP_NAME ROUND_RECIPE CLEAN_TOKEN VERDICT_FIELD STATE_DIR; do
  [ -n "${!req}" ] || { echo "ERROR: autodrive_loop.sh: --${req,,} is required" >&2; exit 2; }
done
mkdir -p "$STATE_DIR" || exit 2

AMPLIHACK_BIN="${AMPLIHACK_BIN:-amplihack}"
export GIT_PAGER=cat GH_PAGER=cat PAGER=cat LESS=FRX

# --- recursion context: propagate, never raise -----------------------------
# AMPLIHACK_MAX_DEPTH is a ceiling handed down by the host. This loop passes it
# through untouched. Raising it is how a run walks into a sealed guard and gets
# refused with 79; that is the behaviour #1327 exists to stop.
AUTODRIVE_INHERITED_MAX_DEPTH="${AMPLIHACK_MAX_DEPTH:-}"
export AMPLIHACK_TREE_ID="${AMPLIHACK_TREE_ID:-}"
export AMPLIHACK_SESSION_DEPTH="${AMPLIHACK_SESSION_DEPTH:-0}"
[ -n "$AUTODRIVE_INHERITED_MAX_DEPTH" ] && export AMPLIHACK_MAX_DEPTH="$AUTODRIVE_INHERITED_MAX_DEPTH"

assert_ceiling_untouched() {
  if [ "${AMPLIHACK_MAX_DEPTH:-}" != "$AUTODRIVE_INHERITED_MAX_DEPTH" ]; then
    echo "ERROR: AMPLIHACK_MAX_DEPTH changed from '${AUTODRIVE_INHERITED_MAX_DEPTH}' to '${AMPLIHACK_MAX_DEPTH:-}' inside the loop. The ceiling is never raised." >&2
    exit 1
  fi
}

# Read one field out of a round record or an agent's output.
#
# `--require-field` (issue #1337, PR #1347) is NOT optional here: plain
# `extract-json` returns the FIRST parseable object and prefers a ```json fence
# over raw prose, so a model that quotes an example or drafts a verdict before
# reconsidering has its FIRST object read instead of its last. Requiring the
# field and taking the LAST object that carries it agrees with the prompt's
# "as the very last thing you emit", and returns nothing — so the blocking
# `--default` applies — when no object carries the field at all.
field() { # field <json> <name> <default>
  printf '%s' "${1:-}" \
    | "$AMPLIHACK_BIN" orch helper extract-json --require-field "$2" \
    | "$AMPLIHACK_BIN" orch helper extract-field --field "$2" --default "$3"
}

# --- did THIS child refuse, or did it merely quote a refusal? ---------------
#
# Issue #1437. A round log is a TRANSCRIPT, not a verdict. It carries every
# byte the round's agents read, printed and recovered from — including a
# structural refusal handed to a DESCENDANT, which the agent then handled
# exactly as the refusal's own text instructs ("complete this step inline and
# return your result"). Grepping that transcript for `BLOCKED_TERMINAL` read an
# already-handled refusal as a refusal of the round: an agent that had done the
# work, committed and pushed, and then lost its API session, was reported as
# "exit 79 terminal policy refusal" and the whole run stopped permanently.
#
# Only two things speak for THIS child:
#
#   * its EXIT STATUS. 79 is the guard's own answer and nothing else emits it
#     (`terminal_block` in crates/amplihack-cli/.../recipe/run/execute.rs).
#   * the STRUCTURED classification `amplihack recipe run` prints for its own
#     run — `amplihack.recipe.failure_class {...}` — whose `structural_refusal`
#     boolean is computed from the FAILED STEP's evidence by the layer that
#     knows, not from the transcript at large.
#
# Everything else in the log is history. `TERMINAL_WHY` / `CHILD_FAILURE_CLASS`
# are set here so the verdict can NAME the cause; "stopped" with no reason is
# what made this expensive to diagnose.
FAILURE_CLASS_MARKER='amplihack.recipe.failure_class '
TERMINAL_WHY=""
CHILD_FAILURE_CLASS=""
CHILD_FAILURE_SIGNAL=""
CHILD_STRUCTURAL_REFUSAL=""

classify_child() { # classify_child <log_file>
  CHILD_FAILURE_CLASS=""; CHILD_FAILURE_SIGNAL=""; CHILD_STRUCTURAL_REFUSAL=""
  local log="${1:-}" marker=""
  [ -n "$log" ] && [ -f "$log" ] || return 0
  # The LAST marker: an earlier attempt may have been retried, and it is the
  # final classification that describes how the run actually ended.
  marker="$(grep -a "^${FAILURE_CLASS_MARKER}" "$log" 2>/dev/null | tail -n 1 || true)"
  [ -n "$marker" ] || return 0
  marker="${marker#"$FAILURE_CLASS_MARKER"}"
  CHILD_FAILURE_CLASS="$(field "$marker" class "")"
  CHILD_FAILURE_SIGNAL="$(field "$marker" signal "")"
  CHILD_STRUCTURAL_REFUSAL="$(field "$marker" structural_refusal "")"
}

terminal_refusal() { # terminal_refusal <exit_code> <log_file>
  TERMINAL_WHY=""
  classify_child "${2:-}"
  if [ "${1:-0}" = "$AUTODRIVE_EXIT_POLICY_REFUSAL" ]; then
    TERMINAL_WHY="the child exited ${AUTODRIVE_EXIT_POLICY_REFUSAL}: a structural guard refused before the work ran (#1327/#1332)"
    return 0
  fi
  if [ "$CHILD_STRUCTURAL_REFUSAL" = "true" ]; then
    TERMINAL_WHY="the child's own failure classification reports structural_refusal=true (class '${CHILD_FAILURE_CLASS:-unknown}', signal '${CHILD_FAILURE_SIGNAL:-unknown}'): the step that ended the run was refused by a structural guard"
    return 0
  fi
  return 1
}

# One line naming what the child's exit actually was, for reports and verdicts.
child_failure_summary() { # child_failure_summary <exit_code>
  local rc="${1:-0}" sig=""
  if [ "$rc" = "0" ]; then printf 'exit 0'; return 0; fi
  case "${CHILD_FAILURE_SIGNAL:-}" in ''|null) sig="" ;; *) sig=" (signal ${CHILD_FAILURE_SIGNAL})" ;; esac
  printf 'exit %s, classified %s%s' "$rc" "${CHILD_FAILURE_CLASS:-unclassified}" "$sig"
}

escalate() { # escalate <verdict> <why>
  echo "AUTO_DRIVE_LOOP: ${1} — loop '${LOOP_NAME}' stops at round label '${ROUND_LABEL:-?}'." >&2
  echo "  Reason: ${2}" >&2
  echo "  Round record: ${RECORD:-<none>}" >&2
  echo "  This is a judgement about absence of progress, not an attempt count." >&2
  printf '{"loop":"%s","loop_result":"%s","round_label":"%s","reason":"%s"}\n' \
    "$LOOP_NAME" "$1" "${ROUND_LABEL:-?}" "$(printf '%s' "$2" | tr -d '"' | tr '\n' ' ')"
}

# --- preflight: the terminator must resolve BEFORE round 1 ------------------
# Both loops delegate termination to `loop-health-evaluator`, invoked by name.
# When that recipe cannot be resolved, the first round still runs in full — a
# crusty review, a builder fix pass, commits pushed — and only then dies with
# "returned STUCK (or an unreadable verdict)", which names the loop as the
# problem instead of the missing dependency. Resolve it first and refuse by
# name, before a single round is spent.
if ! "$AMPLIHACK_BIN" recipe show loop-health-evaluator >/dev/null 2>&1; then
  echo "ERROR: loop '${LOOP_NAME}' cannot start: the required recipe 'loop-health-evaluator' does not resolve." >&2
  echo "  It is this loop's ONLY terminator (issue #1337). Without it a full round runs first — a review, a fix pass, commits pushed — and the loop then stops on an unreadable verdict that blames the loop rather than the missing dependency." >&2
  echo "  Install or update the amplifier bundle so 'loop-health-evaluator' resolves, then re-run. Check with: ${AMPLIHACK_BIN} recipe show loop-health-evaluator" >&2
  printf '{"loop":"%s","loop_result":"MISSING_DEPENDENCY","round_label":"preflight","reason":"the required recipe loop-health-evaluator does not resolve"}\n' \
    "$LOOP_NAME"
  exit 1
fi

ROUND=0
ROUND_LABEL=""
HISTORY=""
PREV_FINDINGS=""
PREV_TEST=""
PREV_CI=""

while :; do
  assert_ceiling_untouched
  # ROUND is a LABEL for reports. Nothing below compares it to a limit.
  ROUND=$((ROUND + 1))
  ROUND_LABEL="round-${ROUND}"
  RECORD="${STATE_DIR}/${LOOP_NAME}-${ROUND_LABEL}.json"
  LOG="${STATE_DIR}/${LOOP_NAME}-${ROUND_LABEL}.log"
  BASELINE="$(git -C "$REPO" rev-parse HEAD 2>/dev/null || printf '')"

  echo "=== auto-drive loop '${LOOP_NAME}': ${ROUND_LABEL} ===" >&2
  "$AMPLIHACK_BIN" recipe run "$ROUND_RECIPE" \
      --working-dir "$REPO" \
      -c "autodrive_round_record=${RECORD}" \
      -c "autodrive_round_label=${ROUND_LABEL}" \
      -c "autodrive_resolved_concerns_file=${STATE_DIR}/resolved-concerns.txt" \
      "${ROUND_CTX[@]}" >"$LOG" 2>&1
  ROUND_RC=$?
  tail -n 200 "$LOG" >&2 || true

  if terminal_refusal "$ROUND_RC" "$LOG"; then
    echo "ERROR: terminal policy refusal from '${ROUND_RECIPE}' (#1327/#1332): ${TERMINAL_WHY}. Final: the guard is never retried into." >&2
    escalate "TERMINAL_POLICY_REFUSAL" "${TERMINAL_WHY}"
    exit "$AUTODRIVE_EXIT_POLICY_REFUSAL"
  fi
  ROUND_FAILURE="$(child_failure_summary "$ROUND_RC")"
  if [ "$ROUND_RC" -ne 0 ]; then
    # NOT terminal. An ordinary round failure — an agent/API rejection, a
    # failing check, an environment problem — is exactly what the agentic
    # terminator below exists to judge. Naming the class here is what stops the
    # next reader from having to reconstruct it from a 40 MB transcript (#1437).
    echo "INFO: ${ROUND_LABEL} of '${ROUND_RECIPE}' failed: ${ROUND_FAILURE}. No structural guard refused it, so this is NOT terminal; loop-health-evaluator decides whether another round is worthwhile." >&2
  fi

  # --- read the round's STRUCTURED record -----------------------------------
  # A missing or unparseable record is never read as a clean round. It becomes
  # evidence of a round that produced nothing and is handed to the evaluator.
  RAW=""
  [ -f "$RECORD" ] && RAW="$(cat "$RECORD")"
  ROUND_VERDICT="$(field "$RAW" "$VERDICT_FIELD" "MISSING")"
  case "$ROUND_VERDICT" in
    "$CLEAN_TOKEN") ROUND_CLEAN="true" ;;
    MISSING) ROUND_CLEAN="false"
      echo "WARNING: ${ROUND_LABEL} produced no parseable '${VERDICT_FIELD}'; treated as NOT clean." >&2 ;;
    *) ROUND_CLEAN="false" ;;
  esac
  FINDINGS=""
  [ -f "${RECORD}.findings" ] && FINDINGS="$(cat "${RECORD}.findings")"
  # Stable path to the most recent round record, so a later gate can bind its
  # evidence to the round that actually produced it without guessing a name.
  [ -f "$RECORD" ] && cp -f "$RECORD" "${STATE_DIR}/${LOOP_NAME}-latest.json"
  [ -f "${RECORD}.findings" ] && cp -f "${RECORD}.findings" "${STATE_DIR}/${LOOP_NAME}-latest.json.findings"
  TEST_SIGNAL="$(field "$RAW" test_signal "")"
  CI_SIGNAL="$(field "$RAW" ci_signal "")"
  # The failure CLASS travels with the history: the evaluator judging whether
  # to continue must be able to tell "the agent's API session died" from "the
  # tests fail again", and so must the operator reading the escalation (#1437).
  HISTORY="${HISTORY}
${ROUND_LABEL}: ${VERDICT_FIELD}=${ROUND_VERDICT} rc=${ROUND_RC} child=${ROUND_FAILURE} findings=$(printf '%s' "$FINDINGS" | grep -c . || true)"

  # --- the agentic terminator ----------------------------------------------
  assert_ceiling_untouched
  HEALTH_LOG="${STATE_DIR}/${LOOP_NAME}-${ROUND_LABEL}-health.log"
  "$AMPLIHACK_BIN" recipe run loop-health-evaluator \
      --working-dir "$REPO" \
      -c "loop_name=auto-drive:${LOOP_NAME}" \
      -c "loop_round_label=${ROUND_LABEL}" \
      -c "loop_history=${HISTORY}" \
      -c "loop_last_round_output=$(tail -c 20000 "$LOG" 2>/dev/null || true)" \
      -c "loop_repo_path=${REPO}" \
      -c "loop_baseline_ref=${BASELINE}" \
      -c "loop_child_exit_code=${ROUND_RC}" \
      -c "loop_findings_current=${FINDINGS}" \
      -c "loop_findings_previous=${PREV_FINDINGS}" \
      -c "loop_test_signal=${TEST_SIGNAL}" \
      -c "loop_test_signal_previous=${PREV_TEST}" \
      -c "loop_ci_signal=${CI_SIGNAL}" \
      -c "loop_ci_signal_previous=${PREV_CI}" \
      >"$HEALTH_LOG" 2>&1
  HEALTH_RC=$?
  tail -n 120 "$HEALTH_LOG" >&2 || true

  if terminal_refusal "$HEALTH_RC" "$HEALTH_LOG"; then
    echo "ERROR: terminal policy refusal from loop-health-evaluator: ${TERMINAL_WHY}. Final; not retried." >&2
    escalate "TERMINAL_POLICY_REFUSAL" "loop-health-evaluator: ${TERMINAL_WHY}"
    exit "$AUTODRIVE_EXIT_POLICY_REFUSAL"
  fi
  if [ "$HEALTH_RC" -ne 0 ]; then
    echo "INFO: loop-health-evaluator for ${ROUND_LABEL} failed: $(child_failure_summary "$HEALTH_RC"). Not a structural refusal; the verdict below fails safe to STUCK." >&2
  fi

  # The evaluator's own enforcing step prints exactly one LOOP_HEALTH: marker
  # and exits non-zero on STUCK. Anything we cannot read as CONTINUE or DONE
  # is STUCK — a missing verdict never authorises another round.
  LOOP_VERDICT="STUCK"
  if [ "$HEALTH_RC" -eq 0 ]; then
    if grep -qE '^LOOP_HEALTH: CONTINUE( |$)' "$HEALTH_LOG"; then
      LOOP_VERDICT="CONTINUE"
    elif grep -qE '^LOOP_HEALTH: DONE( |$)' "$HEALTH_LOG"; then
      LOOP_VERDICT="DONE"
    else
      echo "WARNING: loop-health-evaluator exited 0 with no readable LOOP_HEALTH verdict; failing safe to STUCK." >&2
    fi
  fi

  PREV_FINDINGS="$FINDINGS"; PREV_TEST="$TEST_SIGNAL"; PREV_CI="$CI_SIGNAL"

  # --- decide, on BOTH signals ---------------------------------------------
  # Advancing needs the round's own machine-checked verdict AND the evaluator's
  # DONE. A model saying "looks good" over unresolved findings advances nothing.
  case "$LOOP_VERDICT" in
    DONE)
      if [ "$ROUND_CLEAN" = "true" ]; then
        echo "AUTO_DRIVE_LOOP: DONE — loop '${LOOP_NAME}' converged at ${ROUND_LABEL} with ${VERDICT_FIELD}=${ROUND_VERDICT}." >&2
        printf '{"loop":"%s","loop_result":"DONE","round_label":"%s","round_verdict":"%s"}\n' \
          "$LOOP_NAME" "$ROUND_LABEL" "$ROUND_VERDICT"
        exit 0
      fi
      escalate "STUCK" "loop-health said DONE while the round verdict is '${ROUND_VERDICT}', not '${CLEAN_TOKEN}'. An inconsistent pair never advances a phase."
      exit 1
      ;;
    CONTINUE)
      if [ "$ROUND_CLEAN" = "true" ]; then
        echo "INFO: ${ROUND_LABEL} is ${CLEAN_TOKEN} but the evaluator wants another round; confirming rather than advancing." >&2
      fi
      continue
      ;;
    *)
      escalate "STUCK" "loop-health-evaluator returned STUCK (or an unreadable verdict) for '${LOOP_NAME}'; the last round ended ${ROUND_FAILURE}"
      exit 1
      ;;
  esac
done
