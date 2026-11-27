#!/usr/bin/env bash
# shellcheck source-path=SCRIPTDIR
# autodrive_pr_goal.sh: the auto-drive goal tool, goal contract v1 (#3129).
# The goal: get this pull request merged under the owner's conditions.
#
#   actions                    prints `review wait merge`
#   final                      prints merged, closed or refused (a
#                              cross-repository PR), or nothing; one gh read
#   read <turn_dir>            goal.json (autodrive_pr_reading.sh)
#   act review <turn_dir>      one Crusty round (autodrive-crusty-round); its
#                              verdict is evidence in the next reading
#   act wait <turn_dir>        conditional REST reads of the PR head and its
#                              checks under `amplihack recipe guidance wait`
#   act merge <turn_dir>       autodrive_merge.sh under the captured
#                              authorisation, with --match-head-commit at the
#                              head this turn read; writes merge-result.json.
#                              Before it: guidance must have been read again
#                              after the decision (else guidance-unknown), and
#                              every required outcome is checked by name
#                              against the run's ledger (else outcomes-unmet)
#   publish <body_file>        gh pr comment <pr> --body-file
#   review                     one Crusty round outside a turn (the
#                              autodrive-crusty-loop alias)
#
# AUTODRIVE_GOAL_CONTRACT must be 1, else exit 2. Inputs from the driver:
# AUTODRIVE_STATE_DIR, AUTODRIVE_REPO, AUTODRIVE_WORKTREE, AUTODRIVE_LOOP_HOME,
# AUTODRIVE_INVOCATION_FILE, AUTODRIVE_TASK_FILE; from the run's context:
# RECIPE_VAR_pr_number, merge_method, no_merge, autodrive_dry_run,
# step_recovery_dir, plan_task_id, checks_watch_interval. Only RECIPE_VAR_*
# names are read, so an inherited bare variable never fills one in (#1561).
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
PR="${RECIPE_VAR_pr_number:-}"; STATE="${AUTODRIVE_STATE_DIR:-}"
WORK="${AUTODRIVE_WORKTREE:-${AUTODRIVE_REPO:-$PWD}}"
CALLER_UMASK="$(umask)"; umask 077

TD=""
result() { jq -cn --arg o "$1" --arg s "$2" '{outcome:$o, summary:$s}' | goal_atomic_write "$TD/result.json"; }
ci_result() {
  jq -c '{outcome:"checks-complete", checks_state:.state, summary:(
      if .state == "passed" then "the checks at the head completed and passed"
      elif .state == "failed" then "the checks at the head completed and at least one failed"
      else "the checks at the head completed with unverified results" end)}
    + (if (.unverified_checks | length) > 0 then {unverified_checks} else {} end)' "$1" \
    | goal_atomic_write "$TD/result.json"
}
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
  # A round that ends the run (a terminal refusal, by exit 79 or by the
  # runner's structural_refusal marker, or an orphaned run) keeps its
  # baseline, the only record of what the worktree held before the reviewer.
  terminal_refusal "$rc" "$log" && return 79
  [ "$rc" -ne 75 ] || return 75
  # Otherwise the round's pre-review baselines, which may hold untracked file
  # contents, are reclaimed. A failed removal is said out loud.
  for b in "$STATE"/pre-review.*; do
    [ -d "$b" ] && [ ! -L "$b" ] || continue
    rm -rf -- "$b" || echo "WARNING: the pre-review baseline ${b} could not be removed; it may hold untracked file contents" >&2
  done
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

checks_watch_interval() {
  local LC_ALL=C n="${RECIPE_VAR_checks_watch_interval:-}"
  if [[ "$n" =~ ^[1-9][0-9]{0,9}$ ]] && (( n <= 9223372036 )); then
    printf '%s\n' "$n"
  else
    [ -z "$n" ] || printf '%s\n' 'WARNING: invalid checks_watch_interval. Using 120 seconds.' >&2
    printf '120\n'
  fi
}

# Run in its own shell so the guidance wake can interrupt an API call, a
# rate-limit wait or an ordinary interval sleep. Exit 10 signals a new head.
do_ci_wait() {
  # shellcheck source=workflow_gh_retry.sh
  . "$HERE/workflow_gh_retry.sh" || return 2
  local build_branch="${1:-}" head slug required required_only policy_error="" cache interval rc state=pending
  local GH_READ_RETRIED=false
  local reading="$TD/reading.json"
  head="$(jq -r '.goal.pr.head // empty | strings' "$reading")"
  slug="$(goal_github_pr_repo "$(jq -r '.goal.pr.url // empty | strings' "$reading")" "$PR")"
  if ! goal_is_sha "$head" || [ -z "$slug" ]; then
    echo 'the reading has no usable pull request repository or head' >&2
    return 2
  fi
  required_only="$(jq -r '.args.required == true' "$TD/decision.json")" || return 2
  if ! required="$(jq -ce '.goal.checks | select(.required_known == true) | .required_checks | arrays' "$reading")"; then
    required='[]'
    if [ "$required_only" = true ]; then
      policy_error="$(jq -r '.goal.checks.required_error // empty | strings' "$reading")"
      policy_error="${policy_error:-the reading has no complete required-check policy}"
    fi
  fi
  interval="$(checks_watch_interval)"
  GH_READ_DIR="$(mktemp -d "$TD/.ci-wait.XXXXXX")" || return 2
  trap 'gh_goal_cancel; rm -rf -- "$GH_READ_DIR"' EXIT
  trap 'exit 129' HUP; trap 'exit 130' INT; trap 'exit 143' TERM
  cache="$(goal_ci_cache_dir "${STATE:-${TD%/turns/*}}")" || return 2

  while :; do
    gh_rest_get "repos/$slug/pulls/$PR" "$cache" \
      '(.head.sha | type == "string" and test("^([0-9a-f]{40}|[0-9a-f]{64})$"))' "$head"
    rc=$?
    case "$rc" in
      75|79) return "$rc" ;;
      0) ;;
      *) printf 'the PR head read failed (exit %s): %s\n' "$rc" "${GH_READ_ERROR:-unreadable response}" >&2; return 2 ;;
    esac
    if [ -n "$build_branch" ] && ! jq -e --arg branch "$build_branch" --arg repo "${slug,,}" '
      .state == "open" and .head.ref == $branch and
      (.head.repo.full_name | strings | ascii_downcase) == $repo
    ' "$GH_READ_BODY" >/dev/null; then
      echo "the pull request is no longer open on the build's branch and repository" >&2
      return 2
    fi
    [ "$(jq -r '.head.sha' "$GH_READ_BODY")" = "$head" ] || return 10

    # A rate-limit retry can span a rerun even while the head stays the same.
    # Read the checks again if any request since the start of the scan retried.
    if [ "$state" != pending ] && [ "$GH_READ_RETRIED" = false ]; then
      if [ -n "$policy_error" ]; then
        printf 'the required-check policy is incomplete: %s\n' "$policy_error" >&2
        return 2
      fi
      ci_result "$GH_READ_DIR/ci.json" || return 2
      return 0
    fi
    GH_READ_RETRIED=false
    gh_head_checks "$slug" "$head" "$cache" "$required" "$required_only" > "$GH_READ_DIR/ci.json"
    rc=$?
    case "$rc" in
      75|79) return "$rc" ;;
      0) ;;
      *) printf 'the CI read failed (exit %s): %s\n' "$rc" "${GH_READ_ERROR:-unreadable response}" >&2; return 2 ;;
    esac
    state="$(jq -r '.state' "$GH_READ_DIR/ci.json")"
    case "$state" in
      passed|failed|unverified) ;; # Revalidate the head before using the result.
      pending) gh_goal_command sleep "$interval" || return 2 ;;
      *) echo 'the CI read returned an unknown state' >&2; return 2 ;;
    esac
  done
}

do_act_wait() {
  local target since out rc ev code
  local -a cmd
  need_pr
  target="$(jq -r '.goal.guidance_target // empty | strings' "$TD/reading.json" 2>/dev/null)"
  case "$target" in https://*) ;; *) target="" ;; esac
  since="$(jq -r '.guidance_newest // empty | strings' "$TD/reading.json" 2>/dev/null)"; goal_is_entry "$since" || since=""
  cmd=(bash "$HERE/autodrive_pr_goal.sh" ci-wait "$TD" "$@")
  out="$(cd "$WORK" && "$AMPLIHACK_BIN" recipe guidance wait "--since=$since" ${target:+"$target"} -- "${cmd[@]}" 2>"$TD/wait.err")"; rc=$?
  case "$rc" in 79|75) exit "$rc" ;; esac
  ev="$(jq -r '.event // empty' 2>/dev/null <<< "$out")"
  case "$rc:$ev" in
    0:guidance) result guidance-arrived "new guidance arrived during the wait. The next turn reads it"; return 0 ;;
    0:command) code="$(jq -r '.exit // empty' <<< "$out")" ;;
    *)
      echo "the guidance wake was not available (exit ${rc}). Waiting for checks directly" >&2
      ( cd "$WORK" && exec "${cmd[@]}" ) >>"$TD/wait.err" 2>&1; code=$? ;;
  esac
  case "$code" in
    79|75) exit "$code" ;;
    0) return 0 ;; # The completed wait wrote result.json.
    10) result head-changed "the pull request head changed during the wait" ;;
    *) result wait-error "the CI wait exited ${code:-unknown}. $(tail -n 1 "$TD/wait.err" | goal_clean_line 300)" ;;
  esac
}

# outcome_problems: one line per required outcome that keeps a merge from
# going ahead (R2). The merge decision must list `outcomes` (an empty list
# when the task text and guidance require none), and every name in the run's
# ledger (autodrive_goal_turn.sh outcomes) must be in it, by name, as met, or
# as rejected with that same reason already answered in a PR comment. Fails
# only when the ledger exists and cannot be read.
outcome_problems() {
  local led="$STATE/outcomes.json" ledf
  if ! jq -e '(.outcomes | type) == "array"' "$TD/decision.json" >/dev/null 2>&1; then
    echo "the merge decision lists no outcomes; name each required outcome of the task text and guidance with met, unmet or rejected, or give an empty list when there are none"
    return 0
  fi
  ledf="$led"
  if [ ! -e "$led" ] && [ ! -L "$led" ]; then ledf="$(mktemp "$TD/.no-ledger.XXXXXX")" || return 1; echo '{"outcomes":[]}' > "$ledf"
  elif ! goal_json_file "$led" 524288; then return 1; fi
  jq -r --slurpfile led "$ledf" '($led[0].outcomes // []) as $L | .outcomes as $D | ($L | map(.name)) as $names
    | ($L[] | . as $e | ([$D[] | select(.name == $e.name)] | .[0]) as $d
        | if $d == null then "\($e.name): named in turn \($e.turn) and not checked by name in this merge decision"
          elif $d.status == "unmet" then "\($e.name): unmet"
          elif $d.status == "rejected" and (($e.answered != true) or ($e.answered_reason != $d.reason)) then "\($e.name): rejected, and its reason is not yet answered in a PR comment"
          else empty end),
      ($D[] | select(.name as $n | ($names | index($n)) == null) | "\(.name): \(.status), but not in the run'"'"'s outcome ledger")' "$TD/decision.json"
  local rc=$?
  [ "$ledf" = "$led" ] || rm -f -- "$ledf"
  return "$rc"
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
  auth="$("$AMPLIHACK_BIN" orch helper autodrive authorization --invocation "${AUTODRIVE_INVOCATION_FILE:-}" --directory "$STATE" --pr "$PR" 2>"$TD/auth.err")"; rc=$?
  case "$rc" in 79|75) exit "$rc" ;; esac
  if [ "$rc" -ne 0 ]; then
    result authorization-unreadable "the captured authorisation could not be read ($(goal_clean_line 300 < "$TD/auth.err")); nothing was merged"
    return 0
  fi
  run_id="$(printf '%s' "$auth" | jq -r '.run_id // empty')"
  allows="$(printf '%s' "$auth" | jq -r '.allows_merge | tostring')"; sel="$(printf '%s' "$auth" | jq -r '.pr_selected | tostring')"
  # The run's own context can narrow the captured authority, never grant it:
  # anything but exactly "false" for no_merge or autodrive_dry_run is off.
  if [ "${RECIPE_VAR_no_merge:-}" != false ] || [ "${RECIPE_VAR_autodrive_dry_run:-}" != false ]; then allows=false; fi
  if [ "$allows" != true ] || [ "$sel" != true ]; then
    merge_record "$(jq -cn --argjson pr "$PR" --arg h "$head" '{result:"not-authorised", pr:$pr, expected_head:$h, reason:"the captured invocation does not allow this merge"}')"
    result not-authorised "the captured invocation does not allow a merge of this pull request (allows_merge ${allows}, pr_selected ${sel}); merging is off for this run, so nothing was merged"
    return 0
  fi
  # R2: guidance and the required outcomes, before any merge. Guidance that
  # could not be read again after the decision is unknown, not absent.
  if [ "${AUTODRIVE_GUIDANCE_UNKNOWN:-false}" != false ]; then
    result guidance-unknown "guidance could not be read again after this turn's decision, so it is unknown whether new guidance arrived; nothing was merged, and the next turn reads guidance again and decides"
    return 0
  fi
  local problems
  if ! problems="$(outcome_problems)"; then
    result outcomes-unreadable "the run's outcome ledger (outcomes.json in the state folder) could not be read, so the required outcomes named in earlier turns are unknown; nothing was merged"
    return 0
  fi
  if [ -n "$problems" ]; then
    result outcomes-unmet "$(printf 'nothing was merged, because each required outcome must be met, or rejected and answered in a PR comment, before a merge: %s' \
      "$(printf '%s\n' "$problems" | paste -sd ';' - | sed 's/;/; /g')" | goal_clean_line 2000)"
    return 0
  fi
  # The launch bundle's tools are regular files only: a symlinked one is refused.
  goal_bundle_file "${HERE%/tools}" tools/autodrive_merge.sh >/dev/null || { result refused "autodrive_merge.sh is missing or a symlink in the launch bundle; nothing was merged"; return 0; }
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
    # Use the launch checkout's repository, as the worktree resolver does.
    unset GH_REPO
    exec bash "$HERE/autodrive_pr_reading.sh" "$TD" ;;
  ci-wait)
    TD="${2:-}"; goal_check_turn_dir "$TD" || exit 2
    do_ci_wait "${3:-}" ;;
  act)
    TD="${3:-}"; goal_check_turn_dir "$TD" || exit 2
    case "${2:-}" in
      review) do_act_review ;;
      wait) do_act_wait "${4:-}" ;;
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
