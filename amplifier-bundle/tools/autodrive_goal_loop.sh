#!/usr/bin/env bash
# autodrive_goal_loop.sh drives one fresh goal-loop turn at a time.
# It dispatches each turn at constant depth without timers or retry counters.
# Repeated identical pre-agent failures publish one hand-off, then use the
# existing owner wait until guidance, pull-request, worktree, or stop changes.
# See docs/reference/autodrive-goal-loop.md for the contract and records.

set -uo pipefail
LOOP_HOME="$(cd "$(dirname "${BASH_SOURCE[0]}")" 2>/dev/null && pwd -P)" || exit 2
BUNDLE_ROOT="${LOOP_HOME%/tools}"
for lib in autodrive_goal_lib.sh autodrive_refusal.sh autodrive_state.sh; do
  # shellcheck source=/dev/null
  . "$LOOP_HOME/$lib" || { echo "ERROR: autodrive_goal_loop.sh: $lib is missing beside it" >&2; exit 2; }
done
AMPLIHACK_BIN="${AMPLIHACK_BIN:-amplihack}"
export GIT_PAGER=cat GH_PAGER=cat PAGER=cat LESS=FRX
SHARED_WORDS=" continue wait-owner handoff ready stop "
CALLER_UMASK="$(umask)"

GOAL_TOOL=""; GOAL_FILE=""; STATE=""; REPO=""; TASK_FILE=""; NOTES_FILE=""; INVOCATION=""; RECOVERY=""; ONCE=false
LOOP_END_WRITTEN=false; TURN=0; RUN_ID=""; MERGE_ALLOWED=false; SUBJECT=""
PRE_AGENT_SIGNATURE=""

say() { printf 'auto-drive goal loop: %s\n' "$*" >&2; }
finish() { # finish <rc> [error]: the one stdout line, then exit
  # A loop-end.json this driver did not write is never left for the report.
  [ "$LOOP_END_WRITTEN" = true ] || [ -z "$STATE" ] || rm -f -- "$STATE/loop-end.json"
  if [ "$LOOP_END_WRITTEN" = true ] && [ -f "$STATE/loop-end.json" ]; then
    jq -c --argjson rc "$1" '. + {exit:$rc}' "$STATE/loop-end.json" 2>/dev/null || printf '{"exit":%s}\n' "$1"
  else
    jq -cn --argjson rc "$1" --arg e "${2:-}" --argjson t "$TURN" '{end:null, exit:$rc, turn:$t, error:$e}'
  fi
  exit "$1"
}
usage() { echo "ERROR: autodrive_goal_loop.sh: $1" >&2; finish 2 "$1"; }

while [ $# -gt 0 ]; do
  case "$1" in
    --goal-tool) GOAL_TOOL="${2:-}"; shift 2 ;;
    --goal-file) GOAL_FILE="${2:-}"; shift 2 ;;
    --state-dir) STATE="${2:-}"; shift 2 ;;
    --repo) REPO="${2:-}"; shift 2 ;;
    --task-file) TASK_FILE="${2:-}"; shift 2 ;;
    --notes-file) NOTES_FILE="${2:-}"; shift 2 ;;
    --invocation) INVOCATION="${2:-}"; shift 2 ;;
    --recovery-dir) RECOVERY="${2:-}"; shift 2 ;;
    --once) ONCE=true; shift ;;
    *) usage "unknown argument '$(printf '%s' "$1" | goal_clean_line 64)'" ;;
  esac
done
# --- checks before turn 1 (exit 2; no turn runs) ---------------------------
[[ "$GOAL_TOOL" =~ ^[a-z0-9_]+\.sh$ ]] || usage "--goal-tool must name a script in the bundle's tools/ folder"
GOAL_PATH="$(goal_bundle_file "$BUNDLE_ROOT" "tools/$GOAL_TOOL")" || usage "the goal tool is not a regular file in the launch bundle"
GOAL_TEXT="$(goal_bundle_file "$BUNDLE_ROOT" "$GOAL_FILE")" || usage "the goal text is not a regular file in the launch bundle"
TURN_RECIPE="$(goal_bundle_file "$BUNDLE_ROOT" recipes/autodrive-goal-turn.yaml)" || usage "the turn recipe is missing from the launch bundle"
TURN_TOOL="$LOOP_HOME/autodrive_goal_turn.sh"
case "$STATE" in /*) ;; *) usage "--state-dir must be an absolute path" ;; esac
case "/$STATE/" in */../*) usage "--state-dir must not hold a .. component" ;; esac
[ -n "$REPO" ] && [ -d "$REPO" ] || usage "--repo must be a folder"
REPO="$(cd "$REPO" && pwd -P)" || usage "--repo cannot be entered"
WORK="$REPO" # the folder the agent works in; the goal's reading may name the PR's own worktree
autodrive_private_dir "$STATE" || finish 2 "the state folder could not be made private"
STATE="${STATE%/}"
umask 077
if [ -L "$STATE/turns" ]; then usage "$STATE/turns is a symlink"; fi
mkdir -p -- "$STATE/turns" && [ -O "$STATE/turns" ] && chmod -- 700 "$STATE/turns" || usage "$STATE/turns cannot be made a private folder"
W="$(mktemp -d "$STATE/.loop.XXXXXX")" || finish 1 "no private scratch folder"
trap 'rm -rf -- "$W"' EXIT
trap 'stopped_by 129' HUP; trap 'stopped_by 130' INT; trap 'stopped_by 143' TERM
if [ -n "$TASK_FILE" ] && [ -f "$TASK_FILE" ]; then cp -- "$TASK_FILE" "$W/TASK"; else : > "$W/TASK"; fi
if [ -n "$NOTES_FILE" ] && [ -f "$NOTES_FILE" ]; then cp -- "$NOTES_FILE" "$W/NOTES"; else : > "$W/NOTES"; fi
cp -- "$GOAL_TEXT" "$W/GOAL" || finish 1 "the goal text cannot be read"
goal_call() { # the goal tool, with the contract and the driver's own state only
  ( cd "$REPO" && AUTODRIVE_GOAL_CONTRACT=1 AUTODRIVE_STATE_DIR="$STATE" AUTODRIVE_REPO="$REPO" AUTODRIVE_WORKTREE="$WORK" \
      AUTODRIVE_LOOP_HOME="$LOOP_HOME" AUTODRIVE_INVOCATION_FILE="$INVOCATION" \
      AUTODRIVE_TASK_FILE="$W/TASK" AUTODRIVE_TURN_DIR="${TD:-}" AUTODRIVE_GUIDANCE_UNKNOWN="${GUIDE_UNKNOWN:-false}" \
      AUTODRIVE_GUIDANCE_MARK="${MARK:-}" AUTODRIVE_GUIDANCE_TARGET="${GUIDE_TARGET:-}" exec bash "$GOAL_PATH" "$@" )
}
turn_call() { # the shared turn tool, with what it needs from the driver
  AUTODRIVE_STATE_DIR="$STATE" AUTODRIVE_GOAL_TOOL="$GOAL_PATH" AUTODRIVE_GOAL_FILE="$GOAL_TEXT" \
    AUTODRIVE_GOAL_ACTIONS="$ACTIONS" AUTODRIVE_MERGE_ALLOWED="$MERGE_ALLOWED" \
    AUTODRIVE_GUIDANCE_MARK="$MARK" bash "$TURN_TOOL" "$@"
}
if [ -n "$INVOCATION" ]; then
  AUTH="$("$AMPLIHACK_BIN" orch helper autodrive authorization --invocation "$INVOCATION" --directory "$STATE" 2>"$W/auth.err")" \
    || say "WARNING: the captured authorisation could not be read ($(goal_clean_line 300 < "$W/auth.err")); merging is treated as off and the end record names no run"
  RUN_ID="$(printf '%s' "${AUTH:-}" | jq -r '.run_id // empty | strings' 2>/dev/null)"
  [ "$(printf '%s' "${AUTH:-}" | jq -r '.allows_merge' 2>/dev/null)" = true ] && MERGE_ALLOWED=true
fi
ACTIONS="$(goal_call actions 2>"$W/actions.err")" || usage "the goal tool refused the contract (actions: $(goal_clean_line 200 < "$W/actions.err"))"
ACTIONS="$(printf '%s' "$ACTIONS" | tr '\n\t' '  ')"
for word in $ACTIONS; do
  goal_is_word "$word" || usage "the goal tool names an action that is not a word"
  case "$SHARED_WORDS" in *" $word "*) usage "the goal tool's action '$word' collides with a shared action word" ;; esac
done
SUBJECT="the goal of ${GOAL_TOOL}"

# loop-end.json is written only by this driver. One left by an earlier run, or
# by an agent, is removed, so the report never reads it as this run's end.
rm -f -- "$STATE/loop-end.json"

# The guidance mark: the newest entry the last reading saw (an earlier run's too).
MARK=""
for t in $(goal_turn_numbers "$STATE/turns" | sort -rn); do
  [ -f "$STATE/turns/$t/reading.json" ] && [ ! -L "$STATE/turns/$t/reading.json" ] || continue
  MARK="$(jq -r '.guidance_newest // empty | strings' "$STATE/turns/$t/reading.json" 2>/dev/null)"; break
done
goal_is_entry "$MARK" || MARK=""

end_run() { # end_run <word> <detail> [extra object]
  local extra="${3:-}"
  [ -n "$extra" ] || extra='{}'
  jq -cn --arg e "$1" --arg r "$RUN_ID" --arg s "$SUBJECT" --arg d "$(printf '%s' "$2" | goal_clean_line 2000)" \
    --argjson t "$TURN" --arg time "$(date -u +%Y-%m-%dT%H:%M:%SZ)" --argjson x "$extra" \
    '{end:$e, run_id:$r, subject:$s, detail:$d, turn:$t, time:$time} + $x' | goal_atomic_write "$STATE/loop-end.json" \
    || { say "ERROR: loop-end.json could not be written"; finish 1 "loop-end.json could not be written"; }
  LOOP_END_WRITTEN=true
  say "the run ended: $1 ($(printf '%s' "$2" | goal_clean_line 300))"
}
stopped_by() { end_run stopped "the run was stopped by a signal (exit $1) at turn ${TURN}"; finish "$1"; }
passed_up() { # passed_up <rc> <who>: 79 and 75 end the run at once, unchanged
  case "$1" in
    79) end_run refused "exit 79 from ${2}: a terminal policy refusal, never retried"; finish 79 ;;
    75) end_run orphaned "exit 75 from ${2}: the recipe run above this loop exited"; finish 75 ;;
  esac
}

AFTER_MERGE='[]'
final_check() {
  local word rc
  word="$(goal_call final 2>"$W/final.err")"; rc=$?
  passed_up "$rc" "the goal's final reading"
  [ "$rc" -eq 0 ] || { say "the final reading failed (exit ${rc}: $(goal_clean_line 200 < "$W/final.err")); the turn reads the state again"; return 0; }
  word="$(printf '%s' "$word" | tr -d '[:space:]')"
  [ -n "$word" ] || return 0
  goal_is_word "$word" || { say "WARNING: the goal's final word is not a word; ignored"; return 0; }
  if [ "$word" = merged ]; then
    end_run merged "the goal tool reads the goal merged" "$(jq -cn --argjson a "$AFTER_MERGE" '{after_merge:$a}')"
  else
    end_run "$word" "the goal tool's final reading is ${word}"
  fi
  finish 0
}

run_turn() { # one fresh turn run, without the driver's AUTODRIVE_* state
  ( umask "$CALLER_UMASK"
    for v in $(compgen -e); do case "$v" in AUTODRIVE_*|AMPLIHACK_AUTODRIVE_*) unset "$v" ;; esac; done
    [ -z "$GUIDE_DIRS" ] || export AMPLIHACK_RUN_GUIDANCE_DIRS="$GUIDE_DIRS"
    exec "$AMPLIHACK_BIN" recipe run "$TURN_RECIPE" --working-dir "$WORK" \
      -c "turn_dir=${TD}" -c "repo_path=${WORK}" \
      --context-from-file "goal_text=$W/GOAL" --context-from-file "task_description=$W/TASK" \
      --context-from-file "goal_notes=$W/NOTES" --context-from-file "turn_reading=${TD}/reading.json" \
      --context-from-file "previous_record=$W/PREV" ) >"$TD/turn.log" 2>&1
}

guidance_show() { # guidance_show <since> <out>: metadata only (never text)
  "$AMPLIHACK_BIN" recipe guidance show --json "--since=$1" ${GUIDE_TARGET:+"$GUIDE_TARGET"} > "$2" 2>"$2.err"
}

# failed_turn <rc>: a turn run that failed after the runner's own handling
# (R1). The evidence goes into result.json (the next reading's `previous`).
# Nothing here classifies the failure or reads the log's wording: class,
# retryable and the step lists are the runner's own marker, copied as evidence.
# What else runs, from facts read directly (--once, `amplihack config get`, the
# bundle's files, whether the marker shows no step ran), is in docs/
# reference/autodrive-goal-loop.md#failures-inside-a-turn. Returns 0 for the next turn.
bundle_whole() {
  local f
  for f in recipes/autodrive-goal-turn.yaml tools/autodrive_goal_turn.sh "tools/$GOAL_TOOL"; do
    goal_bundle_file "$BUNDLE_ROOT" "$f" >/dev/null 2>&1 || return 1
  done
}
failed_turn() {
  local rc="$1" summary judged=false limit=false lwait="" recovery=next-turn wrc err pre=false repeat=false nosteps=false marker_valid=false marker_sig=""
  local why="the next turn starts at once; its agent reads this failure and the turn log and decides"
  classify_child "$TD/turn.log" >"$W/marker" # sets CHILD_FAILURE_* from the runner's last marker
  summary="$(child_failure_summary "$rc" "$TD/turn.log")"
  sed 's/^amplihack\.recipe\.failure_class //' "$W/marker" | jq -c '{completed:[.completed_steps // [] | .[] | strings], failed:[.failed_steps // [] | .[] | strings]}' >"$W/steps" 2>/dev/null
  goal_json_file "$W/steps" 65536 || echo '{"completed":[],"failed":[]}' >"$W/steps"
  if sed 's/^amplihack\.recipe\.failure_class //' "$W/marker" | jq -e 'type == "object" and (.completed_steps | type == "array") and (.failed_steps | type == "array")' >/dev/null 2>&1; then
    marker_valid=true; sed 's/^amplihack\.recipe\.failure_class //' "$W/marker" | jq -e '.completed_steps == [] and .failed_steps == []' >/dev/null 2>&1 && nosteps=true
  fi
  jq -e '.completed | index("decide") != null' "$W/steps" >/dev/null 2>&1 && judged=true; [ -f "$TD/decision.json" ] && [ ! -L "$TD/decision.json" ] && judged=true
  err="$(goal_last_line "$TD/turn.log")"; [ -n "$err" ] || err="$summary"
  if [ "$judged" = false ] && [ "$marker_valid" = true ] && [ "$CHILD_FAILURE_CLASS" != unknown ]; then
    pre=true
    marker_sig="$(sed 's/^amplihack\.recipe\.failure_class //' "$W/marker" | jq -c 'if type == "object" then {class:(.class // null), detail:(.signal // null)} else empty end' 2>/dev/null)"
    [ -n "$marker_sig" ] || pre=false
    [ "$pre" = true ] && [ -n "$PRE_AGENT_SIGNATURE" ] && [ "$marker_sig" = "$PRE_AGENT_SIGNATURE" ] && repeat=true
    [ "$pre" = true ] && PRE_AGENT_SIGNATURE="$marker_sig" || PRE_AGENT_SIGNATURE=""
  else
    PRE_AGENT_SIGNATURE=""
  fi
  goal_run_log_line "auto-drive goal loop: turn ${TURN} failed (${summary}): ${err}"
  if [ "$CHILD_FAILURE_CLASS" = usage_limit ]; then
    limit=true
    lwait="$(cd "$WORK" && "$AMPLIHACK_BIN" config get recipe.usage_limit_wait 2>/dev/null | tr -d '[:space:]')"
  fi
  if [ "$ONCE" = true ]; then
    recovery=ended; why="this was a --once turn, which asks for one turn"
  elif [ "$lwait" = false ]; then
    recovery=ended
    why="the runner classed the failure usage_limit, and the operator's own recipe.usage_limit_wait is false, which asks for a limit to be passed up at once; the same command resumes the run"
  elif ! bundle_whole; then
    recovery=owner-wait
    why="the launch bundle no longer holds the turn recipe, the turn tool or the goal tool, so no turn can run until the owner restores it; the run waits for the owner's word"
  elif [ "$repeat" = true ]; then
    recovery=owner-wait
    why="the turn failed before the agent started with the same structured failure class and detail as the previous turn; the owner wait is armed until guidance, a pull-request or worktree change, or the owner's stop"
  elif [ "$pre" = true ]; then
    why="the turn failed before the agent started; a different failure or an event clears the comparison, and the next turn reads the failure"
  fi
  jq -cn --arg s "the turn run failed after the runner's own recovery (${summary}): ${why}" --arg rc "$rc" \
    --arg class "$CHILD_FAILURE_CLASS" --arg detail "$CHILD_FAILURE_SIGNAL" --arg retryable "$CHILD_FAILURE_RETRYABLE" --slurpfile st "$W/steps" \
    --argjson judged "$judged" --argjson limit "$limit" --arg lw "$lwait" --arg rec "$recovery" --arg log "$TD/turn.log" \
    '{outcome:"turn-failed", summary:$s, recovery:$rec,
      failure:{exit:($rc | tonumber? // $rc), class:$class, detail:$detail, retryable:$retryable, usage_limit:$limit,
        usage_limit_wait:(if $lw == "true" then true elif $lw == "false" then false else null end), agent_finished:$judged,
        completed_steps:$st[0].completed, failed_steps:$st[0].failed, log:$log}}' | goal_atomic_write "$TD/result.json"
  turn_call record "$TD" || say "WARNING: the record of failed turn ${TURN} could not be written"
  say "turn $(printf '%03d' "$TURN"): the turn run failed (${summary}); ${why}"
  case "$recovery" in
    next-turn) return 0 ;;
    ended)
      if [ "$limit" = true ]; then end_run usage_limit "$why"; else end_run failed "the turn run failed (${summary}) and this was a --once turn"; fi
      finish 1 "$why" ;;
  esac
  # owner-wait: an event, never a timer. Reuse the existing wait-owner action
  # for an unchanged pre-agent failure. Bundle loss keeps its older direct
  # guidance wait and fallback hand-off.
  if [ "$repeat" = true ]; then
    { printf 'The turn failed before the agent started twice with the same structured failure class and detail. The failure is %s. A changed guidance entry, pull request, worktree, or owner stop is required before another turn can start.\n\n' "$marker_sig"
      printf 'Hand-off from the auto-drive goal loop, turn %03d. Owner: the operator who started this run. Decision: provide a change that clears the repeated pre-agent failure or stop the run.\n' "$TURN"
    } | goal_neutralise | goal_atomic_write "$TD/end.md"
    if goal_secret_screen "$TD/end.md"; then
      goal_call publish "$TD/end.md" >"$TD/publish.log" 2>&1 || say "WARNING: the repeated-failure hand-off could not be published; the text is in end.md"
    else
      rm -f -- "$TD/end.md"
    fi
    jq -cn '{schema_version:1, valid:true, action:"wait-owner", args:{owner:"the operator who started this run", question:"the repeated pre-agent failure must change before another turn starts"}, why:"the repeated pre-agent failure is unchanged", problem:null}' \
      | goal_atomic_write "$TD/decision.json"
    turn_call wait-owner "$TD"; wrc=$?
    passed_up "$wrc" "the owner wait after failed turn ${TURN}"
    if [ "$wrc" -eq 0 ]; then
      if [ "$(jq -r '.outcome // empty' "$TD/result.json" 2>/dev/null)" = ended ] && [ "$(jq -r '.end // empty' "$TD/result.json" 2>/dev/null)" = handoff ]; then
        end_run handoff "the repeated pre-agent failure could not arm the owner wait; the question was handed off"
        finish 0
      fi
      PRE_AGENT_SIGNATURE=""; say "the owner wait returned; the next turn reads the changed state"; return 0
    fi
  else
    # Guidance that arrived since this turn's reading wakes it at once.
    "$AMPLIHACK_BIN" recipe guidance wait "--since=$NEWEST" ${GUIDE_TARGET:+"$GUIDE_TARGET"} >"$TD/owner-wait.json" 2>"$TD/owner-wait.err"; wrc=$?
    passed_up "$wrc" "the owner wait after failed turn ${TURN}"
    if [ "$wrc" -eq 0 ]; then say "new guidance arrived; the next turn reads it"; return 0; fi
  fi
  # No wake could be armed (3), or the wait itself failed: the question goes to
  # the operator, as wait-owner's fallback does. The text is mechanical.
  { printf 'Turn %03d of the auto-drive goal loop failed (%s), and the launch bundle no longer holds the turn recipe, the turn tool or the goal tool, so no turn can run.\n\n' "$TURN" "$summary"
    printf 'The run would wait for the owner'"'"'s word before the next turn, but that wait could not run here (exit %s: %s). The turn log is %s.\n\n' \
      "$wrc" "$(goal_clean_line 300 < "$TD/owner-wait.err")" "$TD/turn.log"
    printf 'Hand-off from the auto-drive goal loop, turn %03d. Owner: the operator who started this run. Decision: whether to restore the launch bundle and start the run again.\n' "$TURN"
  } | goal_neutralise | goal_atomic_write "$TD/end.md"
  if goal_secret_screen "$TD/end.md"; then goal_call publish "$TD/end.md" >"$TD/publish.log" 2>&1 || say "WARNING: the hand-off comment could not be published; the text is in end.md"
  else rm -f -- "$TD/end.md"; fi
  end_run handoff "Owner: the operator who started this run. Turn ${TURN} failed, the launch bundle no longer holds the files a turn needs (${summary}), and no wake for the owner's word could be armed (exit ${wrc})"
  finish 0
}

GUIDE_TARGET=""; GUIDE_DIRS=""; GUIDE_UNKNOWN=false
while :; do
  final_check
  # --- a new turn folder: numbers continue after the highest one -----------
  n="$(goal_turn_numbers "$STATE/turns" | tail -n 1)"; n=$(( 10#${n:-0} + 1 ))
  until mkdir -- "$STATE/turns/$(printf '%03d' "$n")" 2>/dev/null; do
    [ -e "$STATE/turns/$(printf '%03d' "$n")" ] || finish 1 "the turn folder cannot be created"
    n=$((n + 1))
  done
  TURN="$n"; TD="$STATE/turns/$(printf '%03d' "$n")"
  say "turn $(printf '%03d' "$n"): reading"
  # --- read: the goal's state, then guidance --------------------------------
  goal_call read "$TD" >"$TD/read.log" 2>&1; rc=$?
  passed_up "$rc" "the goal's reading"
  [ "$rc" -eq 0 ] || say "the goal's reading failed (exit ${rc}); the reading records it"
  if goal_json_file "$TD/goal.json" 1048576; then
    s="$(jq -r '.subject // empty | strings' "$TD/goal.json" | goal_clean_line 200)"; [ -z "$s" ] || SUBJECT="$s"
    GUIDE_TARGET="$(jq -r '.guidance_target // empty | strings' "$TD/goal.json")"
    [[ "$GUIDE_TARGET" =~ ^https://[A-Za-z0-9._~/:@%+-]+$ ]] || GUIDE_TARGET=""
    wt="$(jq -r '.worktree // empty | strings' "$TD/goal.json")"
    if [[ "$wt" == /* ]] && ! [[ "$wt" =~ [[:cntrl:]] ]] && [ -d "$wt" ]; then WORK="$wt"; fi
  fi
  guidance_show "$MARK" "$TD/guidance.json" || say "guidance could not be read ($(goal_clean_line 200 < "$TD/guidance.json.err")); the reading records it"
  mv -f -- "$TD/guidance.json.err" "$TD/guidance.err" 2>/dev/null
  GUIDE_DIRS="$(jq -r '[.folders[]?.path | strings] | join(":")' "$TD/guidance.json" 2>/dev/null)"
  turn_call reading "$TD" || finish 1 "reading.json could not be written"
  NEWEST="$(jq -r '.guidance_newest // empty | strings' "$TD/reading.json")"; goal_is_entry "$NEWEST" || NEWEST="$MARK"
  pl="$(goal_turn_numbers "$STATE/turns" | awk -v me="$n" '$0 + 0 < me + 0' | tail -n 1)"
  if [ -n "$pl" ] && [ -f "$STATE/turns/$pl/record.md" ] && [ ! -L "$STATE/turns/$pl/record.md" ]; then
    head -c 65536 -- "$STATE/turns/$pl/record.md" > "$W/PREV"
  else
    printf 'There is no earlier turn record.\n' > "$W/PREV"
  fi
  # --- the turn run: one agent step names the action ------------------------
  say "turn $(printf '%03d' "$n"): the agent step is deciding"
  run_turn; rc=$?
  # Only this driver writes loop-end.json, and only the action below writes
  # this turn's result: anything the agent step left under those names goes.
  rm -f -- "$STATE/loop-end.json" "$TD/result.json" "$TD/end.md"
  passed_up "$rc" "the turn run"
  # #1437: the runner's structural_refusal marker is a terminal refusal too,
  # whatever the exit; nothing reads the transcript's wording.
  if terminal_refusal "$rc" "$TD/turn.log"; then
    end_run refused "the turn run returned a terminal policy refusal (${TERMINAL_WHY}); it is never retried"
    finish 79
  fi
  if [ "$rc" -ne 0 ]; then
    tail -n 60 -- "$TD/turn.log" >&2
    # The guidance mark stays where it was: this turn's agent may not have
    # read the guidance it was given, so the next reading shows it as new.
    failed_turn "$rc"
    continue
  fi
  PRE_AGENT_RUN=0; ACTION=""
  if goal_json_file "$TD/decision.json" 131072 && [ "$(jq -r '.valid' "$TD/decision.json")" = true ]; then
    ACTION="$(jq -r '.action // empty | strings' "$TD/decision.json")"
    goal_is_word "$ACTION" || ACTION=""
  fi
  # --- the required outcomes the agent named go into the run's ledger -------
  # (R2) before any action reads it; a rejected one is answered in a comment.
  turn_call outcomes "$TD" 2>"$TD/outcomes.err"; orc=$?
  passed_up "$orc" "the outcome ledger"
  [ "$orc" -eq 0 ] || say "WARNING: the outcome ledger step failed (exit ${orc}: $(goal_clean_line 300 < "$TD/outcomes.err")); a merge reads the ledger as it stands"
  # --- guidance that arrived during the turn skips the action ---------------
  # A recheck that cannot be read leaves guidance unknown (R2): the goal's
  # actions are told so (AUTODRIVE_GUIDANCE_UNKNOWN), and a goal's merge
  # refuses on it, so the next turn's agent judges before any merge.
  SKIP=false; GUIDE_UNKNOWN=false
  if guidance_show "$NEWEST" "$W/after.json"; then
    [ "$(jq '[.entries[]? | select(.new)] | length' "$W/after.json" 2>/dev/null)" = 0 ] || SKIP=true
  else
    GUIDE_UNKNOWN=true
    cp -- "$W/after.json.err" "$TD/guidance-after.err" 2>/dev/null
    say "guidance could not be read again after the decision ($(goal_clean_line 200 < "$W/after.json.err")); guidance is unknown for this turn's action"
  fi
  if [ "$SKIP" = true ]; then
    jq -cn --arg a "${ACTION:-none}" '{outcome:"skipped-guidance", summary:("new guidance arrived after this turn'"'"'s reading, so the action " + $a + " did not run; the next turn decides again")}' \
      | goal_atomic_write "$TD/result.json"
  elif [ -z "$ACTION" ]; then
    jq -cn --arg p "$(jq -r '.problem // "no decision.json was written" | tostring' "$TD/decision.json" 2>/dev/null || echo 'no decision.json was written')" \
      '{outcome:"no-action", summary:("no mechanical action ran: " + $p)}' | goal_atomic_write "$TD/result.json"
  else
    case "$ACTION" in
      continue)
        jq -cn '{outcome:"continued", summary:"no mechanical action; the next turn reads the live state again"}' | goal_atomic_write "$TD/result.json" ;;
      wait-owner)
        turn_call wait-owner "$TD"; passed_up "$?" "the wait-owner action" ;;
      handoff|ready|stop)
        turn_call end "$ACTION" "$TD"; passed_up "$?" "the ${ACTION} action" ;;
      *)
        if [[ " $ACTIONS " == *" $ACTION "* ]]; then
          a="$(jq -c '.args.after_merge // [] | if type == "array" and length <= 32 and all(.[]; type == "string") and (map(length) | add // 0) <= 8192 then . else [] end' "$TD/decision.json" 2>/dev/null)"
          [ -z "$a" ] || AFTER_MERGE="$a"
          goal_call act "$ACTION" "$TD" >"$TD/act.log" 2>&1; rc=$?
          passed_up "$rc" "the ${ACTION} action"
          goal_json_file "$TD/result.json" 65536 || jq -cn --arg a "$ACTION" --argjson rc "$rc" \
            '{outcome:"action-failed", summary:("the " + $a + " action exited \($rc) and wrote no result")}' | goal_atomic_write "$TD/result.json"
        else
          jq -cn --arg a "$ACTION" '{outcome:"no-action", summary:("no mechanical action ran: " + $a + " is not an action of this goal")}' | goal_atomic_write "$TD/result.json"
        fi ;;
    esac
  fi
  # #2539: a hand-off that step recovery recorded during this turn (inside a
  # review round, say) is an agent's decision for an owner; it ends the run.
  if [ -n "$RECOVERY" ]; then
    rec="$("$AMPLIHACK_BIN" orch helper step-recovery report --dir "$RECOVERY" 2>&1)"; rrc=$?
    if [ "$rrc" -eq 81 ]; then
      jq -cn --arg d "a hand-off recorded by step recovery: $(printf '%s' "$rec" | goal_clean_line 600)" \
        '{outcome:"recovery-handoff", summary:$d}' | goal_atomic_write "$TD/result.json"
      turn_call record "$TD" || :
      end_run handoff "a hand-off recorded by step recovery during turn ${TURN}: $(printf '%s' "$rec" | goal_clean_line 600)"
      finish 81
    fi
  fi
  turn_call record "$TD" || say "WARNING: the turn record could not be written"
  say "turn $(printf '%03d' "$n"): ${ACTION:-no action} -> $(jq -r '.outcome // "none"' "$TD/result.json" 2>/dev/null)"
  MARK="$NEWEST"
  # Only the shared end actions end a run here, and only with their own words;
  # `merged` comes only from the goal tool's final reading above.
  case "$ACTION:$SKIP:$(jq -r '.outcome // empty' "$TD/result.json" 2>/dev/null)" in
    handoff:false:ended|ready:false:ended|stop:false:ended|wait-owner:false:ended) e="$(jq -r '.end // empty | strings' "$TD/result.json")" ;;
    *) e="" ;;
  esac
  case "$e" in handoff|ready|stopped) ;; *) e="" ;; esac
  if [ -n "$e" ]; then
    extra='{}'; [ "$e" != ready ] || extra="$(jq -cn --argjson v "$([ "$MERGE_ALLOWED" = true ] && echo false || echo true)" '{verify_readiness:$v}')"
    end_run "$e" "$(jq -r '.detail // empty | strings' "$TD/result.json")" "$extra"
    finish 0
  fi
  [ "$ONCE" = false ] || finish 0
done
