#!/usr/bin/env bash
# autodrive_pr_goal.sh: the auto-drive goal tool, goal contract v1 (#3129).
# The goal: get this pull request merged under the owner's conditions.
#
#   actions                    prints `review wait merge`
#   final                      prints merged, closed or refused (a
#                              cross-repository PR), or nothing; one gh read
#   read <turn_dir>            goal.json (autodrive_pr_reading.sh)
#   act review <turn_dir>      one Crusty round (autodrive-crusty-round); its
#                              verdict is evidence in the next reading
#   act wait <turn_dir>        `gh pr checks <pr> --watch [--required]` under
#                              `amplihack recipe guidance wait`, so new
#                              guidance ends the wait too; no timer or poll
#   act merge <turn_dir>       autodrive_merge.sh under the captured
#                              authorisation, with --match-head-commit at the
#                              head this turn read; writes merge-result.json
#   publish <body_file>        gh pr comment <pr> --body-file
#   review                     one Crusty round outside a turn (the
#                              autodrive-crusty-loop alias)
#
# AUTODRIVE_GOAL_CONTRACT must be 1, else exit 2. Inputs from the driver:
# AUTODRIVE_STATE_DIR, AUTODRIVE_REPO, AUTODRIVE_WORKTREE, AUTODRIVE_LOOP_HOME,
# AUTODRIVE_INVOCATION_FILE, AUTODRIVE_TASK_FILE; from the run's context:
# RECIPE_VAR_pr_number, merge_method, autodrive_dry_run, step_recovery_dir.
# AUTODRIVE_PR gives the number only when the run context has none (the
# one-release tool aliases).
# `act` writes <turn_dir>/result.json ({outcome, summary}). Exit 79 and 75
# pass up unchanged; any other failure is an outcome, never an exit.
# docs/reference/autodrive-goal-loop.md#goal-contract-v1.

set -uo pipefail
[ "${AUTODRIVE_GOAL_CONTRACT:-}" = 1 ] || { echo "ERROR: autodrive_pr_goal.sh speaks goal contract 1 only (AUTODRIVE_GOAL_CONTRACT='${AUTODRIVE_GOAL_CONTRACT:-}')" >&2; exit 2; }
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" 2>/dev/null && pwd -P)" || exit 2
for lib in autodrive_goal_lib.sh autodrive_refusal.sh; do
  # shellcheck source=/dev/null
  . "$HERE/$lib" || { echo "ERROR: autodrive_pr_goal.sh: $lib is missing beside it" >&2; exit 2; }
done
AMPLIHACK_BIN="${AMPLIHACK_BIN:-amplihack}"
export GIT_PAGER=cat GH_PAGER=cat PAGER=cat GIT_TERMINAL_PROMPT=0
PR="${RECIPE_VAR_pr_number:-${AUTODRIVE_PR:-}}"; STATE="${AUTODRIVE_STATE_DIR:-}"
WORK="${AUTODRIVE_WORKTREE:-${AUTODRIVE_REPO:-$PWD}}"
CALLER_UMASK="$(umask)"; umask 077

TD=""
result() { jq -cn --arg o "$1" --arg s "$2" '{outcome:$o, summary:$s}' | goal_atomic_write "$TD/result.json"; }
need_pr() { goal_is_pr "$PR" || { result no-pr "no pull request number reached the goal tool (pr_number)"; exit 0; }; }

do_final() {
  local out rc
  goal_is_pr "$PR" || { echo "no pull request number (pr_number); no final reading" >&2; return 0; }
  out="$(gh pr view "$PR" --json state,isCrossRepository 2>/dev/null)"; rc=$?
  case "$rc" in 79|75) return "$rc" ;; 0) ;; *) echo "gh pr view exited ${rc}; no final reading this time" >&2; return 0 ;; esac
  case "$(printf '%s' "$out" | jq -r '.state // empty')" in
    MERGED) echo merged; return 0 ;;
    CLOSED) echo closed; return 0 ;;
  esac
  # A cross-repository PR, or one whose field is missing or null, is refused.
  [ "$(printf '%s' "$out" | jq -r '.isCrossRepository | tostring')" = false ] || echo refused
}

# crusty_round <label> <log>: one autodrive-crusty-round at the worktree's head.
crusty_round() {
  local label="$1" log="$2" head record recipe rc d
  head="$(git -C "$WORK" rev-parse --verify -q 'HEAD^{commit}')" || { echo "HEAD does not resolve in ${WORK}" > "$log"; return 1; }
  record="$STATE/crusty-round-${label}.json"
  recipe="$(goal_bundle_file "${HERE%/tools}" recipes/autodrive-crusty-round.yaml)" || return 1
  d="${RECIPE_VAR_autodrive_dry_run:-false}"; [ "$d" = true ] || d=false
  ( umask "$CALLER_UMASK"
    AMPLIHACK_AUTODRIVE_STATE_DIR="$STATE" exec "$AMPLIHACK_BIN" recipe run "$recipe" --working-dir "$WORK" \
      -c "autodrive_round_record=${record}" -c "autodrive_round_label=${label}" \
      -c "autodrive_resolved_concerns_file=${STATE}/resolved-concerns.txt" \
      --context-from-file "task_description=${AUTODRIVE_TASK_FILE:-/dev/null}" \
      -c "repo_path=${WORK}" -c "reviewed_head_sha=${head}" -c "autodrive_launch_tools=${HERE}" \
      -c "pr_number=${PR}" -c "autodrive_dry_run=${d}" \
      -c "step_recovery_dir=${RECIPE_VAR_step_recovery_dir:-}" ) >"$log" 2>&1
  rc=$?
  # The round's pre-review baselines may hold untracked file contents; once the
  # round has returned they are reclaimed. A failed removal is said out loud.
  for b in "$STATE"/pre-review.*; do
    [ -d "$b" ] && [ ! -L "$b" ] || continue
    rm -rf -- "$b" || echo "WARNING: the pre-review baseline ${b} could not be removed; it may hold untracked file contents" >&2
  done
  case "$rc" in 79|75) return "$rc" ;; esac
  if [ "$rc" -ne 0 ]; then echo "the Crusty round failed ($(child_failure_summary "$rc" "$log"))"; return 1; fi
  if goal_json_file "$record" 65536; then
    jq -r '"Crusty verdict \(.crusty_verdict) at \(.head_sha), reviewed \(.reviewed_head_sha), \(.concern_count) concern(s)"' "$record"
  else echo "the Crusty round wrote no record"; return 1; fi
}

do_act_review() {
  local s rc
  need_pr
  s="$(crusty_round "turn-${TD##*/}" "$TD/review.log")"; rc=$?
  case "$rc" in 79|75) exit "$rc" ;; 0) result reviewed "$s" ;; *) result review-failed "$s; the log is review.log in the turn folder" ;; esac
}

do_act_wait() {
  local req known target since out rc ev code cmd
  need_pr
  req="$(jq -r '.args.required // false | tostring' "$TD/decision.json" 2>/dev/null)"
  known="$(jq -r '.goal.checks.required_known // false | tostring' "$TD/reading.json" 2>/dev/null)"
  target="$(jq -r '.goal.guidance_target // empty | strings' "$TD/reading.json" 2>/dev/null)"
  case "$target" in https://*) ;; *) target="" ;; esac
  since="$(jq -r '.guidance_newest // empty | strings' "$TD/reading.json" 2>/dev/null)"; goal_is_entry "$since" || since=""
  cmd=(gh pr checks "$PR" --watch)
  [ "$req" = true ] && [ "$known" = true ] && cmd+=(--required)
  out="$(cd "$WORK" && "$AMPLIHACK_BIN" recipe guidance wait "--since=$since" ${target:+"$target"} -- "${cmd[@]}" 2>"$TD/wait.err")"; rc=$?
  if [ "$rc" -eq 0 ] && ev="$(printf '%s' "$out" | jq -r '.event // empty' 2>/dev/null)" && [ -n "$ev" ]; then
    if [ "$ev" = guidance ]; then result guidance-arrived "new guidance arrived during the wait; the next turn reads it"; return 0; fi
    code="$(printf '%s' "$out" | jq -r '.exit // empty')"
  else
    # No wake could be armed, or the guidance command failed: watch the checks
    # alone. Guidance is read just before and just after every check wait.
    echo "the guidance wake was not available (exit ${rc}: $(goal_clean_line 200 < "$TD/wait.err")); watching the checks alone" >&2
    ( cd "$WORK" && "${cmd[@]}" ) >&2; code=$?
  fi
  case "$code" in
    79|75) exit "$code" ;;
    0) result checks-complete "the checks at the head completed and passed (gh exit 0)" ;;
    1) result checks-complete "the checks at the head completed and at least one failed (gh exit 1)" ;;
    8) result checks-pending "gh returned while checks were still pending (exit 8)" ;;
    *) result wait-error "gh pr checks --watch exited ${code:-unknown}" ;;
  esac
}

do_act_merge() {
  local head want auth allows sel line rc run_id res
  need_pr
  head="$(jq -r '.args.head // empty | strings' "$TD/decision.json" 2>/dev/null)"
  want="$(jq -r '.pr.head // empty | strings' "$TD/goal.json" 2>/dev/null)"
  goal_is_sha "$head" || { result invalid-head "args.head is not 40 or 64 lowercase hex characters; nothing was merged"; return 0; }
  if [ "$head" != "$want" ]; then
    result head-mismatch "args.head ${head} is not the head this turn read (${want:-unreadable}); if you pushed this turn, choose continue and merge on a later turn"
    return 0
  fi
  merge_record() { # merge_record <json line>: merge-result.json, bound to this run
    printf '%s' "$1" | jq -c --arg r "$run_id" '. + {run_id:$r, handoff:"false"}' | goal_atomic_write "$STATE/merge-result.json"
  }
  run_id=""
  if ! auth="$("$AMPLIHACK_BIN" orch helper autodrive authorization --invocation "${AUTODRIVE_INVOCATION_FILE:-}" --directory "$STATE" --pr "$PR" 2>"$TD/auth.err")"; then
    result authorization-unreadable "the captured authorisation could not be read ($(goal_clean_line 300 < "$TD/auth.err")); nothing was merged"
    return 0
  fi
  run_id="$(printf '%s' "$auth" | jq -r '.run_id // empty')"
  allows="$(printf '%s' "$auth" | jq -r '.allows_merge | tostring')"; sel="$(printf '%s' "$auth" | jq -r '.pr_selected | tostring')"
  if [ "$allows" != true ] || [ "$sel" != true ]; then
    merge_record "$(jq -cn --argjson pr "$PR" --arg h "$head" '{result:"not-authorised", pr:$pr, expected_head:$h, reason:"the captured invocation does not allow this merge"}')"
    result not-authorised "the captured invocation does not allow a merge of this pull request (allows_merge ${allows}, pr_selected ${sel}); merging is off for this run, so nothing was merged"
    return 0
  fi
  # shellcheck source=autodrive_merge.sh
  . "$HERE/autodrive_merge.sh" || { result refused "autodrive_merge.sh could not be loaded; nothing was merged"; return 0; }
  line="$(cd "$WORK" && autodrive_merge "$PR" "$head" "${RECIPE_VAR_merge_method:-}")"; rc=$?
  case "$rc" in 79|75) exit "$rc" ;; esac
  # gh's own output can come first; the tool's result is its last JSON line.
  line="$(printf '%s\n' "$line" | jq -c -R 'fromjson? | select(type == "object" and (.result | type == "string"))' 2>/dev/null | tail -n 1)"
  if [ -z "$line" ]; then
    result refused "the merge tool exited ${rc} with no result line; nothing is known to be merged"; return 0
  fi
  merge_record "$line"
  res="$(printf '%s' "$line" | jq -r .result)"
  result "$res" "$(printf '%s' "$line" | jq -r '.reason // "" | tostring' | goal_clean_line 600)"
}

case "${1:-}" in
  actions) echo "review wait merge" ;;
  final) do_final ;;
  read)
    TD="${2:-}"; goal_check_turn_dir "$TD" || exit 2
    bash "$HERE/autodrive_pr_reading.sh" "$TD" ;;
  act)
    TD="${3:-}"; goal_check_turn_dir "$TD" || exit 2
    case "${2:-}" in
      review) do_act_review ;;
      wait) do_act_wait ;;
      merge) do_act_merge ;;
      *) echo "ERROR: act takes review, wait or merge" >&2; exit 2 ;;
    esac ;;
  publish)
    goal_is_pr "$PR" || { echo "ERROR: no pull request number to publish to" >&2; exit 1; }
    [ -f "${2:-}" ] || { echo "ERROR: publish needs a body file" >&2; exit 2; }
    ( cd "$WORK" && exec gh pr comment "$PR" --body-file "$2" ) ;;
  review)
    goal_is_pr "$PR" || { echo "ERROR: no pull request number (pr_number)" >&2; exit 2; }
    LOG="$(mktemp "$STATE/.review.XXXXXX")" || exit 1
    s="$(crusty_round "review-$(date -u +%Y%m%dT%H%M%SZ)" "$LOG")"; rc=$?
    case "$rc" in 79|75) exit "$rc" ;; esac
    jq -cn --arg s "$s" --argjson ok "$([ "$rc" -eq 0 ] && echo true || echo false)" --arg log "$LOG" '{review_ok:$ok, summary:$s, log:$log}' ;;
  *) echo "usage: autodrive_pr_goal.sh actions|final|read|act|publish|review" >&2; exit 2 ;;
esac
