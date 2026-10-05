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
#   3. None of the sentences that 7ca7b182 corrected comes back, in the
#      shipped docs, the subcommand's --help text or its module doc. Each is
#      matched as a fixed string, not a pattern; see the list below for what
#      each one got wrong.
#
# Until the crusty review of #1490 at 1fbe05f3, this place held two regex
# checks over prose. One failed any line pairing AMPLIHACK_AGENT_BINARY= with
# AMPLIHACK_AGENT_BINARY_SOURCE= unless `env -u` was on it or the line before;
# the other failed any line naming setsid or nohup within a line of a word such
# as "lose", unless a word from a short list sat in the same window. Both
# rejected correct text: "Unlike tmux, `nohup` does not lose the caller's
# environment.", the line after it by proximity alone, and a sentence saying
# what the far side holds after the hand-off. A pattern cannot tell a sentence
# that says setsid loses the environment from one that says it does not, so
# check 3 names the retired sentences and nothing else. Its self-test holds it
# to both sides: it must find each retired line as 665fdaf3 had it, and must
# pass the sentences the regexes rejected.

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

# --- 3. the wording 7ca7b182 corrected -----------------------------------
help_text=crates/amplihack-cli/src/cli_commands.rs
module_doc=crates/amplihack-cli/src/commands/agent_binary.rs
for file in "$help_text" "$module_doc"; do
  [ -f "$file" ] || { echo "  FAIL  $file not found"; exit 1; }
done
# One fixed string per retired sentence, each short enough to sit on one line
# of the text it came from. No entry may be empty: grep -F -f treats an empty
# line as a pattern that matches everything.
# shellcheck disable=SC2016  # the backticks are Markdown, matched literally
retired=(
  # --help text (cli_commands.rs): grouped setsid and nohup with tmux as
  # launches that do not inherit the caller's environment.
  'a detached launch (tmux, setsid, nohup)'
  # Module doc (commands/agent_binary.rs): said the line is for any process
  # that does not inherit the shell's environment, not for tmux new-session.
  'handed to a process that will not inherit'
  # docs/concepts/agent-binary-routing.md: listed setsid among the boundaries
  # the environment does not cross.
  'detached background processes (`setsid`, daemonized hooks)'
  # docs/reference/active-agent-binary.md: same claim, "why file-based" list.
  'Detached background processes started via `setsid` may inherit a stale or stripped env.'
  # docs/reference/environment-variables.md: grouped setsid with tmux.
  "A detached launch (tmux, setsid) does not see the caller's session markers"
  # docs/reference/cli.md: showed the --shell line without its env -u prefix,
  # which is what removes a tmux server's stale markers; a reader who rebuilt
  # the line from it, for instance with `tmux new-session -e`, lost to the
  # server's COPILOT_CLI.
  '`--shell` prints `AMPLIHACK_AGENT_BINARY=… AMPLIHACK_AGENT_BINARY_SOURCE=…`'
)
patterns="$(mktemp)"
samples="$(mktemp -d)"
trap 'rm -rf "$patterns" "$samples"' EXIT
for phrase in "${retired[@]}"; do
  [ -n "$phrase" ] || { echo "  FAIL  empty entry in the retired list"; exit 1; }
  printf '%s\n' "$phrase"
done > "$patterns"

# retired_in <file...>: print file:line: text for each line holding a retired
# sentence.
retired_in() {
  grep -nHF -f "$patterns" "$@" | sed 's/^/    /'
}

# The lines as 665fdaf3 had them, verbatim, one per entry above.
cat > "$samples/stale.txt" <<'EOF'
    /// prefix for a detached launch (tmux, setsid, nohup) that will not
//! from, so it can be handed to a process that will not inherit this shell's
- detached background processes (`setsid`, daemonized hooks)
- Detached background processes started via `setsid` may inherit a stale or stripped env.
A detached launch (tmux, setsid) does not see the caller's session markers,
| `agent-binary` | Print the agent CLI that agent steps would run under, and why. `--shell` prints `AMPLIHACK_AGENT_BINARY=… AMPLIHACK_AGENT_BINARY_SOURCE=…` for handing to a detached launch; see [Active Agent Binary](./active-agent-binary.md#handing-the-binary-to-a-detached-launch). |
EOF
# Correct sentences, including the three the earlier regexes rejected.
cat > "$samples/fine.txt" <<'EOF'
Unlike tmux, `nohup` does not lose the caller's environment.
A run started with setsid sees the same markers as its caller.
After the hand-off the far side holds AMPLIHACK_AGENT_BINARY=claude AMPLIHACK_AGENT_BINARY_SOURCE= and no session marker.
`setsid` and `nohup` keep this shell's environment and need no prefix.
`--shell` prints `env -u <each session marker> AMPLIHACK_AGENT_BINARY=<cli> AMPLIHACK_AGENT_BINARY_SOURCE=<tag>`
EOF
missed=""
for phrase in "${retired[@]}"; do
  grep -qF -- "$phrase" "$samples/stale.txt" || missed+="    $phrase"$'\n'
done
stale_lines="$(retired_in "$samples/stale.txt" | wc -l | tr -d ' ')"
if [ -z "$missed" ] && [ "$stale_lines" -eq "${#retired[@]}" ]; then
  pass "the list finds each of the ${#retired[@]} retired lines as 665fdaf3 had them"
else
  fail "the retired list does not match the old text (found $stale_lines of ${#retired[@]} lines); entries not found:
${missed%$'\n'}"
fi
false_hits="$(retired_in "$samples/fine.txt")"
if [ -z "$false_hits" ]; then
  pass "the list passes over correct sentences about setsid, nohup and the --shell line"
else
  fail "the retired list flagged text that is correct:
$false_hits"
fi

retired_offenders="$(retired_in "${shipped[@]}" "$help_text" "$module_doc")"
if [ -z "$retired_offenders" ]; then
  pass "none of the ${#retired[@]} sentences 7ca7b182 corrected is back ($((${#shipped[@]} + 2)) files)"
else
  fail "a sentence 7ca7b182 corrected is back. setsid and nohup pass the caller's environment on unchanged and only tmux new-session replaces it (setsid(1), nohup(1)); the --shell line starts with env -u:
$retired_offenders"
fi

if [ "$fails" -ne 0 ]; then
  echo "issue #1525 detached launch hand-off: $fails failure(s)"
  exit 1
fi
echo "issue #1525 detached launch hand-off: all checks passed"
