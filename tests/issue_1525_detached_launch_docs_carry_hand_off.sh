#!/usr/bin/env bash
# Issue #1525 — every shipped instruction that starts a detached
# `amplihack recipe run` must hand it the agent binary.
#
# Once a tmux server is running, `tmux new-session` gives the new command the
# server's environment, not the caller's, so the session markers that tell
# `recipe run` which agent CLI the caller is in do not arrive, and the run
# falls back to `copilot` (#1335). The hand-off is
# `$(amplihack agent-binary --shell -w <repo>)`, expanded in the caller's shell.
# An instruction that leaves it out reproduces the bug in whoever follows it.
#
#   1. Every `tmux new-session` command in shipped text (amplifier-bundle/ and
#      docs/), backslash continuations joined, that runs `recipe run` carries
#      `agent-binary --shell`.
#   2. In the context SessionStart injects into every session
#      (amplifier-bundle/context/, which holds USER_PREFERENCES.md), any line
#      telling an agent to use tmux for a recipe run carries it too. That line
#      is prose, not a command, so check 1 cannot see it, and an agent obeying
#      it has no reason to open the dev-orchestrator reference.md that shows
#      the hand-off. The crusty review of #1490 found exactly that line.

set -uo pipefail

[ -d amplifier-bundle/context ] || { echo "run from the repo root"; exit 1; }

fails=0
pass() { printf '  ok    %s\n' "$1"; }
fail() { printf '  FAIL  %s\n' "$1"; fails=$((fails + 1)); }

# --- 1. detached recipe-run commands --------------------------------------
shipped=()
while IFS= read -r file; do
  shipped+=("$file")
done < <(git ls-files 'amplifier-bundle/*.md' 'amplifier-bundle/*.sh' \
  'amplifier-bundle/*.yaml' 'amplifier-bundle/*.yml' 'docs/*.md')
[ "${#shipped[@]}" -gt 0 ] || { echo "  FAIL  no shipped files found (is this a git checkout?)"; exit 1; }
offenders="$(awk '
  FNR == 1 { buf = "" }
  {
    if (buf == "") { buf = $0; start = FNR } else { buf = buf " " $0 }
    if ($0 ~ /\\$/) next
    if (buf ~ /tmux new-session/ && buf ~ /recipe run/ && buf !~ /agent-binary --shell/)
      print "    " FILENAME ":" start ": " buf
    buf = ""
  }' "${shipped[@]}")"
# shellcheck disable=SC2126  # grep -c would count per file, not in total
commands="$(grep -h 'tmux new-session' "${shipped[@]}" | wc -l | tr -d ' ')"
if [ -z "$offenders" ]; then
  pass "no detached recipe run without the hand-off ($commands tmux new-session line(s) scanned)"
else
  fail "a detached recipe run without \$(amplihack agent-binary --shell -w <repo>):
$offenders"
fi

# --- 2. the always-loaded context ----------------------------------------
context_offenders="$(grep -n -i 'tmux' amplifier-bundle/context/*.md \
  | grep -i 'recipe run' | grep -v 'agent-binary --shell')"
context_mentions="$(grep -i 'tmux' amplifier-bundle/context/*.md | grep -ci 'recipe run')"
if [ "$context_mentions" -eq 0 ]; then
  pass "session context does not tell agents to start a recipe run in tmux"
elif [ -z "$context_offenders" ]; then
  pass "session context: every tmux recipe-run instruction carries the hand-off ($context_mentions line(s))"
else
  fail "session context tells agents to start a recipe run in tmux without the hand-off:
$context_offenders"
fi

if [ "$fails" -ne 0 ]; then
  echo "issue #1525 detached launch hand-off: $fails failure(s)"
  exit 1
fi
echo "issue #1525 detached launch hand-off: all checks passed"
