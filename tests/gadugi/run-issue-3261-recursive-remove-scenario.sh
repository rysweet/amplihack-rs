#!/usr/bin/env bash
# Self-asserting gadugi-test scenario body for issue #3261: the command-check
# hook decides on each recursive remove command by that command's own
# arguments, and text that only mentions one is allowed.
#
# It drives the real `amplihack-hooks` binary the way Claude Code does: a
# PreToolUse event for a Bash call goes to `amplihack-hooks pre-tool-use` on
# stdin, and the case reads only the decision from stdout (deny, or no deny).
# It never reads the wording of the reason.
#
# The hook resolves remove targets against its own working directory, so each
# run starts in a fresh scratch directory under TMPDIR with a fresh HOME.
#
# Cases:
#   - heredoc-body: a heredoc whose body mentions a recursive remove of the
#     working directory, in a call that also uses a variable. Allowed.
#   - own-variable: a recursive remove with a variable in its own arguments.
#     Rejected.
#   - literal-after-variable-outside: a literal recursive remove of a path
#     outside the working directory, after a separate command that uses a
#     variable. Allowed.
#   - literal-after-variable-cwd: the same call removing the working directory
#     itself. Rejected, on its own path.
#   - quoted-text: a quoted string and a commit message from a quoted
#     heredoc in a substitution, both mentioning a recursive remove, beside
#     a variable. Allowed: neither is a command.
#   - shell-script: a recursive remove with a variable inside a `bash -c`
#     script. Rejected: text a shell runs is decided on the whole string.
#
# Where it runs: CI's cross-compile job runs this script against the release
# `amplihack-hooks` binary it builds for x86_64 Linux. CI does not run
# gadugi-test itself; tests/gadugi/scenarios/issue-3261-recursive-remove-commands.yaml
# wraps this script for anyone running it through gadugi-test by hand.
#
# Usage: bash tests/gadugi/run-issue-3261-recursive-remove-scenario.sh [path/to/amplihack-hooks]
# (default: $AMPLIHACK_3261_QA_BIN, then target/release, then target/debug).
# On success it prints ALL_CASES_PASSED and exits 0.
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

HOOKS="${1:-${AMPLIHACK_3261_QA_BIN:-}}"
if [ -z "$HOOKS" ]; then
  for candidate in "$REPO_ROOT/target/release/amplihack-hooks" "$REPO_ROOT/target/debug/amplihack-hooks"; do
    if [ -x "$candidate" ]; then
      HOOKS="$candidate"
      break
    fi
  done
fi
if [ -z "$HOOKS" ] || [ ! -x "$HOOKS" ]; then
  echo "FAIL: no amplihack-hooks binary (pass a path or set AMPLIHACK_3261_QA_BIN)"
  echo "SCENARIO_FAILED"
  exit 1
fi
# The cases run in a scratch directory, so a relative path is anchored here.
HOOKS="$(cd "$(dirname "$HOOKS")" && pwd)/$(basename "$HOOKS")"
echo "binary under test: $HOOKS"

WORK="$(mktemp -d -t issue-3261-qa.XXXXXX)" || {
  echo "FAIL: cannot create a temporary directory"; echo "SCENARIO_FAILED"; exit 1
}
trap 'rm -rf -- "$WORK"' EXIT
mkdir -p "$WORK/home" "$WORK/cwd"
CWD="$(cd "$WORK/cwd" && pwd -P)"

# JSON string escaping for the few characters the cases use.
json_string() {
  local s="$1"
  s="${s//\\/\\\\}"
  s="${s//\"/\\\"}"
  s="${s//$'\n'/\\n}"
  s="${s//$'\t'/\\t}"
  printf '"%s"' "$s"
}

fail=0
passed=0

# check <id> <want: deny|allow> <command text>
check() {
  local id="$1" want="$2" command="$3"
  local input got rc
  input="$(printf '{"hook_event_name":"PreToolUse","session_id":"issue-3261-qa","tool_name":"Bash","tool_input":{"command":%s}}' "$(json_string "$command")")"
  ( cd "$CWD" && printf '%s' "$input" | env -i HOME="$WORK/home" PATH="/usr/bin:/bin" "$HOOKS" pre-tool-use ) \
    >"$WORK/$id.stdout" 2>"$WORK/$id.stderr"
  rc=$?
  if grep -q '"permissionDecision":[[:space:]]*"deny"' "$WORK/$id.stdout"; then
    got=deny
  else
    got=allow
  fi
  if [ "$rc" -ne 0 ]; then
    echo "FAIL: $id: the hook exited $rc"
    head -5 "$WORK/$id.stderr" | sed 's/^/    stderr: /'
    fail=1
  elif [ "$got" = "$want" ]; then
    echo "PASS: $id -> $got"
    passed=$((passed + 1))
  else
    echo "FAIL: $id: decision $got, want $want"
    fail=1
  fi
}

check heredoc-body allow "cat > notes.md <<'EOF'
The hook rejects rm -rf . and rm -rf \$(pwd).
EOF
echo \"\$HOME\""

check own-variable deny 'rm -rf "$TARGET/build"'

check literal-after-variable-outside allow 'cd "$DIR" && git status; rm -r /nonexistent/issue-3261-qa/build'

check literal-after-variable-cwd deny "cd \"\$DIR\" && git status; rm -r $CWD"

check quoted-text allow "echo 'rm -rf .' \"\$HOME\"; git commit -m \"\$(cat <<'EOF'
Reject rm -rf \`pwd\` but not text.
EOF
)\""

check shell-script deny "bash -c 'rm -rf \"\$TARGET\"'"

echo "cases passed: $passed"
if [ "$fail" -eq 0 ]; then
  echo "issue 3261 recursive remove decision green"
  echo "ALL_CASES_PASSED"
  exit 0
fi
echo "SCENARIO_FAILED"
exit 1
