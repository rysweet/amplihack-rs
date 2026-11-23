#!/usr/bin/env bash
# autodrive_goal_turn.sh: the per-turn steps of the shared goal loop (#3129).
#
#   autodrive_goal_turn.sh reading <turn_dir>       goal.json + guidance.json -> reading.json
#   autodrive_goal_turn.sh decision <turn_dir>      the agent's reply -> decision.json (exit 0)
#   autodrive_goal_turn.sh end <handoff|ready|stop> <turn_dir>
#                                                   end.md, screened, published -> result.json
#   autodrive_goal_turn.sh wait-owner <turn_dir>    wait for the owner's answer -> result.json
#   autodrive_goal_turn.sh record <turn_dir>        record.md, for people; nothing parses it
#
# Every subcommand re-checks <turn_dir> (autodrive_goal_lib.sh) and exits 2
# when it is not a private turn folder. The driver passes the rest through
# AUTODRIVE_* variables; the turn run (the agent step) gets none of them, so
# `decision` reads only the turn folder and the runner's context.
# Exit codes: 0 done, 2 usage or a refused turn folder, 75 and 79 passed up.
# docs/reference/autodrive-goal-loop.md is the specification.

set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" 2>/dev/null && pwd -P)" || exit 2
# shellcheck source=autodrive_goal_lib.sh
. "$HERE/autodrive_goal_lib.sh" || { echo "ERROR: autodrive_goal_lib.sh is missing beside $0" >&2; exit 2; }
AMPLIHACK_BIN="${AMPLIHACK_BIN:-amplihack}"
umask 077

SUB="${1:-}"; WORD=""
case "$SUB" in
  end) WORD="${2:-}"; TD="${3:-}"; [ "$#" -eq 3 ] || { echo "usage: $0 end <handoff|ready|stop> <turn_dir>" >&2; exit 2; } ;;
  reading|decision|wait-owner|record) TD="${2:-}"; [ "$#" -eq 2 ] || { echo "usage: $0 $SUB <turn_dir>" >&2; exit 2; } ;;
  *) echo "usage: $0 reading|decision|end|wait-owner|record ... <turn_dir>" >&2; exit 2 ;;
esac
goal_check_turn_dir "$TD" || exit 2
case "$SUB:$WORD" in end:handoff|end:ready|end:stop|reading:|decision:|wait-owner:|record:) ;; *) echo "ERROR: end takes handoff, ready or stop" >&2; exit 2 ;; esac

result() { # result <outcome> <summary> [extra json object]
  local extra="${3:-}"
  [ -n "$extra" ] || extra='{}'
  jq -cn --arg o "$1" --arg s "$2" --argjson x "$extra" '{outcome:$o, summary:$s} + $x' | goal_atomic_write "$TD/result.json"
}
# arg <name>: one string argument of the decision (or of ARGS_FILE).
ARGS_FILE=""
arg() { jq -r --arg k "$1" '.args[$k] // empty | strings' "${ARGS_FILE:-$TD/decision.json}" 2>/dev/null; }

do_reading() {
  local goal="$TD/goal.json" gd="$TD/guidance.json" prev="null" pd pl
  if ! goal_json_file "$goal" 1048576 || ! jq -e '.summary | type == "string"' "$goal" >/dev/null 2>&1; then
    jq -cn --arg e "the goal tool wrote no goal.json of at most 1 MiB with a summary" \
      '{schema_version:1, summary:"the goal reading failed", error:$e}' | goal_atomic_write "$goal" || return 1
  fi
  goal_json_file "$gd" 4194304 || jq -cn --arg e "$(goal_clean_line 300 < "$TD/guidance.err" 2>/dev/null)" \
    '{schema_version:1, entries:[], folders:[], newest:"", error:("guidance could not be read: " + $e)}' > "$gd"
  pl="$(find "${TD%/*}" -mindepth 1 -maxdepth 1 -type d -name '[0-9]*' 2>/dev/null | sed 's#.*/##' \
        | grep -E '^[0-9]+$' | awk -v me="${TD##*/}" '$0 + 0 < me + 0' | sort -n | tail -n 1)"
  if [ -n "$pl" ]; then
    pd="${TD%/*}/$pl"
    prev="$(jq -cn --argjson t "$((10#$pl))" \
      --argjson d "$(goal_json_file "$pd/decision.json" 131072 && cat "$pd/decision.json" || echo null)" \
      --argjson r "$(goal_json_file "$pd/result.json" 65536 && cat "$pd/result.json" || echo null)" \
      '{turn:$t, action:($d.action // null), outcome:($r.outcome // null), summary:($r.summary // null)}')"
  fi
  jq -n --slurpfile goal "$goal" --slurpfile gd "$gd" --argjson prev "$prev" \
    --argjson turn "$((10#${TD##*/}))" --arg time "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    --arg goal_file "${AUTODRIVE_GOAL_FILE:-}" --arg turns "${TD%/*}" --arg mark "${AUTODRIVE_GUIDANCE_MARK:-}" \
    --arg actions "${AUTODRIVE_GOAL_ACTIONS:-}" --arg merging "${AUTODRIVE_MERGE_ALLOWED:-false}" \
    '{schema_version:1, turn:$turn, time:$time, goal_file:$goal_file, goal:$goal[0],
      guidance:[($gd[0].entries // [])[] | {name, timestamp, caller, source, from_step, new, bytes}],
      guidance_mark:$mark, guidance_newest:(($gd[0].newest // "") | if . == "" then $mark else . end),
      guidance_error:($gd[0].error // null), previous:$prev, turns_dir:$turns,
      actions:{goal:($actions | split(" ") | map(select(. != ""))), shared:["continue","wait-owner","handoff","ready","stop"]},
      merging_allowed:($merging == "true")}' | goal_atomic_write "$TD/reading.json"
}

do_decision() {
  local raw="$TD/.reply" obj="$TD/.decision" problem=""
  # shellcheck source=recipe_context_value.sh
  . "$HERE/recipe_context_value.sh" || { echo "ERROR: recipe_context_value.sh is missing" >&2; return 1; }
  write_context_value turn_decision - 2>/dev/null | tail -c 262144 > "$raw" || : > "$raw"
  "$AMPLIHACK_BIN" orch helper extract-json --require-field action < "$raw" > "$obj" 2>/dev/null || : > "$obj"
  rm -f -- "$raw"
  if ! jq -e 'type == "object"' "$obj" >/dev/null 2>&1; then problem="the reply held no JSON object with an action field"
  elif [ "$(wc -c < "$obj" | tr -d ' ')" -gt 98304 ]; then problem="the decision object is over 96 KiB"
  elif ! goal_is_word "$(jq -r '.action | strings' "$obj")"; then problem="the action is not a word of a-z, 0-9 and -"
  elif ! jq -e '(.args // {}) | type == "object"' "$obj" >/dev/null 2>&1; then problem="args is not an object"
  elif [ "$(jq -c '.args // {}' "$obj" | wc -c | tr -d ' ')" -gt 65536 ]; then problem="args is over 64 KiB"
  elif [ "$(jq -r '.why // "" | tostring' "$obj" | wc -c | tr -d ' ')" -gt 16384 ]; then problem="why is over 16 KiB"
  fi
  if [ -n "$problem" ]; then
    jq -cn --arg p "$problem" '{schema_version:1, valid:false, action:null, args:{}, why:null, problem:$p}'
  else
    jq -c '{schema_version:1, valid:true, action, args:(.args // {}), why:(.why // "" | tostring), problem:null}' "$obj"
  fi | goal_atomic_write "$TD/decision.json"
  rm -f -- "$obj"
  return 0
}

# compose_end <word>: end.md from the agent's args and a mechanical footer.
compose_end() {
  local word="$1" owner text decision footer label="${TD##*/}"
  owner="$(arg owner | goal_clean_line 200)"; decision="$(arg decision | goal_clean_line 1000)"
  text="$(arg text | goal_clean_text | head -c 58000)"
  [ -n "$owner" ] || owner="the operator who started this run"
  case "$word" in
    handoff)
      [ -n "$decision" ] && [ -n "$text" ] || { echo "a hand-off needs args.owner, args.decision and args.text"; return 1; }
      footer="Hand-off from the auto-drive goal loop, turn ${label}. Owner: ${owner}. Decision: ${decision}." ;;
    ready)
      [ -n "$text" ] || { echo "ready needs args.text"; return 1; }
      if [ "${AUTODRIVE_MERGE_ALLOWED:-false}" = true ]; then
        footer="Ready, from the auto-drive goal loop, turn ${label}. Merging is allowed for this run, but the loop ended before a merge; ${owner} decides what happens next."
      else
        footer="Ready, from the auto-drive goal loop, turn ${label}. Merging is off for this run, so ${owner} decides the merge."
      fi ;;
    stop)
      [ -n "$text" ] || { echo "stop needs args.text, the continuation note"; return 1; }
      footer="Stopped at the request of ${owner}, by the auto-drive goal loop, turn ${label}. The text above is the continuation note." ;;
  esac
  { printf '%s\n\n%s\n' "$text" "$footer"; } | goal_neutralise | goal_atomic_write "$TD/end.md" || { echo "end.md could not be written"; return 1; }
  printf 'Owner: %s. %s' "$owner" "${decision:-$word}" | goal_clean_line 1000 > "$TD/.end-detail"
}

# publish_end <word> [summary]: screen, publish through the goal tool, result.json.
publish_end() {
  local word="$1" why rc detail
  if ! why="$(compose_end "$word")"; then result invalid-args "$why"; return 0; fi
  detail="$(cat "$TD/.end-detail")"; rm -f -- "$TD/.end-detail"
  if ! goal_secret_screen "$TD/end.md"; then
    rm -f -- "$TD/end.md"
    result refused-secret "the ${word} text matched the secret screen (${GOAL_SECRET_CLASS}); nothing was published"
    return 0
  fi
  [ -n "${AUTODRIVE_GOAL_TOOL:-}" ] && [ -f "$AUTODRIVE_GOAL_TOOL" ] || { result publish-failed "no goal tool to publish with"; return 0; }
  AUTODRIVE_GOAL_CONTRACT=1 bash "$AUTODRIVE_GOAL_TOOL" publish "$TD/end.md" > "$TD/publish.log" 2>&1; rc=$?
  case "$rc" in 75|79) return "$rc" ;; esac
  local e="$word"; [ "$word" = stop ] && e=stopped
  if [ "$rc" -eq 0 ]; then
    result ended "the ${word} comment was published" "$(jq -cn --arg e "$e" --arg d "$detail" '{end:$e, detail:$d}')"
  else
    result ended "the ${word} comment could not be published (exit ${rc}); the run ends anyway and the text is in end.md" \
      "$(jq -cn --arg e "$e" --arg d "$detail; the comment was not posted (exit ${rc})" '{end:$e, detail:$d}')"
  fi
}

do_wait_owner() {
  local target since out rc question owner
  target="$(jq -r '.goal.guidance_target // empty | strings' "$TD/reading.json" 2>/dev/null)"
  since="$(jq -r '.guidance_newest // empty | strings' "$TD/reading.json" 2>/dev/null)"
  goal_is_entry "$since" || since=""
  case "$target" in https://*) ;; *) target="" ;; esac
  out="$("$AMPLIHACK_BIN" recipe guidance wait "--since=$since" ${target:+"$target"} 2>"$TD/wait.err")"; rc=$?
  case "$rc" in
    0) result guidance-arrived "new guidance arrived ($(printf '%s' "$out" | jq -r '.entry // "entry"' 2>/dev/null))" ;;
    3) # D4 fallback: no wake could be armed, so the question ends in a hand-off.
       owner="$(arg owner | goal_clean_line 200)"; question="$(arg question | goal_clean_line 1000)"
       ARGS_FILE="$TD/.fallback.json"
       jq -cn --arg o "${owner:-the operator who started this run}" --arg q "${question:-the question in this turn record}" \
         '{args:{owner:$o, decision:$q, text:("The run needs an answer from " + $o + ": " + $q + " It could not wait for guidance here, because no wake folder could be armed, so it hands the question off.")}}' \
         | goal_atomic_write "$ARGS_FILE"
       publish_end handoff; rc=$?; rm -f -- "$ARGS_FILE"; return "$rc" ;;
    75|79) return "$rc" ;;
    *) result wait-error "the owner wait failed (exit ${rc}): $(goal_clean_line 300 < "$TD/wait.err")" ;;
  esac
}

do_record() {
  local r="$TD/reading.json" d="$TD/decision.json" res="$TD/result.json" why seen
  seen="$(jq -r '[.guidance[]?] as $g | [$g[] | select(.new)] as $n
    | if ($g | length) == 0 then "none" else "\($g | length) entries, \($n | length) new\(if ($n | length) > 0 then " (" + ($n | map(.name) | join(", ")) + ")" else "" end)" end' "$r" 2>/dev/null)"
  why="$(jq -r '.why // empty | strings' "$d" 2>/dev/null | goal_clean_text | head -c 16384)"
  printf '%s\n' "$why" > "$TD/.why"
  if ! goal_secret_screen "$TD/.why"; then why="The agent's paragraph was withheld because it matched the secret screen (${GOAL_SECRET_CLASS})."; fi
  rm -f -- "$TD/.why"
  {
    printf 'Turn %s · %s\n' "${TD##*/}" "$(jq -r '.time // "unknown time"' "$r" 2>/dev/null)"
    jq -r '.goal.summary // "no goal summary" | tostring' "$r" 2>/dev/null | goal_clean_line 600; printf '\n'
    printf 'Guidance seen: %s\n\n' "${seen:-unreadable}"
    printf 'Action chosen: %s\n' "$(jq -r '.action // "none" | tostring' "$d" 2>/dev/null || echo none)"
    printf '%s\n\n' "${why:-The agent gave no paragraph. $(jq -r '.problem // empty' "$d" 2>/dev/null)}"
    printf 'Result: %s: %s\n' "$(jq -r '.outcome // "none"' "$res" 2>/dev/null || echo none)" \
      "$(jq -r '.summary // "" | tostring' "$res" 2>/dev/null | goal_clean_line 600)"
  } | goal_neutralise | goal_atomic_write "$TD/record.md"
}

case "$SUB" in
  reading) do_reading ;;
  decision) do_decision ;;
  end) publish_end "$WORD" ;;
  wait-owner) do_wait_owner ;;
  record) do_record ;;
esac
