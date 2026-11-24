#!/usr/bin/env bash
# autodrive_build_run.sh: the bookkeeping around the build loop (#3129 PR 2).
# default-workflow runs `setup`, then `loop`, then `report`.
#
#   setup            makes this invocation's fresh private state folder and
#                    writes identity.json from this run's worktree_setup. The
#                    parent is the first usable one of
#                      $AMPLIHACK_RUN_DIR/build-goal,
#                      <worktree_setup.runtime_root>/build-goal (only when it
#                        is absolute and outside the worktree),
#                      ${TMPDIR:-/tmp};
#                    a parent that is relative, holds `..`, is a symlink,
#                    belongs to another user, or is world-writable without the
#                    sticky bit is skipped. Prints one JSON line
#                    {schema_version, state_dir, branch, worktree, base,
#                     source, parent, fallback}. When worktree_setup cannot be
#                    read, source is `none` and branch and worktree are empty:
#                    the launch checkout's own branch is never used.
#                    It then links host inputs into the worktree
#                    (workflow_worktree_host_inputs.sh, #3102), which covers
#                    a run on an existing branch that step-04 does not link.
#                    Exit 0, or 1 when no parent works.
#   loop <state>     runs the shared goal loop with the build goal: the
#                    launch bundle's tools/autodrive_goal_loop.sh, called the
#                    way the autodrive-goal-loop recipe's own step calls it.
#                    Each turn is then one `amplihack recipe run` from this
#                    step's depth, as under auto-drive-to-merge; a nested run
#                    of the loop recipe would put the turn's agent past the
#                    session-depth ceiling, and a sub-recipe step would merge
#                    the loop's context, autodrive_state_dir included, back
#                    into this run and its callers. The goal tool reads this
#                    run's context: repo_path (made absolute), worktree_setup
#                    and goal_notes (final_requirements) are exported from the
#                    values write_context_value finds (#1570), and the task
#                    text goes to the driver as a file. Its scratch folder is
#                    made inside the state folder and removed on exit. The
#                    loop counts as started when a turn's reading.json is a
#                    regular file (not a link) or loop-end.json is a JSON
#                    object file.
#                      loop run exit   stdout                       exit
#                      0               loop-end.json, or the        0
#                                      fallback line with exit 0
#                      75 or 79        nothing                      the same
#                      other, before   nothing; an ERROR line on    the same
#                      it started      stderr
#                      other, after    the fallback line; a         0
#                      it started      WARNING line on stderr
#                    The fallback line is {end:null, exit, turn, error}: turn
#                    is the last turn with a reading, or null, and error is
#                    one line of at most 300 bytes. A failure after the loop
#                    started is evidence for the report, which then says
#                    INCOMPLETE, never a failed build step. Exit 2 for a
#                    usage error or a launch bundle without the loop recipe.
#   report <state>   reads the loop's loop-end.json, the outcome ledger and
#                    the run's pull request live, and prints workflow_result
#                    (schema 1). `outcomes` is [{name, status}] in ledger
#                    order, [] with no ledger, or null with one `errors` entry
#                    when the ledger cannot be read; names have control
#                    characters (U+2028 and U+2029 too) replaced and are cut
#                    to 200 characters, and no reason or evidence is copied.
#                    Outcomes never change terminal_state. Exit 0 always,
#                    except 2 for a usage error; a failed read, gh's exit 79
#                    included, is listed in `errors`.
#
# workflow_result keeps the fields 1.2.213 readers take from it
# (workflow_finalization_handoff.sh: terminal_state, terminal_success,
# handoff, pr_number). terminal_state:
#   loop end ready, the pull request open and not a draft  IMPLEMENTED_VERIFIED
#   loop end ready, the pull request open as a draft      EXISTING_OPEN_PR
#   the pull request merged                               MERGED
#   the pull request closed unmerged                      FAILED_CLOSED_UNMERGED
#   anything else (a hand-off, a stop, no pull request)   INCOMPLETE
# terminal_success is "true" for the first three. A build never hands the
# finalisation to a merge loop itself, so `handoff` is always "false".
# docs/reference/build-goal-loop.md#inputs-and-outputs.

set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" 2>/dev/null && pwd -P)" || exit 2
# shellcheck source=autodrive_build_lib.sh
. "$HERE/autodrive_build_lib.sh" || exit 2

# parent_ok <dir>: an absolute real folder of this user, without `..`, not
# world-writable unless sticky.
parent_ok() {
  local d="$1"
  case "$d" in /*) ;; *) return 1 ;; esac
  case "/$d/" in */../*) return 1 ;; esac
  [ -d "$d" ] && [ ! -L "$d" ] || return 1
  if [ -O "$d" ]; then return 0; fi
  # Another user's folder is usable only when it is world-writable and sticky
  # (the system's shared temporary folder).
  [ -n "$(find "$d" -maxdepth 0 -perm -1002 -print 2>/dev/null)" ]
}
own_parent() { # own_parent <dir>: makes <dir> (one level) private when missing
  local d="$1"
  case "$d" in /*) ;; *) return 1 ;; esac
  case "/$d/" in */../*) return 1 ;; esac
  [ -e "$d" ] || [ -L "$d" ] || { mkdir -m 700 -- "$d" 2>/dev/null || return 1; }
  [ -d "$d" ] && [ ! -L "$d" ] && [ -O "$d" ] || return 1
  [ -z "$(find "$d" -maxdepth 0 -perm -0002 ! -perm -1000 -print 2>/dev/null)" ]
}

do_setup() {
  local wt branch base rr cand kind dir src=worktree_setup fallback=false
  wt="$(build_ws_field worktree_path)"; branch="$(build_ws_field branch_name)"; base="$(build_ws_field base_branch)"
  rr="$(build_ws_field runtime_root)"
  build_valid_branch "$branch" || branch=""
  case "$wt" in /*) ;; *) wt="" ;; esac
  build_valid_branch "$base" || base="${RECIPE_VAR_base_branch:-}"
  build_valid_branch "$base" || base=""
  [ -n "$branch" ] && [ -n "$wt" ] || { src=none; branch=""; wt=""; }
  dir=""; kind=""
  for cand in "run-dir|${AMPLIHACK_RUN_DIR:+${AMPLIHACK_RUN_DIR%/}/build-goal}" "runtime-root|${rr:+${rr%/}/build-goal}" "tmpdir|${TMPDIR:-/tmp}"; do
    kind="${cand%%|*}"; cand="${cand#*|}"
    [ -n "$cand" ] || continue
    if [ "$kind" = runtime-root ] && [ -n "$wt" ]; then
      case "${cand}/" in "${wt%/}/"*) continue ;; esac
    fi
    if [ "$kind" = tmpdir ]; then parent_ok "$cand" || continue
    else parent_ok "${cand%/*}" && own_parent "$cand" || continue; fi
    dir="$(mktemp -d "${cand%/}/build.XXXXXX" 2>/dev/null)" || continue
    chmod 700 "$dir" && [ -O "$dir" ] && break
    rmdir -- "$dir" 2>/dev/null; dir=""
  done
  [ -n "$dir" ] || { echo "ERROR: autodrive_build_run.sh setup: no usable parent for the state folder (AMPLIHACK_RUN_DIR, the runtime root, TMPDIR)" >&2; exit 1; }
  [ "$kind" = run-dir ] || fallback=true
  mkdir -m 700 -- "$dir/turns" "$dir/no-worktree" || { echo "ERROR: autodrive_build_run.sh setup: the state folder could not be filled" >&2; exit 1; }
  jq -cn --arg b "$branch" --arg w "$wt" --arg s "$src" --arg base "$base" --arg t "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    '{schema_version:1, branch:$b, worktree:$w, source:$s, base:$base, started:$t}' | goal_atomic_write "$dir/identity.json" \
    || { echo "ERROR: autodrive_build_run.sh setup: identity.json could not be written" >&2; exit 1; }
  echo "build goal: state folder ${dir} (${kind}); branch ${branch:-unknown}, worktree ${wt:-unknown}" >&2
  # #3102: step-04 links host inputs only when it creates a branch. Linking
  # here covers a run on an existing branch too; the linker never replaces
  # anything and logs each link on stderr.
  if [ -n "$wt" ] && [ -d "$wt" ] && [ -f "$HERE/workflow_worktree_host_inputs.sh" ]; then
    bash "$HERE/workflow_worktree_host_inputs.sh" link "${RECIPE_VAR_repo_path:-.}" "$wt" >&2 \
      || echo "WARNING: build goal: host inputs could not be linked into ${wt}; the reading lists what is missing" >&2
  fi
  jq -cn --arg d "$dir" --arg b "$branch" --arg w "$wt" --arg s "$src" --arg base "$base" --arg k "$kind" --argjson f "$fallback" \
    '{schema_version:1, state_dir:$d, branch:$b, worktree:$w, base:$base, source:$s, parent:$k, fallback:$f}'
}

do_loop() {
  local sd="${1%/}" driver repo scratch rc k
  LOOP_SCRATCH=""
  case "$sd" in /*) ;; *) echo "usage: autodrive_build_run.sh loop <absolute state folder>" >&2; exit 2 ;; esac
  [ -d "$sd" ] && [ ! -L "$sd" ] && [ -O "$sd" ] \
    || { echo "ERROR: autodrive_build_run.sh loop: ${sd} is not a folder of this user (run setup first)" >&2; exit 2; }
  driver="$HERE/autodrive_goal_loop.sh"
  [ -f "$driver" ] && [ ! -L "$driver" ] \
    || { echo "ERROR: autodrive_build_run.sh loop: the launch bundle has no tools/autodrive_goal_loop.sh" >&2; exit 2; }
  # shellcheck source=recipe_context_value.sh
  . "$HERE/recipe_context_value.sh" || { echo "ERROR: autodrive_build_run.sh loop: recipe_context_value.sh is missing beside it" >&2; exit 2; }
  repo="${RECIPE_VAR_repo_path:-.}"
  repo="$(cd "$repo" 2>/dev/null && pwd -P)" || { echo "ERROR: autodrive_build_run.sh loop: repo_path cannot be entered" >&2; exit 2; }
  # The scratch folder lives in the private state folder, so the run makes no
  # file outside it.
  scratch="$(mktemp -d "$sd/.build-loop.XXXXXX")" || { echo "ERROR: autodrive_build_run.sh loop: no scratch folder in ${sd}" >&2; exit 1; }
  # The trap runs after this function returns, so it names a global.
  LOOP_SCRATCH="$scratch"
  trap '[ -z "$LOOP_SCRATCH" ] || rm -rf -- "$LOOP_SCRATCH"' EXIT
  for k in task_description final_requirements worktree_setup; do
    write_context_value "$k" "$scratch/$k" 2>/dev/null || : > "$scratch/$k"
  done
  # The build goal tool reads this run's context from RECIPE_VAR_* (the step
  # inherits default-workflow's). These four are set from the values the
  # tool needs whichever channel held them (#1570): the launch checkout as an
  # absolute path, worktree_setup, and final_requirements as the goal notes.
  export RECIPE_VAR_repo_path="$repo"
  RECIPE_VAR_worktree_setup="$(head -c 65536 -- "$scratch/worktree_setup")"; export RECIPE_VAR_worktree_setup
  RECIPE_VAR_goal_notes="$(head -c 65536 -- "$scratch/final_requirements")"; export RECIPE_VAR_goal_notes
  set -- --goal-tool autodrive_build_goal.sh --goal-file skills/default-workflow/goal-build-pr.md \
    --state-dir "$sd" --repo "$repo" --task-file "$scratch/task_description" --notes-file "$scratch/final_requirements"
  [ -z "${RECIPE_VAR_step_recovery_dir:-}" ] || set -- "$@" --recovery-dir "$RECIPE_VAR_step_recovery_dir"
  echo "build goal: the shared goal loop runs with its state in ${sd}" >&2
  # The shared driver itself, as the autodrive-goal-loop recipe's step runs
  # it. Each turn is a fresh `amplihack recipe run autodrive-goal-turn` from
  # this step's depth; a nested `recipe run` of the loop recipe would put the
  # turn's agent past the session-depth ceiling.
  (cd "$repo" && exec bash "$driver" "$@") >&2
  rc=$?
  if [ "$rc" -eq 0 ]; then
    if goal_json_file "$sd/loop-end.json" 65536; then jq -c . "$sd/loop-end.json"
    else fallback_line 0 "$(last_turn "$sd")" "the goal loop run exited 0 and left no loop-end.json in ${sd}"; fi
    return 0
  fi
  case "$rc" in 75|79) exit "$rc" ;; esac
  if ! loop_started "$sd"; then
    echo "ERROR: autodrive_build_run.sh loop: the goal loop run exited ${rc} before its first turn; nothing in ${sd} shows the loop started" >&2
    exit "$rc"
  fi
  # The loop ran at least one turn, so the work may be pushed and its pull
  # request open. A later failure is evidence for the report and the next
  # run, never a failed build.
  echo "WARNING: autodrive_build_run.sh loop: the goal loop run exited ${rc} after it started; its turn records are in ${sd}/turns" >&2
  fallback_line "$rc" "$(last_turn "$sd")" "the goal loop run exited ${rc} after it started; its turn records are in ${sd}/turns"
}

# loop_started <state>: a turn's reading.json is a regular file (not a link),
# or loop-end.json is a JSON object file.
loop_started() {
  local n
  goal_json_file "$1/loop-end.json" 65536 && return 0
  for n in $(goal_turn_numbers "$1/turns"); do
    [ -f "$1/turns/$n/reading.json" ] && [ ! -L "$1/turns/$n/reading.json" ] && return 0
  done
  return 1
}

# last_turn <state>: the highest turn number with a regular reading.json, or
# "null".
last_turn() {
  local n last=null
  for n in $(goal_turn_numbers "$1/turns"); do
    [ -f "$1/turns/$n/reading.json" ] && [ ! -L "$1/turns/$n/reading.json" ] && last="$((10#$n))"
  done
  printf '%s' "$last"
}

# fallback_line <exit> <turn|null> <error>: the one JSON line printed in place
# of loop-end.json.
fallback_line() {
  jq -cn --argjson x "$1" --argjson t "$2" --arg e "$(printf '%s' "$3" | goal_clean_line 300)" \
    '{end:null, exit:$x, turn:$t, error:$e}'
}

do_report() {
  local STATE="${1%/}" end="none" detail="" turn=null errs='[]' n="" how="" v rc url="" pstate="" draft=null ts success outcomes
  case "$STATE" in /*) ;; *) echo "usage: autodrive_build_run.sh report <absolute state folder>" >&2; exit 2 ;; esac
  addr() { errs="$(jq -cn --argjson e "$errs" --arg m "$(printf '%s' "$1" | goal_clean_line 300)" '$e + [$m]')"; }
  if goal_json_file "$STATE/loop-end.json" 65536; then
    end="$(jq -r '.end // "none" | tostring' "$STATE/loop-end.json")"
    detail="$(jq -r '.detail // "" | tostring' "$STATE/loop-end.json" | goal_clean_line 600)"
    turn="$(jq -c '.turn // null' "$STATE/loop-end.json")"
  else
    addr "the loop left no loop-end.json in ${STATE}"
  fi
  LAUNCH="${RECIPE_VAR_repo_path:-.}"; LAUNCH="$(cd "$LAUNCH" 2>/dev/null && pwd -P)" || LAUNCH="$PWD"
  if build_repo_slug "$LAUNCH"; then
    build_run_branch || :
    n="$(jq -r '.number // empty | tostring' "$STATE/pr.json" 2>/dev/null)"; how=state
    if ! goal_is_pr "$n" && [ -n "$BUILD_BRANCH" ]; then
      v="$(gh pr list --head "$BUILD_BRANCH" --state all --limit 30 --json number,headRefName,isCrossRepository 2>/dev/null)"; rc=$?
      if [ "$rc" -eq 0 ]; then
        n="$(printf '%s' "$v" | jq -r --arg b "$BUILD_BRANCH" '[.[]? | select(.headRefName == $b and .isCrossRepository == false) | .number] | max // empty' 2>/dev/null)"; how=branch
      else addr "gh pr list --head exited ${rc}"; fi
    fi
    goal_is_pr "$n" || { n="${RECIPE_VAR_pr_number:-}"; how=context; }
    if goal_is_pr "$n"; then
      v="$(gh pr view "$n" --json number,url,state,isDraft,isCrossRepository 2>/dev/null)"; rc=$?
      if [ "$rc" -eq 0 ] && printf '%s' "$v" | jq -e 'type == "object" and .isCrossRepository == false' >/dev/null 2>&1; then
        url="$(printf '%s' "$v" | jq -r '.url // empty | strings')"; pstate="$(printf '%s' "$v" | jq -r '.state // empty | strings')"
        draft="$(printf '%s' "$v" | jq -c '.isDraft // null')"
      else addr "pull request #${n} could not be read (gh pr view exit ${rc})"; fi
    else n=""; how=none; fi
  else
    addr "the repository is unknown: ${BUILD_REPO_ERR}"
  fi
  # The required outcomes, by name and status only, in ledger order. No ledger
  # is an empty list; a ledger that cannot be read is null and one error.
  # reason and evidence stay in the ledger and the turn records.
  outcomes='[]'
  if [ -e "$STATE/outcomes.json" ] || [ -L "$STATE/outcomes.json" ]; then
    if goal_json_file "$STATE/outcomes.json" 524288 && v="$(jq -c '
        .outcomes
        | if type != "array" then error("no outcomes array") else . end
        | map(if type == "object" and (.name | type) == "string" and (.name | length) > 0
                 and (.status == "met" or .status == "unmet" or .status == "rejected")
              then {name:(.name | gsub("[\\p{Cc}  ]"; " ") | .[0:200]), status}
              else error("a malformed outcome") end)' "$STATE/outcomes.json" 2>/dev/null)"; then
      outcomes="$v"
    else
      outcomes=null
      addr "the outcome ledger ${STATE}/outcomes.json could not be read (a link, over 512 KiB, not JSON, no outcomes array, or an outcome without a name or with a status other than met, unmet or rejected)"
    fi
  fi
  case "$pstate:$draft:$end" in
    MERGED:*) ts=MERGED; success=true ;;
    CLOSED:*) ts=FAILED_CLOSED_UNMERGED; success=false ;;
    OPEN:false:ready|OPEN:false:merged) ts=IMPLEMENTED_VERIFIED; success=true ;;
    OPEN:true:ready) ts=EXISTING_OPEN_PR; success=true ;;
    *) ts=INCOMPLETE; success=false ;;
  esac
  jq -cn --arg end "$end" --arg d "$detail" --argjson turn "$turn" --arg n "$n" --arg how "$how" --arg u "$url" --arg ps "$pstate" \
    --argjson draft "$draft" --arg ts "$ts" --arg ok "$success" --argjson errs "$errs" --arg st "$STATE" --argjson outcomes "$outcomes" '
    {schema_version:1, end:$end, detail:$d, turn:$turn, pr_number:$n, pr_url:$u, pr_found_by:$how,
     pr_state:(if $ps == "" then null else $ps end), pr_draft:$draft, terminal_state:$ts, terminal_success:$ok,
     terminal_reason:("the build loop ended " + $end + (if $d == "" then "" else ": " + $d end)), handoff:"false",
     state_dir:$st, outcomes:$outcomes, errors:$errs}'
  return 0
}

case "${1:-}" in
  setup) [ "$#" -eq 1 ] || { echo "usage: autodrive_build_run.sh setup" >&2; exit 2; }; do_setup ;;
  loop) [ "$#" -eq 2 ] || { echo "usage: autodrive_build_run.sh loop <state_dir>" >&2; exit 2; }; do_loop "$2" ;;
  report) [ "$#" -eq 2 ] || { echo "usage: autodrive_build_run.sh report <state_dir>" >&2; exit 2; }; do_report "$2" ;;
  *) echo "usage: autodrive_build_run.sh setup|loop <state_dir>|report <state_dir>" >&2; exit 2 ;;
esac
