#!/usr/bin/env bash
# Self-asserting gadugi-test scenario body for issue #2785: a recipe run that
# commits in its runtime folder and then stops on a usage limit keeps that
# commit on disk, so a continue run can pick it up.
#
# On 1.2.177 three runs stopped on a usage limit and their runtime folders
# under <work root>/r/ were removed at the end of the run, with whatever git
# work was in them. This runs the real `amplihack` binary from the outside,
# through `amplihack recipe run` and the pinned recipe runner, with no stub
# runner. The recipe has two bash steps:
#   step-01-commit       clones a local bare origin quietly into
#                        $TMPDIR/work, which is inside r/<id>, commits a file
#                        and prints only the commit's SHA. The runner ranks a
#                        work or environment marker in a failed step's output
#                        (such as "permission denied" or "no such file or
#                        directory") above a usage limit, so this step's
#                        output holds neither.
#   step-02-usage-limit  prints a usage-limit message, as the agent CLI does,
#                        and fails.
# The run is `amplihack recipe run <recipe> -w <project> --no-detach`, with
# usage-limit waiting off (AMPLIHACK_USAGE_LIMIT_WAIT=0) and the end-of-run
# sweep on (its default). The driver checks that the runner classified the
# stop `usage_limit`, which shows the run took the path a real usage-limit
# stop takes.
#
# Cases:
#   unpushed  the commit is only in the clone. After the stop: run.json has
#             ended_at, the clone and the commit are on disk, the runner
#             printed `runtime folder "<r/id>" is kept: git tree
#             "<r/id>/tmp/work" has unpushed commits`, and
#             `amplihack cleanup --apply --format json --only run-tmp` skips
#             the folder with that reason and leaves the commit in place.
#   pushed    the step also pushes the commit. After the stop the runtime
#             folder is gone and the commit is in the origin.
#
# Every command runs under `env -i` with an explicit allowlist (see run_env):
# a temporary HOME, TMPDIR and work root, a git configuration file of the
# case's own (GIT_CONFIG_GLOBAL) and no system configuration. The origin is
# a local bare repository; nothing reaches the network and no secret is
# referenced. Stub `claude`, `codex` and `copilot` are first on PATH, so no
# agent CLI is ever launched; the recipe has no agent step. Everything is created under one mktemp folder
# that the EXIT trap gives back owner access to and removes. The driver
# prints only SHAs, paths and PASS: or FAIL: lines; on a failure it also
# prints the end of that run's stderr.
#
# Where it runs: CI's "Install Smoke Test" job (.github/workflows/ci.yml)
# runs this script directly against the amplihack binary that job installs,
# with AMPLIHACK_2785_QA_REQUIRE_ALL=1. CI does not run gadugi-test itself;
# tests/gadugi/scenarios/issue-2785-usage-limit-keeps-work.yaml wraps this
# script for anyone running it through gadugi-test by hand.
#
# Run by hand: bash tests/gadugi/run-issue-2785-usage-limit-keeps-work-scenario.sh
# It builds amplihack with cargo first. Set AMPLIHACK_2785_QA_BIN to an
# amplihack binary to skip the build. On success it prints ALL_CASES_PASSED
# and exits 0; otherwise SCENARIO_FAILED and exit 1.
#
# The cases need recipe-runner-rs (RECIPE_RUNNER_RS_PATH, or on PATH) and jq.
# Without them they print SKIP; with AMPLIHACK_2785_QA_REQUIRE_ALL=1 a SKIP
# fails the scenario.
#
# It is bash, not Rust, because every gadugi driver is bash and because it
# drives the installed binary exactly as an operator does.
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd -P)"

GIT="$(command -v git 2>/dev/null || true)"
case "$GIT" in
  /*) ;;
  *) echo "FAIL: git is required"; echo "SCENARIO_FAILED"; exit 1 ;;
esac
JQ="$(command -v jq 2>/dev/null || true)"

WORK="$(mktemp -d "${TMPDIR:-/tmp}/issue-2785-qa.XXXXXX")" || {
  echo "FAIL: cannot create a temporary directory"; echo "SCENARIO_FAILED"; exit 1
}
WORK="$(cd "$WORK" && pwd -P)" || {
  echo "FAIL: cannot resolve the temporary directory"; echo "SCENARIO_FAILED"; exit 1
}
chmod 700 "$WORK"
trap 'chmod -R -- u+rwx "${WORK:?}" 2>/dev/null; rm -rf -- "${WORK:?}"' EXIT

# --- the binary under test --------------------------------------------------
BIN="${AMPLIHACK_2785_QA_BIN:-}"
if [ -z "$BIN" ]; then
  # As run-issue-1619-tier-scenario.sh does: one awk reads every artifact
  # record and prints the last executable path (issue #1434). The build runs
  # in the caller's environment, before any scenario command.
  if ! BIN="$(cd "$REPO_ROOT" && cargo build -p amplihack --bin amplihack --locked \
      --message-format=json-render-diagnostics 2>"$WORK/build.log" \
      | awk 'match($0, /"executable":"[^"]*\/amplihack"/) { bin = substr($0, RSTART + 14, RLENGTH - 15) } END { print bin }')"; then
    echo "FAIL: cargo build -p amplihack --bin amplihack failed"
    tail -40 "$WORK/build.log"
    echo "SCENARIO_FAILED"; exit 1
  fi
fi
if [ -z "$BIN" ] || [ ! -x "$BIN" ]; then
  echo "FAIL: no executable amplihack binary (got '${BIN}')"
  echo "SCENARIO_FAILED"; exit 1
fi
case "$BIN" in
  /*) ;;
  *) BIN="$PWD/$BIN" ;;
esac
echo "binary under test: $BIN"

RUNNER="${RECIPE_RUNNER_RS_PATH:-$(command -v recipe-runner-rs 2>/dev/null || true)}"
SYS_PATH="$(dirname -- "$GIT"):/usr/local/bin:/usr/bin:/bin"

# --- stub agent CLIs --------------------------------------------------------
# The recipe has no agent step; the stubs only stop a lookup from finding a
# real agent CLI. They answer --version and do nothing else.
mkdir -p "$WORK/bin"
cat > "$WORK/stub" <<'STUB'
#!/bin/sh
if [ "${1-}" = "--version" ]; then
  printf '%s\n' '2.1.319 (Claude Code)'
fi
exit 0
STUB
for tool in claude codex copilot; do
  cp "$WORK/stub" "$WORK/bin/$tool"
  chmod 700 "$WORK/bin/$tool"
done

fail=0
passed=0
pass() { echo "PASS: $1"; passed=$((passed + 1)); }
# flunk <message> [case dir]: the failure and the end of that run's stderr.
flunk() {
  echo "FAIL: $1"
  if [ -n "${2-}" ] && [ -f "$2/run.stderr" ]; then
    tail -15 "$2/run.stderr" | sed 's/^/    stderr: /'
  fi
  fail=1
}
# skip <cases> <why>: a SKIP, or a failure under AMPLIHACK_2785_QA_REQUIRE_ALL=1.
skip() {
  if [ "${AMPLIHACK_2785_QA_REQUIRE_ALL:-0}" = 1 ]; then
    flunk "$1 could not run ($2) and AMPLIHACK_2785_QA_REQUIRE_ALL=1"
  else
    echo "SKIP: $1 ($2)"
  fi
}

# qa_git <case dir> <git args...>: git with the case's own configuration only.
qa_git() {
  local dir="$1"; shift
  env -i HOME="$dir/home" PATH="$SYS_PATH" LANG=C \
    GIT_CONFIG_GLOBAL="$dir/gitconfig" GIT_CONFIG_NOSYSTEM=1 GIT_TERMINAL_PROMPT=0 \
    "$GIT" "$@" </dev/null
}

# run_env <case dir> <command...>: the command with only these variables.
#   PATH                       the stub agent CLIs, then git and the system tools
#   HOME, TMPDIR               the case's own folders
#   LANG, TERM                 fixed values
#   GIT_CONFIG_GLOBAL/NOSYSTEM the case's git configuration (identity only)
#   GIT_TERMINAL_PROMPT        git never waits on a credential prompt
#   AMPLIHACK_WORK_ROOT        the case's work root; long enough that the
#                              runtime folder is r/<id>
#   AMPLIHACK_USAGE_LIMIT_WAIT 0: the run stops on the usage limit at once
#   the AMPLIHACK_* switches   no install, update check, rotation, governor,
#                              power steering or reflection
run_env() {
  local dir="$1"; shift
  env -i \
    PATH="$WORK/bin:$SYS_PATH" \
    HOME="$dir/home" \
    TMPDIR="$dir/tmp" \
    LANG=C \
    TERM=dumb \
    GIT_CONFIG_GLOBAL="$dir/gitconfig" \
    GIT_CONFIG_NOSYSTEM=1 \
    GIT_TERMINAL_PROMPT=0 \
    AMPLIHACK_WORK_ROOT="$dir/work" \
    AMPLIHACK_CLAUDE_PROFILE_ROOT="$dir/home/profiles" \
    AMPLIHACK_SKIP_AUTO_INSTALL=1 \
    AMPLIHACK_NO_FRESHNESS_CHECK=1 \
    AMPLIHACK_NONINTERACTIVE=1 \
    AMPLIHACK_SKIP_POWER_STEERING=1 \
    AMPLIHACK_SKIP_REFLECTION=1 \
    AMPLIHACK_ROTATE_AUTOSTART=0 \
    AMPLIHACK_GOVERNOR=off \
    AMPLIHACK_USAGE_LIMIT_WAIT=0 \
    AMPLIHACK_LAUNCHER_BINARY="$BIN" \
    AMPLIHACK_AGENT_BINARY=claude \
    AMPLIHACK_CLAUDE_BINARY_PATH="$WORK/bin/claude" \
    CODEX_HOME="$dir/home/.codex" \
    RECIPE_RUNNER_RS_PATH="$RUNNER" \
    "$@"
}

# new_case <id> <push: 0 or 1>: a private case folder with a home, a git
# configuration, a project (the caller's checkout), a bare origin cloned from
# it, and the two-step recipe. Prints the folder.
new_case() {
  local id="$1" push="$2" dir="$WORK/case-$1"
  mkdir -p "$dir/home/profiles" "$dir/home/.codex" "$dir/project" "$dir/work" "$dir/tmp" || return 1
  chmod -R go-rwx "$dir" || return 1
  printf '[user]\n\tname = qa\n\temail = qa@example.invalid\n[commit]\n\tgpgsign = false\n' \
    > "$dir/gitconfig" || return 1
  qa_git "$dir" -C "$dir/project" init -q -b main >/dev/null 2>&1 || return 1
  printf 'issue 2785 QA\n' > "$dir/project/README.md"
  qa_git "$dir" -C "$dir/project" add README.md >/dev/null 2>&1 || return 1
  qa_git "$dir" -C "$dir/project" commit -q -m init >/dev/null 2>&1 || return 1
  qa_git "$dir" clone -q --bare "$dir/project" "$dir/origin.git" >/dev/null 2>&1 || return 1

  local push_line=":"
  if [ "$push" = 1 ]; then
    push_line="git -C \"\$work\" push -q origin HEAD >/dev/null 2>&1 || exit 3"
  fi
  # Paths come from mktemp and hold no quote. Every git command is quiet, so
  # the step's output is the SHA and nothing else.
  cat > "$dir/recipe.yaml" <<RECIPE || return 1
name: "issue-2785-usage-limit-qa"
description: "commits in the runtime folder, then stops on a usage limit (issue 2785 QA)"
version: "1.0.0"
context: {}
steps:
  - id: "step-01-commit"
    type: "bash"
    command: |
      work="\${TMPDIR:?}/work"
      if [ ! -d "\$work/.git" ]; then
        git clone -q '$dir/origin.git' "\$work" >/dev/null 2>&1 || exit 3
        printf 'made before the stop\n' > "\$work/step-07-tests.txt" || exit 3
        git -C "\$work" add step-07-tests.txt >/dev/null 2>&1 || exit 3
        git -C "\$work" commit -q -m 'work made before the stop' >/dev/null 2>&1 || exit 3
        $push_line
      fi
      sha="\$(git -C "\$work" rev-parse HEAD 2>/dev/null)" || exit 3
      printf '%s\n' "\$TMPDIR" > '$dir/step-tmpdir' || exit 3
      printf '%s\n' "\$sha" > '$dir/step-sha' || exit 3
      printf '%s\n' "\$sha"
  - id: "step-02-usage-limit"
    type: "bash"
    command: |
      printf '%s\n' 'Claude usage limit reached. Your limit will reset at 9pm (UTC).'
      printf '%s\n' 'Claude usage limit reached. Your limit will reset at 9pm (UTC).' >&2
      exit 1
RECIPE
  printf '%s\n' "$dir"
}

# run_case <dir>: the recipe through amplihack and the pinned runner, from
# the case's project. Returns the run's exit status.
run_case() {
  local dir="$1"
  ( cd "$dir/project" && run_env "$dir" \
      "$BIN" recipe run "$dir/recipe.yaml" -w "$dir/project" --no-detach ) \
    </dev/null >"$dir/run.stdout" 2>"$dir/run.stderr"
}

# The runtime folder the step ran in: the parent of the TMPDIR it recorded.
runtime_of() {
  local tmpdir
  tmpdir="$(cat "$1/step-tmpdir" 2>/dev/null)" || return 1
  [ -n "$tmpdir" ] || return 1
  dirname -- "$tmpdir"
}

# Checks shared by both cases: the run failed, was classified usage_limit and
# stopped, ended, and ran its step in r/<id>. Sets RT and SHA.
common_checks() {
  local id="$1" dir="$2" rc="$3"
  RT="" SHA=""
  if [ "$rc" -eq 0 ]; then
    flunk "$id: the run exited 0; the usage-limit step must fail it" "$dir"
    return 1
  fi
  if grep '^amplihack\.recipe\.failure_class ' "$dir/run.stderr" \
      | grep -F '"class":"usage_limit"' | grep -qF '"action":"terminal'; then
    pass "$id: classified usage_limit and stopped"
  else
    flunk "$id: no terminal usage_limit classification" "$dir"
    return 1
  fi
  local runs=("$dir"/work/runs/*/run.json)
  if [ "${#runs[@]}" -ne 1 ] || [ ! -f "${runs[0]}" ]; then
    flunk "$id: want one run folder, found ${#runs[@]}" "$dir"
    return 1
  fi
  if "$JQ" -e '(.ended_at | type) == "string" and (.ended_at | length) > 0' "${runs[0]}" >/dev/null; then
    pass "$id: run.json records ended_at"
  else
    flunk "$id: run.json has no ended_at" "$dir"
  fi
  if ! RT="$(runtime_of "$dir")"; then
    flunk "$id: step-01-commit did not record its TMPDIR" "$dir"
    return 1
  fi
  SHA="$(cat "$dir/step-sha" 2>/dev/null)"
  if ! printf '%s' "$SHA" | grep -Eq '^[0-9a-f]{40,64}$'; then
    flunk "$id: step-01-commit did not record a commit SHA" "$dir"
    return 1
  fi
  echo "    commit made before the stop: $SHA"
  case "$RT" in
    "$dir/work/r/"*) pass "$id: the step ran in the short runtime folder $RT" ;;
    *) flunk "$id: runtime folder $RT is not under $dir/work/r/" "$dir"; return 1 ;;
  esac
  return 0
}

unpushed_case() {
  local id=unpushed dir rc
  if ! dir="$(new_case "$id" 0)"; then
    flunk "$id: cannot set up the case"
    return
  fi
  run_case "$dir"; rc=$?
  common_checks "$id" "$dir" "$rc" || return
  local tree="$RT/tmp/work"
  local reason="git tree \"$tree\" has unpushed commits"

  if qa_git "$dir" -C "$tree" cat-file -e "$SHA" 2>/dev/null; then
    pass "$id: after the stop the commit is on disk in $tree"
  else
    flunk "$id: after the stop the commit is gone from $tree" "$dir"
  fi
  if qa_git "$dir" -C "$dir/origin.git" cat-file -e "$SHA" 2>/dev/null; then
    flunk "$id: the commit reached the origin; the case needs it unpushed" "$dir"
  fi
  if grep -qF "amplihack: runtime folder \"$RT\" is kept: $reason" "$dir/run.stderr"; then
    pass "$id: the runner said the runtime folder is kept, and why"
  else
    flunk "$id: no 'runtime folder \"$RT\" is kept: $reason' line" "$dir"
  fi

  run_env "$dir" "$BIN" cleanup --apply --format json --only run-tmp \
    </dev/null >"$dir/cleanup.json" 2>"$dir/cleanup.stderr"
  local action got
  # $p is a jq variable, bound by --arg.
  # shellcheck disable=SC2016
  action="$("$JQ" -r --arg p "$RT" '[.entries[] | select(.path == $p)][0].action // "none"' "$dir/cleanup.json" 2>/dev/null)"
  # shellcheck disable=SC2016
  got="$("$JQ" -r --arg p "$RT" '[.entries[] | select(.path == $p)][0].reason // ""' "$dir/cleanup.json" 2>/dev/null)"
  if [ "$action" = skipped ] && [ "$got" = "$reason" ]; then
    pass "$id: cleanup --apply skips $RT: $reason"
  else
    flunk "$id: cleanup --apply gave '$action' for $RT, reason '$got'" "$dir"
  fi
  if qa_git "$dir" -C "$tree" cat-file -e "$SHA" 2>/dev/null; then
    pass "$id: after cleanup the commit is still on disk"
  else
    flunk "$id: after cleanup the commit is gone" "$dir"
  fi
}

pushed_case() {
  local id=pushed dir rc
  if ! dir="$(new_case "$id" 1)"; then
    flunk "$id: cannot set up the case"
    return
  fi
  run_case "$dir"; rc=$?
  common_checks "$id" "$dir" "$rc" || return

  if qa_git "$dir" -C "$dir/origin.git" cat-file -e "$SHA" 2>/dev/null; then
    pass "$id: the commit is in the origin"
  else
    flunk "$id: the commit is not in the origin" "$dir"
  fi
  if [ -e "$RT" ]; then
    flunk "$id: runtime folder $RT is still there, with nothing unpushed in it" "$dir"
  else
    pass "$id: the runtime folder is removed once its work is on the remote"
  fi
  if grep -qF "amplihack: runtime folder \"$RT\" is kept" "$dir/run.stderr"; then
    flunk "$id: the runner kept a runtime folder whose work is pushed" "$dir"
  fi
}

if [ -z "$RUNNER" ] || [ ! -x "$RUNNER" ]; then
  skip "unpushed, pushed" "no recipe-runner-rs"
elif [ -z "$JQ" ]; then
  skip "unpushed, pushed" "no jq"
else
  unpushed_case
  pushed_case
fi

echo "checks passed: $passed"
if [ "$fail" -eq 0 ]; then
  echo "issue 2785 usage-limit stop keeps unpushed work"
  echo "ALL_CASES_PASSED"
  exit 0
fi
echo "SCENARIO_FAILED"
exit 1
