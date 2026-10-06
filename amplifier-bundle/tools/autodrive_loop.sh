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
# --before-round <script> (optional) runs `bash <script> --repo <repo>
# --state-dir <dir> -c <key=value>...`, with the context the round gets,
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
# Policy: this workflow NEVER passes a hook-skipping commit flag or any
# branch-protection bypass. See
# docs/reference/auto-drive-to-merge.md#two-absolute-prohibitions.

set -uo pipefail

AUTODRIVE_EXIT_POLICY_REFUSAL=79

LOOP_NAME=""; ROUND_RECIPE=""; CLEAN_TOKEN=""; VERDICT_FIELD=""
REPO="."; STATE_DIR=""; ROUND_CTX=(); BEFORE_ROUND=""

while [ $# -gt 0 ]; do
  case "$1" in
    --loop-name)     LOOP_NAME="${2:-}"; shift 2 ;;
    --round-recipe)  ROUND_RECIPE="${2:-}"; shift 2 ;;
    --clean-token)   CLEAN_TOKEN="${2:-}"; shift 2 ;;
    --verdict-field) VERDICT_FIELD="${2:-}"; shift 2 ;;
    --repo)          REPO="${2:-}"; shift 2 ;;
    --state-dir)     STATE_DIR="${2:-}"; shift 2 ;;
    --context)       ROUND_CTX+=("-c" "${2:-}"); shift 2 ;;
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
# The merge gate reads the records, copies and manifest written below only
# when no one else could have written them (autodrive_state.sh, PRIVATE
# STATE). Under umask 0002 a plain mkdir, `>` or cp would make every one of
# them group-writable and the gate would refuse every merge, so the state dir
# is made private here and this script writes under umask 077. The round
# recipe and the evaluator run with the caller's umask, so the files agents
# create in the repository are unaffected; the round steps that write into
# the state dir set umask 077 themselves. The state helper is sourced from
# beside this script only.
LOOP_HOME="$(cd "$(dirname "${BASH_SOURCE[0]}")" 2>/dev/null && pwd -P)"
# shellcheck source=/dev/null
. "${LOOP_HOME}/autodrive_state.sh" 2>/dev/null \
  || { echo "ERROR: autodrive_loop.sh: autodrive_state.sh is missing beside it (${LOOP_HOME:-<unknown>}); the state dir cannot be made private" >&2; exit 2; }
autodrive_private_dir "$STATE_DIR" || exit 2
CALLER_UMASK="$(umask)"
umask 077

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

# copy_private <from> <to>: copies a regular file under this script's umask
# 077. The old copy is removed first, because cp onto an existing file keeps
# that file's mode and writes through it when it is a symlink.
copy_private() {
  [ -f "$1" ] || return 0
  { rm -f -- "$2" && cp -- "$1" "$2"; } || echo "WARNING: could not copy $1 to $2." >&2
}

# terminal_refusal <exit_code> <log_file>: exit 79, or the guard's own
# refusal line in the log. Agent output lines (`[HH:MM:SS] [amplihack:...]`)
# are skipped first: a review that QUOTES the refusal, as crusty's review of
# PR #1520 did, is not one, and reading it as one stopped the loop.
terminal_refusal() {
  [ "${1:-0}" = "$AUTODRIVE_EXIT_POLICY_REFUSAL" ] && return 0
  [ -f "${2:-}" ] && grep -vE '^[[:space:]]*\[[0-9]{2}:[0-9]{2}:[0-9]{2}\] \[amplihack:' "$2" 2>/dev/null \
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

  # --- the before-round step, at this loop's depth --------------------------
  # The baseline above is taken first, so the evaluator counts commits the
  # step made (crusty's fixes) as this round's work.
  BEFORE_RESULT=""
  if [ -n "$BEFORE_ROUND" ]; then
    BEFORE_LOG="${STATE_DIR}/${LOOP_NAME}-${ROUND_LABEL}-before.log"
    echo "=== auto-drive loop '${LOOP_NAME}': before ${ROUND_LABEL} ===" >&2
    ( umask "$CALLER_UMASK" && exec bash "$BEFORE_ROUND" --repo "$REPO" --state-dir "$STATE_DIR" \
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
  # The log is created by this shell under umask 077; the round runs with the
  # caller's umask.
  ( umask "$CALLER_UMASK" && exec "$AMPLIHACK_BIN" recipe run "$ROUND_RECIPE" \
      --working-dir "$REPO" \
      -c "autodrive_round_record=${RECORD}" \
      -c "autodrive_round_label=${ROUND_LABEL}" \
      -c "autodrive_resolved_concerns_file=${STATE_DIR}/resolved-concerns.txt" \
      "${ROUND_CTX[@]}" ) >"$LOG" 2>&1
  ROUND_RC=$?
  tail -n 200 "$LOG" >&2 || true

  if terminal_refusal "$ROUND_RC" "$LOG"; then
    echo "ERROR: exit ${AUTODRIVE_EXIT_POLICY_REFUSAL} terminal policy refusal from '${ROUND_RECIPE}' (#1327/#1332). Final: the guard is never retried into." >&2
    escalate "TERMINAL_POLICY_REFUSAL" "child returned the exit-${AUTODRIVE_EXIT_POLICY_REFUSAL} policy refusal"
    exit "$AUTODRIVE_EXIT_POLICY_REFUSAL"
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
  copy_private "$RECORD" "${STATE_DIR}/${LOOP_NAME}-latest.json"
  copy_private "${RECORD}.findings" "${STATE_DIR}/${LOOP_NAME}-latest.json.findings"
  # Manifest of the round records THIS loop wrote (issue #1517): one row per
  # round in <loop>-records.tsv with the label, the record file name and the
  # record's git blob hash. It is written here, before the loop-health
  # evaluator or any later agent runs, so an agent cannot get a row for a
  # record it wrote itself; autodrive_crusty_final (autodrive_state.sh) trusts
  # a crusty record, and autodrive_qa_trusted (autodrive_trust.sh) a
  # merge-ready record, only when the last row names it and the hash still
  # matches. git runs from the state dir with GIT_DIR/GIT_WORK_TREE unset, as
  # autodrive_blob_hash does, so both compute the same hash. A check that
  # fails writes a WARNING and no row.
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
${ROUND_LABEL}: ${VERDICT_FIELD}=${ROUND_VERDICT} rc=${ROUND_RC} findings=$(printf '%s' "$FINDINGS" | grep -c . || true)${BEFORE_RESULT:+ before=${BEFORE_RESULT}}"

  # --- the agentic terminator ----------------------------------------------
  assert_ceiling_untouched
  HEALTH_LOG="${STATE_DIR}/${LOOP_NAME}-${ROUND_LABEL}-health.log"
  ( umask "$CALLER_UMASK" && exec "$AMPLIHACK_BIN" recipe run loop-health-evaluator \
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
      -c "loop_ci_signal_previous=${PREV_CI}" ) \
      >"$HEALTH_LOG" 2>&1
  HEALTH_RC=$?
  tail -n 120 "$HEALTH_LOG" >&2 || true

  if terminal_refusal "$HEALTH_RC" "$HEALTH_LOG"; then
    echo "ERROR: exit ${AUTODRIVE_EXIT_POLICY_REFUSAL} terminal policy refusal from loop-health-evaluator. Final; not retried." >&2
    escalate "TERMINAL_POLICY_REFUSAL" "loop-health-evaluator returned the policy refusal"
    exit "$AUTODRIVE_EXIT_POLICY_REFUSAL"
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
      escalate "STUCK" "loop-health-evaluator returned STUCK (or an unreadable verdict) for '${LOOP_NAME}'"
      exit 1
      ;;
  esac
done
