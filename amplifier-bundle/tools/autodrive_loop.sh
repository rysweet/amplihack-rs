#!/usr/bin/env bash
# autodrive_loop.sh — the agentic loop driver for auto-drive-to-merge.
#
# Runs one round recipe over and over until an AGENTIC evaluator says to stop.
# There is no iteration cap in this file — not a `max_rounds`, not a backstop
# integer, not a wall-clock budget. `ROUND` below is a LABEL that appears in
# reports and names the round's files; it is never compared against a limit
# and no branch reads it. It starts after the rounds this loop already has in
# the state dir, so a second loop on the same dir (the crusty re-review) or a
# resumed run never overwrites an earlier round's record, findings or logs.
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
#             DONE, or a repeated DONE over an unchanged non-clean round (#1597).
#   Exit 79 — a child returned the terminal policy refusal. Surfaced, final,
#             and NEVER retried into.
#
# --before-round <script> (optional) runs `bash <script> --repo <repo> --review-dir
# <dir> --state-dir <dir>` plus the round's -c and --context-from-file context
# before EVERY round, round 1 included. It runs in this shell's process tree,
# so a loop it starts runs at the depth of THIS loop, never one recipe runner
# deeper. The merge-ready loop uses it for autodrive_crusty_rereview.sh, which
# sends commits made after the clean crusty round back to the crusty loop: a
# crusty loop started from inside a merge round would be one runner deeper,
# and the recursion guard refuses it there with exit 79 (PR #1520 review).
# Exit 0 runs the round; exit 79, or the guard's refusal line in its stderr,
# is the terminal refusal; any other exit stops the loop with loop_result
# BEFORE_ROUND_FAILED. The script's stdout may carry `before_round_result`,
# which joins the round's line in the history the evaluator reads.
#
# Policy: this workflow NEVER passes a hook-skipping commit flag or any branch-
# protection bypass. See docs/reference/auto-drive-to-merge.md#two-absolute-prohibitions.

set -uo pipefail

AUTODRIVE_EXIT_POLICY_REFUSAL=79

LOOP_NAME=""; ROUND_RECIPE=""; CLEAN_TOKEN=""; VERDICT_FIELD=""
REPO="."; REVIEW_DIR=""; STATE_DIR=""; TASK_FILE_IN=""; ROUND_CTX=(); BEFORE_ROUND=""; CTX_REFUSED=""

while [ $# -gt 0 ]; do
  case "$1" in
    --loop-name)     LOOP_NAME="${2:-}"; shift 2 ;;
    --round-recipe)  ROUND_RECIPE="${2:-}"; shift 2 ;;
    --clean-token)   CLEAN_TOKEN="${2:-}"; shift 2 ;;
    --verdict-field) VERDICT_FIELD="${2:-}"; shift 2 ;;
    --repo)          REPO="${2:-}"; shift 2 ;;
    --review-dir)    REVIEW_DIR="${2:-}"; shift 2 ;;
    --state-dir)     STATE_DIR="${2:-}"; shift 2 ;;
    --task-file)     TASK_FILE_IN="${2:-}"; shift 2 ;;
    --context)       case "${2:-}" in review_dir=*|autodrive_launch_tools=*) CTX_REFUSED="${2%%=*}" ;; esac; ROUND_CTX+=("-c" "${2:-}"); shift 2 ;;
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
# The merge gate trusts the records, copies and manifest below only if no
# one else could have written them (autodrive_state.sh, PRIVATE STATE), so
# the state dir is made private and this script writes under umask 077. The
# round recipe and the evaluator keep the caller's umask (their round steps
# set umask 077 themselves). The state helper comes from beside this script.
LOOP_HOME="$(cd "$(dirname "${BASH_SOURCE[0]}")" 2>/dev/null && pwd -P)"
# shellcheck source=/dev/null
. "${LOOP_HOME}/autodrive_state.sh" 2>/dev/null \
  || { echo "ERROR: autodrive_loop.sh: autodrive_state.sh is missing beside it (${LOOP_HOME:-<unknown>}); the state dir cannot be made private" >&2; exit 2; }
autodrive_private_dir "$STATE_DIR" || exit 2
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

# Read one field out of a round record or an agent's output. `--require-field`
# (issue #1337, PR #1347) is NOT optional: plain `extract-json` returns the FIRST
# parseable object (a quoted example, a draft verdict), not the LAST that carries
# the field, and with it no carrying object at all means the blocking `--default`.
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

escalate() { # escalate <verdict> <why>
  echo "AUTO_DRIVE_LOOP: ${1} — loop '${LOOP_NAME}' stops at round label '${ROUND_LABEL:-?}'." >&2
  echo "  Reason: ${2}" >&2
  echo "  Round record: ${RECORD:-<none>}" >&2
  echo "  This is a judgement about absence of progress, not an attempt count." >&2
  printf '{"loop":"%s","loop_result":"%s","round_label":"%s","reason":"%s"}\n' \
    "$LOOP_NAME" "$1" "${ROUND_LABEL:-?}" "$(printf '%s' "$2" | tr -d '"' | tr '\n' ' ')"
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
TOKEN_SAFE="$(printf '%s' "$CLEAN_TOKEN" | LC_ALL=C tr -cd 'A-Za-z0-9_')" # see VERDICT_SAFE
[ -n "$TOKEN_SAFE" ] || echo "WARNING: loop '${LOOP_NAME}': the clean token has no A-Za-z0-9_ characters, so loop-health-evaluator gets an empty loop_clean_token." >&2

while :; do
  assert_ceiling_untouched
  # ROUND is a LABEL for reports. Nothing below compares it to a limit.
  ROUND=$((ROUND + 1))
  ROUND_LABEL="round-${ROUND}"
  RECORD="${STATE_DIR}/${LOOP_NAME}-${ROUND_LABEL}.json"
  LOG="${STATE_DIR}/${LOOP_NAME}-${ROUND_LABEL}.log"
  BASELINE="$(git -C "$REPO" rev-parse HEAD 2>/dev/null || printf '')"
  stamp_review_dir

  # --- the before-round step, at this loop's depth --------------------------
  # The baseline above is taken first, so the evaluator counts commits the
  # step made (crusty's fixes) as this round's work.
  BEFORE_RESULT=""
  if [ -n "$BEFORE_ROUND" ]; then
    BEFORE_LOG="${STATE_DIR}/${LOOP_NAME}-${ROUND_LABEL}-before.log"
    echo "=== auto-drive loop '${LOOP_NAME}': before ${ROUND_LABEL} ===" >&2
    ( umask "$CALLER_UMASK" && exec bash "$BEFORE_ROUND" --repo "$REPO" --review-dir "$REVIEW_REAL" --state-dir "$STATE_DIR" \
        ${ROUND_CTX[@]+"${ROUND_CTX[@]}"} ) >"${BEFORE_LOG}.out" 2>"$BEFORE_LOG"
    BEFORE_RC=$?
    tail -n 200 "$BEFORE_LOG" >&2 || true
    BEFORE_RESULT="$(field "$(cat -- "${BEFORE_LOG}.out" 2>/dev/null)" before_round_result "" | LC_ALL=C tr -cd 'A-Za-z0-9_.:-')"
    assert_ceiling_untouched
    if terminal_refusal "$BEFORE_RC" "$BEFORE_LOG"; then
      echo "ERROR: exit ${AUTODRIVE_EXIT_POLICY_REFUSAL} terminal policy refusal from the before-round step (#1327/#1332). Final: the guard is never retried into." >&2
      escalate "TERMINAL_POLICY_REFUSAL" "the before-round step returned the exit-${AUTODRIVE_EXIT_POLICY_REFUSAL} policy refusal"
      exit "$AUTODRIVE_EXIT_POLICY_REFUSAL"
    fi
    if [ "$BEFORE_RC" -ne 0 ]; then
      escalate "BEFORE_ROUND_FAILED" "the before-round step ${BEFORE_ROUND##*/} exited ${BEFORE_RC} (${BEFORE_RESULT:-no before_round_result}); ${ROUND_LABEL} did not run"
      exit 1
    fi
  fi

  echo "=== auto-drive loop '${LOOP_NAME}': ${ROUND_LABEL} ===" >&2
  # The log is created by this shell under umask 077; the round runs with the caller's umask.
  ( umask "$CALLER_UMASK" && exec "$AMPLIHACK_BIN" recipe run "$ROUND_RECIPE_PATH" \
      --working-dir "$REPO" \
      -c "autodrive_round_record=${RECORD}" \
      -c "autodrive_round_label=${ROUND_LABEL}" \
      -c "autodrive_resolved_concerns_file=${STATE_DIR}/resolved-concerns.txt" \
      "${ROUND_CTX[@]}" -c "review_dir=${REVIEW_REAL}" -c "autodrive_launch_tools=${LOOP_HOME}" ) >"$LOG" 2>&1
  ROUND_RC=$?
  tail -n 200 "$LOG" >&2 || true

  if terminal_refusal "$ROUND_RC" "$LOG"; then
    echo "ERROR: exit ${AUTODRIVE_EXIT_POLICY_REFUSAL} terminal policy refusal from '${ROUND_RECIPE}' (#1327/#1332). Final: the guard is never retried into." >&2
    escalate "TERMINAL_POLICY_REFUSAL" "child returned the exit-${AUTODRIVE_EXIT_POLICY_REFUSAL} policy refusal"
    exit "$AUTODRIVE_EXIT_POLICY_REFUSAL"
  fi
  check_review_dir "${ROUND_LABEL}${BEFORE_ROUND:+ or its before-round step}"

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
  # Only A-Za-z0-9_ of the verdict reaches the evaluator, the history and the #1597 messages: no forged lines.
  VERDICT_SAFE="$(printf '%s' "$ROUND_VERDICT" | LC_ALL=C tr -cd 'A-Za-z0-9_')"
  FINDINGS=""
  [ -f "${RECORD}.findings" ] && FINDINGS="$(cat "${RECORD}.findings")"
  # Stable path to the most recent round record, so a later gate can bind its
  # evidence to the round that actually produced it without guessing a name.
  copy_private "$RECORD" "${STATE_DIR}/${LOOP_NAME}-latest.json"
  copy_private "${RECORD}.findings" "${STATE_DIR}/${LOOP_NAME}-latest.json.findings"
  # Manifest of the round records THIS loop wrote (#1517): one row per round in
  # <loop>-records.tsv (label, record name, git blob hash), written before any later
  # agent runs, so no agent gets a row for a record it wrote. autodrive_crusty_final
  # (autodrive_state.sh) and autodrive_qa_trusted (autodrive_trust.sh) trust a record
  # only when the last row names it and the hash matches; git runs from the state dir,
  # GIT_DIR/GIT_WORK_TREE unset, as autodrive_blob_hash does. A failed check: WARNING, no row.
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
  TEST_SIGNAL="$(field "$RAW" test_signal "")"
  CI_SIGNAL="$(field "$RAW" ci_signal "")"
  HISTORY="${HISTORY}
${ROUND_LABEL}: ${VERDICT_FIELD}=${VERDICT_SAFE} rc=${ROUND_RC} findings=$(printf '%s' "$FINDINGS" | grep -c . || true)${BEFORE_RESULT:+ before=${BEFORE_RESULT}}"

  # --- the agentic terminator ----------------------------------------------
  assert_ceiling_untouched
  HEALTH_LOG="${STATE_DIR}/${LOOP_NAME}-${ROUND_LABEL}-health.log"
  ( umask "$CALLER_UMASK" && exec "$AMPLIHACK_BIN" recipe run "$HEALTH_RECIPE_PATH" \
      --working-dir "$REVIEW_REAL" \
      -c "loop_name=auto-drive:${LOOP_NAME}" \
      -c "loop_round_label=${ROUND_LABEL}" \
      -c "loop_history=${HISTORY}" \
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
      -c "loop_clean_token=${TOKEN_SAFE}" -c "loop_round_verdict=${VERDICT_SAFE}" ) \
      >"$HEALTH_LOG" 2>&1
  HEALTH_RC=$?
  tail -n 120 "$HEALTH_LOG" >&2 || true

  if terminal_refusal "$HEALTH_RC" "$HEALTH_LOG"; then
    echo "ERROR: exit ${AUTODRIVE_EXIT_POLICY_REFUSAL} terminal policy refusal from loop-health-evaluator. Final; not retried." >&2
    escalate "TERMINAL_POLICY_REFUSAL" "loop-health-evaluator returned the policy refusal"
    exit "$AUTODRIVE_EXIT_POLICY_REFUSAL"
  fi
  check_review_dir loop-health-evaluator

  # The evaluator's step-04 (its last step) prints one LOOP_HEALTH: marker and
  # exits non-zero on STUCK. The runner's formatter (issue #1512) prints
  #   `  ✓ step-04-enforce-loop-verdict[ (<name>)]: completed[ [<details>]]`
  # then `    Output: LOOP_HEALTH: <verdict>`. Only the line straight after the
  # LAST completed step-04 status line, with no status line after it, is
  # trusted; its prefix is exactly `    Output: ` or none. Anything else is
  # STUCK (docs/reference/auto-drive-to-merge.md#reading-the-loop-health-line). BSD awk and
  # mawk safe. format_recipe_run_table (crates/amplihack-cli/src/commands/recipe/run/format.rs)
  # owns the format; tests_autodrive_health_log.rs runs this awk over its output.
  LOOP_VERDICT="STUCK"
  if [ "$HEALTH_RC" -eq 0 ]; then
    HEALTH_READ="$(LC_ALL=C awk '
      function verdict(s, p) {
        if (s == p "CONTINUE" || index(s, p "CONTINUE ") == 1) return "CONTINUE"
        if (s == p "DONE" || index(s, p "DONE ") == 1) return "DONE"
        return "STUCK"
      }
      pend { rec = $0; pend = 0 }
      /^  [^ ]+ step-[^ :(]+( \([^)]*\))?: / {
        rec = ""; pend = 0
        if ($0 ~ /^  ✓ step-04-enforce-loop-verdict( \([^)]*\))?: completed( \[[^]]*\])?$/) pend = 1
      }
      END { v = verdict(rec, "    Output: LOOP_HEALTH: "); if (v == "STUCK") v = verdict(rec, "LOOP_HEALTH: "); print v }' "$HEALTH_LOG" || echo STUCK)"
    case "$HEALTH_READ" in
      CONTINUE) LOOP_VERDICT="CONTINUE" ;;
      DONE) LOOP_VERDICT="DONE" ;;
      *)
        echo "WARNING: loop-health-evaluator exited 0 with no readable LOOP_HEALTH verdict; failing safe to STUCK." >&2
        ;;
    esac
  fi

  OVERRIDE_ARGS=("$ROUND_VERDICT" "$VERDICT_SAFE" "$PREV_OVERRIDDEN" "$PREV_VERDICT" "$FINDINGS" "$PREV_FINDINGS" "$BASELINE"
    "$(git -C "$REPO" rev-parse HEAD || printf '')")
  PREV_FINDINGS="$FINDINGS"; PREV_TEST="$TEST_SIGNAL"; PREV_CI="$CI_SIGNAL"; PREV_VERDICT="$VERDICT_SAFE"; PREV_OVERRIDDEN=0

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
      # #1597: re-review once; a missing record, or a repeat that moved nothing, is STUCK.
      WHY="$(bash "${LOOP_HOME}/autodrive_done_override.sh" "${OVERRIDE_ARGS[@]}")" \
        || { escalate "STUCK" "loop-health said DONE while the round verdict is '${VERDICT_SAFE}', not '${TOKEN_SAFE}'${WHY:+: ${WHY}}. An inconsistent pair never advances a phase."; exit 1; }
      echo "INFO: loop-health said DONE but ${VERDICT_FIELD}=${VERDICT_SAFE}, not ${TOKEN_SAFE}; running another round to re-review." >&2
      HISTORY="${HISTORY} done_overridden=1"; PREV_OVERRIDDEN=1
      continue
      ;;
    CONTINUE)
      if [ "$ROUND_CLEAN" = "true" ]; then
        echo "INFO: ${ROUND_LABEL} is ${CLEAN_TOKEN} but the evaluator wants another round; confirming rather than advancing." >&2
      fi
      continue
      ;;
    *)
      escalate "STUCK" "loop-health-evaluator returned STUCK (or an unreadable verdict) for '${LOOP_NAME}'"
      exit 1
      ;;
  esac
done
