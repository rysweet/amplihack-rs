#!/usr/bin/env bash
# Self-asserting gadugi-test scenario body for issue #2784: publish opened a
# second pull request into the default branch when the branch already had an
# open pull request into another base.
#
# With base_branch empty, step-16-create-draft-pr of
# amplifier-bundle/recipes/workflow-publish.yaml takes the base from
# origin/HEAD. Its duplicate lookup was scoped to that base, so an open pull
# request from the same branch into `dev` was missed and `gh pr create` opened
# a second one into `main`. The fixed step adopts the open pull request and
# records its base.
#
# What runs, unchanged: the real `amplihack` binary, through
# `amplihack recipe run` and the pinned recipe runner, executes the shipped
# step-16 body (read from workflow-publish.yaml with the awk extractor, the
# same way run-step-03b.sh reads its step) and the shipped publish and scope
# helpers it calls. A second generated step writes the parsed publish result to
# a file in the case folder.
#
# Fixtures, per case: a local bare repository whose HEAD is `main`, with
# branches `main` and `dev`; a clone on the feature branch, pushed to it; the
# clone's origin URL set to https://github.com/owner/repo.git, so the GitHub
# identity parses, with a repository-local insteadOf rule that sends any Git
# transport to the bare repository. The recipe is run from a folder outside
# any Git checkout, so the steps are not moved into a private copy and
# step-16 reaches the clone through worktree_setup.
#
# Stand-in: `gh`, first on PATH, logs every call, answers `pr list --head` from
# the case fixture, other `pr list` calls with [], `issue view` with a fixed
# title and `repo view` (the claim check's default-branch lookup) with `main`;
# `pr create` is recorded and answered with a URL; any other call is recorded
# as unexpected and exits 96. Nothing reaches GitHub.
#
# Cases:
#   existing_pr_on_other_base_adopted   open PR #615 into dev: the run records
#                                       it, base dev, and makes no pr create
#   no_pr_creates_one_without_base      no PR from the branch: one pr create,
#                                       without --base (the control case)
#
# Every scenario command runs under `env -i` with an explicit allowlist: no
# token, GH_* setting, SSH_AUTH_SOCK or GIT_ASKPASS reaches it, and
# GIT_ALLOW_PROTOCOL=file stops any Git transport the insteadOf rule did not
# rewrite. Each case has its own HOME. Everything is created under one mktemp
# folder that the EXIT trap removes.
#
# Where it runs: CI's "Install Smoke Test" job (.github/workflows/ci.yml) runs
# this script directly against the binary it installs, after installing the
# pinned recipe runner. tests/gadugi/scenarios/
# issue-2784-existing-pr-on-other-base.yaml wraps it for gadugi-test.
#
# Run by hand: bash tests/gadugi/run-issue-2784-existing-pr-other-base-scenario.sh
# It builds amplihack with cargo first. Set AMPLIHACK_2784_QA_BIN to an
# amplihack binary to skip the build. It needs recipe-runner-rs
# (RECIPE_RUNNER_RS_PATH, or on PATH); without it the scenario fails. On
# success it prints ALL_CASES_PASSED and exits 0; otherwise SCENARIO_FAILED and
# exit 1.
#
# It is bash, not Rust, because every gadugi driver is bash and because it
# runs the shipped shell step as the recipe does.
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd -P)"
RECIPE="$REPO_ROOT/amplifier-bundle/recipes/workflow-publish.yaml"
BRANCH="feat/issue-2784-qa"
KEPT_DEV="existing open PR #615 targets dev; the resolved base was main; kept dev"

for c in git jq; do
  p="$(command -v "$c" 2>/dev/null)"
  case "$p" in
    /*) ;;
    *) echo "FAIL: $c is required"; echo "SCENARIO_FAILED"; exit 1 ;;
  esac
done
[ -f "$RECIPE" ] || { echo "FAIL: missing amplifier-bundle/recipes/workflow-publish.yaml"; echo "SCENARIO_FAILED"; exit 1; }

WORK="$(mktemp -d "${TMPDIR:-/tmp}/issue-2784-qa.XXXXXX")" || {
  echo "FAIL: cannot create a temporary directory"; echo "SCENARIO_FAILED"; exit 1
}
trap 'rm -rf -- "$WORK"' EXIT
chmod 700 "$WORK"
# worktree_setup_path.sh accepts only a path equal to its own realpath.
WORK="$(cd "$WORK" && pwd -P)" || { echo "FAIL: cannot resolve the temporary directory"; echo "SCENARIO_FAILED"; exit 1; }

# --- the binary under test --------------------------------------------------
BIN="${AMPLIHACK_2784_QA_BIN:-}"
if [ -z "$BIN" ]; then
  # Same build idiom as run-issue-1527-model-scenario.sh: one awk reads every
  # artifact record and prints the last executable path (issue #1434).
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
case "$BIN" in
  /*) ;;
  *) BIN="$PWD/$BIN" ;;
esac
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
case "$RUNNER" in
  /*) ;;
  *) RUNNER="$PWD/$RUNNER" ;;
esac

# --- stub gh ------------------------------------------------------------------
mkdir -p "$WORK/bin"
cat > "$WORK/bin/gh" <<'STUB'
#!/bin/sh
printf '%s\n' "$*" >> "$QA_CASE_DIR/gh.log"
case "${1-} ${2-}" in
  'pr list')
    case " $* " in
      *' --head '*) cat "$QA_CASE_DIR/prs.json" ;;
      *) printf '[]\n' ;;
    esac ;;
  'issue view') printf '{"title":"Publish opens a duplicate pull request"}\n' ;;
  'repo view') printf 'main\n' ;;
  'pr create')
    for a in "$@"; do printf '%s\000' "$a"; done >> "$QA_CASE_DIR/create.argv"
    printf 'https://github.com/owner/repo/pull/700\n' ;;
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
# flunk <message> [case dir]: the failure, then the end of the run's stderr and
# the gh call log.
flunk() {
  echo "FAIL: $1"
  local dir="${2-}"
  if [ -n "$dir" ]; then
    [ -f "$dir/stderr" ] && tail -n 20 "$dir/stderr" | sed 's/^/    stderr: /'
    [ -f "$dir/gh.log" ] && sed 's/^/    gh: /' "$dir/gh.log"
  fi
  fail=1
}

if [ "$(env -i PATH="$CASE_PATH" sh -c 'command -v gh')" != "$WORK/bin/gh" ]; then
  flunk "the stub gh is not first on the case PATH"
  echo "SCENARIO_FAILED"; exit 1
fi

# --- the recipe: the shipped step-16 body, then a step that records its result
STEP_BODY="$(bash "$SCRIPT_DIR/recipe-step-command.sh" "$RECIPE" step-16-create-draft-pr)" || { echo "FAIL: cannot read step-16-create-draft-pr from workflow-publish.yaml"; echo "SCENARIO_FAILED"; exit 1; }
{
  cat <<'HEAD'
name: "issue-2784-publish-qa"
description: "the shipped step-16 body, then a step that records its result (issue 2784 QA)"
version: "1.0.0"
context: {}
steps:
  - id: "step-16-create-draft-pr"
    type: "bash"
    command: |
HEAD
  printf '%s\n' "$STEP_BODY" | sed '/./s/^/      /'
  cat <<'TAIL'
    output: "pr_publish_result"
    parse_json: true

  - id: "record-publish-result"
    type: "bash"
    command: |
      printf '%s\n' "${RECIPE_VAR_pr_publish_result:-}" > "$QA_RESULT_FILE"
TAIL
} > "$WORK/publish-qa.yaml"

# git_env <case dir> <git args...>: git under env -i, with no user or system
# configuration and no transport but local files.
git_env() {
  local dir="$1"; shift
  env -i HOME="$dir/home" PATH="$CASE_PATH" GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null \
    GIT_TERMINAL_PROMPT=0 GIT_ALLOW_PROTOCOL=file \
    git -c user.name=qa -c user.email=qa@example.invalid -c commit.gpgsign=false \
    -c init.defaultBranch=main "$@" </dev/null
}

# qa_git <case dir> <git args...>: git_env for fixture setup, output logged.
qa_git() {
  local dir="$1"
  git_env "$@" >>"$dir/setup.log" 2>&1
}

# new_case <id> <prs.json jq filter>: a private folder for the case with a bare
# origin (HEAD main; branches main and dev), a clone on $BRANCH pushed to it,
# and the gh fixture. The filter is given the clone's HEAD as $sha.
new_case() {
  local filter="$2" dir="$WORK/case-$1" sha
  mkdir -p "$dir/home/.amplihack" "$dir/home/profiles" "$dir/tmp" "$dir/work" "$dir/launch" || return 1
  chmod -R go-rwx "$dir" || return 1
  qa_git "$dir" init -q --bare "$dir/origin.git" || return 1
  qa_git "$dir" -C "$dir/origin.git" symbolic-ref HEAD refs/heads/main || return 1
  qa_git "$dir" init -q "$dir/seed" || return 1
  printf 'issue 2784 QA\n' > "$dir/seed/README.md"
  qa_git "$dir" -C "$dir/seed" add README.md || return 1
  qa_git "$dir" -C "$dir/seed" commit -q -m base || return 1
  qa_git "$dir" -C "$dir/seed" push -q "$dir/origin.git" HEAD:refs/heads/main HEAD:refs/heads/dev || return 1
  qa_git "$dir" clone -q "$dir/origin.git" "$dir/clone" || return 1
  qa_git "$dir" -C "$dir/clone" checkout -q -b "$BRANCH" || return 1
  printf 'feature\n' > "$dir/clone/feature.txt"
  qa_git "$dir" -C "$dir/clone" add feature.txt || return 1
  qa_git "$dir" -C "$dir/clone" commit -q -m "feat: publish fixture" || return 1
  qa_git "$dir" -C "$dir/clone" push -q -u origin "$BRANCH" || return 1
  qa_git "$dir" -C "$dir/clone" remote set-url origin https://github.com/owner/repo.git || return 1
  qa_git "$dir" -C "$dir/clone" config "url.file://$dir/origin.git.insteadOf" https://github.com/owner/repo.git || return 1
  sha="$(git_env "$dir" -C "$dir/clone" rev-parse HEAD 2>>"$dir/setup.log")" || return 1
  jq -nc --arg sha "$sha" --arg head "$BRANCH" "$filter" > "$dir/prs.json" || return 1
  # The run must not be moved into a private copy of a checkout.
  if git_env "$dir" -C "$dir/launch" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    echo "the launch folder $dir/launch is inside a Git checkout" >> "$dir/setup.log"
    return 1
  fi
  printf '%s\n' "$dir"
}

# run_case <case dir>: the generated recipe through amplihack and the pinned
# runner, launched from the case's non-Git folder, under env -i.
run_case() {
  local dir="$1" setup
  setup="$(jq -nc --arg p "$dir/clone" '{worktree_path: $p}')" || return 1
  ( cd "$dir/launch" && exec env -i HOME="$dir/home" PATH="$CASE_PATH" TERM=dumb LANG=C \
      TMPDIR="$dir/tmp" GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null GIT_TERMINAL_PROMPT=0 \
      GIT_ALLOW_PROTOCOL=file AMPLIHACK_HOME="$REPO_ROOT" REMOTE_HOST_TYPE=github ISSUE_NUMBER=2784 \
      AMPLIHACK_SKIP_AUTO_INSTALL=1 AMPLIHACK_NO_FRESHNESS_CHECK=1 AMPLIHACK_NONINTERACTIVE=1 \
      AMPLIHACK_SKIP_POWER_STEERING=1 AMPLIHACK_SKIP_REFLECTION=1 AMPLIHACK_ROTATE_AUTOSTART=0 \
      AMPLIHACK_GOVERNOR=off AMPLIHACK_USAGE_LIMIT_WAIT=0 AMPLIHACK_CLEANUP_ON_EXIT=0 \
      AMPLIHACK_WORK_ROOT="$dir/work" AMPLIHACK_CLAUDE_PROFILE_ROOT="$dir/home/profiles" \
      AMPLIHACK_LAUNCHER_BINARY="$BIN" AMPLIHACK_AGENT_BINARY=claude \
      RECIPE_RUNNER_RS_PATH="$RUNNER" RECIPE_RUNNER_NO_UPDATE_CHECK=1 \
      QA_CASE_DIR="$dir" QA_RESULT_FILE="$dir/publish-result.json" \
      "$BIN" recipe run "$WORK/publish-qa.yaml" -w "$dir/launch" --no-detach \
      -c "worktree_setup=$setup" ) </dev/null >"$dir/stdout" 2>"$dir/stderr"
}

# field <case dir> <key>: one field of the recorded publish result, or
# <missing> unless the file holds exactly one JSON object with that field.
field() {
  jq -rs --arg k "$2" \
    'if length == 1 and (.[0] | type) == "object" and (.[0] | has($k)) then (.[0][$k] | tostring) else "<missing>" end' \
    "$1/publish-result.json" 2>/dev/null || printf '<missing>\n'
}

# expect_field <case dir> <key> <want>: adds a problem when the field differs.
expect_field() {
  local got
  got="$(field "$1" "$2")"
  [ "$got" = "$3" ] || problems+=("$2 '$got', want '$3'")
}

create_calls() {
  local count status
  if count="$(grep -c '^pr create' "$1/gh.log")"; then
    printf '%s\n' "$count"
  else
    status=$?
    if [ "$status" -eq 1 ]; then
      printf '0\n'
    else
      printf 'ERROR: cannot count pr create calls (grep exit %s)\n' "$status" >&2
      return "$status"
    fi
  fi
}

# --- case A: open PR #615 into dev, origin/HEAD main, base_branch empty ------
# shellcheck disable=SC2016 # $head and $sha are jq variables, not shell ones
if dir="$(new_case existing-pr \
    '[{number: 615, url: "https://github.com/owner/repo/pull/615",
       title: "feat: ui platform (#2784)", body: "Fixes #2784", state: "OPEN",
       createdAt: "2026-11-17T00:00:00Z", mergedAt: null,
       headRefName: $head, baseRefName: "dev", headRefOid: $sha,
       headRepositoryOwner: {login: "owner"}, headRepository: {name: "repo"},
       isCrossRepository: false, isDraft: true}]')"; then
  run_case "$dir"
  rc=$?
  problems=()
  [ "$rc" -eq 0 ] || problems+=("exit code $rc")
  expect_field "$dir" state existing-open-pr
  expect_field "$dir" legacy_state existing-open-pr
  expect_field "$dir" terminal_status success
  expect_field "$dir" pr_number 615
  expect_field "$dir" pr_url https://github.com/owner/repo/pull/615
  expect_field "$dir" base_branch dev
  expect_field "$dir" message "$KEPT_DEV"
  grep -q "^pr list .*--head $BRANCH" "$dir/gh.log" 2>/dev/null || problems+=("no pr list --head $BRANCH call")
  [ "$(create_calls "$dir")" = 0 ] || problems+=("$(create_calls "$dir") pr create call(s), want 0")
  [ ! -e "$dir/create.argv" ] || problems+=("gh pr create was reached")
  [ ! -e "$dir/unexpected" ] || problems+=("unexpected gh call: $(head -n 1 "$dir/unexpected")")
  if [ "${#problems[@]}" -eq 0 ]; then
    pass "existing_pr_on_other_base_adopted - PR #615 into dev recorded, base dev, no pr create"
  else
    flunk "existing_pr_on_other_base_adopted: ${problems[*]}" "$dir"
  fi
else
  flunk "existing_pr_on_other_base_adopted: cannot set up the case" "$WORK/case-existing-pr"
  [ -f "$WORK/case-existing-pr/setup.log" ] && tail -n 10 "$WORK/case-existing-pr/setup.log" | sed 's/^/    setup: /'
fi

# --- case B (control): no PR from the branch ---------------------------------
if dir="$(new_case no-pr '[]')"; then
  run_case "$dir"
  rc=$?
  problems=()
  [ "$rc" -eq 0 ] || problems+=("exit code $rc")
  expect_field "$dir" state FOLLOWUP_CREATED
  expect_field "$dir" terminal_status success
  expect_field "$dir" pr_url https://github.com/owner/repo/pull/700
  expect_field "$dir" base_branch ""
  [ "$(create_calls "$dir")" = 1 ] || problems+=("$(create_calls "$dir") pr create call(s), want 1")
  if [ -f "$dir/create.argv" ] && tr '\000' '\n' < "$dir/create.argv" | grep -q '^--base'; then
    problems+=("gh pr create was given --base")
  fi
  [ ! -e "$dir/unexpected" ] || problems+=("unexpected gh call: $(head -n 1 "$dir/unexpected")")
  if [ "${#problems[@]}" -eq 0 ]; then
    pass "no_pr_creates_one_without_base - one pr create, no --base, base_branch empty"
  else
    flunk "no_pr_creates_one_without_base: ${problems[*]}" "$dir"
  fi
else
  flunk "no_pr_creates_one_without_base: cannot set up the case" "$WORK/case-no-pr"
  [ -f "$WORK/case-no-pr/setup.log" ] && tail -n 10 "$WORK/case-no-pr/setup.log" | sed 's/^/    setup: /'
fi

echo "cases passed: $passed"
if [ "$fail" -eq 0 ] && [ "$passed" -eq 2 ]; then
  echo "issue 2784 existing pull request contract green"
  echo "ALL_CASES_PASSED"
  exit 0
fi
echo "SCENARIO_FAILED"
exit 1
