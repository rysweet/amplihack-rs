#!/usr/bin/env bash
# autodrive_loop.sh — the agentic loop driver for auto-drive-to-merge.
#
# Runs one round recipe over and over until an AGENTIC evaluator says to stop.
# There is no iteration cap in this file — not a `max_rounds`, not a backstop
# integer. `ROUND` below is a LABEL for reports and file names; it is never
# compared against a limit. It starts after this loop's earlier rounds in the
# state dir, so a second loop there or a resumed run overwrites nothing.
# Two stops are decided in code (#1743): a non-clean round that left the PR
# head, open items and CI signal unchanged (autodrive_progress.sh, STUCK), and the
# operator's resumable usage budget (autodrive_budget.sh, BUDGET_EXHAUSTED),
# a cost ceiling and never the convergence terminator; budget keys arrive as
# --context autodrive_budget_*=N and never reach a round recipe.
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
#   Exit 1  — evaluator STUCK (or unreadable), a missing round record under
#             DONE, a repeated DONE over an unchanged non-clean round (#1597),
#             a child that failed on a usage limit (USAGE_LIMIT, #1801), or a
#             stop decided in code (STUCK, BUDGET_EXHAUSTED; #1743).
#   Exit 79 — a child returned the terminal policy refusal. Surfaced, final,
#             and NEVER retried into.
#
# --before-round <script> (optional) runs `bash <script> --repo <repo> --review-dir
# <dir> --state-dir <dir>` plus the round's -c context and the budget keys
# before EVERY round, round 1 included, in this shell's process tree, so a
# loop it starts runs at THIS loop's depth. The merge-ready loop uses it for
# autodrive_crusty_rereview.sh: a crusty loop started inside a merge round is
# one runner deeper, where the recursion guard refuses it (PR #1520 review).
# Exit 0 runs the round; exit 79, or the guard's refusal line in its stderr,
# is the terminal refusal; any other exit stops the loop with loop_result
# BEFORE_ROUND_FAILED. The script's stdout may carry `before_round_result`,
# which joins the round's line in the history the evaluator reads.
#
# Policy: this workflow NEVER passes a hook-skipping commit flag or any branch-
# protection bypass. See docs/reference/auto-drive-to-merge.md#two-absolute-prohibitions.

set -uo pipefail

AUTODRIVE_EXIT_POLICY_REFUSAL=79

LOOP_NAME=""; ROUND_RECIPE=""; CLEAN_TOKEN=""; ALSO_CLEAN=""; VERDICT_FIELD=""
REPO="."; REVIEW_DIR=""; STATE_DIR=""; TASK_FILE_IN=""; ROUND_CTX=(); BEFORE_ROUND=""; CTX_REFUSED=""; BUDGET_CTX=(); BEFORE_CTX=(); AD_PR=""

# shellcheck disable=SC2034 # AD_PR is read by the sourced autodrive_escalate.sh
while [ $# -gt 0 ]; do
  case "$1" in
    --loop-name)     LOOP_NAME="${2:-}"; shift 2 ;;
    --round-recipe)  ROUND_RECIPE="${2:-}"; shift 2 ;;
    --clean-token)   CLEAN_TOKEN="${2:-}"; shift 2 ;;
    --also-clean-token) ALSO_CLEAN="${2:-}"; shift 2 ;; # crusty loops pass MINOR_ONLY (#1979)
    --verdict-field) VERDICT_FIELD="${2:-}"; shift 2 ;;
    --repo)          REPO="${2:-}"; shift 2 ;;
    --review-dir)    REVIEW_DIR="${2:-}"; shift 2 ;;
    --state-dir)     STATE_DIR="${2:-}"; shift 2 ;;
    --task-file)     TASK_FILE_IN="${2:-}"; shift 2 ;;
    --context)       case "${2:-}" in review_dir=*|autodrive_launch_tools=*) CTX_REFUSED="${2%%=*}" ;; pr_number=*) AD_PR="${2#*=}" ;; esac
                     case "${2:-}" in autodrive_budget_*) BUDGET_CTX+=("${2:-}"); BEFORE_CTX+=("-c" "${2:-}") ;; *) ROUND_CTX+=("-c" "${2:-}") ;; esac; shift 2 ;;
    --before-round)  BEFORE_ROUND="${2:-}"; shift 2 ;;
    *) echo "ERROR: autodrive_loop.sh: unknown argument '$1'" >&2; exit 2 ;;
  esac
done
for req in LOOP_NAME ROUND_RECIPE CLEAN_TOKEN VERDICT_FIELD STATE_DIR; do
  # tr, not bash 4 lowercase expansion: macOS /bin/bash is 3.2 (issue #1423).
  flag="$(printf '%s' "$req" | tr '[:upper:]' '[:lower:]')"
  [ -n "${!req}" ] || { echo "ERROR: autodrive_loop.sh: --$flag is required" >&2; exit 2; }
done
if [ -n "$BEFORE_ROUND" ] && [ ! -f "$BEFORE_ROUND" ]; then
  echo "ERROR: autodrive_loop.sh: --before-round '${BEFORE_ROUND}' is not a file" >&2; exit 2
fi

# --- private state, whatever the caller's umask ----------------------------
# The merge gate trusts the records below only if no one else could have written
# them (autodrive_state.sh, PRIVATE STATE): the state dir is private and this script
# writes under umask 077; children keep the caller's umask. Helpers come from beside it.
LOOP_HOME="$(cd "$(dirname "${BASH_SOURCE[0]}")" 2>/dev/null && pwd -P)"
# shellcheck source=/dev/null
. "${LOOP_HOME}/autodrive_state.sh" 2>/dev/null \
  || { echo "ERROR: autodrive_loop.sh: autodrive_state.sh is missing beside it (${LOOP_HOME:-<unknown>}); the state dir cannot be made private" >&2; exit 2; }
# shellcheck source=autodrive_escalate.sh # escalate, usage_limit_stop (#1801)
. "${LOOP_HOME}/autodrive_escalate.sh" 2>/dev/null || { echo "ERROR: autodrive_loop.sh: autodrive_escalate.sh is missing beside it (${LOOP_HOME:-<unknown>}); the loop cannot report how it stopped" >&2; exit 2; }
stops_preflight && autodrive_private_dir "$STATE_DIR" || exit 2 # #1743: usage and progress files first
CALLER_UMASK="$(umask)"
umask 077
autodrive_use_task_file "$TASK_FILE_IN" "${STATE_DIR}/${LOOP_NAME}-task_description.txt" || exit 2 # issue #1570

AMPLIHACK_BIN="${AMPLIHACK_BIN:-amplihack}"
export GIT_PAGER=cat GH_PAGER=cat PAGER=cat LESS=FRX
# Recipes come from this driver's tree, never the PR worktree that is --repo,
# whose own recipes would judge the PR (#1543): a bare name, or the path of that
# same file. loop-preflight (#1528, below) re-checks it and the evaluator by name.
ROUND_RECIPE="$(autodrive_launch_recipe "$ROUND_RECIPE")" \
  || { echo "ERROR: loop '${LOOP_NAME}': the round recipe is not in ${LOOP_HOME%/tools}/recipes; refusing to resolve it from --repo" >&2; printf '{"loop":"%s","loop_result":"UNTRUSTED_RECIPE","round_label":"preflight","reason":"the round recipe is not in the launch-side bundle"}\n' "$LOOP_NAME"; exit 2; }

# --- recursion context: propagate, never raise -----------------------------
# AMPLIHACK_MAX_DEPTH is a ceiling handed down by the host, passed through untouched.
# Raising it walks a run into a sealed guard's exit-79 refusal, which #1327 exists to stop.
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

# One field of a record or agent output. `--require-field` (#1337) is NOT optional: without it
# extract-json returns the FIRST parseable object, not the LAST carrying the field.
field() { # field <json> <name> <default>
  printf '%s' "${1:-}" \
    | "$AMPLIHACK_BIN" orch helper extract-json --require-field "$2" \
    | "$AMPLIHACK_BIN" orch helper extract-field --field "$2" --default "$3"
}

# copy_private <from> <to>: copies a regular file under umask 077, removing the old copy
# first: cp onto an existing file keeps its mode and writes through a symlink.
copy_private() {
  [ -f "$1" ] || return 0
  { rm -f -- "$2" && cp -- "$1" "$2"; } || echo "WARNING: could not copy $1 to $2." >&2
}

# terminal_refusal <exit_code> <log_file>: exit 79, or the guard's full marker
# outside input echoes: run summary, agent and log_pointer lines (#1510). Same
# awk+ERE as loop-evidence-collector.yaml; docs/reference/loop-health-evaluator.md.
terminal_refusal() {
  [ "${1:-0}" = "$AUTODRIVE_EXIT_POLICY_REFUSAL" ] && return 0
  [ -f "${2:-}" ] && awk 'NR > 1 { if (prev ~ /^Recipe: / && $0 ~ /^(Run ID|Status): /) { cut = 1; exit } print prev } { prev = $0 } END { if (NR > 0 && !cut) print prev }' "$2" 2>/dev/null \
    | grep -vE '^[[:space:]]*(\[[0-9]{2}:[0-9]{2}:[0-9]{2}\] \[amplihack:|amplihack\.recipe\.log_pointer \{)' \
    | grep -qF 'BLOCKED_TERMINAL orchestration_unavailable' && return 0
  return 1
}

# #1528: the review dir (the main checkout) is stamped before each round and
# compared after it and after the evaluator. Any change stops the loop: the
# evaluator may continue a failed round, and the next would take it as baseline.
stamp_review_dir() { # stamp_review_dir: sets REVIEW_STAMP, or stops as REVIEW_DIR_UNSTAMPED
  REVIEW_STAMP="$(bash "${LOOP_HOME}/autodrive_worktree_guard.sh" index-stamp "$REVIEW_REAL")"
  [ $? -eq 0 ] && [ -n "$REVIEW_STAMP" ] && return 0
  escalate "REVIEW_DIR_UNSTAMPED" "could not stamp the review dir ${REVIEW_REAL} (a lock or git error), so ${ROUND_LABEL} did not run (#1528)"
  exit 1
}
check_review_dir() { # check_review_dir <who>: stops the loop unless REVIEW_STAMP still holds
  bash "${LOOP_HOME}/autodrive_worktree_guard.sh" index-check "$REVIEW_REAL" "$REVIEW_STAMP" "$1"
  case $? in
    0) return 0 ;;
    3) escalate "REVIEW_DIR_UNSTAMPED" "the review dir ${REVIEW_REAL} could not be stamped again after ${1}; nothing was compared (#1528)" ;;
    *) escalate "REVIEW_DIR_CHANGED" "${1} changed the index or HEAD of the review dir ${REVIEW_REAL}; nothing runs from that state (#1528)" ;;
  esac
  exit 1
}

# #1528: the review-dir and launch-side recipe checks; see that file.
PRE="$(bash "${LOOP_HOME}/autodrive_worktree_guard.sh" loop-preflight \
  "$LOOP_NAME" "$REPO" "$REVIEW_DIR" "$(basename "$ROUND_RECIPE" .yaml)" "$CTX_REFUSED")" || { printf '%s\n' "$PRE"; exit 1; }
{ read -r REVIEW_REAL; read -r ROUND_RECIPE_PATH; read -r HEALTH_RECIPE_PATH; } <<<"$PRE"
# --- preflight: the terminator must resolve BEFORE round 1 (why: see the ERROR)
if [ -z "$HEALTH_RECIPE_PATH" ] || ! "$AMPLIHACK_BIN" recipe show "$HEALTH_RECIPE_PATH" >/dev/null 2>&1; then
  echo "ERROR: loop '${LOOP_NAME}' cannot start: the required recipe 'loop-health-evaluator' does not resolve in ${LOOP_HOME%/tools}/recipes." >&2
  echo "  It is this loop's ONLY terminator (issue #1337). Without it a full round runs first — a review, a fix pass, commits pushed — and the loop then stops on an unreadable verdict that blames the loop rather than the missing dependency." >&2
  echo "  Install or update the amplifier bundle beside this script (${LOOP_HOME%/tools}/recipes, never the tree under review) so it resolves there, then re-run. Check with: ${AMPLIHACK_BIN} recipe show ${LOOP_HOME%/tools}/recipes/loop-health-evaluator.yaml" >&2
  printf '{"loop":"%s","loop_result":"MISSING_DEPENDENCY","round_label":"preflight","reason":"the required recipe loop-health-evaluator does not resolve"}\n' \
    "$LOOP_NAME"
  exit 1
fi

# Round labels continue after this loop's earlier rounds (autodrive_last_round.sh).
EARLIER_ROUNDS="$(bash "${LOOP_HOME}/autodrive_last_round.sh" "$STATE_DIR" "$LOOP_NAME")" || exit 2
if [ "$EARLIER_ROUNDS" != "0" ]; then
  echo "INFO: loop '${LOOP_NAME}': ${STATE_DIR} already holds this loop's rounds up to round-${EARLIER_ROUNDS}; this run labels its rounds from round-$((EARLIER_ROUNDS + 1)) and overwrites none of them." >&2
fi
ROUND="$EARLIER_ROUNDS"
ROUND_LABEL=""
HISTORY=""
PREV_FINDINGS=""
PREV_TEST=""
PREV_CI=""; PREV_VERDICT=""; PREV_OVERRIDDEN=0
DISCREPANCY=""
EVIDENCE_HISTORY="${STATE_DIR}/${LOOP_NAME}-evidence-history.jsonl"
TOKEN_SAFE="$(printf '%s' "$CLEAN_TOKEN" | LC_ALL=C tr -cd 'A-Za-z0-9_')" # see VERDICT_SAFE
[ -n "$TOKEN_SAFE" ] || echo "WARNING: loop '${LOOP_NAME}': the clean token has no A-Za-z0-9_ characters, so loop-health-evaluator gets an empty loop_clean_token." >&2

while :; do
  assert_ceiling_untouched
  budget_stop pre # #1743: an exhausted budget starts no agent; a malformed limit exits 2
  # ROUND is a LABEL for reports. Nothing below compares it to a limit.
  ROUND=$((ROUND + 1))
  ROUND_LABEL="round-${ROUND}"
  RECORD="${STATE_DIR}/${LOOP_NAME}-${ROUND_LABEL}.json"
  LOG="${STATE_DIR}/${LOOP_NAME}-${ROUND_LABEL}.log"
  BASELINE="$(git -C "$REPO" rev-parse HEAD 2>/dev/null || printf '')"
  stamp_review_dir

  # --- the before-round step, at this loop's depth --------------------------
  # The baseline above is taken first: commits the step made count as this round's work.
  BEFORE_RESULT=""
  if [ -n "$BEFORE_ROUND" ]; then
    BEFORE_LOG="${STATE_DIR}/${LOOP_NAME}-${ROUND_LABEL}-before.log"
    echo "=== auto-drive loop '${LOOP_NAME}': before ${ROUND_LABEL} ===" >&2
    MARK="$(bash "${LOOP_HOME}/autodrive_budget.sh" mark "$STATE_DIR")" || exit 2; T0=$SECONDS
    ( umask "$CALLER_UMASK" && AMPLIHACK_USAGE_WAIT_LEDGER="$(wait_ledger before)" exec bash "$BEFORE_ROUND" --repo "$REPO" --review-dir "$REVIEW_REAL" --state-dir "$STATE_DIR" \
        ${ROUND_CTX[@]+"${ROUND_CTX[@]}"} ${BEFORE_CTX[@]+"${BEFORE_CTX[@]}"} ) >"${BEFORE_LOG}.out" 2>"$BEFORE_LOG"
    BEFORE_RC=$?; usage_add before "$T0" "$MARK"
    tail -n 200 "$BEFORE_LOG" >&2 || true
    BEFORE_RESULT="$(field "$(cat -- "${BEFORE_LOG}.out" 2>/dev/null)" before_round_result "" | LC_ALL=C tr -cd 'A-Za-z0-9_.:-')"
    assert_ceiling_untouched
    if terminal_refusal "$BEFORE_RC" "$BEFORE_LOG"; then
      echo "ERROR: exit ${AUTODRIVE_EXIT_POLICY_REFUSAL} terminal policy refusal from the before-round step (#1327/#1332). Final: the guard is never retried into." >&2
      escalate "TERMINAL_POLICY_REFUSAL" "the before-round step returned the exit-${AUTODRIVE_EXIT_POLICY_REFUSAL} policy refusal"
      exit "$AUTODRIVE_EXIT_POLICY_REFUSAL"
    fi
    usage_limit_stop "$BEFORE_RC" "$BEFORE_LOG" "the before-round step failed on a Claude usage limit that no login lifted"
    if [ "$BEFORE_RC" -ne 0 ]; then # a nested BUDGET_EXHAUSTED, or a limit reached, is reported as such
      [ "$BEFORE_RESULT" = "crusty-rereview-budget-exhausted" ] && budget_stop before --force; budget_stop before
      escalate "BEFORE_ROUND_FAILED" "the before-round step ${BEFORE_ROUND##*/} exited ${BEFORE_RC} (${BEFORE_RESULT:-no before_round_result}); ${ROUND_LABEL} did not run"
      exit 1
    fi
  fi

  echo "=== auto-drive loop '${LOOP_NAME}': ${ROUND_LABEL} ===" >&2
  T0=$SECONDS # the log is created under umask 077; the round runs with the caller's umask
  ( umask "$CALLER_UMASK" && AMPLIHACK_USAGE_WAIT_LEDGER="$(wait_ledger round)" AMPLIHACK_AUTODRIVE_STATE_DIR="$STATE_DIR" exec "$AMPLIHACK_BIN" recipe run "$ROUND_RECIPE_PATH" \
      --working-dir "$REPO" \
      -c "autodrive_round_record=${RECORD}" \
      -c "autodrive_round_label=${ROUND_LABEL}" \
      -c "autodrive_resolved_concerns_file=${STATE_DIR}/resolved-concerns.txt" \
      "${ROUND_CTX[@]}" -c "review_dir=${REVIEW_REAL}" -c "autodrive_launch_tools=${LOOP_HOME}" ) >"$LOG" 2>&1
  ROUND_RC=$?; usage_add round "$T0"
  tail -n 200 "$LOG" >&2 || true; orphan_stop "$ROUND_RC" "round ${ROUND_LABEL}" # exit 75 (#1976)

  if terminal_refusal "$ROUND_RC" "$LOG"; then
    echo "ERROR: exit ${AUTODRIVE_EXIT_POLICY_REFUSAL} terminal policy refusal from '${ROUND_RECIPE}' (#1327/#1332). Final: the guard is never retried into." >&2
    escalate "TERMINAL_POLICY_REFUSAL" "child returned the exit-${AUTODRIVE_EXIT_POLICY_REFUSAL} policy refusal"
    exit "$AUTODRIVE_EXIT_POLICY_REFUSAL"
  fi
  usage_limit_stop "$ROUND_RC" "$LOG" "round ${ROUND_LABEL} failed on a Claude usage limit that no login lifted; it is not passed to the evaluator"
  check_review_dir "${ROUND_LABEL}${BEFORE_ROUND:+ or its before-round step}"

  # --- read the round's STRUCTURED record -----------------------------------
  # A missing or unparseable record is never clean; it is evidence of a round that produced nothing.
  RAW=""
  [ -f "$RECORD" ] && RAW="$(cat "$RECORD")"
  ROUND_VERDICT="$(field "$RAW" "$VERDICT_FIELD" "MISSING")"
  case "$ROUND_VERDICT" in
    "$CLEAN_TOKEN"|"${ALSO_CLEAN:-$CLEAN_TOKEN}") ROUND_CLEAN="true" ;;
    MISSING) ROUND_CLEAN="false"
      echo "WARNING: ${ROUND_LABEL} produced no parseable '${VERDICT_FIELD}'; treated as NOT clean." >&2 ;;
    *) ROUND_CLEAN="false" ;;
  esac
  # Only A-Za-z0-9_ of the verdict reaches the evaluator, history and #1597 messages; a non-clean one that sanitises to the token ('CLEAN!') is tagged _NONCANONICAL so none of them reads it as clean.
  VERDICT_SAFE="$(printf '%s' "$ROUND_VERDICT" | LC_ALL=C tr -cd 'A-Za-z0-9_')"; [ "$ROUND_CLEAN" = "true" ] || [ "$VERDICT_SAFE" != "$TOKEN_SAFE" ] || VERDICT_SAFE="${VERDICT_SAFE}_NONCANONICAL"
  # #1979: MINOR_ONLY stays MINOR_ONLY in the history and messages. Only the
  # evaluator's loop_round_verdict, which its DONE rule compares with the clean token, reads it as clean.
  EVAL_VERDICT="$VERDICT_SAFE"; [ "$ROUND_CLEAN" != "true" ] || EVAL_VERDICT="$TOKEN_SAFE"
  FINDINGS=""
  [ -f "${RECORD}.findings" ] && FINDINGS="$(cat "${RECORD}.findings")"
  # Stable path to the latest round record, so a later gate binds evidence to the round that made it.
  copy_private "$RECORD" "${STATE_DIR}/${LOOP_NAME}-latest.json"
  copy_private "${RECORD}.findings" "${STATE_DIR}/${LOOP_NAME}-latest.json.findings"
  # #1517: <loop>-records.tsv gets one row per round THIS loop wrote (label, record, git blob hash)
  # before any later agent runs. autodrive_crusty_final and autodrive_qa_trusted trust a record only
  # when the last row names it and the hash matches (git from the state dir, GIT_DIR/GIT_WORK_TREE
  # unset, as autodrive_blob_hash). A failed check: WARNING, no row.
  if [ -f "$RECORD" ]; then
    MANIFEST_FILE="${RECORD##*/}"; MANIFEST_HASH=""
    case "$MANIFEST_FILE" in
      .*|*[!A-Za-z0-9._-]*|'')
        echo "WARNING: round record name '${MANIFEST_FILE}' is not a plain file name; no manifest row for ${ROUND_LABEL}." >&2 ;;
      *)
        if ! cmp -s "$RECORD" "${STATE_DIR}/${LOOP_NAME}-latest.json"; then
          echo "WARNING: ${RECORD} and its ${LOOP_NAME}-latest.json copy differ; no manifest row for ${ROUND_LABEL}." >&2
        else
          MANIFEST_HASH="$( (cd -- "$STATE_DIR" && env -u GIT_DIR -u GIT_WORK_TREE git hash-object --no-filters --stdin) < "$RECORD" 2>/dev/null)"
          case "$MANIFEST_HASH" in *[!0-9a-f]*) MANIFEST_HASH="" ;; esac
          case "${#MANIFEST_HASH}" in 40|64) ;; *) MANIFEST_HASH="" ;; esac
          [ -n "$MANIFEST_HASH" ] || echo "WARNING: git could not hash ${RECORD}; no manifest row for ${ROUND_LABEL}, so this round cannot count as loop-written evidence." >&2
        fi ;;
    esac
    if [ -n "$MANIFEST_HASH" ]; then
      ( umask 077
        printf '%s\t%s\t%s\n' "$(printf '%s' "$ROUND_LABEL" | tr -c 'A-Za-z0-9._-' '_')" "$MANIFEST_FILE" "$MANIFEST_HASH" \
          >> "${STATE_DIR}/${LOOP_NAME}-records.tsv" ) \
        || echo "WARNING: could not append the manifest row for ${ROUND_LABEL} to ${LOOP_NAME}-records.tsv." >&2
    fi
  fi
  # #1743, decided in code before the evaluator: no progress, then the budget. RF: 1 clean, 2 failed (rc!=0 or no verdict; the evaluator judges faults), 0 otherwise.
  HEAD_NOW="$(git -C "$REPO" rev-parse HEAD || printf '')"; CI_SIGNAL="$(field "$RAW" ci_signal "")"
  RF=0; [ "$ROUND_CLEAN" = "true" ] && RF=1 || { [ "$ROUND_RC" -eq 0 ] && [ "$ROUND_VERDICT" != "MISSING" ]; } || RF=2
  bash "${LOOP_HOME}/autodrive_progress.sh" record "$STATE_DIR" "$LOOP_NAME" "$ROUND_LABEL" "$HEAD_NOW" "$RF" "${RECORD}.findings" "$CI_SIGNAL"
  [ "$ROUND_CLEAN" = "true" ] || budget_stop post
  TEST_SIGNAL="$(field "$RAW" test_signal "")"
  HISTORY="${HISTORY}
${ROUND_LABEL}: ${VERDICT_FIELD}=${VERDICT_SAFE} rc=${ROUND_RC} findings=$(printf '%s' "$FINDINGS" | grep -c . || true)${BEFORE_RESULT:+ before=${BEFORE_RESULT}}"

  # Preserve full observations; no observation is a convergence verdict.
  if [ -n "$RAW" ]; then
    printf '%s\n' "$RAW" >> "$EVIDENCE_HISTORY" || { escalate "RESULT_PERSIST_FAILED" "cannot persist evaluator evidence"; exit 1; }
  fi

  # --- the agentic terminator ----------------------------------------------
  assert_ceiling_untouched
  HEALTH_LOG="${STATE_DIR}/${LOOP_NAME}-${ROUND_LABEL}-health.log"; T0=$SECONDS
  ( umask "$CALLER_UMASK" && AMPLIHACK_USAGE_WAIT_LEDGER="$(wait_ledger evaluator)" exec "$AMPLIHACK_BIN" recipe run "$HEALTH_RECIPE_PATH" \
      --working-dir "$REVIEW_REAL" \
      -c "loop_name=auto-drive:${LOOP_NAME}" \
      -c "loop_round_label=${ROUND_LABEL}" \
      -c "loop_history=${HISTORY} ${DISCREPANCY}" \
      -c "loop_evidence_history_file=${EVIDENCE_HISTORY}" \
      -c "loop_round_record=${RECORD}" \
      -c "loop_last_round_output=$(bash "${LOOP_HOME}/autodrive_round_tail.sh" "$LOG")" \
      -c "loop_repo_path=${REPO}" -c "loop_agent_working_dir=${REVIEW_REAL}" \
      -c "loop_baseline_ref=${BASELINE}" \
      -c "loop_child_exit_code=${ROUND_RC}" \
      -c "loop_findings_current=${FINDINGS}" \
      -c "loop_findings_previous=${PREV_FINDINGS}" \
      -c "loop_test_signal=${TEST_SIGNAL}" \
      -c "loop_test_signal_previous=${PREV_TEST}" \
      -c "loop_ci_signal=${CI_SIGNAL}" \
      -c "loop_ci_signal_previous=${PREV_CI}" \
      -c "loop_clean_token=${TOKEN_SAFE}" -c "loop_round_verdict=${EVAL_VERDICT}" ) \
      >"$HEALTH_LOG" 2>&1
  HEALTH_RC=$?; usage_add evaluator "$T0"
  tail -n 120 "$HEALTH_LOG" >&2 || true; orphan_stop "$HEALTH_RC" "the loop-health evaluator for ${ROUND_LABEL}"

  if terminal_refusal "$HEALTH_RC" "$HEALTH_LOG"; then
    echo "ERROR: exit ${AUTODRIVE_EXIT_POLICY_REFUSAL} terminal policy refusal from loop-health-evaluator. Final; not retried." >&2
    escalate "TERMINAL_POLICY_REFUSAL" "loop-health-evaluator returned the policy refusal"
    exit "$AUTODRIVE_EXIT_POLICY_REFUSAL"
  fi
  usage_limit_stop "$HEALTH_RC" "$HEALTH_LOG" "the loop-health evaluator failed on a Claude usage limit that no login lifted"
  check_review_dir loop-health-evaluator

  # The evaluator's last step prints one LOOP_HEALTH: marker, exiting non-zero on STUCK. The runner
  # (#1512) prints `  ✓ step-04-enforce-loop-verdict[ (<name>)]: completed[ [<details>]]`, then
  # `    Output: LOOP_HEALTH: <verdict>`. Only the line straight after the LAST completed step-04 status
  # line, with no status line after it and prefix exactly `    Output: ` or none, is trusted; anything
  # else is STUCK (docs/reference/auto-drive-to-merge.md#reading-the-loop-health-line). BSD awk and mawk
  # safe. format_recipe_run_table (crates/amplihack-cli/src/commands/recipe/run/format.rs) owns the
  # format; tests_autodrive_health_log.rs runs this awk over its output.
  LOOP_VERDICT="EVALUATION_FAILED"
  if [ "$HEALTH_RC" -eq 0 ]; then
    HEALTH_READ="$(LC_ALL=C awk '
      function verdict(s, p) {
        if (s == p "CONTINUE" || index(s, p "CONTINUE ") == 1) return "CONTINUE"
        if (s == p "DONE" || index(s, p "DONE ") == 1) return "DONE"
        return "EVALUATOR_INVALID"
      }
      pend { rec = $0; pend = 0 }
      /^  [^ ]+ step-[^ :(]+( \([^)]*\))?: / {
        rec = ""; pend = 0
        if ($0 ~ /^  ✓ step-04-enforce-loop-verdict( \([^)]*\))?: completed( \[[^]]*\])?$/) pend = 1
      }
      END { v = verdict(rec, "    Output: LOOP_HEALTH: "); if (v == "EVALUATOR_INVALID") v = verdict(rec, "LOOP_HEALTH: "); print v }' "$HEALTH_LOG" || echo EVALUATOR_INVALID)"
    case "$HEALTH_READ" in
      CONTINUE) LOOP_VERDICT="CONTINUE" ;;
      DONE) LOOP_VERDICT="DONE" ;;
      *)
        LOOP_VERDICT="EVALUATOR_INVALID"
        echo "ERROR: EVALUATOR_INVALID: no readable agent verdict." >&2
        ;;
    esac
  elif grep -qE '^LOOP_HEALTH: STUCK .*source=evaluator' "$HEALTH_LOG"; then
    LOOP_VERDICT="STUCK"
  elif grep -q 'EVALUATOR_INVALID' "$HEALTH_LOG"; then
    LOOP_VERDICT="EVALUATOR_INVALID"
  fi

  OVERRIDE_ARGS=("$ROUND_VERDICT" "$VERDICT_SAFE" "$PREV_OVERRIDDEN" "$PREV_VERDICT" "$FINDINGS" "$PREV_FINDINGS" "$BASELINE" "$HEAD_NOW")
  PREV_FINDINGS="$FINDINGS"; PREV_TEST="$TEST_SIGNAL"; PREV_CI="$CI_SIGNAL"; PREV_VERDICT="$VERDICT_SAFE"; PREV_OVERRIDDEN=0

  # --- decide, on BOTH signals ---------------------------------------------
  # Advancing needs the round's machine-checked verdict AND the evaluator's DONE.
  case "$LOOP_VERDICT" in
    DONE)
      if [ "$ROUND_CLEAN" = "true" ]; then
        echo "AUTO_DRIVE_LOOP: DONE — loop '${LOOP_NAME}' converged at ${ROUND_LABEL} with ${VERDICT_FIELD}=${ROUND_VERDICT}." >&2
        record_result "$(printf '{"loop":"%s","loop_result":"DONE","round_label":"%s","round_verdict":"%s"}' \
          "$LOOP_NAME" "$ROUND_LABEL" "$ROUND_VERDICT")"
        exit 0
      fi
      DISCREPANCY="DONE_WITH_NONCLEAN_ROUND: evaluator DONE conflicts with ${VERDICT_SAFE}; reassess with fresh evidence"
      echo "INFO: ${DISCREPANCY}" >&2
      HISTORY="${HISTORY} done_overridden=1"; PREV_OVERRIDDEN=1
      continue
      ;;
    CONTINUE)
      if [ "$ROUND_CLEAN" = "true" ]; then
        echo "INFO: ${ROUND_LABEL} is ${CLEAN_TOKEN} but the evaluator wants another round; confirming rather than advancing." >&2
      fi
      continue
      ;;
    STUCK)
      escalate "STUCK" "loop-health-evaluator explicitly judged '${LOOP_NAME}' unable to progress"
      exit 1
      ;;
    *)
      escalate "$LOOP_VERDICT" "loop-health evaluation did not produce a valid judgment"
      exit 1
      ;;
  esac
done
