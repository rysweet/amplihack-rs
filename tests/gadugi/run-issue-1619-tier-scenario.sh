#!/usr/bin/env bash
# Self-asserting gadugi-test scenario body for issue #1619: the model and
# reasoning effort each harness receives when a recipe agent step declares a
# model tier.
#
# This runs the real `amplihack` binary from the outside, through
# `amplihack recipe run` and the pinned recipe runner. Every case runs under
# `env -i` with a fresh HOME and a fresh git project directory of its own.
# Stub `claude`, `codex` (with the `codex-code-mode-host` helper beside it)
# and `copilot` executables are first on PATH, so nothing real is launched or
# installed. Each stub records its argv, NUL-separated, to a file of its own,
# drains stdin and prints `done`; the Codex stub also writes the final message
# the runner reads, under umask 077. Nothing inside amplihack is replaced.
#
# Each case runs one agent step that declares `model: "strong"`:
#   claude-strong                  --model claude-opus-5[1m]
#   codex-strong                   no --model, -c model_reasoning_effort=high
#   copilot-strong                 no --model
#   codex-strong-claude-default    AMPLIHACK_DEFAULT_MODEL=claude-sonnet-5-5:
#                                  no Claude id reaches Codex, effort high
#   claude-strong-claude-default   AMPLIHACK_DEFAULT_MODEL=claude-sonnet-5-5:
#                                  --model claude-sonnet-5-5
#
# Run by hand: bash tests/gadugi/run-issue-1619-tier-scenario.sh
# It builds amplihack with cargo first. Set AMPLIHACK_1619_QA_BIN to an
# amplihack binary to skip the build. On success it prints ALL_CASES_PASSED and
# exits 0.
#
# The cases need recipe-runner-rs (RECIPE_RUNNER_RS_PATH, or on PATH). Without
# it they print SKIP; with AMPLIHACK_1619_QA_REQUIRE_ALL=1 a SKIP fails the
# scenario. CI's Install Smoke Test job installs the pinned runner and runs it
# that way, against the binary it installs.
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

WORK="$(mktemp -d "${TMPDIR:-/tmp}/issue-1619-qa.XXXXXX")" || {
  echo "FAIL: cannot create a temporary directory"; echo "SCENARIO_FAILED"; exit 1
}
chmod 700 "$WORK"
trap 'rm -rf -- "$WORK"' EXIT

# --- the binary under test --------------------------------------------------
BIN="${AMPLIHACK_1619_QA_BIN:-}"
if [ -z "$BIN" ]; then
  # Same build idiom as run-issue-1527-model-scenario.sh: one awk reads every
  # artifact record and prints the last executable path (issue #1434).
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

# --- stub tools -------------------------------------------------------------
# One script, copied under each tool's name: it answers by the name it was
# run as, so a copy (not a symlink) keeps that name however it is resolved.
mkdir -p "$WORK/bin"
cat > "$WORK/stub" <<'STUB'
#!/bin/sh
case "${0##*/}" in
  codex|codex-code-mode-host) version='codex-cli 0.166.0' ;;
  copilot) version='0.0.400' ;;
  *) version='2.1.319 (Claude Code)' ;;
esac
if [ "${1-}" = "--version" ]; then
  printf '%s\n' "$version"
  exit 0
fi
[ "${0##*/}" = codex-code-mode-host ] && exit 0
umask 077
final=""
prev=""
for a in "$@"; do
  if [ "$prev" = "--output-last-message" ]; then final="$a"; fi
  prev="$a"
done
for a in "$@"; do printf '%s\000' "$a"; done > "$AMPLIHACK_1619_ARGV_DIR/launch.$$"
cat > /dev/null
if [ -n "$final" ]; then printf 'done\n' > "$final"; fi
printf 'done\n'
exit 0
STUB
for tool in claude codex codex-code-mode-host copilot; do
  cp "$WORK/stub" "$WORK/bin/$tool"
  chmod 700 "$WORK/bin/$tool"
done
BASE_PATH="$WORK/bin:/usr/local/bin:/usr/bin:/bin"

# --- the recipe -------------------------------------------------------------
cat > "$WORK/tier-qa.yaml" <<'RECIPE'
name: "issue-1619-tier-qa"
description: "one agent step that declares a model tier (issue 1619 QA)"
version: "1.0.0"
context: {}
steps:
  - id: "step-08-implement"
    type: "agent"
    model: "strong"
    prompt: |
      Reply with the single word done and stop. Change nothing.
RECIPE

fail=0
passed=0
pass() { echo "PASS: $1"; passed=$((passed + 1)); }
# flunk <message> <case dir>: the case, the problems and the recorded argv.
# A run that recorded no launch also shows the end of its stderr, which is
# the only account of why the harness was never reached.
flunk() {
  echo "FAIL: $1"
  local dir="${2-}" f found=0
  if [ -n "$dir" ] && [ -d "$dir/launches" ]; then
    for f in "$dir"/launches/launch.*; do
      [ -f "$f" ] || continue
      found=1
      echo "    argv: $(tr '\000' ' ' < "$f")"
    done
    if [ "$found" -eq 0 ] && [ -f "$dir/stderr" ]; then
      tail -8 "$dir/stderr" | sed 's/^/    stderr: /'
    fi
  fi
  fail=1
}

# skip <cases> <why>: a SKIP, or a failure under AMPLIHACK_1619_QA_REQUIRE_ALL=1.
skip() {
  if [ "${AMPLIHACK_1619_QA_REQUIRE_ALL:-0}" = 1 ]; then
    flunk "$1 could not run ($2) and AMPLIHACK_1619_QA_REQUIRE_ALL=1"
  else
    echo "SKIP: $1 ($2)"
  fi
}

qa_git() {
  env -i HOME="$1/home" PATH="$BASE_PATH" GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null \
    GIT_TERMINAL_PROMPT=0 git -C "$1/project" \
    -c user.name=qa -c user.email=qa@example.invalid -c commit.gpgsign=false "${@:2}" \
    </dev/null >/dev/null 2>&1
}

# new_case <id>: a fresh, private directory for case <id>, holding a home, a
# git project with one commit (checkout discovery needs a repository), and the
# directories the run and the stubs write to.
new_case() {
  local dir="$WORK/case-$1"
  mkdir -p "$dir/home/.amplihack" "$dir/project" "$dir/launches" "$dir/codex-home" \
    "$dir/work" "$dir/home/profiles" || return 1
  chmod -R go-rwx "$dir" || return 1
  qa_git "$dir" init -q || return 1
  printf 'issue 1619 QA\n' > "$dir/project/README.md"
  qa_git "$dir" add README.md || return 1
  qa_git "$dir" commit -q -m init || return 1
  printf '%s\n' "$dir"
}

# run_case <dir> <harness> [VAR=value ...]: the recipe through amplihack and
# the pinned runner, from the case's project directory under env -i.
run_case() {
  local dir="$1" harness="$2"; shift 2
  local upper
  upper="$(printf '%s' "$harness" | tr '[:lower:]' '[:upper:]')"
  ( cd "$dir/project" && exec env -i HOME="$dir/home" PATH="$BASE_PATH" TERM=dumb \
      AMPLIHACK_SKIP_AUTO_INSTALL=1 AMPLIHACK_NO_FRESHNESS_CHECK=1 AMPLIHACK_NONINTERACTIVE=1 \
      AMPLIHACK_SKIP_POWER_STEERING=1 AMPLIHACK_SKIP_REFLECTION=1 AMPLIHACK_ROTATE_AUTOSTART=0 \
      AMPLIHACK_GOVERNOR=off AMPLIHACK_USAGE_LIMIT_WAIT=0 AMPLIHACK_CLEANUP_ON_EXIT=0 \
      AMPLIHACK_WORK_ROOT="$dir/work" AMPLIHACK_CLAUDE_PROFILE_ROOT="$dir/home/profiles" \
      AMPLIHACK_LAUNCHER_BINARY="$BIN" AMPLIHACK_AGENT_BINARY="$harness" \
      "AMPLIHACK_${upper}_BINARY_PATH=$WORK/bin/$harness" \
      CODEX_HOME="$dir/codex-home" RECIPE_RUNNER_RS_PATH="$RUNNER" \
      AMPLIHACK_1619_ARGV_DIR="$dir/launches" \
      "$@" \
      "$BIN" recipe run "$WORK/tier-qa.yaml" -w "$dir/project" --no-detach ) \
    </dev/null >"$dir/stdout" 2>"$dir/stderr"
}

# The last value of `--model` (either spelling) in one launch record.
model_of() {
  tr '\000' '\n' < "$1" | awk '
    prev == "--model" { m = $0 }
    /^--model=/ { m = substr($0, 9) }
    { prev = $0 }
    END { print m }'
}

# The last `-c model_reasoning_effort=<value>` value in one launch record.
effort_of() {
  tr '\000' '\n' < "$1" | awk '
    prev == "-c" && /^model_reasoning_effort=/ { e = substr($0, 24) }
    { prev = $0 }
    END { print e }'
}

# tier_case <id> <harness> <want model, or - for none> <want effort, or - for
#           none> [VAR=value ...]
tier_case() {
  local id="$1" harness="$2" want_model="$3" want_effort="$4"; shift 4
  local dir
  if ! dir="$(new_case "$id")"; then
    flunk "$id: cannot set up the case"
    return
  fi
  run_case "$dir" "$harness" "$@"
  local rc=$? problems=() files=() f
  [ "$rc" -eq 0 ] || problems+=("exit code $rc")
  for f in "$dir"/launches/launch.*; do [ -f "$f" ] && files+=("$f"); done
  if [ "${#files[@]}" -ne 1 ]; then
    problems+=("${#files[@]} launches of $harness, want 1")
  else
    f="${files[0]}"
    local got_model got_effort
    got_model="$(model_of "$f")"
    got_effort="$(effort_of "$f")"
    if [ "$want_model" = "-" ]; then
      [ -z "$got_model" ] || problems+=("--model $got_model, want none")
    else
      [ "$got_model" = "$want_model" ] || problems+=("--model '$got_model', want $want_model")
    fi
    if [ "$want_effort" = "-" ]; then
      [ -z "$got_effort" ] || problems+=("effort $got_effort, want none")
    else
      [ "$got_effort" = "$want_effort" ] || problems+=("effort '$got_effort', want $want_effort")
    fi
    if tr '\000' '\n' < "$f" | grep -qx 'strong'; then
      problems+=("the tier word reached $harness")
    fi
    # A Claude id by its own shape: a path amplihack adds may contain `.claude`.
    if [ "$harness" != claude ] \
       && tr '\000' '\n' < "$f" | grep -qE '^(--model=)?claude-(opus|sonnet|haiku)'; then
      problems+=("a Claude id reached $harness")
    fi
  fi
  if [ "${#problems[@]}" -eq 0 ]; then
    pass "$id -> model ${want_model}, effort ${want_effort}"
  else
    flunk "$id: ${problems[*]}" "$dir"
  fi
}

if [ -n "$RUNNER" ] && [ -x "$RUNNER" ]; then
  tier_case claude-strong claude 'claude-opus-5[1m]' -
  tier_case codex-strong codex - high
  tier_case copilot-strong copilot - -
  tier_case codex-strong-claude-default codex - high AMPLIHACK_DEFAULT_MODEL=claude-sonnet-5-5
  tier_case claude-strong-claude-default claude claude-sonnet-5-5 - AMPLIHACK_DEFAULT_MODEL=claude-sonnet-5-5
else
  skip "claude-strong, codex-strong, copilot-strong, codex-strong-claude-default, claude-strong-claude-default" "no recipe-runner-rs"
fi

echo "cases passed: $passed"
if [ "$fail" -eq 0 ]; then
  echo "issue 1619 model tier contract green"
  echo "ALL_CASES_PASSED"
  exit 0
fi
echo "SCENARIO_FAILED"
exit 1
