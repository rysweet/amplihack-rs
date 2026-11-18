#!/usr/bin/env bash
# Self-asserting gadugi-test scenario body for issue #2783 (first piece):
# `amplihack recipe guide` finds a run from what the run recorded when it
# started, whatever the calling shell's work-root variables say.
#
# Before this change the command resolved a target only under
# `<work root>/runs/`, taken from AMPLIHACK_WORK_ROOT in the caller's own
# environment. A factory lane starts its runs with a work root of its own, so
# an operator in an ordinary shell could not address them. The run now records
# `run_dir` in its `amplihack.recipe.log_pointer` line, and `recipe guide`
# reads that line back from the recipe log directory, which is derived from
# HOME (or `recipe.log_dir`) and so does not depend on the work root.
#
# Black boxes, all shipped code, run unchanged: the real binary's
# `recipe run` (through the pinned recipe runner) and `recipe guide`. Nothing
# is stubbed and nothing reaches the network; the recipe is one `true` step.
#
# The run is started in the foreground with its stderr redirected into
# `<recipe log dir>/<session>.log`. That file is byte for byte what
# `recipe run --detach` leaves behind, so the scenario exercises the same
# artefact without needing tmux in the harness.
#
# Cases, each a `recipe guide` run under `env -i` with HOME and PATH only, so
# neither AMPLIHACK_WORK_ROOT nor AMPLIHACK_WORK_ROOT_EXPLICIT is set:
#   by_session_name       the detached session name
#   by_run_folder_name    the run folder under the alternative work root
#   by_correlation_id     the pointer line's run_id
#   by_issue_number       the issue the run recorded in its context
#   unmatched_names_where a target that matches nothing exits 1 and the line
#                         names the log directory and the runs directory
#
# Every command runs under `env -i` with an explicit allowlist, and everything
# is created under one mktemp folder that the EXIT trap removes. The real
# ~/.claude and ~/.amplihack are never read or written: HOME is the scenario's
# own folder throughout.
#
# Run by hand: bash tests/gadugi/run-issue-2783-guide-without-work-root-scenario.sh
# It builds amplihack with cargo first. Set AMPLIHACK_2783_QA_BIN to an
# amplihack binary to skip the build. It needs recipe-runner-rs
# (RECIPE_RUNNER_RS_PATH, or on PATH); without it the scenario fails. On
# success it prints ALL_CASES_PASSED and exits 0; otherwise SCENARIO_FAILED
# and exit 1.
#
# It is bash, not Rust, because every gadugi driver is bash.
# crates/amplihack-cli/tests/issue_2783_recipe_guide_work_root.rs covers the
# same contract, and the hostile inputs, from inside the crate.
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd -P)"
SESSION="amp-issue-2783-qa-20261118-205000"
ISSUE="2783"

# A private umask, so every folder this scenario makes passes the trust rule
# that `recipe guide` applies (no group or other write).
umask 022

for c in jq; do
  p="$(command -v "$c" 2>/dev/null)"
  case "$p" in
    /*) ;;
    *) echo "FAIL: $c is required"; echo "SCENARIO_FAILED"; exit 1 ;;
  esac
done

WORK="$(mktemp -d "${TMPDIR:-/tmp}/issue-2783-qa.XXXXXX")" || {
  echo "FAIL: cannot create a temporary directory"; echo "SCENARIO_FAILED"; exit 1
}
trap 'rm -rf -- "$WORK"' EXIT
chmod 700 "$WORK"
WORK="$(cd "$WORK" && pwd -P)" || { echo "FAIL: cannot resolve the temporary directory"; echo "SCENARIO_FAILED"; exit 1; }

# --- the binary under test --------------------------------------------------
BIN="${AMPLIHACK_2783_QA_BIN:-}"
if [ -z "$BIN" ]; then
  if ! BIN="$(cd "$REPO_ROOT" && cargo build -p amplihack --bin amplihack --locked \
      --message-format=json-render-diagnostics 2>"$WORK/build.log" \
      | awk 'match($0, /"executable":"[^"]*\/amplihack"/) { bin = substr($0, RSTART + 14, RLENGTH - 15) } END { print bin }')"; then
    echo "FAIL: cargo build -p amplihack --bin amplihack failed"
    tail -n 40 "$WORK/build.log"
    echo "SCENARIO_FAILED"; exit 1
  fi
fi
if [ -z "$BIN" ] || [ ! -x "$BIN" ]; then
  echo "FAIL: no executable amplihack binary (got '${BIN}')"
  echo "SCENARIO_FAILED"; exit 1
fi
case "$BIN" in /*) ;; *) BIN="$PWD/$BIN" ;; esac
echo "binary under test: $BIN"

RUNNER="${RECIPE_RUNNER_RS_PATH:-}"
if [ -z "$RUNNER" ]; then
  RUNNER="$(command -v recipe-runner-rs || true)"
fi
if [ -z "$RUNNER" ] || [ ! -x "$RUNNER" ]; then
  echo "FAIL: recipe-runner-rs is required (set RECIPE_RUNNER_RS_PATH or put it on PATH)"
  echo "SCENARIO_FAILED"; exit 1
fi
case "$RUNNER" in /*) ;; *) RUNNER="$PWD/$RUNNER" ;; esac

# --- the scenario's own home, state and work roots --------------------------
HOME_DIR="$WORK/home"
ALT_WORK="$WORK/factory-lane/work"
LOG_DIR="$HOME_DIR/.amplihack/recipe-logs"
RUN_CWD="$WORK/cwd"
mkdir -p -m 700 "$HOME_DIR/.amplihack" "$ALT_WORK" "$LOG_DIR" "$RUN_CWD" || {
  echo "FAIL: cannot create the scenario folders"; echo "SCENARIO_FAILED"; exit 1
}

cat > "$WORK/recipe.yaml" <<'YAML'
name: "issue-2783-guide-qa"
description: "one bash step, so the run exists and records where it put itself"
version: "1.0.0"
context: {}
steps:
  - id: "noop"
    type: "bash"
    command: "true"
YAML

CASE_PATH="$(dirname -- "$(command -v jq)"):/usr/local/bin:/usr/bin:/bin"

fail=0
passed=0
pass() { echo "PASS: $1"; passed=$((passed + 1)); }
flunk() {
  echo "FAIL: $1"
  shift
  for f in "$@"; do
    [ -f "$f" ] && tail -n 20 "$f" | sed "s|^|    $(basename -- "$f"): |"
  done
  fail=1
}

# `recipe guide <target> -` with the text on stdin, under env -i and with no
# work-root variable in sight. stdout and stderr are kept per case.
guide() {
  local name="$1" target="$2" dir="$WORK/case-$1"
  mkdir -p -m 700 "$dir"
  printf 'Keep the public API; this is the %s case.\n' "$name" \
    | env -i \
        HOME="$HOME_DIR" \
        PATH="$CASE_PATH" \
        TMPDIR="$WORK/tmp" \
        AMPLIHACK_NONINTERACTIVE=1 \
        AMPLIHACK_GOVERNOR=off \
        AMPLIHACK_ROTATE_AUTOSTART=0 \
        AMPLIHACK_SKIP_AUTO_INSTALL=1 \
        AMPLIHACK_NO_UPDATE_CHECK=1 \
        AMPLIHACK_NO_FRESHNESS_CHECK=1 \
        "$BIN" recipe guide "$target" - \
        > "$dir/stdout" 2> "$dir/stderr"
  echo "$?" > "$dir/status"
}

# --- the run, with its stderr kept where a detached run keeps it ------------
mkdir -p -m 700 "$WORK/tmp"
RUN_LOG="$LOG_DIR/$SESSION.log"
: > "$RUN_LOG"
chmod 600 "$RUN_LOG"
env -i \
  HOME="$HOME_DIR" \
  PATH="$CASE_PATH" \
  TMPDIR="$WORK/tmp" \
  RECIPE_RUNNER_RS_PATH="$RUNNER" \
  AMPLIHACK_WORK_ROOT="$ALT_WORK" \
  AMPLIHACK_WORK_ROOT_EXPLICIT=1 \
  AMPLIHACK_NONINTERACTIVE=1 \
  AMPLIHACK_GOVERNOR=off \
  AMPLIHACK_ROTATE_AUTOSTART=0 \
  AMPLIHACK_SKIP_AUTO_INSTALL=1 \
  AMPLIHACK_SKIP_REFLECTION=1 \
  AMPLIHACK_NO_UPDATE_CHECK=1 \
  AMPLIHACK_NO_FRESHNESS_CHECK=1 \
  "$BIN" recipe run "$WORK/recipe.yaml" -c "issue_number=$ISSUE" -w "$RUN_CWD" \
  > "$WORK/run.stdout" 2>> "$RUN_LOG"
run_status=$?

POINTER="$(grep -m 1 -a '^amplihack\.recipe\.log_pointer ' "$RUN_LOG" | sed 's/^amplihack\.recipe\.log_pointer //')"
if [ -z "$POINTER" ]; then
  flunk "the run left no amplihack.recipe.log_pointer line (recipe run exited $run_status)" "$RUN_LOG"
  echo "SCENARIO_FAILED"; exit 1
fi
RUN_DIR="$(printf '%s' "$POINTER" | jq -r '.run_dir // empty')"
CORRELATION="$(printf '%s' "$POINTER" | jq -r '.run_id // empty')"
RECORDED_ISSUE="$(printf '%s' "$POINTER" | jq -r '.issue_number // empty')"

if [ -z "$RUN_DIR" ]; then
  flunk "the pointer line records no run_dir" "$RUN_LOG"
  echo "SCENARIO_FAILED"; exit 1
fi
case "$RUN_DIR" in
  "$ALT_WORK"/runs/*) pass "run_dir_is_under_the_alternative_work_root - $RUN_DIR" ;;
  *) flunk "run_dir '$RUN_DIR' is not under $ALT_WORK/runs" "$RUN_LOG" ;;
esac
[ -f "$RUN_DIR/run.json" ] \
  && pass "the_recorded_folder_holds_run_json" \
  || flunk "the recorded folder has no run.json" "$RUN_LOG"
[ "$RECORDED_ISSUE" = "$ISSUE" ] \
  && pass "the_pointer_records_the_issue" \
  || flunk "the pointer records issue '$RECORDED_ISSUE', not $ISSUE" "$RUN_LOG"

RUN_FOLDER="$(basename -- "$RUN_DIR")"

# --- the four targets that must reach the recorded folder -------------------
check_lands_in_run() {
  local name="$1" target="$2" dir="$WORK/case-$1"
  guide "$name" "$target"
  local status line
  status="$(cat "$dir/status")"
  if [ "$status" != "0" ]; then
    flunk "$name: recipe guide exited $status for target '$target'" "$dir/stderr"
    return
  fi
  line="$(head -n 1 "$dir/stdout")"
  case "$line" in
    "guidance appended: $RUN_DIR/guidance/"*" (run)")
      pass "$name - $line" ;;
    *)
      flunk "$name: the entry did not land in $RUN_DIR/guidance (stdout: $line)" "$dir/stderr" ;;
  esac
}

check_lands_in_run by_session_name "$SESSION"
check_lands_in_run by_run_folder_name "$RUN_FOLDER"
check_lands_in_run by_correlation_id "$CORRELATION"
check_lands_in_run by_issue_number "$ISSUE"

# Nothing was written under the default work root this shell would have used.
if [ -e "$HOME_DIR/.amplihack/work/runs/$RUN_FOLDER" ]; then
  flunk "an entry was written under the default work root"
else
  pass "nothing_written_under_the_default_work_root"
fi

# --- a target that matches nothing says what was looked for and where -------
guide unmatched_names_where "nightly-fix-2783"
status="$(cat "$WORK/case-unmatched_names_where/status")"
err="$(cat "$WORK/case-unmatched_names_where/stderr")"
guide_line="$(printf '%s\n' "$err" | grep -m 1 '^recipe guide: ' || true)"
if [ "$status" != "1" ]; then
  flunk "unmatched_names_where: expected exit 1, got $status" "$WORK/case-unmatched_names_where/stderr"
elif [ -s "$WORK/case-unmatched_names_where/stdout" ]; then
  flunk "unmatched_names_where: stdout is not empty"
elif [ -z "$guide_line" ]; then
  flunk "unmatched_names_where: no 'recipe guide: ' line on stderr" "$WORK/case-unmatched_names_where/stderr"
else
  missing=""
  case "$guide_line" in *'"nightly-fix-2783"'*) ;; *) missing="$missing target" ;; esac
  case "$guide_line" in *"$LOG_DIR"*) ;; *) missing="$missing log-directory" ;; esac
  case "$guide_line" in *"$HOME_DIR/.amplihack/work/runs"*) ;; *) missing="$missing runs-directory" ;; esac
  if [ -n "$missing" ]; then
    flunk "unmatched_names_where: the line omits$missing: $guide_line"
  else
    pass "unmatched_names_where - $guide_line"
  fi
fi

echo "cases passed: $passed"
if [ "$fail" -ne 0 ]; then
  echo "SCENARIO_FAILED"
  exit 1
fi
echo "ALL_CASES_PASSED"
