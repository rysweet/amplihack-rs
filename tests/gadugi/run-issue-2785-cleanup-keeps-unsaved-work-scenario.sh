#!/usr/bin/env bash
# Self-asserting gadugi-test scenario body for issue #2785: when a run ends,
# cleanup keeps a folder that holds unsaved Git work and removes one that does
# not.
#
# A step can leave a repository copy in its temporary folder, with a commit it
# never pushed or a file it never committed. The run's temporary folder
# (`runtime/`) is removed when the run ends, and that destroyed the work. Before
# removing a folder, cleanup now judges whether it holds a Git work tree with
# uncommitted changes, or commits that are not on its upstream. If it does, the
# folder stays and one line on stderr names the folder, the work tree and the
# reason. The run still ends normally.
#
# Black box, all shipped code, run unchanged: the real binary's `recipe run`
# (through the pinned recipe runner) and its own end-of-run cleanup. Nothing is
# stubbed and nothing reaches the network: the "remote" is a bare repository in
# the scenario's own folder, and each recipe is one bash step.
#
# Cases, each a separate run with a work root of its own, under `env -i`:
#   unpushed_commit_survives     the step clones the remote and commits without
#                                pushing: the folder and the commit are still
#                                there, the line says `has unpushed commits`,
#                                the run exits 0
#   uncommitted_file_survives    the step clones the remote and leaves a file it
#                                never committed: the folder and the file are
#                                still there, the line says `has uncommitted
#                                changes`, the run exits 0
#   clean_clone_is_removed       the step clones the remote and changes nothing:
#                                the folder is removed as before, no line is
#                                printed, the run exits 0
#
# A line is `amplihack: leaving "<folder>" in place: work tree "<clone>":
# <reason>`. At the end of a run two passes can reach the same folder (the
# runner's own removal, then the end-of-run sweep), each says so once, so a case
# accepts one or two identical lines and no line that differs.
#
# A last case holds the scenario descriptor beside this script to the set of
# PASS lines this run actually emitted. CI runs this script directly, so without
# it a descriptor left behind costs nothing.
#
# Every command runs under `env -i` with an explicit allowlist, and everything is
# created under one mktemp folder that the EXIT trap removes. The real ~/.claude
# and ~/.amplihack are never read or written: HOME is the scenario's own folder
# throughout.
#
# Run by hand: bash tests/gadugi/run-issue-2785-cleanup-keeps-unsaved-work-scenario.sh
# It builds amplihack with cargo first. Set AMPLIHACK_2785_QA_BIN to an
# amplihack binary to skip the build. It needs git and recipe-runner-rs
# (RECIPE_RUNNER_RS_PATH, or on PATH); without them the scenario fails. On
# success it prints ALL_CASES_PASSED and exits 0; otherwise SCENARIO_FAILED and
# exit 1.
#
# It is bash, not Rust, because every gadugi driver is bash.
# crates/amplihack-cli/tests/issue_1808_run_cleanup/unsaved_work.rs covers the
# same contract, and more of it, from inside the crate.
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd -P)"

umask 022

GIT="$(command -v git 2>/dev/null)"
case "$GIT" in
  /*) ;;
  *) echo "FAIL: git is required"; echo "SCENARIO_FAILED"; exit 1 ;;
esac

WORK="$(mktemp -d "${TMPDIR:-/tmp}/issue-2785-qa.XXXXXX")" || {
  echo "FAIL: cannot create a temporary directory"; echo "SCENARIO_FAILED"; exit 1
}
# Only the folder just made, and only when it is a real, non-empty path.
trap '[ -n "${WORK:-}" ] && rm -rf -- "$WORK"' EXIT
chmod 700 "$WORK"
WORK="$(cd "$WORK" && pwd -P)" || { echo "FAIL: cannot resolve the temporary directory"; echo "SCENARIO_FAILED"; exit 1; }

# --- the binary under test --------------------------------------------------
BIN="${AMPLIHACK_2785_QA_BIN:-}"
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

# --- the scenario's own home, run folder and "remote" -----------------------
HOME_DIR="$WORK/home"
RUN_CWD="$WORK/cwd"
RUN_TMP="$WORK/tmp"
ORIGIN="$WORK/origin.git"
SEED="$WORK/seed"
private_dir() {
  mkdir -p -- "$1" && chmod 700 "$1"
}
for d in "$HOME_DIR" "$HOME_DIR/.amplihack" "$RUN_CWD" "$RUN_TMP"; do
  private_dir "$d" || {
    echo "FAIL: cannot create the scenario folders"; echo "SCENARIO_FAILED"; exit 1
  }
done

CASE_PATH="$(dirname -- "$GIT"):/usr/local/bin:/usr/bin:/bin"

# `git` with no global or system configuration, a fixed identity and no
# signing, so nothing on the host changes what the scenario does.
gitq() {
  env -i \
    HOME="$HOME_DIR" \
    PATH="$CASE_PATH" \
    GIT_CONFIG_NOSYSTEM=1 \
    GIT_CONFIG_GLOBAL=/dev/null \
    GIT_TERMINAL_PROMPT=0 \
    "$GIT" -c user.name=qa -c user.email=qa@example.invalid -c commit.gpgsign=false "$@"
}

# The "remote": a bare repository with one commit on main. A clone of it has
# origin/main as its upstream, so "pushed" is real.
if ! { gitq init -q --bare -b main "$ORIGIN" \
    && gitq clone -q "$ORIGIN" "$SEED" 2>/dev/null \
    && printf 'base\n' > "$SEED/file.txt" \
    && gitq -C "$SEED" add file.txt \
    && gitq -C "$SEED" commit -q -m base \
    && gitq -C "$SEED" push -q -u origin main; }; then
  echo "FAIL: cannot prepare the local remote"
  echo "SCENARIO_FAILED"; exit 1
fi

fail=0
passed=0
# Every case name this run emitted, one per line, kept so the last case can
# check the scenario descriptor against what actually ran. The ` - <detail>`
# some cases append is stripped: the name is the part the descriptor asserts.
CASE_NAMES="$WORK/emitted-case-names"
: > "$CASE_NAMES"
pass() {
  echo "PASS: $1"
  passed=$((passed + 1))
  printf '%s\n' "${1%% - *}" >> "$CASE_NAMES"
}
flunk() {
  echo "FAIL: $1"
  shift
  for f in "$@"; do
    [ -f "$f" ] && tail -n 20 "$f" | sed "s|^|    $(basename -- "$f"): |"
  done
  fail=1
}

# A one-step recipe. The step clones the remote into its own TMPDIR (which is
# the run's temporary folder), writes that TMPDIR to a file the scenario reads,
# and then runs the lines in $3 (already indented, `$TMPDIR` left for the
# step's shell to expand).
write_recipe() {
  local file="$1" case_dir="$2" state="$3"
  {
    printf '%s\n' \
      'name: "issue-2785-qa"' \
      'description: "one bash step that leaves a repository copy in its TMPDIR"' \
      'version: "1.0.0"' \
      'context: {}' \
      'steps:' \
      '  - id: "leave-a-clone"' \
      '    type: "bash"' \
      '    command: |' \
      '      set -eu'
    printf '      printf "%%s\\n" "$TMPDIR" > "%s/step-tmpdir"\n' "$case_dir"
    printf '      git clone -q "%s" "$TMPDIR/repo"\n' "$ORIGIN"
    [ -n "$state" ] && printf '%s\n' "$state"
  } > "$file"
}

# One run, with a work root of its own so nothing is shared between cases.
# stdout, stderr and the exit status are kept per case.
run_case() {
  local name="$1" state="$2" dir="$WORK/case-$1"
  private_dir "$dir" && private_dir "$dir/work" || return 1
  write_recipe "$dir/recipe.yaml" "$dir" "$state"
  env -i \
    HOME="$HOME_DIR" \
    PATH="$CASE_PATH" \
    TMPDIR="$RUN_TMP" \
    RECIPE_RUNNER_RS_PATH="$RUNNER" \
    AMPLIHACK_WORK_ROOT="$dir/work" \
    AMPLIHACK_WORK_ROOT_EXPLICIT=1 \
    AMPLIHACK_NONINTERACTIVE=1 \
    AMPLIHACK_GOVERNOR=off \
    AMPLIHACK_ROTATE_AUTOSTART=0 \
    AMPLIHACK_SKIP_AUTO_INSTALL=1 \
    AMPLIHACK_SKIP_REFLECTION=1 \
    AMPLIHACK_NO_UPDATE_CHECK=1 \
    AMPLIHACK_NO_FRESHNESS_CHECK=1 \
    GIT_CONFIG_NOSYSTEM=1 \
    GIT_CONFIG_GLOBAL=/dev/null \
    "$BIN" recipe run "$dir/recipe.yaml" -w "$RUN_CWD" \
    > "$dir/stdout" 2> "$dir/stderr"
  echo "$?" > "$dir/status"
}

# The number of lines in $1 that are exactly $2, and the number that begin
# `amplihack: leaving `. ENVIRON, not -v, so the text is taken as it is.
count_exact() {
  WANT="$2" awk '$0 == ENVIRON["WANT"] { n++ } END { print n + 0 }' "$1"
}
count_leaving() {
  awk 'index($0, "amplihack: leaving ") == 1 { n++ } END { print n + 0 }' "$1"
}

# What each kept clone must still hold, taking the clone's path.
unpushed_commit_is_intact() {
  [ "$(gitq -C "$1" log -n 1 --format=%s)" = "local only" ]
}
unsaved_file_is_intact() {
  [ "$(cat "$1/unsaved.txt" 2>/dev/null)" = "not committed" ]
}

# --- a clone that is kept: folder, work, status and the one line ------------
check_kept() {
  local name="$1" state="$2" reason="$3" intact="$4"
  local dir="$WORK/case-$name"
  run_case "$name" "$state" || { flunk "$name: cannot create the case folders"; return; }
  local status tmpdir runtime clone
  status="$(cat "$dir/status")"
  tmpdir="$(cat "$dir/step-tmpdir" 2>/dev/null)"
  if [ -z "$tmpdir" ]; then
    flunk "$name: the step did not run (exit $status)" "$dir/stderr"
    return
  fi
  runtime="$(dirname -- "$tmpdir")"
  clone="$tmpdir/repo"
  if [ "$status" != "0" ]; then
    flunk "$name: the run exited $status; keeping a folder must not change that" "$dir/stderr"
    return
  fi
  if [ ! -d "$runtime" ] || [ ! -e "$clone/.git" ]; then
    flunk "$name: the folder with unsaved work was removed ($runtime)" "$dir/stderr"
    return
  fi
  if ! "$intact" "$clone"; then
    flunk "$name: the unsaved work is not intact in $clone" "$dir/stderr"
    return
  fi
  local want exact all
  want="amplihack: leaving \"$runtime\" in place: work tree \"$clone\": $reason"
  exact="$(count_exact "$dir/stderr" "$want")"
  all="$(count_leaving "$dir/stderr")"
  if [ "$exact" -lt 1 ] || [ "$exact" -gt 2 ] || [ "$all" != "$exact" ]; then
    flunk "$name: expected 1 or 2 identical lines '$want', found $exact of them among $all 'leaving' lines" "$dir/stderr"
    return
  fi
  pass "$name - $want"
}

check_kept unpushed_commit_survives \
  "      git -C \"\$TMPDIR/repo\" -c user.name=qa -c user.email=qa@example.invalid -c commit.gpgsign=false commit -q --allow-empty -m \"local only\"" \
  "has unpushed commits" \
  unpushed_commit_is_intact

check_kept uncommitted_file_survives \
  "      printf 'not committed\\n' > \"\$TMPDIR/repo/unsaved.txt\"" \
  "has uncommitted changes" \
  unsaved_file_is_intact

# --- a clean clone is removed as before, with no line -----------------------
name="clean_clone_is_removed"
dir="$WORK/case-$name"
if ! run_case "$name" ""; then
  flunk "$name: cannot create the case folders"
else
  status="$(cat "$dir/status")"
  tmpdir="$(cat "$dir/step-tmpdir" 2>/dev/null)"
  if [ -z "$tmpdir" ]; then
    flunk "$name: the step did not run (exit $status)" "$dir/stderr"
  elif [ "$status" != "0" ]; then
    flunk "$name: the run exited $status" "$dir/stderr"
  elif [ -e "$(dirname -- "$tmpdir")" ]; then
    flunk "$name: a clean, pushed clone kept the folder $(dirname -- "$tmpdir")" "$dir/stderr"
  elif [ "$(count_leaving "$dir/stderr")" != "0" ]; then
    flunk "$name: a folder that was removed was still reported as kept" "$dir/stderr"
  else
    pass "$name"
  fi
fi

# --- the descriptor states this scenario in full ----------------------------
# The YAML beside this script is read as the scenario's specification, but CI
# runs the script directly, so a case added here and not asserted there fails
# nothing and the descriptor quietly understates what runs. Every name this run
# emitted, this one included, must appear in the descriptor.
DESCRIPTOR="$REPO_ROOT/tests/gadugi/scenarios/issue-2785-cleanup-keeps-unsaved-work.yaml"
DESCRIPTOR_CASE="every_case_this_run_emitted_is_asserted_by_the_descriptor"
printf '%s\n' "$DESCRIPTOR_CASE" >> "$CASE_NAMES"
if [ ! -f "$DESCRIPTOR" ]; then
  flunk "the scenario descriptor is missing: $DESCRIPTOR"
else
  sort -u -- "$CASE_NAMES" > "$CASE_NAMES.sorted"
  undescribed=""
  while IFS= read -r case_name; do
    [ -n "$case_name" ] || continue
    if ! grep -qF -- "PASS: $case_name" "$DESCRIPTOR"; then
      undescribed="$undescribed $case_name"
    fi
  done < "$CASE_NAMES.sorted"
  if [ -n "$undescribed" ]; then
    flunk "the descriptor does not assert:$undescribed"
  else
    pass "$DESCRIPTOR_CASE"
  fi
fi

echo "cases passed: $passed"
if [ "$fail" -ne 0 ]; then
  echo "SCENARIO_FAILED"
  exit 1
fi
echo "ALL_CASES_PASSED"
