#!/usr/bin/env bash
# autodrive_escalate.sh: sourced by autodrive_loop.sh. How the loop stops:
# escalate writes the stop to stderr and its JSON record to stdout, and
# usage_limit_stop tells a child that failed on a Claude usage limit from any
# other failure, so the loop stops as USAGE_LIMIT and never as STUCK (#1801).
# progress_stop and budget_stop are the two stops decided in code (#1743);
# wait_stop ends a round whose only blockers are waits as WAITING (#2472).
# docs/reference/auto-drive-to-merge.md. Defines functions only; the caller
# sets LOOP_NAME, ROUND_LABEL and RECORD, and for #1743 STATE_DIR, LOOP_HOME,
# AD_PR and BUDGET_CTX.

# log_body <log_file>: the log without input echoes, as terminal_refusal reads
# it. workflow_hook_repair.sh keeps a copy, which a test holds equal to this.
log_body() {
  awk 'NR > 1 { if (prev ~ /^Recipe: / && $0 ~ /^(Run ID|Status): /) { cut = 1; exit } print prev } { prev = $0 } END { if (NR > 0 && !cut) print prev }' "$1" 2>/dev/null \
    | grep -vE '^[[:space:]]*(\[[0-9]{2}:[0-9]{2}:[0-9]{2}\] \[amplihack:|amplihack\.recipe\.log_pointer \{)'
}

# usage_limit_failure <rc> <log>: a non-zero exit and a column-0 failure_class
# marker in log_body whose top-level class is usage_limit (#1801). jq when
# installed, else anchored to the runner's alphabetical key order, so a class
# nested in evidence cannot match; docs/reference/auto-drive-to-merge.md.
usage_limit_failure() {
  local m=""
  [ "${1:-0}" != "0" ] && [ -f "${2:-}" ] || return 1
  m="$(log_body "$2" | grep -E '^amplihack\.recipe\.failure_class \{')"
  [ -n "$m" ] || return 1
  if command -v jq >/dev/null 2>&1; then
    m="$(printf '%s\n' "$m" | jq -R -r 'ltrimstr("amplihack.recipe.failure_class ") | (fromjson? // null) | select(type == "object" and .class == "usage_limit") | .class' 2>/dev/null)"
  else
    m="$(printf '%s\n' "$m" | grep -E '^amplihack\.recipe\.failure_class \{"action":"[a-z_]+","attempt":[0-9]+,"class":"usage_limit"')"
  fi
  [ -n "$m" ]
}
# usage_limit_stop <rc> <log> <reason>: escalate USAGE_LIMIT and exit 1 on one.
usage_limit_stop() { usage_limit_failure "$1" "$2" || return 0; escalate "USAGE_LIMIT" "$3" usage_limit; exit 1; }

# orphan_stop <rc> <who>: a child that exited 75 lost the recipe run above the
# loop (#1976). Stop as ORPHANED with exit 75 before the progress row and the
# evaluator: the evaluator would inherit the same dead parent, stop at once
# and leave no LOOP_HEALTH line, so the loop would report a driver crash as
# STUCK. docs/reference/auto-drive-to-merge.md#how-the-loop-treats-exit-75.
AUTODRIVE_EXIT_ORPHANED=75 # a nested recipe run whose parent run exited (#1976)
orphan_stop() {
  [ "${1:-0}" = "$AUTODRIVE_EXIT_ORPHANED" ] || return 0
  escalate "ORPHANED" "${2} exited ${AUTODRIVE_EXIT_ORPHANED}: the recipe run above this loop exited, so the nested run stopped itself; see orphaned.json in the state directory" orphaned
  exit "$AUTODRIVE_EXIT_ORPHANED"
}

# held_by_lines: the `  held by:` lines of a stop (issue #2219). The reason is
# the evaluator's sentence and names no measurement, so without these the
# operator cannot see which criterion held the pull request up. Two sources:
#
#   the round record's held_by field  one condition line per disagreeing
#       measurement, written by autodrive_round_evidence.sh round-record.
#   ${RECORD}.findings                one `reported <id>` line per blocker
#       the round reported.
#
# BOTH ARE FILTERED HERE, ON READ, and differently. The state directory is
# already treated as possibly untrusted, and .findings holds unfiltered model
# output.
#   held_by   control characters become spaces and the entry is cut to 120
#             bytes. NOT the token byte set: a condition line legitimately
#             holds spaces, `=` and `:`, which that set would shred.
#   findings  control characters become spaces, then only [A-Za-z0-9_.:/-]
#             survives, then the entry is cut to 120 bytes. At most ten
#             print, then `(+N more)` — the same cap as _autodrive_qa_list.
#             The cap is a findings cap; every condition the record names is
#             printed. The count is of lines that SURVIVE the filter, so a
#             line that sanitises to nothing is neither printed nor counted
#             and `(+N more)` names exactly the lines withheld. That costs
#             the filter's two forks on a line past the cap, which is at
#             most the tail of one small file, once per stop.
# The two-space prefix is unconditional, so no attacker-influenced byte ever
# begins a line at column 0 — the column usage_limit_failure greps.
#
# The record is read with parameter expansion and not with a helper binary:
# the binary may itself be what failed. A record with no held_by field prints
# no condition line, so a record written before this change still works.
held_by_lines() {
  local -x LC_ALL=C # so the 120-byte cuts below are bytes, not characters
  local rec="${RECORD:-}" raw="" e="" n=0
  [ -n "$rec" ] && [ -f "$rec" ] && [ ! -L "$rec" ] || rec=""
  if [ -n "$rec" ]; then
    raw="$(tr '\001-\037\177' ' ' < "$rec" 2>/dev/null)"
    case "$raw" in
      *'"held_by":"'*)
        raw="${raw#*'"held_by":"'}"
        raw="${raw%%'"'*}"
        while [ -n "$raw" ]; do
          e="${raw%%|*}"
          case "$raw" in *'|'*) raw="${raw#*|}" ;; *) raw="" ;; esac
          [ -n "$e" ] || continue
          printf '  held by: %s\n' "${e:0:120}"
        done ;;
    esac
  fi
  [ -n "$rec" ] && [ -f "${rec}.findings" ] && [ ! -L "${rec}.findings" ] || return 0
  while IFS= read -r e; do
    e="$(printf '%s' "$e" | tr '\001-\037\177' ' ' | tr -cd 'A-Za-z0-9_.:/-')"
    [ -n "$e" ] || continue
    n=$((n + 1))
    [ "$n" -le 10 ] || continue
    printf '  held by: reported %s\n' "${e:0:120}"
  done < "${rec}.findings"
  [ "$n" -le 10 ] || printf '  held by: reported (+%s more)\n' "$((n - 10))"
}

escalate() { # escalate <verdict> <why> [usage_limit|budget|orphaned|waiting <wait_reason> <wait_target>]
  echo "AUTO_DRIVE_LOOP: ${1} — loop '${LOOP_NAME}' stops at round label '${ROUND_LABEL:-?}'." >&2
  echo "  Reason: ${2}" >&2
  held_by_lines >&2
  echo "  Round record: ${RECORD:-<none>}" >&2
  case "${3:-}" in
    usage_limit) echo "  This is a usage limit, not a judgement about progress. Run again when a login has capacity." >&2 ;;
    orphaned) echo "  This is a lost driver, not a judgement about progress. Relaunch auto-drive to resume from the recorded round." >&2 ;;
    budget) echo "  This is a usage budget set by the operator, not a judgement about progress. Relaunch with a higher budget to resume from the recorded round." >&2 ;;
    waiting) echo "  This is a wait, not a judgement about progress. Resume the drive after ${5}." >&2 ;;
    *) echo "  This is a judgement about absence of progress, not an attempt count." >&2 ;;
  esac
  local f=() v w="" # JSON-escaped; control characters become spaces (#1801)
  for v in "$LOOP_NAME" "$1" "${ROUND_LABEL:-?}" "$2"; do f+=("$(printf '%s' "$v" | tr '\001-\037' ' ' | sed 's/\\/\\\\/g; s/"/\\"/g')"); done
  # wait_reason and wait_target passed wait_valid, so they hold no quote (#2333).
  [ "${3:-}" != waiting ] || w="$(printf ',"wait_reason":"%s","wait_target":"%s"' "$4" "$5")"
  record_result "$(printf '{"loop":"%s","loop_result":"%s","round_label":"%s","reason":"%s"' "${f[@]}")${w}}"
}

# record_result <json>: prints the loop's result line and writes the same line
# to <state>/<loop>-result.json (mktemp + mv -f, never through a symlink).
record_result() {
  local t=""
  printf '%s\n' "$1"
  [ -n "${STATE_DIR:-}" ] && [ -d "${STATE_DIR}" ] || return 0
  t="$(umask 077; mktemp "${STATE_DIR}/.result.XXXXXX")" && printf '%s\n' "$1" > "$t" \
    && mv -f -- "$t" "${STATE_DIR}/${LOOP_NAME}-result.json" && return 0
  rm -f -- "$t"; echo "WARNING: could not write ${LOOP_NAME}-result.json in the state dir." >&2
}

# wait_ledger <phase>: the path, cleared, a child's usage-limit waits are written
# to, through AMPLIHACK_USAGE_WAIT_LEDGER (profile-command.md#usage-limit-wait).
wait_ledger() {
  local lf="${STATE_DIR}/.wait-${LOOP_NAME}-${ROUND_LABEL}-${1}"
  rm -f -- "$lf"; printf "%s" "$lf"
}

# usage_add <phase> <start SECONDS> [mark]: one usage row for a timed child,
# less the time it waited for a usage limit to lift (a row of its own, #1743).
usage_add() {
  local lf="${STATE_DIR}/.wait-${LOOP_NAME}-${ROUND_LABEL}-${1}"
  bash "${LOOP_HOME}/autodrive_budget.sh" add "$STATE_DIR" "$LOOP_NAME" "$ROUND_LABEL" "$1" "$((SECONDS - $2))" ${3:+--since "$3"} --waited "$lf" \
    || echo "WARNING: the ${1} usage row for ${ROUND_LABEL} was not recorded (see above)." >&2
  rm -f -- "$lf"
}

# post_stop <file>: the PR comment for a stop decided in code; never fatal.
post_stop() { bash "${LOOP_HOME}/autodrive_budget.sh" post "$STATE_DIR" "${AD_PR:-}" "$1" || echo "WARNING: the stop comment was not posted (see above); the loop result is unchanged." >&2; }

# stops_preflight: runs before autodrive_private_dir sets untrusted state
# entries aside. An untrusted usage file stops the run (return 1, the caller
# exits 2: fails closed); an untrusted progress file turns the no-progress
# rule off for this run (PROGRESS_OFF, fails open to the evaluator).
stops_preflight() {
  [ -d "$STATE_DIR" ] && [ ! -L "$STATE_DIR" ] || return 0
  bash "${LOOP_HOME}/autodrive_budget.sh" mark "$STATE_DIR" >/dev/null || return 1
  bash "${LOOP_HOME}/autodrive_progress.sh" last "$STATE_DIR" "$LOOP_NAME" >/dev/null
  [ $? -ne 3 ] || PROGRESS_OFF=1
}

# progress_stop <head sha> [<ci signal>]: the no-progress rule for a round that is not clean.
progress_stop() {
  local why rc
  [ -z "${PROGRESS_OFF:-}" ] || return 0
  local waiting; waiting="$(field "${RAW:-}" round_waiting false)"
  [ -z "${WAIT_REJECTED:-}" ] || waiting=false # a wait wait_stop refused is not skipped (#2472)
  why="$(bash "${LOOP_HOME}/autodrive_progress.sh" check "$STATE_DIR" "$LOOP_NAME" "$ROUND_LABEL" "$1" "${RECORD}.findings" "${2:-}" "$waiting")"
  rc=$?
  [ "$rc" -eq 10 ] || { [ "$rc" -eq 0 ] || echo "WARNING: the no-progress check exited ${rc}; the evaluator decides." >&2; return 0; }
  # Record the stopped round too, so a resumed run's summary names it.
  bash "${LOOP_HOME}/autodrive_progress.sh" record "$STATE_DIR" "$LOOP_NAME" "$ROUND_LABEL" "$1" 0 "${RECORD}.findings" "${2:-}"
  post_stop "${STATE_DIR}/${LOOP_NAME}-stuck-reason.txt"
  escalate "STUCK" "$why"
  exit 1
}

# budget_stop pre|post|before [--force]: BUDGET_EXHAUSTED when a limit is
# reached (--force: the nested crusty loop reached one). A malformed limit or
# an untrusted usage file exits 2 with no result line.
budget_stop() {
  local hit rc label="$ROUND_LABEL"
  [ "$1" = pre ] && label="$(bash "${LOOP_HOME}/autodrive_progress.sh" last "$STATE_DIR" "$LOOP_NAME")"
  hit="$(bash "${LOOP_HOME}/autodrive_budget.sh" check "$STATE_DIR" "$LOOP_NAME" "$1" "${label:-?}" "${AD_PR:-}" ${2:+"$2"} ${BUDGET_CTX[@]+"${BUDGET_CTX[@]}"})"
  rc=$?
  case "$rc" in
    0) return 0 ;;
    10) ;;
    *) echo "ERROR: loop '${LOOP_NAME}': the budget check exited ${rc} (see above); no agent starts." >&2; exit 2 ;;
  esac
  if [ "$1" = pre ]; then ROUND_LABEL="${label:-?}"; RECORD="${label:+${STATE_DIR}/${LOOP_NAME}-${label}.json}"; fi
  # --force: the nested crusty loop has already posted its own stop comment.
  [ "${2:-}" = --force ] || post_stop "${STATE_DIR}/autodrive-budget-summary.md"
  escalate "BUDGET_EXHAUSTED" "usage budget reached (${hit}); see autodrive-budget-summary.md in the state directory" budget
  exit 1
}

# wait_valid <wait_target> <wait_reason>: both match the closed patterns of
# docs/reference/autodrive-github-reads.md#waits-and-fixable-findings (#2333).
wait_valid() {
  local s="${1#*@}"
  case "$1" in
    ratelimit-reset@unknown|read@pr|read@pr-head|read@pr-number|read@checks|read@reviews|read@threads) ;;
    ratelimit-reset@*) case "$s" in ''|*[!0-9]*) return 1 ;; esac; [ "${#s}" -le 12 ] || return 1 ;;
    ci@*|mergeability@*) case "$s" in *[!0-9a-f]*) return 1 ;; esac; [ "${#s}" -eq 40 ] || return 1 ;;
    *) return 1 ;;
  esac
  case "$2" in ''|,*|*,|*[!a-z0-9,-]*) return 1 ;; esac
  [ "${#2}" -le 200 ]
}

# wait_stop: WAITING when every blocker of the round is a wait (#2472), read
# from the round record, or from the ${RECORD}.wait sidecar crusty step-01
# writes when no read of the PR head answered. Never STUCK and never DONE:
# loop_result WAITING, exit 0, resumable. A target or reason outside the
# closed patterns is ignored with a WARNING and sets WAIT_REJECTED, so the
# no-progress rule counts the round (it is not skipped as waiting) and the
# evaluator judges it. AUTODRIVE_EXIT_WAITING is the completion dispatch's exit
# when the PR head is unavailable; the loop maps it to WAITING and never
# exits with it.

AUTODRIVE_EXIT_WAITING=76
wait_stop() {
  local src="" t r
  if [ -f "$RECORD" ] && [ ! -L "$RECORD" ]; then src="${RAW:-}"
  elif [ -f "${RECORD}.wait" ] && [ ! -L "${RECORD}.wait" ] && [ -O "${RECORD}.wait" ]; then src="$(head -c 4096 -- "${RECORD}.wait")"; fi
  [ -n "$src" ] && [ "$(field "$src" round_waiting false)" = true ] || return 0
  t="$(field "$src" wait_target '')"
  r="$(field "$src" wait_reason '')"
  [ -n "$r" ] || r="$(field "$src" waits '[]' | tr -d '[]" ' | LC_ALL=C tr -cd 'a-z0-9,-')"
  if ! wait_valid "$t" "$r"; then
    echo "WARNING: ${ROUND_LABEL} says it is waiting, but its wait target or reason is not in the closed set; the round is judged as usual." >&2
    WAIT_REJECTED=1 # progress_stop then counts this round like any other
    return 0
  fi
  escalate "WAITING" "every blocker this round is a wait (${r})" waiting "$r" "$t"
  exit 0
}

# dispatch_wait_stop <rc> <evaluator_invoked> <dispatch json>: the completion
# dispatch found the PR head unavailable on every read path (exit 76, #2333).
# The round is WAITING: the dispatch's own line is the result, the loop exits
# 0, and 76 is never re-exited. Anything else returns to the caller.
dispatch_wait_stop() {
  [ "${1:-}" = "$AUTODRIVE_EXIT_WAITING" ] && [ "${2:-}" = false ] && [ "$(field "${3:-}" loop_result '')" = WAITING ] || return 0
  echo "AUTO_DRIVE_LOOP: WAITING — loop '${LOOP_NAME}' stops at round label '${ROUND_LABEL:-?}': the PR head is unavailable on every read path; resume the drive later." >&2
  record_result "$3"
  exit 0
}
