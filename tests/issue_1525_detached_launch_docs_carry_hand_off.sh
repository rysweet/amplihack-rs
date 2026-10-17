#!/usr/bin/env bash
# Issue #1525 — every shipped instruction that starts a detached
# `amplihack recipe run` must hand it the agent binary, in double quotes.
#
# `tmux new-session` gives the new command the tmux server's global
# environment, not the caller's, and tmux copied that from whatever started the
# server. The session markers that tell `recipe run` which agent CLI the caller
# is in do not arrive, and the starter's do: the run takes `copilot` from the
# default, or from a server a Copilot session started (#1335, crusty review of
# #1490). The hand-off is `$(amplihack agent-binary --shell -w <repo>)`,
# expanded in the caller's shell; it unsets the far side's markers and sets the
# caller's answer. It expands in the caller's shell only inside a double-quoted
# command. In single quotes it runs in the new session, reads the server's
# markers and hands over the server's CLI (crusty review of #1490 at
# ef441d81). An instruction that leaves it out, or single-quotes it,
# reproduces the bug in whoever follows it.
#
#   1. Every `tmux new-session` command in shipped text (amplifier-bundle/ and
#      docs/), backslash continuations joined, that runs `recipe run` carries
#      `agent-binary --shell`, and the hand-off sits inside double quotes. The
#      quoting is read the way sh reads it, from `tmux new-session` up to the
#      hand-off; a self-test runs the same scan over the forms that matter.
#   2. In the context SessionStart injects into every session
#      (amplifier-bundle/context/, which holds USER_PREFERENCES.md), any line
#      telling an agent to use tmux for a recipe run carries the hand-off and
#      says it goes in double quotes. That line is prose, not a command, so
#      check 1 cannot see it, and it is the line agents act on: an agent
#      obeying it has no reason to open the dev-orchestrator reference.md that
#      shows the full template.
#
# What the docs say about setsid, nohup and update-check skips is not checked
# here by matching sentences. That behaviour is pinned by Rust tests
# (issue_1525_agent_binary_is_not_subprocess_safe.rs, and
# agent_binary_env_is_not_a_skip_signal in the update-check tests).

set -uo pipefail

[ -d amplifier-bundle/context ] || { echo "run from the repo root"; exit 1; }

fails=0
pass() { printf '  ok    %s\n' "$1"; }
fail() { printf '  FAIL  %s\n' "$1"; fails=$((fails + 1)); }

# --- 1. detached recipe-run commands --------------------------------------
# Prints one line per offending command: "missing<TAB>file:line: command" for
# a command without the hand-off, "quoted<TAB>file:line: command" for one whose
# hand-off is not inside double quotes. quote_at() follows sh: nothing is
# special inside single quotes; a backslash escapes the next character
# elsewhere; inside double quotes a single quote is literal. \047 is '.
# shellcheck disable=SC2016  # the awk program is single-quoted on purpose
scan='
  function quote_at(s, end,    i, c, q) {
    q = "none"
    for (i = 1; i < end; i++) {
      c = substr(s, i, 1)
      if (q == "single") { if (c == "\047") q = "none"; continue }
      if (c == "\\") { i++; continue }
      if (q == "double") { if (c == "\"") q = "none"; continue }
      if (c == "\047") q = "single"
      else if (c == "\"") q = "double"
    }
    return q
  }
  FNR == 1 { buf = "" }
  {
    if (buf == "") { buf = $0; start = FNR } else { buf = buf " " $0 }
    if ($0 ~ /\\$/) next
    at = index(buf, "tmux new-session")
    if (at > 0 && buf ~ /recipe run/) {
      cmd = substr(buf, at)
      hand_off = index(cmd, "agent-binary --shell")
      if (hand_off == 0)
        print "missing\t" FILENAME ":" start ": " buf
      else if (quote_at(cmd, hand_off) != "double")
        print "quoted\t" FILENAME ":" start ": " buf
    }
    buf = ""
  }'
offenders_of() { awk "$scan" "$@"; }
kind() { grep "^$1"$'\t' | cut -f2- | sed 's/^/    /'; }

samples="$(mktemp -d)"
trap 'rm -rf "$samples"' EXIT
# The forms the scan has to tell apart. Only the first and the last pass.
cat > "$samples/forms.md" <<'EOF'
tmux new-session -d -s ok "cd /r && $(amplihack agent-binary --shell -w /r) amplihack recipe run x"
tmux new-session -d -s single 'cd /r && $(amplihack agent-binary --shell -w /r) amplihack recipe run x'
tmux new-session -d -s single-split \
  'cd /r && $(amplihack agent-binary --shell -w /r) \
   amplihack recipe run x'
tmux new-session -d -s bare cd /r \&\& $(amplihack agent-binary --shell -w /r) amplihack recipe run x
tmux new-session -d -s none "cd /r && amplihack recipe run x"
tmux new-session -d -s "recipe-$(date +%s)" \
  "cd /r && it's $(amplihack agent-binary --shell -w /r) amplihack recipe run x"
EOF
found="$(offenders_of "$samples/forms.md")"
quoted_lines="$(printf '%s\n' "$found" | grep -c $'^quoted\t')"
missing_lines="$(printf '%s\n' "$found" | grep -c $'^missing\t')"
if [ "$quoted_lines" -eq 3 ] && [ "$missing_lines" -eq 1 ] \
  && ! printf '%s\n' "$found" | grep -q -e ' -s ok ' -e ' -s "recipe-'; then
  pass "the scan flags single-quoted, unquoted and missing hand-offs and passes double-quoted ones"
else
  fail "the scan does not tell the hand-off forms apart (expected 3 quoted and 1 missing):
$found"
fi

shipped=()
while IFS= read -r file; do
  shipped+=("$file")
done < <(git ls-files 'amplifier-bundle/*.md' 'amplifier-bundle/*.sh' \
  'amplifier-bundle/*.yaml' 'amplifier-bundle/*.yml' 'docs/*.md')
[ "${#shipped[@]}" -gt 0 ] || { echo "  FAIL  no shipped files found (is this a git checkout?)"; exit 1; }
found="$(offenders_of "${shipped[@]}")"
missing="$(printf '%s\n' "$found" | kind missing)"
quoted="$(printf '%s\n' "$found" | kind quoted)"
# shellcheck disable=SC2126  # grep -c would count per file, not in total
commands="$(grep -h 'tmux new-session' "${shipped[@]}" | wc -l | tr -d ' ')"
if [ -z "$missing" ]; then
  pass "no detached recipe run without the hand-off ($commands tmux new-session line(s) scanned)"
else
  fail "a detached recipe run without \$(amplihack agent-binary --shell -w <repo>):
$missing"
fi
if [ -z "$quoted" ]; then
  pass "every hand-off in a detached recipe run is inside double quotes"
else
  fail "a hand-off outside double quotes runs in the new session and hands over the tmux server's CLI:
$quoted"
fi

# --- 2. the always-loaded context ----------------------------------------
context_lines="$(grep -n -i 'tmux' amplifier-bundle/context/*.md | grep -i 'recipe run')"
if [ -z "$context_lines" ]; then
  pass "session context does not tell agents to start a recipe run in tmux"
else
  context_mentions="$(printf '%s\n' "$context_lines" | wc -l | tr -d ' ')"
  without_hand_off="$(printf '%s\n' "$context_lines" | grep -v 'agent-binary --shell')"
  without_quoting="$(printf '%s\n' "$context_lines" | grep -v -i 'double')"
  if [ -z "$without_hand_off" ]; then
    pass "session context: every tmux recipe-run instruction carries the hand-off ($context_mentions line(s))"
  else
    fail "session context tells agents to start a recipe run in tmux without the hand-off:
$without_hand_off"
  fi
  if [ -z "$without_quoting" ]; then
    pass "session context: every such instruction says the hand-off goes in double quotes"
  else
    fail "session context gives the hand-off without saying it goes in a double-quoted command:
$without_quoting"
  fi
fi

if [ "$fails" -ne 0 ]; then
  echo "issue #1525 detached launch hand-off: $fails failure(s)"
  exit 1
fi
echo "issue #1525 detached launch hand-off: all checks passed"
