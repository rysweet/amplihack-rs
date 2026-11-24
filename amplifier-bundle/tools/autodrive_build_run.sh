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
#                    launch bundle's recipes/autodrive-goal-loop.yaml, as its
#                    own `amplihack recipe run`, the way the loop runs each
#                    turn. A sub-recipe step would merge the loop's whole
#                    context back into this run and its callers, so the build's
#                    state folder would replace a caller's autodrive_state_dir,
#                    which autodrive-build and auto-drive-to-merge read after
#                    the build. The context the build goal tool reads is passed
#                    whole: task_description, final_requirements (as
#                    goal_notes) and worktree_setup as files (#1570), the
#                    other keys with -c. Prints the loop's end line
#                    (loop-end.json) as one JSON line. Exit: the run's own exit
#                    (0 when the loop ended; 75 and 79 passed up), 2 for a
#                    usage error or a launch bundle without the loop recipe.
#   report <state>   reads the loop's loop-end.json and the run's pull request
#                    live, and prints workflow_result (schema 1). Exit 0
#                    always, except 2 for a usage error; a failed read,
#                    gh's exit 79 included, is listed in `errors`.
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
  local sd="${1%/}" recipe repo scratch rc k v
  case "$sd" in /*) ;; *) echo "usage: autodrive_build_run.sh loop <absolute state folder>" >&2; exit 2 ;; esac
  [ -d "$sd" ] && [ ! -L "$sd" ] && [ -O "$sd" ] \
    || { echo "ERROR: autodrive_build_run.sh loop: ${sd} is not a folder of this user (run setup first)" >&2; exit 2; }
  recipe="${HERE%/tools}/recipes/autodrive-goal-loop.yaml"
  [ -f "$recipe" ] && [ ! -L "$recipe" ] \
    || { echo "ERROR: autodrive_build_run.sh loop: the launch bundle has no recipes/autodrive-goal-loop.yaml" >&2; exit 2; }
  # shellcheck source=recipe_context_value.sh
  . "$HERE/recipe_context_value.sh" || { echo "ERROR: autodrive_build_run.sh loop: recipe_context_value.sh is missing beside it" >&2; exit 2; }
  repo="${RECIPE_VAR_repo_path:-.}"
  repo="$(cd "$repo" 2>/dev/null && pwd -P)" || { echo "ERROR: autodrive_build_run.sh loop: repo_path cannot be entered" >&2; exit 2; }
  scratch="$(mktemp -d)" || { echo "ERROR: autodrive_build_run.sh loop: no scratch folder" >&2; exit 1; }
  trap 'rm -rf -- "$scratch"' EXIT
  for k in task_description final_requirements worktree_setup; do
    write_context_value "$k" "$scratch/$k" 2>/dev/null || : > "$scratch/$k"
  done
  set -- -c goal_tool=autodrive_build_goal.sh -c goal_file=skills/default-workflow/goal-build-pr.md \
    -c goal_once=false -c "autodrive_state_dir=${sd}" -c "repo_path=${repo}"
  # Only the keys this run holds; an unset one keeps the loop's own default.
  for k in step_recovery_dir base_branch existing_branch pr_number issue_number allow_existing_pr \
      strict_delivery closing_reference allow_no_op pr_repository; do
    v="RECIPE_VAR_${k}"
    [ -z "${!v+set}" ] || set -- "$@" -c "${k}=${!v}"
  done
  echo "build goal: the shared goal loop runs with its state in ${sd}" >&2
  amplihack recipe run "$recipe" --no-detach -w "$repo" "$@" \
    --context-from-file "task_description=${scratch}/task_description" \
    --context-from-file "goal_notes=${scratch}/final_requirements" \
    --context-from-file "worktree_setup=${scratch}/worktree_setup" >&2
  rc=$?
  if [ "$rc" -ne 0 ]; then
    echo "ERROR: autodrive_build_run.sh loop: the goal loop run exited ${rc}; its turn records are in ${sd}/turns" >&2
    exit "$rc"
  fi
  if goal_json_file "$sd/loop-end.json" 65536; then jq -c . "$sd/loop-end.json"
  else jq -cn --arg d "$sd" '{end:null, error:("the goal loop run exited 0 and left no loop-end.json in " + $d)}'; fi
}

do_report() {
  local STATE="${1%/}" end="none" detail="" turn=null errs='[]' n="" how="" v rc url="" pstate="" draft=null ts success
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
  case "$pstate:$draft:$end" in
    MERGED:*) ts=MERGED; success=true ;;
    CLOSED:*) ts=FAILED_CLOSED_UNMERGED; success=false ;;
    OPEN:false:ready|OPEN:false:merged) ts=IMPLEMENTED_VERIFIED; success=true ;;
    OPEN:true:ready) ts=EXISTING_OPEN_PR; success=true ;;
    *) ts=INCOMPLETE; success=false ;;
  esac
  jq -cn --arg end "$end" --arg d "$detail" --argjson turn "$turn" --arg n "$n" --arg how "$how" --arg u "$url" --arg ps "$pstate" \
    --argjson draft "$draft" --arg ts "$ts" --arg ok "$success" --argjson errs "$errs" --arg st "$STATE" '
    {schema_version:1, end:$end, detail:$d, turn:$turn, pr_number:$n, pr_url:$u, pr_found_by:$how,
     pr_state:(if $ps == "" then null else $ps end), pr_draft:$draft, terminal_state:$ts, terminal_success:$ok,
     terminal_reason:("the build loop ended " + $end + (if $d == "" then "" else ": " + $d end)), handoff:"false",
     state_dir:$st, errors:$errs}'
  return 0
}

case "${1:-}" in
  setup) [ "$#" -eq 1 ] || { echo "usage: autodrive_build_run.sh setup" >&2; exit 2; }; do_setup ;;
  loop) [ "$#" -eq 2 ] || { echo "usage: autodrive_build_run.sh loop <state_dir>" >&2; exit 2; }; do_loop "$2" ;;
  report) [ "$#" -eq 2 ] || { echo "usage: autodrive_build_run.sh report <state_dir>" >&2; exit 2; }; do_report "$2" ;;
  *) echo "usage: autodrive_build_run.sh setup|loop <state_dir>|report <state_dir>" >&2; exit 2 ;;
esac
