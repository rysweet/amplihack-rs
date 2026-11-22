#!/usr/bin/env bash
# autodrive_merge_loop_run.sh: runs the merge-ready loop for
# autodrive-merge-loop.yaml (step-02, and step-03c after a base sync, #2575).
# docs/reference/autodrive-merge-ready-round.md#autodrive_merge_loop_runsh.
#
#   autodrive_merge_loop_run.sh [<rounds>]
#
# The body of the former step-02 command, moved here so the base sync can run
# the same loop once more. The step's values arrive as RECIPE_VAR_* environment
# variables (merge_loop_preflight, task_description, plan_task_id, no_merge,
# autodrive_dry_run, merge_method, autodrive_budget_*), never by templating
# into a command.
# The loop driver, the crusty re-review and the round recipe are taken from
# beside this tool, never from the PR worktree.
#
# <rounds>, when given, asks for that many further rounds: it must match
# ^[0-9]+$ (at most 9 digits) and be at least 1, otherwise the tool exits 2
# without running the loop. The loop budget counts merge-ready rounds across
# runs that share the state directory, so the tool passes
# autodrive_budget_loop_rounds=<rounds already used + N>, never more than the
# operator's own autodrive_budget_loop_rounds when one is set. Without it the
# operator's budget passes through unchanged.
#
# Prints one loop line (JSON) read from the driver's own result file; the
# driver's standard output and standard error go to standard error, for the
# log, and a copy of them is kept in <state_dir>/merge-ready-driver.err, which
# outlives the run folder (#2849). merge-ready-result.json, or the partial file
# the driver left, stays in the state directory too. An UNREADABLE line names
# the kept file, or says it could not be kept; the agent reads the file itself.
#   0   DONE or WAITING (driver exit 0), or STUCK/BUDGET_EXHAUSTED/HANDOFF
#       (driver exit 1), or UNREADABLE when the result file does not match
#   1   USAGE_LIMIT, or a setup failure (named on standard error)
#   2   a bad <rounds> argument, a missing context helper, or a merge_method
#       that is not empty, merge, squash or rebase (#1888); no round runs
#   75  ORPHANED
#   79  terminal refusal, with a TERMINAL_REFUSAL line; never retried
#   *   any other driver exit, passed through
set -uo pipefail

ROUNDS=""
if [ "$#" -gt 1 ]; then
  echo "ERROR: autodrive_merge_loop_run.sh takes at most one argument, <rounds> [autodrive-merge-loop]" >&2
  exit 2
fi
if [ "$#" -eq 1 ]; then
  if ! [[ "$1" =~ ^[0-9]{1,9}$ ]] || [ "$((10#$1))" -lt 1 ]; then
    echo "ERROR: <rounds> must be 1 to 9 digits and at least 1 [autodrive-merge-loop]" >&2
    exit 2
  fi
  ROUNDS="$((10#$1))"
fi

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" 2>/dev/null && pwd -P)" \
  || { echo "ERROR: autodrive-tools-not-found: cannot resolve the directory of autodrive_merge_loop_run.sh [autodrive-merge-loop]" >&2; exit 1; }
cd "${REPO_PATH:-.}" 2>/dev/null || true
LOOP_HELPER="${HERE}/autodrive_loop.sh"
[ -f "$LOOP_HELPER" ] || { echo "ERROR: trusted autodrive loop helper unavailable" >&2; exit 1; }
# #1888: every merge-ready round's prompt names the run's merge method, so the
# method is checked here, before any round starts and before the state folder
# is touched, against the one list autodrive_merge.sh keeps. A method outside
# it stops the run. It is never emptied instead: an empty method lets the
# repository choose, and the PR could then be merged with a method nobody
# asked for.
MERGE_TOOL="${HERE}/autodrive_merge.sh"
[ -f "$MERGE_TOOL" ] || { echo "ERROR: autodrive-tools-not-found: ${MERGE_TOOL} [autodrive-merge-loop]" >&2; exit 1; }
# shellcheck source=autodrive_merge.sh
. "$MERGE_TOOL" || { echo "ERROR: cannot load ${MERGE_TOOL} [autodrive-merge-loop]" >&2; exit 1; }
if ! autodrive_merge_method_known "${RECIPE_VAR_merge_method:-}"; then
  printf "ERROR: the run's merge_method '%s' is not merge, squash or rebase, so no merge-ready round was started (#1888) [autodrive-merge-loop]\n" \
    "$(autodrive_merge_printable "${RECIPE_VAR_merge_method:-}" 32)" >&2
  exit 2
fi
g() { printf '%s' "${RECIPE_VAR_merge_loop_preflight:-}" | amplihack orch helper extract-json --require-field "$1" \
        | amplihack orch helper extract-field --field "$1" --default ''; }
DIR="$(g state_dir)"; PR="$(g pr)"; REPO="$(g repo)"
# Refuse an empty or unsafe state_dir before anything touches it.
case "$DIR" in
  ''|/|-*|*$'\n'*) echo "ERROR: no state_dir (empty or unsafe: refusing to touch it) [autodrive-merge-loop/step-02]" >&2; exit 1 ;;
esac
# Issue #1570: task_description is read with write_context_value
# (RECIPE_VAR_* or AMPLIHACK_CONTEXT_FILE) into a private file that the
# loop copies to its state directory. The uppercase TASK_DESCRIPTION
# alias is not used: the runner omits it for large values.
[ -f "$HERE/recipe_context_value.sh" ] || { echo "ERROR: amplifier-bundle/tools/recipe_context_value.sh not found beside autodrive_merge_loop_run.sh" >&2; exit 2; }
. "$HERE/recipe_context_value.sh" || exit 1
TASK_IN="$(umask 077; mktemp)" || exit 1
trap 'rm -f -- "$TASK_IN"' EXIT
write_context_value task_description "$TASK_IN" || { echo "ERROR: task_description unavailable" >&2; exit 1; }
# Commits after the clean crusty round go back to crusty BEFORE each
# merge round, from this shell, so that crusty loop runs at this step's
# depth like phase 2's. Started from a merge round step it would be one
# runner deeper and refused with exit 79 (PR #1520 review).
REREVIEW="$(dirname "$LOOP_HELPER")/autodrive_crusty_rereview.sh"
[ -f "$REREVIEW" ] || { echo "ERROR: autodrive-tools-not-found: ${REREVIEW} (the crusty re-review must come from the loop driver's install)" >&2; exit 1; }
[ -n "$REPO" ] || { echo "ERROR: preflight produced no repo; refusing to run a loop that would measure the launch checkout (#1543) [autodrive-merge-loop/step-02]" >&2; exit 1; }
# #1543: the loop runs in the preflight's repo (the PR worktree when
# pr_number was supplied). Tools stay on the launch side: pin
# AMPLIHACK_HOME, when it is unset, to the tree this tool lives in, so the
# rounds' tool ladders never fall through to PR-authored tools. The round
# recipe is passed by absolute path from that same tree: by name,
# `recipe run --working-dir <PR worktree>` would take the PR branch's own
# amplifier-bundle/recipes first and let the PR judge itself (PR #1588).
ROUND_RECIPE="$(cd "$(dirname "$LOOP_HELPER")/../recipes" 2>/dev/null && pwd -P)/autodrive-merge-round.yaml"
[ -f "$ROUND_RECIPE" ] || { echo "ERROR: autodrive-recipe-not-found: ${ROUND_RECIPE} (the round recipe must come from the loop driver's install, never the PR worktree) [autodrive-merge-loop/step-02]" >&2; exit 1; }
if [ -z "${AMPLIHACK_HOME:-}" ]; then
  AMPLIHACK_HOME="$(cd "$(dirname "$LOOP_HELPER")/../.." && pwd -P)" \
    || { echo "ERROR: cannot resolve the tool root of $LOOP_HELPER; refusing to run the loop" >&2; exit 1; }
  export AMPLIHACK_HOME
fi

# Review and fix steps use the legitimate PR worktree.
PR_WORKTREE="$REPO"
# #2575 M1: <rounds> more rounds is a loop budget of the merge-ready rounds
# already recorded in this state directory plus <rounds>, capped by the
# operator's own budget. An unreadable usage file counts 0 rounds used, which
# can only make the budget smaller, never larger.
LOOP_BUDGET="${RECIPE_VAR_autodrive_budget_loop_rounds:-}"
if [ -n "$ROUNDS" ]; then
  USED=0; USAGE="${DIR%/}/autodrive-usage.tsv"
  if [ -f "$USAGE" ] && [ ! -L "$USAGE" ] && [ -O "$USAGE" ]; then
    USED="$(awk -F '\t' '$1 == "merge-ready" && $3 == "round" { n++ } END { printf "%d\n", n }' "$USAGE" 2>/dev/null)" || USED=0
    [[ "$USED" =~ ^[0-9]{1,9}$ ]] || USED=0
  fi
  WANT=$((10#$USED + ROUNDS))
  if [[ "$LOOP_BUDGET" =~ ^[0-9]{1,12}$ ]] && [ "$((10#$LOOP_BUDGET))" -gt 0 ] && [ "$((10#$LOOP_BUDGET))" -lt "$WANT" ]; then
    WANT="$((10#$LOOP_BUDGET))"
  fi
  LOOP_BUDGET="$WANT"
  echo "INFO: ${ROUNDS} more merge-ready round(s): ${USED} used, loop budget ${LOOP_BUDGET} [autodrive-merge-loop]" >&2
fi

# #2575: the loop result is read from the driver's own result file, never
# from text on its standard output, which goes to stderr for the log.
RESULT_FILE="${DIR%/}/merge-ready-result.json"
rm -f -- "$RESULT_FILE"
# #2849: the driver's output is the only account of a driver that dies before
# it writes a result, and the run folder it was logged into does not always
# outlive the drive. Keep a copy beside the result file, in the PR's state
# directory. The copy is evidence, never a condition: nothing reads it to
# decide anything, and a copy that cannot be made is not an error.
# KEPT holds the file's path while the copy is this run's to write and read,
# and is emptied the moment it is not, so nothing below can name or write a
# file this run did not create.
#
# Write only into a state directory that is already there, is not a symbolic
# link and is ours. This runs before the driver re-checks the directory.
# Whatever is at the name is removed first and the file opened new, once, as
# descriptor 4: an exclusive open (`set -C`) under umask 077, so a foreign
# file keeps neither its content nor its mode, and a planted regular file, a
# link to one or a dangling link is replaced rather than written through.
# Noclobber refuses only an existing regular file: a link to a device or a
# FIFO, planted in the window between the removal and the open, is opened
# rather than refused, and a FIFO with no reader would block the open.
# Planting one needs write access to a state folder that is 0700 and owned by
# the caller. See #2879. The descriptor is held open for the rest of the run,
# so there is no second open by name for a link planted after this point to
# catch. A directory at the name cannot be removed, so the open fails and no
# copy is made. The tool never creates the state directory:
# autodrive_private_dir stays the only authority for that.
#
# The open is a grouped command so that its `2>/dev/null` lasts only for the
# open. Written as `exec 4>"$KEPT" 2>/dev/null` the second redirection would
# apply to this shell from then on, and this shell's standard error is the
# step's whole log. The umask is restored for the same reason: the driver
# below would otherwise inherit 077.
#
# POSIX mode is off for the open and restored after it. A redirection error
# on `exec` ends a POSIX-mode shell outright instead of returning 1, and bash
# enters that mode on its own when POSIXLY_CORRECT is in the environment at
# startup. The step would then end, silently and before the driver ran, over
# a copy it could not open: the copy deciding the run, by another route.
KEPT="${DIR%/}/merge-ready-driver.err"
if [ -d "$DIR" ] && [ ! -L "${DIR%/}" ] && [ -O "$DIR" ]; then
  rm -f -- "$KEPT" 2>/dev/null
  OLD_UMASK="$(umask)"; umask 077
  POSIX_WAS=""; if [[ -o posix ]]; then POSIX_WAS=1; fi
  set +o posix
  { set -C; exec 4>"$KEPT"; } 2>/dev/null || KEPT=""
  set +C; umask "$OLD_UMASK"
  [ -z "$POSIX_WAS" ] || set -o posix
else
  KEPT=""
fi
# Opening the copy is never a condition either: with no file to keep, the
# descriptor goes to /dev/null so that the writer below always runs. Putting
# the redirection on the writer instead would skip the writer when the open
# failed, and the driver would then die of SIGPIPE on its next write. That
# would let the copy decide the run, which is exactly what it must not do.
[ -n "$KEPT" ] || exec 4>/dev/null
# #2849: a copy that was not made is said once in the log, on every ending.
# Only an UNREADABLE reason carried it before, so a STUCK or HANDOFF run lost
# the copy in silence, the very loss this change exists to fix. Printed
# before the driver starts, so it never lands between the driver's own lines.
# Still not a check: no gate, no exit status, no decision reads it.
[ -n "$KEPT" ] || printf 'NOTICE: the loop driver output could not be kept in %s (the state folder is missing, is a link, is owned by another user or is not writable): this log is the only account of it [autodrive-merge-loop]\n' "$DIR" >&2
# The copy is made here, by this shell, and not by `tee`: where /usr/bin/tee
# is the uutils implementation, which Ubuntu 25.10 ships as the default, it
# can abort part-way through any input larger than one pipe buffer, about one
# run in ten, and the bytes it has already read die with it, so the copy AND
# this step's own log lose the driver's output. That is the very loss this
# change exists to prevent.
# A read loop cannot abort that way and writes the same bytes, reaching the
# log a line at a time instead of a block at a time. NUL bytes are the one
# difference: `read` drops them. It is also slower than a block copy, because
# bash reads a pipe a byte at a time; the reference page records the rate
# measured.
#
# The writer's exit status has one purpose: non-zero when a write to the copy
# failed, which the PARTIAL clause below reports and nothing else reads. A
# write to the log is never counted, so the log stays whole even while the
# copy is failing.
keep_and_log() {
  local line="" rc=0
  while IFS= read -r line; do
    printf '%s\n' "$line" >&2
    printf '%s\n' "$line" >&4 2>/dev/null || rc=1
  done
  # A last line the driver ended without a newline: the same bytes, with
  # nothing added, so the copy is what the driver wrote.
  if [ -n "$line" ]; then
    printf '%s' "$line" >&2
    printf '%s' "$line" >&4 2>/dev/null || rc=1
  fi
  return "$rc"
}
# Deliberately not a check: with KEPT empty the copy goes to /dev/null, the
# driver still runs, its output still reaches this step's standard error in
# full, and every exit status below is unchanged. Do not turn this into a gate.
# `4>&-` keeps the copy's descriptor out of the driver and its agents.
{ bash "$LOOP_HELPER" --task-file "$TASK_IN" \
  --loop-name "merge-ready" \
  --before-round "$REREVIEW" \
  --round-recipe "$ROUND_RECIPE" \
  --clean-token "MERGE_READY" \
  --verdict-field "merge_ready_verdict" \
  --repo "$PR_WORKTREE" \
  --state-dir "$DIR" \
  --context "repo_path=$PR_WORKTREE" \
  --context "step_recovery_dir=${RECIPE_VAR_step_recovery_dir:-}" \
  --context "pr_number=${PR}" \
  --context "plan_task_id=${RECIPE_VAR_plan_task_id:-}" \
  --context "pr_url=${PR_URL:-}" \
  --context "no_merge=${RECIPE_VAR_no_merge:-true}" \
  --context "autodrive_dry_run=${RECIPE_VAR_autodrive_dry_run:-false}" \
  --context "merge_method=${RECIPE_VAR_merge_method:-}" \
  --context "autodrive_base_branch=$(g base_branch)" \
  --context "autodrive_qa_evidence=${DIR}/qa-evidence.json" \
  --context "autodrive_state_dir=${DIR}" \
  --context "autodrive_budget_loop_rounds=${LOOP_BUDGET}" \
  --context "autodrive_budget_total_rounds=${RECIPE_VAR_autodrive_budget_total_rounds:-}" \
  --context "autodrive_budget_wall_seconds=${RECIPE_VAR_autodrive_budget_wall_seconds:-}" 2>&1; } 4>&- | keep_and_log
# The driver's own status, never the pipeline's, and never `$?`: this step
# runs under `pipefail`, so a copy that fails (a full disk) would otherwise
# become the loop's exit and read a finished DONE as UNREADABLE, or lose a 79,
# a 75 or a pass-through. Keep this on the line directly after the pipeline.
PS=("${PIPESTATUS[@]}")
RC=${PS[0]}
# The copy is written; nothing below writes to it. Close the descriptor so the
# readers below, and anything they start, do not hold the file open for write.
exec 4>&-
# The copy's own status, for the reason only. A write that failed part-way (a
# full disk) leaves a kept file that stops mid-run. It changes no exit status
# and no decision; it stops the reason claiming a complete copy of output the
# file does not hold. Said in the log too, after the output.
PARTIAL=""
[ -n "$KEPT" ] && [ "${PS[1]:-0}" -ne 0 ] && PARTIAL="1"
[ -z "$PARTIAL" ] || printf 'NOTICE: only part of the loop driver output could be kept in %s (the copy failed part-way): this log is the only complete account of it [autodrive-merge-loop]\n' "$KEPT" >&2
# The tool maps the driver's exit itself, so a 79 is never hidden by
# continue_on_error (#2490).
line() { jq -cn --arg r "$1" --arg why "$2" '{loop:"merge-ready",loop_result:$r,round_label:"unknown",reason:$why}'; }

# What the reason says about the driver's output. The bare file name comes
# first and the absolute path after it: the hand-off leaves home-directory
# paths out of its public comment, so the name has to survive a redacted path.
#
# The clause names the file and stops. No code path opens the kept file, so no
# text the driver wrote can reach the reason, which lands in the trusted part
# of the hand-off prompt and probably a public PR comment. The hand-off prompt
# tells the agent to read the file itself, as untrusted evidence; an agent can
# judge what it finds there, which no filter here could do for it.
#
# A copy that stopped part-way keeps its name and its path, because a truncated
# file is still the evidence a person has to open, but says so first.
output_clause() {
  if [ -z "$KEPT" ]; then
    printf '%s' "its output could not be kept (the state folder is missing, is a link, is owned by another user or is not writable)"
    return 0
  fi
  if [ -n "$PARTIAL" ]; then
    printf 'only part of its output could be kept, in merge-ready-driver.err at %s' "$KEPT"
    return 0
  fi
  printf 'its output was kept in merge-ready-driver.err at %s' "$KEPT"
}

# The UNREADABLE reason: the opening words the tool has always printed, then
# where the driver's output went, then what became of the result file. Every
# part is derived from files this run already wrote and from RESULT, which the
# reader below sets before this is called; none of it changes an exit status or
# a decision. Capped at 900 printable characters so that the 24-character
# prefix the outcome step prepends (`loop result UNREADABLE: `) still fits
# inside its own 1000-byte cut, and so that a cut from the end eats the
# result-file clause first, never the file name.
unreadable_why() {
  local state said
  if [ ! -f "$RESULT_FILE" ]; then
    state="is absent"
  else
    said="$(printf '%s' "$RESULT" | LC_ALL=C tr -cd 'A-Za-z0-9_-' | head -c 64)"
    if [ -n "$said" ]; then state="says ${said}"; else state="is unreadable"; fi
  fi
  printf 'the loop driver exited %s without a readable merge-ready-result.json; %s; merge-ready-result.json %s at %s' \
    "$RC" "$(output_clause)" "$state" "$RESULT_FILE" | LC_ALL=C tr -cd '\040-\176' | head -c 900
}

LINE=""
if [ -f "$RESULT_FILE" ] && [ ! -L "$RESULT_FILE" ]; then
  LINE="$(jq -c 'select(type == "object" and (.loop_result | type) == "string")' "$RESULT_FILE" 2>/dev/null | tail -n 1)"
fi
RESULT="$(printf '%s' "$LINE" | jq -r '.loop_result // empty' 2>/dev/null)"
case "$RC" in
  79)
    echo "ERROR: exit 79 terminal policy refusal in the merge-ready loop (#1327/#1332). Final — surfaced and stopped, never retried into the guard." >&2
    line TERMINAL_REFUSAL "the merge-ready loop exited 79"
    exit 79 ;;
  75) [ -n "$LINE" ] && printf '%s\n' "$LINE"; exit 75 ;;
  0|1)
    # The driver writes DONE and WAITING only on its exit-0 path, so they are
    # read only with exit 0 (#2575 B2: WAITING is a round that waits for checks).
    case "$RC:$RESULT" in
      0:DONE|0:WAITING) ;;
      0:*|1:DONE|1:WAITING|1:)
        line UNREADABLE "$(unreadable_why)"
        exit 0 ;;
    esac
    printf '%s\n' "$LINE"
    # A usage limit stops the run; a later run continues the work.
    [ "$RESULT" = USAGE_LIMIT ] && exit 1
    exit 0 ;;
  *) exit "$RC" ;;
esac
