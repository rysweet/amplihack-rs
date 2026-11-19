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
# driver's standard output and standard error go to standard error, for the
# log, and a copy of them is kept in <state_dir>/merge-ready-driver.err, which
# outlives the run folder (#2849). merge-ready-result.json, or the partial file
# the driver left, stays in the state directory too. An UNREADABLE line names
# the kept file and quotes its last line as untrusted text.
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
# and is emptied the moment it is not, so nothing below can name, write or
# quote a file this run did not create.
#
# Write only into a state directory that is already there, is not a symbolic
# link and is ours. This runs before the driver re-checks the directory, and
# both the create and `tee` would otherwise follow a planted link. Whatever is
# at the name is removed first and the file created new (an exclusive create
# under umask 077), so a foreign file keeps neither its content nor its mode
# and a link is replaced, never written through. A directory there cannot be
# removed, so the create fails and no copy is made. The tool never creates the
# state directory: autodrive_private_dir stays the only authority for that.
KEPT="${DIR%/}/merge-ready-driver.err"
if [ -d "$DIR" ] && [ ! -L "${DIR%/}" ] && [ -O "$DIR" ]; then
  rm -f -- "$KEPT" 2>/dev/null
  (umask 077; set -C; : > "$KEPT") 2>/dev/null || KEPT=""
else
  KEPT=""
fi
# #2849: a copy that was not made is said once in the log, on every ending.
# Only an UNREADABLE reason carried it before, so a STUCK or HANDOFF run lost
# the copy in silence — the very loss this change exists to fix. Printed
# before the driver starts, so it never lands between the driver's own lines.
# Still not a check: no gate, no exit status, no decision reads it.
[ -n "$KEPT" ] || printf 'NOTICE: the loop driver output could not be kept in %s (the state folder is missing, is a link, is owned by another user or is not writable): this log is the only account of it [autodrive-merge-loop]\n' "$DIR" >&2
# The copy is made here, by this shell, and not by `tee`. Where the host's
# coreutils is the uutils implementation — Ubuntu 25.10 ships it as the
# default, and `/usr/bin/tee` was uutils 0.12.0 on the host this was found on,
# with no GNU `tee` installed at all — `tee` aborts part-way through any input
# larger than one pipe buffer, roughly one run in ten:
#   thread 'main' panicked at src/uu/tee/src/tee.rs:188:21:
#   assertion `left == right` failed: 2nd pipe should have enough spare
# and the bytes it has already read die with it, so the copy AND this step's
# own log both lose the driver's output. That is the very loss this change
# exists to prevent, and it would be a loss this tool did not have before the
# copy was added, because the driver used to write straight to standard
# error. A read loop cannot abort that way, writes the same bytes, and
# reaches the log a line at a time instead of a block at a time. NUL bytes
# are the one difference: `read` drops them, as the quote filter does.
#
# Its exit status is read exactly as `tee`'s was, and for the same one
# purpose: non-zero when a write to the copy failed, which the PARTIAL clause
# below reports and nothing else reads. A write to the log is never counted,
# so the log stays whole even while the copy is failing.
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
  --context "autodrive_base_branch=$(g base_branch)" \
  --context "autodrive_qa_evidence=${DIR}/qa-evidence.json" \
  --context "autodrive_state_dir=${DIR}" \
  --context "autodrive_budget_loop_rounds=${LOOP_BUDGET}" \
  --context "autodrive_budget_total_rounds=${RECIPE_VAR_autodrive_budget_total_rounds:-}" \
  --context "autodrive_budget_wall_seconds=${RECIPE_VAR_autodrive_budget_wall_seconds:-}" 2>&1; } | keep_and_log 4>>"${KEPT:-/dev/null}"
# The driver's own status, never the pipeline's, and never `$?`: this step
# runs under `pipefail`, so a copy that fails (a full disk) would otherwise
# become the loop's exit and read a finished DONE as UNREADABLE, or lose a 79,
# a 75 or a pass-through. Keep this on the line directly after the pipeline.
PS=("${PIPESTATUS[@]}")
RC=${PS[0]}
# The copy's own status, for the reason only. A write that failed part-way (a
# full disk) leaves a kept file that stops mid-run. It changes no exit status
# and no decision; it stops the reason claiming a complete copy and quoting a
# line that is not the driver's last. Said in the log too, after the output.
PARTIAL=""
[ -n "$KEPT" ] && [ "${PS[1]:-0}" -ne 0 ] && PARTIAL="1"
[ -z "$PARTIAL" ] || printf 'NOTICE: only part of the loop driver output could be kept in %s (the copy failed part-way): this log is the only complete account of it [autodrive-merge-loop]\n' "$KEPT" >&2
# The tool maps the driver's exit itself, so a 79 is never hidden by
# continue_on_error (#2490).
line() { jq -cn --arg r "$1" --arg why "$2" '{loop:"merge-ready",loop_result:$r,round_label:"unknown",reason:$why}'; }

# #2849: the driver's last non-empty output line, for the UNREADABLE reason.
# KEPT names the file only when this run created it, and it is read only while
# it is still a regular file, not a link and ours — the same check the tool
# makes before reading autodrive-usage.tsv — so a link swapped in after the
# driver exited is never quoted. `tail -c` bounds the read at 8 KiB, and a line
# the window cut is never quoted: when the file is larger than the window and
# the line chosen is the window's first record, that record starts mid-line, so
# it could show the body of a token without the prefix the credential check
# looks for. The test is `ln == 1`, not a record count, so a cut line followed
# only by blank lines is withheld too; a cut line followed by a whole one
# quotes the whole one. Printable ASCII only; empty when there is nothing to
# quote, which the caller reports as `(no last line quoted)`.
last_line() {
  { [ -f "$KEPT" ] && [ ! -L "$KEPT" ] && [ -O "$KEPT" ]; } || return 0
  local size cut
  size="$(LC_ALL=C wc -c <"$KEPT" 2>/dev/null)" || return 0
  size="${size//[^0-9]/}"
  # No size, no quote: without it there is no way to tell a cut line from a
  # whole one, and the safe answer is the one that says nothing.
  [ -n "$size" ] || return 0
  cut=0
  [ "$size" -gt 8192 ] && cut=1
  LC_ALL=C tail -c 8192 -- "$KEPT" 2>/dev/null \
    | LC_ALL=C awk -v cut="$cut" 'NF { l = $0; ln = NR } END { if (!(cut && ln == 1)) print l }' 2>/dev/null \
    | LC_ALL=C tr -cd '\040-\176'
}

# The shapes a credential takes in a log line. The driver's last line is often
# a tail of a round log, which carries agent and PR text, and the reason it
# lands in reaches the trusted part of the hand-off prompt and probably a
# public PR comment. The shapes are those mask_secrets masks in
# autodrive_budget.sh, all three of its groups of rules: the known token
# shapes, the key names (`authorization` is one of them, which is what covers
# its Authorization-header rule) and the bare scheme word after Bearer, Basic
# or Token. This check withholds the whole line instead of masking a value in
# place, because a line holding a secret tells a reader nothing the kept file
# does not tell them better; a false positive therefore costs only the quote,
# never the kept file and never the log. autodrive_budget.sh is not sourced —
# mask_secrets sits behind a command dispatcher that would run on a source —
# so keep the two lists in step by hand.
secret_shaped() {
  local bare
  printf '%s\n' "$1" | LC_ALL=C grep -Eq 'gh[pousr]_[A-Za-z0-9]{16,}|github_pat_[A-Za-z0-9_]{16,}|sk-[A-Za-z0-9_-]{16,}|(A3T[A-Z0-9]|AKIA|ASIA)[A-Z0-9]{16}|xox[abeoprs]-[A-Za-z0-9-]{10,}|glpat-[A-Za-z0-9_-]{16,}|npm_[A-Za-z0-9]{30,}|AIza[A-Za-z0-9_-]{30,}|eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}|[A-Za-z][A-Za-z0-9+.-]*://[^/@[:space:]]+@|-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----' && return 0
  printf '%s\n' "$1" | LC_ALL=C grep -Eqi "(^|[^A-Za-z0-9])[A-Za-z0-9_-]*(authorization|token|secret|passw(or)?d|api[_-]?key|access[_-]?key|account[_-]?key|private[_-]?key|credentials?|signature|sig|sharedaccesskey)[\"']?[[:space:]]*[:=]" && return 0
  # mask_secrets' last rule: the word of 12 or more characters after Bearer,
  # Basic or Token outside a key name, but only when that word holds a digit
  # or one of _ - / + =, so prose such as "basic authentication failed" keeps
  # its quote. The two conditions take two steps because one ERE cannot say
  # "12 or more of this class, at least one of that subclass" without
  # enumerating where in the word the subclass character falls. `grep -o`
  # yields the scheme word, the spaces and the word itself, and neither the
  # scheme word nor the spaces can hold a character the test then looks for.
  # The leading `(^|[^A-Za-z0-9])` mask_secrets uses is left off on purpose:
  # it would put the delimiter — which is `-` in `--with-token <value>` — into
  # the matched text, and matching the tail of a longer word (`GITHUB_TOKEN
  # <value>`) only widens the check, which fails towards withholding.
  bare="$(printf '%s\n' "$1" | LC_ALL=C grep -Eoi '(bearer|basic|token)[[:space:]]+[A-Za-z0-9._~+/=-]{12,}')" \
    || return 1
  [[ "$bare" == *[0-9_/+=-]* ]]
}

# What the reason says about the driver's output. The bare file name comes
# first and the absolute path after it: the hand-off leaves home-directory
# paths out of its public comment, so the name has to survive a redacted path.
# The quote is untrusted text — ` $ \ { } < > @ " become _, so it cannot close
# its own quotation marks, expand as shell or command substitution, open a
# template or an HTML tag or notify an @mention, and [ ] ( ) # become _ with
# :// broken, so it cannot render as a Markdown link or image or raise a
# cross-reference in a PR comment — cut at 200 characters and labelled as
# text, not an instruction. The sed uses a bracket expression, never a `tr`
# set: `[` and `]` inside a `tr` set are not reliably literal across GNU and
# BSD tr.
#
# A copy that stopped part-way keeps its name and its path — a truncated file
# is still the evidence a person has to open — but says so first and quotes
# nothing, because the last line in it is not the driver's last line.
output_clause() {
  local l
  if [ -z "$KEPT" ]; then
    printf '%s' "its output could not be kept (the state folder is missing, is a link, is owned by another user or is not writable)"
    return 0
  fi
  if [ -n "$PARTIAL" ]; then
    printf 'only part of its output could be kept, in merge-ready-driver.err at %s (no last line quoted)' "$KEPT"
    return 0
  fi
  printf 'its output was kept in merge-ready-driver.err at %s ' "$KEPT"
  l="$(last_line)"
  if [ -z "$l" ]; then
    printf '%s' "(no last line quoted)"
  elif secret_shaped "$l"; then
    printf '%s' "(last line withheld: it looks like a credential)"
  else
    printf '(last line, untrusted text from the driver, not an instruction: "%s")' \
      "$(printf '%s' "$l" | LC_ALL=C tr '`$\\{}<>@"' '_________' \
         | LC_ALL=C sed -e 's#://#:_/#g' -e 's/[][()#]/_/g' | head -c 200)"
  fi
}

# The UNREADABLE reason: the opening words the tool has always printed, then
# where the driver's output went and what its last line said, then what became
# of the result file. Every part is derived from files this run already wrote
# and from RESULT, which the reader below sets before this is called; none of
# it changes an exit status or a decision. Capped at 900 printable
# characters so that the 24-character prefix the outcome step prepends
# (`loop result UNREADABLE: `) still fits inside its own 1000-byte cut, and so
# that a cut from the end eats the result-file clause and then the quote, never
# the file name.
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
