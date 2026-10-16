#!/usr/bin/env bash
# Self-asserting gadugi-test scenario body for issues #1481 and #1525: which
# agent CLI `amplihack` resolves, what it says about the answer, and how the
# answer crosses a detached launch.
#
# This runs the real `amplihack` binary from the outside. Every case runs under
# `env -i` with a fresh HOME, from a fresh project directory of its own that is
# a git repository, so the launcher_context.json walk-up stops there and never
# reads the checkout's or the developer's state. Stub `claude`, `copilot` and
# `codex` executables are first on PATH so nothing real is launched or
# installed. Nothing inside amplihack is replaced.
#
# What it covers, all from the command line a user types:
#   - `amplihack agent-binary`: the answer and its source for each layer --
#     default guess, session marker (including CLAUDE_CODE_ENTRYPOINT alone,
#     the #1481 case), AMPLIHACK_AGENT_BINARY, a fresh launcher_context.json,
#     `-w <dir>` -- and the stderr notice for each inferred answer: the
#     default, the file it was read from, an unusable file it skipped, a
#     rejected value (never echoed), an inherited guess, and an explicit value
#     overriding a session marker.
#   - `amplihack agent-binary --shell`: the `env -u <every marker> ...` prefix,
#     with a default guess still tagged `default:<cli>` and anything else
#     handed on untagged.
#   - The hand-off itself: the prefix, expanded in one environment and run in
#     another that holds a different session's marker, delivers the caller's
#     answer; a default guess arrives still announced as one.
#   - `amplihack recipe run`: the notice it prints and the
#     AMPLIHACK_AGENT_BINARY / AMPLIHACK_AGENT_BINARY_SOURCE a bash step sees.
#   - A real detached `tmux new-session` launch (when tmux is installed), into
#     a private tmux server started from a "Copilot session": without the
#     hand-off the pane resolves copilot from the server's marker and says so;
#     with it the pane gets the caller's claude.
#
# Run by hand: bash tests/gadugi/run-issue-1481-1525-agent-binary-scenario.sh
# It builds amplihack with cargo first. Set AMPLIHACK_1481_QA_BIN to an
# amplihack binary to skip the build. On success it prints ALL_CASES_PASSED and
# exits 0.
#
# The recipe-run cases need recipe-runner-rs (RECIPE_RUNNER_RS_PATH, or on
# PATH) and the detached-launch cases need tmux. Without them those cases
# print SKIP; with AMPLIHACK_1481_QA_REQUIRE_ALL=1 a SKIP fails the scenario.
# CI's Install Smoke Test job installs both and runs it that way, against the
# binary it installs.
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

WORK="$(mktemp -d "${TMPDIR:-/tmp}/issue-1481-qa.XXXXXX")" || {
  echo "FAIL: cannot create a temporary directory"; echo "SCENARIO_FAILED"; exit 1
}
# A private tmux socket, in a short directory of its own under /tmp: a Unix
# socket path is limited to about 100 bytes, and TMPDIR may be deep. The EXIT
# trap kills its server if a case started one.
SOCK_DIR="$(mktemp -d /tmp/ah1481.XXXXXX)" || {
  rm -rf -- "$WORK"; echo "FAIL: cannot create a socket directory"; echo "SCENARIO_FAILED"; exit 1
}
TMUX_SOCKET="$SOCK_DIR/s"
# shellcheck disable=SC2329 # invoked by the EXIT trap
cleanup() {
  if [ -S "$TMUX_SOCKET" ]; then
    tmux -S "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  fi
  rm -rf -- "$WORK" "$SOCK_DIR"
}
trap cleanup EXIT

# --- the binary under test --------------------------------------------------
BIN="${AMPLIHACK_1481_QA_BIN:-}"
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

# --- stub tools -------------------------------------------------------------
mkdir -p "$WORK/bin"
cat > "$WORK/bin/stub" <<'STUB'
#!/bin/sh
case "${1-}" in
  --version|-v|version) echo "2.1.247 (Claude Code)"; exit 0 ;;
esac
exit 0
STUB
chmod +x "$WORK/bin/stub"
for tool in claude copilot codex; do ln -s stub "$WORK/bin/$tool"; done
BASE_PATH="$WORK/bin:/usr/bin:/bin"

# Every session marker amplihack knows, in its own order
# (crates/amplihack-utils/src/agent_binary.rs, SESSION_MARKERS).
UNSET_ALL="-u CLAUDECODE -u CLAUDE_CODE -u CLAUDE_CODE_SESSION_ID -u CLAUDE_PROJECT_DIR -u CLAUDE_CODE_ENTRYPOINT -u COPILOT_CLI -u GITHUB_COPILOT -u GITHUB_COPILOT_AGENT -u COPILOT_AGENT"

fail=0
passed=0
pass() { echo "PASS: $1"; passed=$((passed + 1)); }
flunk() {
  echo "FAIL: $1"
  shift
  for f in "$@"; do
    [ -f "$f" ] && awk -v f="$(basename "$f")" 'n < 8 { print "    " f ": " $0; n++ }' "$f"
  done
  fail=1
}

# skip <cases> <why>: a SKIP, or a failure under AMPLIHACK_1481_QA_REQUIRE_ALL=1.
skip() {
  if [ "${AMPLIHACK_1481_QA_REQUIRE_ALL:-0}" = 1 ]; then
    flunk "$1 could not run ($2) and AMPLIHACK_1481_QA_REQUIRE_ALL=1"
  else
    echo "SKIP: $1 ($2)"
  fi
}

# A fresh project directory for case <id>, with a .git directory so the walk-up
# for launcher_context.json stops at it. The resolver only checks that .git
# exists (crates/amplihack-utils/src/agent_binary.rs). `git init` is not used:
# under an inherited GIT_DIR it creates no .git here.
new_project() {
  local dir="$WORK/case-$1"
  mkdir -p "$dir/home" "$dir/project"
  mkdir -p "$dir/project/.git"
  printf '%s\n' "$dir"
}

# run_in <case dir> [VAR=value ...] -- <amplihack args ...>
# Runs amplihack from the case's project directory under env -i.
run_in() {
  local dir="$1"; shift
  local envs=()
  while [ "$#" -gt 0 ] && [ "$1" != "--" ]; do envs+=("$1"); shift; done
  [ "$#" -gt 0 ] && shift
  ( cd "$dir/project" && exec env -i HOME="$dir/home" PATH="$BASE_PATH" TERM=dumb \
      AMPLIHACK_SKIP_AUTO_INSTALL=1 ${envs[@]+"${envs[@]}"} "$BIN" "$@" ) \
    </dev/null >"$dir/stdout" 2>"$dir/stderr"
}

# expect <id> <want stdout line> <want stderr substring, '-' for none, '=' for empty> \
#        [VAR=value ...] -- <amplihack args ...>
expect() {
  local id="$1" want_out="$2" want_err="$3"; shift 3
  local dir; dir="$(new_project "$id")"
  run_in "$dir" "$@"
  local rc=$? problems=()
  [ "$rc" -eq 0 ] || problems+=("exit code $rc")
  local got_out; got_out="$(cat "$dir/stdout")"
  [ "$got_out" = "$want_out" ] || problems+=("stdout '$got_out', want '$want_out'")
  case "$want_err" in
    -) ;;
    =) [ -s "$dir/stderr" ] && problems+=("stderr is not empty") ;;
    *) grep -qF -- "$want_err" "$dir/stderr" || problems+=("stderr lacks: $want_err") ;;
  esac
  if [ "${#problems[@]}" -eq 0 ]; then pass "$id -> $got_out"; else flunk "$id: ${problems[*]}" "$dir/stderr"; fi
}

# --- amplihack agent-binary: each layer -------------------------------------
expect default 'copilot (default)' \
  "amplihack: resolved the agent binary to 'copilot' (no AMPLIHACK_AGENT_BINARY or agent session marker was found)." \
  -- agent-binary
expect claudecode-marker 'claude (session_marker)' = CLAUDECODE=1 -- agent-binary
# Issue #1481: `env -u CLAUDECODE` left only CLAUDE_CODE_ENTRYPOINT.
expect entrypoint-only-marker 'claude (session_marker)' = CLAUDE_CODE_ENTRYPOINT=cli -- agent-binary
expect copilot-marker 'copilot (session_marker)' = COPILOT_CLI=1 -- agent-binary
expect explicit-env 'codex (env)' = AMPLIHACK_AGENT_BINARY=codex -- agent-binary
expect explicit-overrides-marker 'codex (env)' \
  "(AMPLIHACK_AGENT_BINARY is set and overrides CLAUDECODE, the claude session marker in this environment)" \
  AMPLIHACK_AGENT_BINARY=codex CLAUDECODE=1 -- agent-binary
expect inherited-guess 'copilot (default)' \
  "AMPLIHACK_AGENT_BINARY was inherited as a parent's default guess and no agent session marker was found" \
  AMPLIHACK_AGENT_BINARY=copilot AMPLIHACK_AGENT_BINARY_SOURCE=default:copilot -- agent-binary
expect rejected-value 'copilot (default)' \
  "AMPLIHACK_AGENT_BINARY is set but is not one of amplifier, claude, codex or copilot, and no agent session marker was found" \
  'AMPLIHACK_AGENT_BINARY=evil;touch /tmp/x' -- agent-binary
if grep -qF 'evil' "$WORK/case-rejected-value/stderr"; then
  flunk "rejected-value-not-echoed: the notice quotes the rejected value" "$WORK/case-rejected-value/stderr"
else
  pass "rejected-value-not-echoed"
fi

# A fresh launcher_context.json answers, and the notice names the file.
dir="$(new_project launcher-context)"
mkdir -p "$dir/project/.claude/runtime"
printf '{"launcher":"claude","command":"amplihack claude","timestamp":"%s"}\n' \
  "$(date -u +%Y-%m-%dT%H:%M:%SZ)" > "$dir/project/.claude/runtime/launcher_context.json"
run_in "$dir" -- agent-binary
ctx_real="$(cd "$dir/project" && pwd -P)/.claude/runtime/launcher_context.json"
if [ "$(cat "$dir/stdout")" = 'claude (launcher_context)' ] \
   && grep -qF "resolved the agent binary to 'claude' (read from $ctx_real)" "$dir/stderr"; then
  pass "launcher-context -> claude (launcher_context), read from its file"
else
  flunk "launcher-context: stdout '$(cat "$dir/stdout")'" "$dir/stderr"
fi

# -w resolves from the directory the recipe will run in, not from here.
other="$(new_project working-dir-elsewhere)"
run_in "$other" -- agent-binary -w "$dir/project"
if [ "$(cat "$other/stdout")" = 'claude (launcher_context)' ]; then
  pass "working-dir -> claude (launcher_context) from -w"
else
  flunk "working-dir: stdout '$(cat "$other/stdout")'" "$other/stderr"
fi

# Issue #1525 gap 2: an unusable file is named with the reason, not skipped
# silently, and its contents are never echoed.
dir="$(new_project malformed-context)"
mkdir -p "$dir/project/.claude/runtime"
printf '{"launcher": SECRET_TOKEN' > "$dir/project/.claude/runtime/launcher_context.json"
run_in "$dir" -- agent-binary
ctx_real="$(cd "$dir/project" && pwd -P)/.claude/runtime/launcher_context.json"
if [ "$(cat "$dir/stdout")" = 'copilot (default)' ] \
   && grep -qF "amplihack: ignored $ctx_real: it is not valid JSON" "$dir/stderr" \
   && grep -qF "Fix or delete it." "$dir/stderr" \
   && ! grep -qF SECRET_TOKEN "$dir/stderr"; then
  pass "malformed-context -> copilot (default), file named with its reason, contents not echoed"
else
  flunk "malformed-context: stdout '$(cat "$dir/stdout")'" "$dir/stderr"
fi

# --- amplihack agent-binary --shell -----------------------------------------
expect shell-default "env $UNSET_ALL AMPLIHACK_AGENT_BINARY=copilot AMPLIHACK_AGENT_BINARY_SOURCE=default:copilot" - \
  -- agent-binary --shell
expect shell-marker "env $UNSET_ALL AMPLIHACK_AGENT_BINARY=claude AMPLIHACK_AGENT_BINARY_SOURCE=" = \
  CLAUDECODE=1 -- agent-binary --shell
expect shell-explicit-clears-stale-tag "env $UNSET_ALL AMPLIHACK_AGENT_BINARY=codex AMPLIHACK_AGENT_BINARY_SOURCE=" = \
  AMPLIHACK_AGENT_BINARY=codex AMPLIHACK_AGENT_BINARY_SOURCE=session:claude -- agent-binary --shell

# --- the hand-off: expanded in the caller, run on the far side --------------
# The caller is a Claude Code session; the far side holds a Copilot session's
# marker (what a tmux server started from Copilot gives every new session).
dir="$(new_project hand-off)"
prefix="$(cd "$dir/project" && env -i HOME="$dir/home" PATH="$BASE_PATH" TERM=dumb AMPLIHACK_SKIP_AUTO_INSTALL=1 \
  CLAUDECODE=1 "$BIN" agent-binary --shell 2>/dev/null </dev/null)"
# shellcheck disable=SC2086 # the prefix is meant to be word-split, as $(...) is in a tmux command
( cd "$dir/project" && exec env -i HOME="$dir/home" PATH="$BASE_PATH" TERM=dumb AMPLIHACK_SKIP_AUTO_INSTALL=1 COPILOT_CLI=1 \
    $prefix "$BIN" agent-binary ) </dev/null >"$dir/stdout" 2>"$dir/stderr"
if [ "$(cat "$dir/stdout")" = 'claude (env)' ] && [ ! -s "$dir/stderr" ]; then
  pass "hand-off -> the far side runs the caller's claude, not its own COPILOT_CLI, silently"
else
  flunk "hand-off: far side got '$(cat "$dir/stdout")'" "$dir/stderr"
fi

# A default guess crosses still tagged, so the far side announces it as one.
dir="$(new_project hand-off-default)"
prefix="$(cd "$dir/project" && env -i HOME="$dir/home" PATH="$BASE_PATH" TERM=dumb AMPLIHACK_SKIP_AUTO_INSTALL=1 \
  "$BIN" agent-binary --shell 2>/dev/null </dev/null)"
# shellcheck disable=SC2086
( cd "$dir/project" && exec env -i HOME="$dir/home" PATH="$BASE_PATH" TERM=dumb AMPLIHACK_SKIP_AUTO_INSTALL=1 \
    $prefix "$BIN" agent-binary ) </dev/null >"$dir/stdout" 2>"$dir/stderr"
if [ "$(cat "$dir/stdout")" = 'copilot (default)' ] \
   && grep -qF "inherited as a parent's default guess" "$dir/stderr"; then
  pass "hand-off-default -> still a guess on the far side, and announced"
else
  flunk "hand-off-default: far side got '$(cat "$dir/stdout")'" "$dir/stderr"
fi

# --- amplihack recipe run ----------------------------------------------------
# `recipe run` executes through recipe-runner-rs. The cases use the one named
# by RECIPE_RUNNER_RS_PATH, or the one on the caller's PATH, and are skipped
# (and say so) when there is none.
RUNNER="${RECIPE_RUNNER_RS_PATH:-$(command -v recipe-runner-rs 2>/dev/null || true)}"
# A one-step bash recipe that prints what its step was handed.
cat > "$WORK/show-agent-binary.yaml" <<'RECIPE'
name: show-agent-binary
description: Print the agent binary a step is handed (issue 1481 QA)
version: "1.0"
author: qa
steps:
  - id: show
    type: bash
    command: echo "STEP_SAW binary=[${AMPLIHACK_AGENT_BINARY-<unset>}] source=[${AMPLIHACK_AGENT_BINARY_SOURCE-<unset>}]"
RECIPE

# recipe_case <id> <want STEP_SAW line> <want stderr substring, or -> [VAR=value ...]
# The values are bracketed, so an unset tag is told apart from an empty one and
# each tag is matched whole. What recipe run exports
# (EnvBuilder::with_resolved_agent_binary): the user's own value untagged, with
# any inherited tag removed; a session marker's or launcher context's answer
# tagged session:<cli>; the built-in default tagged default:<cli>.
recipe_case() {
  local id="$1" want_step="$2" want_err="$3"; shift 3
  local dir; dir="$(new_project "$id")"
  run_in "$dir" "$@" RECIPE_RUNNER_RS_PATH="$RUNNER" -- recipe run "$WORK/show-agent-binary.yaml" -w "$dir/project"
  local rc=$? problems=()
  [ "$rc" -eq 0 ] || problems+=("exit code $rc")
  grep -qF -- "$want_step" "$dir/stdout" "$dir/stderr" || problems+=("step did not print: $want_step")
  if [ "$want_err" != "-" ] && ! grep -qF -- "$want_err" "$dir/stderr"; then
    problems+=("stderr lacks: $want_err")
  fi
  if [ "${#problems[@]}" -eq 0 ]; then pass "$id -> $want_step"; else flunk "$id: ${problems[*]}" "$dir/stdout" "$dir/stderr"; fi
}
if [ -n "$RUNNER" ] && [ -x "$RUNNER" ]; then
recipe_case recipe-run-default 'STEP_SAW binary=[copilot] source=[default:copilot]' \
  "amplihack: agent steps will run under 'copilot' (no AMPLIHACK_AGENT_BINARY or agent session marker was found)."
recipe_case recipe-run-entrypoint-marker 'STEP_SAW binary=[claude] source=[session:claude]' - CLAUDE_CODE_ENTRYPOINT=cli
recipe_case recipe-run-explicit-overrides-marker 'STEP_SAW binary=[claude] source=[<unset>]' \
  "amplihack: agent steps will run under 'claude' (AMPLIHACK_AGENT_BINARY is set and overrides COPILOT_CLI, the copilot session marker in this environment)" \
  AMPLIHACK_AGENT_BINARY=claude COPILOT_CLI=1
recipe_case recipe-run-explicit-drops-stale-tag 'STEP_SAW binary=[claude] source=[<unset>]' - \
  AMPLIHACK_AGENT_BINARY=claude AMPLIHACK_AGENT_BINARY_SOURCE=session:copilot
else
  skip "recipe-run-default, recipe-run-entrypoint-marker, recipe-run-explicit-overrides-marker, recipe-run-explicit-drops-stale-tag" "no recipe-runner-rs"
fi

# --- a real detached tmux launch --------------------------------------------
if command -v tmux >/dev/null 2>&1; then
  dir="$(new_project tmux)"
  TPATH="$(dirname "$(command -v tmux)"):$BASE_PATH"
  # The server is started from a "Copilot session": its global environment
  # holds COPILOT_CLI=1, and every new session gets it.
  env -i HOME="$dir/home" PATH="$TPATH" TERM=xterm COPILOT_CLI=1 \
    tmux -S "$TMUX_SOCKET" -f /dev/null new-session -d -s keeper "sleep 600" </dev/null
  # pane_script <name>: a script, run by the pane from $dir, that resolves from
  # the project directory and writes <name>.out, <name>.err, then <name>.done.
  # Paths are quoted with printf %q, so a TMPDIR with spaces still works.
  pane_script() {
    printf 'cd %q && %q agent-binary >%q 2>%q; touch %q\n' \
      "$dir/project" "$BIN" "$dir/$1.out" "$dir/$1.err" "$dir/$1.out.done" > "$dir/$1-pane.sh"
  }
  # wait_for <file>: the pane writes its result, then a .done marker.
  wait_for() { local i=0; while [ ! -e "$1.done" ] && [ "$i" -lt 100 ]; do sleep 0.1; i=$((i + 1)); done; }

  # From a Claude Code shell, without the hand-off.
  pane_script plain
  ( cd "$dir" && env -i HOME="$dir/home" PATH="$TPATH" TERM=xterm CLAUDECODE=1 \
      tmux -S "$TMUX_SOCKET" new-session -d -s plain -c "$dir" "sh ./plain-pane.sh" ) </dev/null
  wait_for "$dir/plain.out"
  if [ "$(cat "$dir/plain.out" 2>/dev/null)" = 'copilot (session_marker)' ] \
     && grep -qF "COPILOT_CLI is set, but this tmux server's global environment holds the same value" "$dir/plain.err"; then
    pass "tmux-without-hand-off -> copilot from the server's marker, and the pane says so"
  else
    flunk "tmux-without-hand-off: pane got '$(cat "$dir/plain.out" 2>/dev/null)'" "$dir/plain.err"
  fi

  # The same launch with the hand-off, double-quoted as documented.
  # The inner sh is the caller's Claude Code shell. It expands $(...) itself,
  # as the documented double-quoted tmux command does; its arguments arrive as
  # $1..$3.
  pane_script handed
  # shellcheck disable=SC2016
  ( cd "$dir" && env -i HOME="$dir/home" PATH="$TPATH" TERM=xterm CLAUDECODE=1 AMPLIHACK_SKIP_AUTO_INSTALL=1 \
      sh -c 'tmux -S "$1" new-session -d -s handed -c "$3" "$("$2" agent-binary --shell) sh ./handed-pane.sh"' \
      sh "$TMUX_SOCKET" "$BIN" "$dir" ) </dev/null 2>"$dir/handed.launch.err"
  wait_for "$dir/handed.out"
  if [ "$(cat "$dir/handed.out" 2>/dev/null)" = 'claude (env)' ] && [ ! -s "$dir/handed.err" ]; then
    pass "tmux-with-hand-off -> the pane runs the caller's claude"
  else
    flunk "tmux-with-hand-off: pane got '$(cat "$dir/handed.out" 2>/dev/null)'" "$dir/handed.err" "$dir/handed.launch.err"
  fi
  tmux -S "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
else
  skip "tmux-without-hand-off, tmux-with-hand-off" "tmux is not installed"
fi

echo "cases passed: $passed"
if [ "$fail" -eq 0 ]; then
  echo "issue 1481 1525 agent binary contract green"
  echo "ALL_CASES_PASSED"
  exit 0
fi
echo "SCENARIO_FAILED"
exit 1
