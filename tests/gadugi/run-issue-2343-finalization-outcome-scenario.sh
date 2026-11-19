#!/usr/bin/env bash
# Self-asserting gadugi-test scenario body for issue #2343: finalization marked
# finished runs as failed.
#
# The #2784 run (18 November) ended FAILED_IMPLEMENTATION at
# validate-agentic-finalization with "WARN: invalid authoritative workflow
# context", while its work was pushed and its pull request was open; that pull
# request merged on green CI the same day. The step had asked the run's own
# bookkeeping whether it had succeeded. The log line before the failure was
# "open PR for the run branch has checks=pending and verification_completed=false".
#
# The fixed steps read the run's real outcome: whether an open same-repo pull
# request sits at the local head, and that request's CI check class.
#
# What runs, unchanged: the real `amplihack` binary, through
# `amplihack recipe run` and the pinned recipe runner, executes the shipped
# collect-finalization-evidence and validate-agentic-finalization bodies (read
# from amplifier-bundle/recipes/workflow-finalize.yaml with the awk extractor,
# the same way run-issue-2784-existing-pr-other-base-scenario.sh reads step-16)
# and every shipped finalization helper they call. The collection body is
# inlined in the generated recipe; the validation body is written to a file and
# run from it, byte for byte, because it carries a heredoc whose terminator
# must stay at column 0. Its result and its exit code are both recorded in the
# case folder, since it exits 1 on a non-success classification, which in
# production ends the run.
#
# Fixtures, per case: a local bare repository whose HEAD is `main`; a clone on
# the run branch, pushed to it; the clone's origin URL set to
# https://github.com/owner/repo.git, so the GitHub identity parses, with a
# repository-local insteadOf rule that sends any Git transport to the bare
# repository. The recipe is run from a folder outside any Git checkout.
#
# Stand-in: `gh`, first on PATH, logs every call, answers `pr view` and
# `pr list` from the case fixture, and records any other call as unexpected and
# exits 96. Nothing reaches GitHub, and no mutating call is answered.
#
# Cases:
#   pending_checks_hand_the_pr_on   branch pushed at the local head, pull
#                                   request #7 open at that head with a check
#                                   still in progress, and no implementation or
#                                   verification booleans at all: the run ends
#                                   EXISTING_OPEN_PR, terminal_success true,
#                                   carrying #7 and a next action naming it
#   diverged_pr_head_fails          the same branch and push, but #7 sits at an
#                                   unrelated commit: the run fails, and the
#                                   reason carries both commit ids and the
#                                   pull request number
#
# Every scenario command runs under `env -i` with an explicit allowlist: no
# token, GH_* setting, SSH_AUTH_SOCK or GIT_ASKPASS reaches it, and
# GIT_ALLOW_PROTOCOL=file stops any Git transport the insteadOf rule did not
# rewrite. Each case has its own HOME. Everything is created under one mktemp
# folder that the EXIT trap removes.
#
# Where it runs: CI's "Install Smoke Test" job (.github/workflows/ci.yml) runs
# this script directly against the binary it installs, after installing the
# pinned recipe runner. tests/gadugi/scenarios/issue-2343-finalization-outcome.yaml
# wraps it for gadugi-test.
#
# Run by hand: bash tests/gadugi/run-issue-2343-finalization-outcome-scenario.sh
# It builds amplihack with cargo first. Set AMPLIHACK_2343_QA_BIN to an
# amplihack binary to skip the build. It needs recipe-runner-rs
# (RECIPE_RUNNER_RS_PATH, or on PATH); without it the scenario fails. On
# success it prints ALL_CASES_PASSED and exits 0; otherwise SCENARIO_FAILED and
# exit 1.
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd -P)"
RECIPE="$REPO_ROOT/amplifier-bundle/recipes/workflow-finalize.yaml"
BRANCH="feat/issue-2343-qa"

for c in git jq; do
  p="$(command -v "$c" 2>/dev/null)"
  case "$p" in
    /*) ;;
    *) echo "FAIL: $c is required"; echo "SCENARIO_FAILED"; exit 1 ;;
  esac
done
[ -f "$RECIPE" ] || { echo "FAIL: missing amplifier-bundle/recipes/workflow-finalize.yaml"; echo "SCENARIO_FAILED"; exit 1; }

WORK="$(mktemp -d "${TMPDIR:-/tmp}/issue-2343-qa.XXXXXX")" || {
  echo "FAIL: cannot create a temporary directory"; echo "SCENARIO_FAILED"; exit 1
}
trap 'rm -rf -- "$WORK"' EXIT
chmod 700 "$WORK"
WORK="$(cd "$WORK" && pwd -P)" || { echo "FAIL: cannot resolve the temporary directory"; echo "SCENARIO_FAILED"; exit 1; }

# --- the binary under test --------------------------------------------------
BIN="${AMPLIHACK_2343_QA_BIN:-}"
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
  if RUNNER="$(command -v recipe-runner-rs)"; then
    :
  else
    status=$?
    if [ "$status" -eq 1 ]; then
      RUNNER=""
    else
      echo "FAIL: recipe-runner-rs lookup failed (exit $status)" >&2
      echo "SCENARIO_FAILED"; exit "$status"
    fi
  fi
fi
if [ -z "$RUNNER" ] || [ ! -x "$RUNNER" ]; then
  echo "FAIL: recipe-runner-rs is required (set RECIPE_RUNNER_RS_PATH or put it on PATH)"
  echo "SCENARIO_FAILED"; exit 1
fi
case "$RUNNER" in /*) ;; *) RUNNER="$PWD/$RUNNER" ;; esac

# --- stub gh ------------------------------------------------------------------
mkdir -p "$WORK/bin"
cat > "$WORK/bin/gh" <<'STUB'
#!/bin/sh
printf '%s\n' "$*" >> "$QA_CASE_DIR/gh.log"
case "${1-} ${2-}" in
  'pr view') cat "$QA_CASE_DIR/view.json" ;;
  'pr list') cat "$QA_CASE_DIR/list.json" ;;
  'auth status') exit 0 ;;
  *)
    printf '%s\n' "$*" >> "$QA_CASE_DIR/unexpected"
    printf 'stub gh: unexpected operation\n' >&2
    exit 96 ;;
esac
STUB
chmod 700 "$WORK/bin/gh"
CASE_PATH="$WORK/bin:$(dirname -- "$(command -v git)"):$(dirname -- "$(command -v jq)"):/usr/local/bin:/usr/bin:/bin"

fail=0
passed=0
pass() { echo "PASS: $1"; passed=$((passed + 1)); }
flunk() {
  echo "FAIL: $1"
  local dir="${2-}"
  if [ -n "$dir" ]; then
    [ -f "$dir/stderr" ] && tail -n 20 "$dir/stderr" | sed 's/^/    stderr: /'
    [ -f "$dir/validate.err" ] && tail -n 20 "$dir/validate.err" | sed 's/^/    validate: /'
    [ -f "$dir/gh.log" ] && sed 's/^/    gh: /' "$dir/gh.log"
  fi
  fail=1
}

if [ "$(env -i PATH="$CASE_PATH" sh -c 'command -v gh')" != "$WORK/bin/gh" ]; then
  flunk "the stub gh is not first on the case PATH"
  echo "SCENARIO_FAILED"; exit 1
fi

# --- the recipe: the two shipped finalization bodies, then a recording step --
COLLECT_BODY="$(bash "$SCRIPT_DIR/recipe-step-command.sh" "$RECIPE" collect-finalization-evidence)" \
  || { echo "FAIL: cannot read collect-finalization-evidence from workflow-finalize.yaml"; echo "SCENARIO_FAILED"; exit 1; }
VALIDATE_BODY="$(bash "$SCRIPT_DIR/recipe-step-command.sh" "$RECIPE" validate-agentic-finalization)" \
  || { echo "FAIL: cannot read validate-agentic-finalization from workflow-finalize.yaml"; echo "SCENARIO_FAILED"; exit 1; }
{
  cat <<'HEAD'
name: "issue-2343-finalization-qa"
description: "the shipped finalization evidence and validation bodies, then a step that records the result (issue 2343 QA)"
version: "1.0.0"
context:
  step_recovery_dir: ""
  base_branch: ""
  finalization_evidence: ""
  workflow_result: ""
steps:
  - id: "collect-finalization-evidence"
    type: "bash"
    parse_json: true
    command: |
HEAD
  printf '%s\n' "$COLLECT_BODY" | sed '/./s/^/      /'
  cat <<'TAIL'
    output: "finalization_evidence"

  # The shipped validation body, byte for byte, run from a file. It carries a
  # heredoc whose terminator must stay at column 0, so it cannot be indented
  # into a nested block; and it exits 1 on a non-success classification, which
  # in production ends the run, so neither its stdout nor a later step can be
  # relied on to carry the result out. Running it as a script keeps its bytes
  # and records both.
  - id: "validate-agentic-finalization"
    type: "bash"
    command: |
      set +e
      bash "$QA_VALIDATE_BODY" > "$QA_RESULT_FILE" 2> "$QA_CASE_DIR/validate.err"
      printf '%s' "$?" > "$QA_CASE_DIR/validate.exit"
      exit 0
TAIL
} > "$WORK/finalize-qa.yaml"
printf '%s\n' "$VALIDATE_BODY" > "$WORK/validate-body.sh"

git_env() {
  local dir="$1"; shift
  env -i HOME="$dir/home" PATH="$CASE_PATH" GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null \
    GIT_TERMINAL_PROMPT=0 GIT_ALLOW_PROTOCOL=file \
    git -c user.name=qa -c user.email=qa@example.invalid -c commit.gpgsign=false \
    -c init.defaultBranch=main "$@" </dev/null
}

qa_git() {
  local dir="$1"
  git_env "$@" >>"$dir/setup.log" 2>&1
}

# new_case <id> <head selector>: a private folder with a bare origin (HEAD
# main), a clone on $BRANCH pushed to it, and the gh fixtures. The selector is
# `local` for a pull request at the clone's head, or `unrelated` for one at a
# commit that is no ancestor or descendant of it. Prints "<dir> <local> <pr>".
new_case() {
  local selector="$2" dir="$WORK/case-$1" sha pr_head
  mkdir -p "$dir/home/.amplihack" "$dir/home/profiles" "$dir/tmp" "$dir/work" "$dir/launch" || return 1
  chmod -R go-rwx "$dir" || return 1
  qa_git "$dir" init -q --bare "$dir/origin.git" || return 1
  qa_git "$dir" -C "$dir/origin.git" symbolic-ref HEAD refs/heads/main || return 1
  qa_git "$dir" init -q "$dir/seed" || return 1
  printf 'issue 2343 QA\n' > "$dir/seed/README.md"
  qa_git "$dir" -C "$dir/seed" add README.md || return 1
  qa_git "$dir" -C "$dir/seed" commit -q -m base || return 1
  qa_git "$dir" -C "$dir/seed" push -q "$dir/origin.git" HEAD:refs/heads/main || return 1
  qa_git "$dir" clone -q "$dir/origin.git" "$dir/clone" || return 1
  qa_git "$dir" -C "$dir/clone" checkout -q -b "$BRANCH" || return 1
  printf 'the run work\n' > "$dir/clone/feature.txt"
  qa_git "$dir" -C "$dir/clone" add feature.txt || return 1
  qa_git "$dir" -C "$dir/clone" commit -q -m "feat: the run work" || return 1
  qa_git "$dir" -C "$dir/clone" push -q -u origin "$BRANCH" || return 1
  qa_git "$dir" -C "$dir/clone" remote set-url origin https://github.com/owner/repo.git || return 1
  qa_git "$dir" -C "$dir/clone" config "url.file://$dir/origin.git.insteadOf" https://github.com/owner/repo.git || return 1
  sha="$(git_env "$dir" -C "$dir/clone" rev-parse HEAD 2>>"$dir/setup.log")" || return 1
  if [ "$selector" = unrelated ]; then
    pr_head="$(git_env "$dir" -C "$dir/clone" commit-tree 'HEAD^{tree}' -m unrelated 2>>"$dir/setup.log")" || return 1
  else
    pr_head="$sha"
  fi
  # A check still in progress: the state the #2784 run was refused for.
  jq -nc --arg head "$BRANCH" --arg oid "$pr_head" \
    '{number: 7, url: "https://github.com/owner/repo/pull/7", state: "OPEN",
      headRefName: $head, headRefOid: $oid, isCrossRepository: false,
      statusCheckRollup: [{__typename: "CheckRun", name: "ci",
                           status: "IN_PROGRESS", conclusion: null}]}' \
    > "$dir/view.json" || return 1
  jq -nc '[{number: 7, url: "https://github.com/owner/repo/pull/7", isCrossRepository: false}]' \
    > "$dir/list.json" || return 1
  if git_env "$dir" -C "$dir/launch" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    echo "the launch folder $dir/launch is inside a Git checkout" >> "$dir/setup.log"
    return 1
  fi
  printf '%s %s %s\n' "$dir" "$sha" "$pr_head"
}

# run_case <case dir>: the generated recipe through amplihack and the pinned
# runner, launched from the case's non-Git folder, under env -i. The run
# carries NO implementation or verification booleans: the question is whether
# finalization reads the pull request rather than this run's own bookkeeping.
run_case() {
  local dir="$1" setup
  setup="$(jq -nc --arg p "$dir/clone" '{worktree_path: $p}')" || return 1
  ( cd "$dir/launch" && exec env -i HOME="$dir/home" PATH="$CASE_PATH" TERM=dumb LANG=C \
      TMPDIR="$dir/tmp" GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null GIT_TERMINAL_PROMPT=0 \
      GIT_ALLOW_PROTOCOL=file AMPLIHACK_HOME="$REPO_ROOT" REMOTE_HOST_TYPE=github ISSUE_NUMBER=2343 \
      AMPLIHACK_SKIP_AUTO_INSTALL=1 AMPLIHACK_NO_FRESHNESS_CHECK=1 AMPLIHACK_NONINTERACTIVE=1 \
      AMPLIHACK_SKIP_POWER_STEERING=1 AMPLIHACK_SKIP_REFLECTION=1 AMPLIHACK_ROTATE_AUTOSTART=0 \
      AMPLIHACK_GOVERNOR=off AMPLIHACK_USAGE_LIMIT_WAIT=0 AMPLIHACK_CLEANUP_ON_EXIT=0 \
      AMPLIHACK_WORK_ROOT="$dir/work" AMPLIHACK_CLAUDE_PROFILE_ROOT="$dir/home/profiles" \
      AMPLIHACK_LAUNCHER_BINARY="$BIN" AMPLIHACK_AGENT_BINARY=claude \
      RECIPE_RUNNER_RS_PATH="$RUNNER" RECIPE_RUNNER_NO_UPDATE_CHECK=1 \
      QA_CASE_DIR="$dir" QA_RESULT_FILE="$dir/workflow-result.json" \
      QA_VALIDATE_BODY="$WORK/validate-body.sh" \
      "$BIN" recipe run "$WORK/finalize-qa.yaml" -w "$dir/launch" --no-detach \
      -c "worktree_setup=$setup" -c "repo_path=$dir/clone" -c "branch_name=$BRANCH" \
      -c "finalizer_step_status={\"status\":\"ok\",\"reporting_failure\":\"false\"}" \
      -c "pr_url=https://github.com/owner/repo/pull/7" ) </dev/null >"$dir/stdout" 2>"$dir/stderr"
}

# field <case dir> <key>: one field of the recorded result, or <missing>.
field() {
  jq -rs --arg k "$2" \
    'if length == 1 and (.[0] | type) == "object" and (.[0] | has($k)) then (.[0][$k] | tostring) else "<missing>" end' \
    "$1/workflow-result.json"
}

expect_field() {
  local got
  if got="$(field "$1" "$2")"; then
    [ "$got" = "$3" ] || problems+=("$2 '$got', want '$3'")
  else
    problems+=("cannot read $2 from the workflow result (jq exit $?)")
  fi
}

no_mutation() {
  [ ! -e "$1/unexpected" ] || problems+=("unexpected gh call: $(head -n 1 "$1/unexpected")")
}

# expect_exit <case dir> <want>: the exit code the shipped body chose.
expect_exit() {
  local got
  got="$(cat "$1/validate.exit" 2>/dev/null)" || got="<missing>"
  [ "$got" = "$2" ] || problems+=("validate exit '$got', want '$2'")
}

# --- case A: pull request open at the pushed head, checks still pending ------
if read -r dir _local_sha _pr_sha <<<"$(new_case pending local)"; then
  run_case "$dir"
  rc=$?
  problems=()
  [ "$rc" -eq 0 ] || problems+=("recipe run exit code $rc")
  expect_field "$dir" terminal_state EXISTING_OPEN_PR
  expect_field "$dir" terminal_success true
  expect_field "$dir" terminal_failure false
  expect_field "$dir" pr_number 7
  expect_field "$dir" pr_url https://github.com/owner/repo/pull/7
  expect_exit "$dir" 0
  next="$(field "$dir" required_next_action)"
  case "$next" in *7*) ;; *) problems+=("required_next_action does not name the pull request: $next") ;; esac
  grep -q '^pr view 7 ' "$dir/gh.log" 2>/dev/null || problems+=("no pr view 7 call")
  no_mutation "$dir"
  if [ "${#problems[@]}" -eq 0 ]; then
    pass "pending_checks_hand_the_pr_on - EXISTING_OPEN_PR carrying PR #7 with no completion booleans"
  else
    flunk "pending_checks_hand_the_pr_on: ${problems[*]}" "$dir"
  fi
else
  flunk "pending_checks_hand_the_pr_on: cannot set up the case" "$WORK/case-pending"
  [ -f "$WORK/case-pending/setup.log" ] && tail -n 10 "$WORK/case-pending/setup.log" | sed 's/^/    setup: /'
fi

# --- case B: the pull request sits at an unrelated commit --------------------
if read -r dir local_sha pr_sha <<<"$(new_case diverged unrelated)"; then
  run_case "$dir"
  rc=$?
  problems=()
  [ "$rc" -eq 0 ] || problems+=("recipe run exit code $rc")
  expect_field "$dir" terminal_success false
  expect_field "$dir" terminal_failure true
  expect_exit "$dir" 1
  state="$(field "$dir" terminal_state)"
  case "$state" in EXISTING_OPEN_PR|IMPLEMENTED_VERIFIED|ALLOW_NO_OP) problems+=("a diverged pull request head reported success state $state") ;; esac
  reason="$(field "$dir" terminal_reason)"
  for fact in "$local_sha" "$pr_sha" "7"; do
    case "$reason" in *"$fact"*) ;; *) problems+=("terminal_reason does not carry '$fact': $reason") ;; esac
  done
  no_mutation "$dir"
  if [ "${#problems[@]}" -eq 0 ]; then
    pass "diverged_pr_head_fails - the reason carries both commit ids and the pull request number"
  else
    flunk "diverged_pr_head_fails: ${problems[*]}" "$dir"
  fi
else
  flunk "diverged_pr_head_fails: cannot set up the case" "$WORK/case-diverged"
  [ -f "$WORK/case-diverged/setup.log" ] && tail -n 10 "$WORK/case-diverged/setup.log" | sed 's/^/    setup: /'
fi

echo "cases passed: $passed"
if [ "$fail" -eq 0 ] && [ "$passed" -eq 2 ]; then
  echo "issue 2343 finalization outcome contract green"
  echo "ALL_CASES_PASSED"
  exit 0
fi
echo "SCENARIO_FAILED"
exit 1
