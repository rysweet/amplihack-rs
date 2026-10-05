#!/usr/bin/env bash
# Issue #1525 — every shipped instruction that starts a detached
# `amplihack recipe run` must hand it the agent binary.
#
# `tmux new-session` gives the new command the tmux server's global
# environment, not the caller's, and tmux copied that from whatever started the
# server. The session markers that tell `recipe run` which agent CLI the caller
# is in do not arrive, and the starter's do: the run takes `copilot` from the
# default, or from a server a Copilot session started (#1335, crusty review of
# #1490). The hand-off is `$(amplihack agent-binary --shell -w <repo>)`,
# expanded in the caller's shell; it unsets the far side's markers and sets the
# caller's answer.
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
#   3. Every description of the `agent-binary --shell` line (the pair
#      `AMPLIHACK_AGENT_BINARY=... AMPLIHACK_AGENT_BINARY_SOURCE=...`) in the
#      shipped docs and in the subcommand's --help text shows the `env -u`
#      prefix, on its own line or the one before (a wrapped doc comment). The
#      prefix is what removes a tmux server's stale markers; a reader who
#      rebuilds the line from a description without it, for instance with
#      `tmux new-session -e`, loses to the server's `COPILOT_CLI`. The crusty
#      review of #1490 found docs/reference/cli.md describing the line that way.
#   4. In the --help text and the agent-binary routing docs, no line says
#      `setsid` or `nohup` loses the caller's environment, unless it or a line
#      next to it says they keep it. Both pass the environment on unchanged
#      (setsid(1), nohup(1), execve(2)); only `tmux new-session` replaces it.
#      An agent told otherwise, chasing a wrong-CLI run started with `nohup`,
#      looks for lost variables that were never lost. The crusty review of
#      #1490 found the --help text and environment-variables.md saying so.

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

# --- 3. descriptions of the --shell line ---------------------------------
help_text=crates/amplihack-cli/src/cli_commands.rs
[ -f "$help_text" ] || { echo "  FAIL  $help_text not found"; exit 1; }
pair='AMPLIHACK_AGENT_BINARY=[^ ]* +AMPLIHACK_AGENT_BINARY_SOURCE='
shell_offenders="$(awk -v pair="$pair" '
  FNR == 1 { prev = "" }
  {
    if ($0 ~ pair && (prev " " $0) !~ /env -u/)
      print "    " FILENAME ":" FNR ": " $0
    prev = $0
  }' "${shipped[@]}" "$help_text")"
# shellcheck disable=SC2126  # grep -c would count per file, not in total
descriptions="$(grep -hE "$pair" "${shipped[@]}" "$help_text" | wc -l | tr -d ' ')"
if [ "$descriptions" -eq 0 ]; then
  fail "no description of the agent-binary --shell line found in the docs or $help_text"
elif [ -z "$shell_offenders" ]; then
  pass "every description of the --shell line shows the env -u prefix ($descriptions line(s))"
else
  fail "a description of the agent-binary --shell line without its env -u prefix:
$shell_offenders"
fi

# --- 4. what setsid and nohup do -----------------------------------------
# Scope: the --help text, the subcommand's module and every shipped doc about
# agent-binary routing. Elsewhere `nohup` appears for unrelated reasons.
routing_files=("$help_text" crates/amplihack-cli/src/commands/agent_binary.rs)
while IFS= read -r file; do
  routing_files+=("$file")
done < <(grep -lE 'agent-binary|AMPLIHACK_AGENT_BINARY' "${shipped[@]}")
# "detached launch" and "detached background" count as a loss: the old text
# grouped setsid with tmux under those names as launches that do not see the
# caller's markers. A bare "detached" does not: "a detached `nohup` process"
# (docs/SIGNAL_ONBOARDING.md) is detached from the terminal, which is true.
loss='not inherit|does not see|do not see|lose|lost|stale|stripped|[Dd]etached (launch|background)'
keep='keep|unchanged|do inherit|need no|lose nothing'
launcher_offenders=""
launcher_mentions=0
for file in "${routing_files[@]}"; do
  [ -f "$file" ] || { fail "$file not found"; continue; }
  # Rust comment markers are stripped so a sentence wrapped across `///`
  # lines reads as one when the window joins them.
  found="$(awk -v loss="$loss" -v keep="$keep" '
    {
      text = $0
      sub(/^[ \t]*\/\/[\/!]?[ \t]*/, "", text)
      line[NR] = text
    }
    END {
      for (i = 1; i <= NR; i++) {
        if (line[i] !~ /setsid|nohup/) continue
        print "mention"
        window = line[i - 1] " " line[i] " " line[i + 1]
        if (window ~ loss && window !~ keep)
          print "    " FILENAME ":" i ": " line[i]
      }
    }' "$file")"
  launcher_mentions=$((launcher_mentions + $(printf '%s\n' "$found" | grep -c '^mention$')))
  offenders_here="$(printf '%s\n' "$found" | grep -v '^mention$' | grep -v '^$')"
  [ -n "$offenders_here" ] && launcher_offenders+="$offenders_here"$'\n'
done
if [ -z "$launcher_offenders" ]; then
  pass "setsid and nohup are never said to lose the caller's environment ($launcher_mentions mention(s) in ${#routing_files[@]} file(s))"
else
  fail "setsid and nohup pass the caller's environment on unchanged; only tmux new-session replaces it, but:
${launcher_offenders%$'\n'}"
fi

if [ "$fails" -ne 0 ]; then
  echo "issue #1525 detached launch hand-off: $fails failure(s)"
  exit 1
fi
echo "issue #1525 detached launch hand-off: all checks passed"
