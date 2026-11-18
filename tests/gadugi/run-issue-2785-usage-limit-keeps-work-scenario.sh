#!/usr/bin/env bash
# CI-only qa-team regression for issue #2785. Uses the installed CLI and
# pinned real runner under env -i, with temporary HOME and work roots.
# A recipe commits inside runtime storage, stops through production usage-limit
# finalization, and relaunches with the same identity. Recovery must restore
# its repository path and SHA; an executed continuation reads that repository.
# A clean, pushed counterpart must remove its runtime folder.
#
# Run only in CI, never on a development host. Missing prerequisites fail.
# The Install Smoke Test job runs this driver; gadugi's scenario wraps it.
# Bash is appropriate here because this existing harness drives installed
# commands as an operator would; the implementation and component tests use Rust.
# Historical deletion causation remains unverified. Passing this scenario
# demonstrates the interruption and continuation behavior of the tested build.
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
# it, and the recoverable recipe. Prints the folder.
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
  qa_git "$dir" -C "$dir/project" remote add origin "$dir/origin.git" >/dev/null 2>&1 || return 1
  qa_git "$dir" -C "$dir/project" push -qu origin main >/dev/null 2>&1 || return 1

  local push_line=":"
  if [ "$push" = 1 ]; then
    push_line="git -C \"\$work\" push -q origin HEAD >/dev/null 2>&1 || exit 3"
  fi
  # Paths come from mktemp and hold no quote. Every git command is quiet, so
  # step outputs persist the repository path and commit as JSON strings.
  cat > "$dir/recipe.yaml" <<RECIPE || return 1
name: "issue-2785-usage-limit-qa"
description: "retains runtime work through usage-limit interruption and continuation"
version: "1.0.0"
context: {}
recovery:
  version: 1
  persist:
    context: [issue_number, issue_url, repo_path, task_description, input, repository_path, saved_commit]
    outputs: [step-01-commit, step-02-read-commit]
steps:
  - id: "step-01-commit"
    type: "bash"
    output: repository_path
    parse_json: true
    parse_json_required: true
    recovery:
      replay: reuse
      resources:
        - kind: file
          path: '$dir/step-tmpdir'
        - kind: file
          path: '$dir/step-sha'
    command: |
      printf 'commit\n' >> '$dir/commit-executions' || exit 3
      work="\${TMPDIR:?}/work"
      git clone -q '$dir/origin.git' "\$work" >/dev/null 2>&1 || exit 3
      printf 'made before the stop\n' > "\$work/step-07-tests.txt" || exit 3
      git -C "\$work" add step-07-tests.txt >/dev/null 2>&1 || exit 3
      git -C "\$work" commit -q -m 'work made before the stop' >/dev/null 2>&1 || exit 3
      $push_line
      sha="\$(git -C "\$work" rev-parse HEAD 2>/dev/null)" || exit 3
      printf '%s\n' "\$TMPDIR" > '$dir/step-tmpdir' || exit 3
      printf '%s\n' "\$sha" > '$dir/step-sha' || exit 3
      printf '"%s"\n' "\$work"
  - id: "step-02-read-commit"
    type: "bash"
    output: saved_commit
    parse_json: true
    parse_json_required: true
    recovery:
      replay: reuse
      resources:
        - kind: repository
          path: "{{repository_path}}"
    command: |
      sha="\$(git -C '{{repository_path}}' rev-parse HEAD 2>/dev/null)" || exit 3
      printf '"%s"\n' "\$sha"
  - id: "step-03-usage-limit-or-continue"
    type: "bash"
    recovery:
      replay: never
    command: |
      if [ ! -f '$dir/allow-continuation' ]; then
        : > '$dir/allow-continuation' || exit 3
        printf '%s\n' 'Claude usage limit reached. Your limit will reset at 9pm (UTC).'
        printf '%s\n' 'Claude usage limit reached. Your limit will reset at 9pm (UTC).' >&2
        exit 1
      fi
      work='{{repository_path}}'
      sha='{{saved_commit}}'
      actual="\$(git -C "\$work" rev-parse HEAD 2>/dev/null)" || exit 3
      [ "\$actual" = "\$sha" ] || exit 3
      git -C "\$work" cat-file -e "\$sha^{commit}" 2>/dev/null || exit 3
      content="\$(git -C "\$work" show "\$sha:step-07-tests.txt" 2>/dev/null)" || exit 3
      [ "\$content" = 'made before the stop' ] || exit 3
      [ "\$(cat "\$work/step-07-tests.txt")" = 'made before the stop' ] || exit 3
      printf '%s\n' "\$sha" > '$dir/continuation-sha' || exit 3
      printf '%s\n' "\$work" > '$dir/continuation-tree' || exit 3
      printf '%s\n' 'continued from the retained repository'
RECIPE
  printf '%s\n' "$dir"
}

# run_case <dir>: the recipe through amplihack and the pinned runner, from
# the case's project. Returns the run's exit status.
run_case() {
  local dir="$1"
  ( cd "$dir/project" && run_env "$dir" \
      "$BIN" recipe run "$dir/recipe.yaml" -w "$dir/project" --no-detach \
      --format json -c issue_number=2785 -c "repo_path=$dir/project" ) \
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

  # Relaunch with the same work identity and recipe. Production finalization
  # and recovery supply the lifecycle; no manual cleanup substitutes for it.
  cp "$dir/run.stdout" "$dir/interrupted.json"
  cp "$dir/run.stderr" "$dir/interrupted.stderr"
  local source_run
  source_run="$("$JQ" -r '.run_id' "$dir/interrupted.json")"
  run_case "$dir"; rc=$?
  if [ "$rc" -ne 0 ]; then
    flunk "$id: continuation exited $rc" "$dir"
    return
  fi
  # shellcheck disable=SC2016
  if "$JQ" -e --arg source "$source_run" '
      .success == true and .run_id != $source and
      .recovery.mode == "resumed" and .recovery.reason == "usage_limit" and
      .recovery.source_run_id == $source and
      (.recovery.source_sequence | type) == "number" and
      any(.recovery.restored_steps[]; .step_id == "step-01-commit") and
      any(.recovery.restored_steps[]; .step_id == "step-02-read-commit") and
      .recovery.restart.step_id == "step-03-usage-limit-or-continue" and
      any(.step_results[]; .step_id == "step-03-usage-limit-or-continue" and .status == "completed")
    ' "$dir/run.stdout" >/dev/null; then
    pass "$id: continuation recovered the immediate usage-limit predecessor"
  else
    flunk "$id: missing predecessor, restored outputs or executed continuation evidence" "$dir"
  fi
  if [ "$(cat "$dir/commit-executions")" = commit ]; then
    pass "$id: continuation reused the completed commit step"
  else
    flunk "$id: completed commit step executed again" "$dir"
  fi
  if [ "$(cat "$dir/continuation-sha" 2>/dev/null)" = "$SHA" ] &&
      [ "$(cat "$dir/continuation-tree" 2>/dev/null)" = "$tree" ] &&
      qa_git "$dir" -C "$tree" cat-file -e "$SHA" 2>/dev/null; then
    pass "$id: executed continuation read the original commit and retained contents"
  else
    flunk "$id: continuation did not read the original retained repository" "$dir"
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
  flunk "recipe-runner-rs is required"
elif [ -z "$JQ" ]; then
  flunk "jq is required"
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
