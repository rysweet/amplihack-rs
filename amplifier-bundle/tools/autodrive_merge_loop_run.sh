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
# autodrive_dry_run, autodrive_budget_*), never by templating into a command.
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
# driver's standard output goes to standard error, for the log.
#   0   DONE or WAITING (driver exit 0), or STUCK/BUDGET_EXHAUSTED/HANDOFF
#       (driver exit 1), or UNREADABLE when the result file does not match
#   1   USAGE_LIMIT, or a setup failure (named on standard error)
#   2   a bad <rounds> argument or a missing context helper
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

# The PR worktree is the preflight's repo ($REPO, #1543): the PR head's
# linked worktree, where only each round's commit step works. Rust then
# allocates a private review generation at that PR commit; reviewed
# instruction files remain data and judging starts from separate trusted
# launch material (#1528/#2096). The loop owns its generation, including
# nested re-reviews, and releases it on exit.
PR_WORKTREE="$REPO"
[ -n "$PR_WORKTREE" ] || { echo "ERROR: prepared PR worktree is required (#1528)" >&2; exit 1; }
REVIEW_DIR="$(amplihack internal review-allocate "$PR_WORKTREE" "$DIR")" || exit 1
release_review_dir() { amplihack internal review-release "$REVIEW_DIR"; }
# Replaces the task-file trap above: the checkout is released and the task file removed on every exit from here.
on_exit() { local rc=$?; release_review_dir || rc=1; rm -f -- "${TASK_IN:-}"; exit "$rc"; }
trap on_exit EXIT

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
bash "$LOOP_HELPER" --task-file "$TASK_IN" \
  --loop-name "merge-ready" \
  --before-round "$REREVIEW" \
  --round-recipe "$ROUND_RECIPE" \
  --clean-token "MERGE_READY" \
  --verdict-field "merge_ready_verdict" \
  --repo "$PR_WORKTREE" \
  --review-dir "$REVIEW_DIR" \
  --state-dir "$DIR" \
  --context "repo_path=$PR_WORKTREE" \
  --context "step_recovery_dir=${RECIPE_VAR_step_recovery_dir:-}" \
  --context "pr_number=${PR}" \
  --context "plan_task_id=${RECIPE_VAR_plan_task_id:-}" \
  --context "pr_url=${PR_URL:-}" \
  --context "no_merge=${RECIPE_VAR_no_merge:-true}" \
  --context "autodrive_dry_run=${RECIPE_VAR_autodrive_dry_run:-false}" \
  --context "autodrive_base_branch=$(g base_branch)" \
  --context "autodrive_qa_evidence=${DIR}/qa-evidence.json" \
  --context "autodrive_state_dir=${DIR}" \
  --context "autodrive_budget_loop_rounds=${LOOP_BUDGET}" \
  --context "autodrive_budget_total_rounds=${RECIPE_VAR_autodrive_budget_total_rounds:-}" \
  --context "autodrive_budget_wall_seconds=${RECIPE_VAR_autodrive_budget_wall_seconds:-}" >&2
RC=$?
# The tool maps the driver's exit itself, so a 79 is never hidden by
# continue_on_error (#2490).
line() { jq -cn --arg r "$1" --arg why "$2" '{loop:"merge-ready",loop_result:$r,round_label:"unknown",reason:$why}'; }
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
        line UNREADABLE "the loop driver exited ${RC} without a readable merge-ready-result.json"
        exit 0 ;;
    esac
    printf '%s\n' "$LINE"
    # A usage limit stops the run; a later run continues the work.
    [ "$RESULT" = USAGE_LIMIT ] && exit 1
    exit 0 ;;
  *) exit "$RC" ;;
esac
