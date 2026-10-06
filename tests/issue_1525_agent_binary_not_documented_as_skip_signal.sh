#!/usr/bin/env bash
# Issue #1525: none of the sentences that listed AMPLIHACK_AGENT_BINARY among
# the signals that skip the startup update check, or make a launch
# subprocess-safe, may come back.
#
# #625 made it one; #1525 removed it from `classify_skip_reason` and
# `resolve_subprocess_safe`, because it names an agent CLI, and a user who
# exports it to choose one is still at a terminal. The crusty review of #1490
# then found five places still saying otherwise, among them the reference page
# an operator reads before exporting a host-wide default. b5a13757 corrected
# them. Each is matched below as a fixed string, not a pattern; the list says
# where each one was.
#
# The behaviour itself is pinned by
# bins/amplihack/tests/issue_1525_agent_binary_is_not_subprocess_safe.rs and
# `agent_binary_env_is_not_a_skip_signal` in
# crates/amplihack-cli/src/update/tests/classify_skip_reason.rs. This script
# only keeps the five sentences from returning through a revert, a rebase of
# an older branch or a copy from an old page.
#
# Until the crusty review of #1490 at 351b4c1e, this place held a regex over
# prose: any list putting AGENT_BINARY next to NONINTERACTIVE or CI, with `/`,
# `,`, "or" or "and" between them, within two lines of any Markdown file or
# Rust comment. It rejected correct text, for instance:
#
#   `amplihack recipe run` exports `AMPLIHACK_AGENT_BINARY` and
#   `AMPLIHACK_NONINTERACTIVE=1` to the runner.
#
# which is what #1490 itself says about the runner's environment. A pattern
# cannot tell a sentence that lists the two variables as skip signals from one
# that lists them as what a process inherits, the same lesson
# tests/issue_1525_detached_launch_docs_carry_hand_off.sh records. Its
# self-test holds this list to both sides: it must find each retired line as
# b5a13757's parent had it, and must pass the sentences the regex rejected.

set -uo pipefail

[ -d docs ] && [ -d crates ] || { echo "run from the repo root"; exit 1; }

fails=0
pass() { printf '  ok    %s\n' "$1"; }
fail() { printf '  FAIL  %s\n' "$1"; fails=$((fails + 1)); }

# One fixed string per retired sentence, each short enough to sit on one line
# of the text it came from, and long enough to name that sentence rather than
# any line mentioning both variables. No entry may be empty: grep -F -f treats
# an empty line as a pattern that matches everything.
# shellcheck disable=SC2016  # the backticks are Markdown, matched literally
retired=(
  # SkipReason::SubprocessSafe doc comment (crates/amplihack-cli/src/update/check.rs).
  'Subprocess-safe skip: any of NONINTERACTIVE / AGENT_BINARY (non-empty)'
  # docs/features/README.md, the Startup Self-Update Prompt entry.
  'or when `AMPLIHACK_NONINTERACTIVE` / `AMPLIHACK_AGENT_BINARY` is set; emits a single skip-line'
  # docs/howto/manage-tool-update-checks.md, the note pointing at that page.
  '`AMPLIHACK_AGENT_BINARY` / `CI` is set, see [Startup Self-Update Prompt'
  # docs/reference/environment-variables.md, the AMPLIHACK_NO_UPDATE_CHECK
  # entry contrasting itself with the skip signals.
  'the subprocess-safe skip signals (`CI`, `AMPLIHACK_AGENT_BINARY`,'
  # docs/reference/environment-variables.md, the "see also" link to that page.
  'How `CI`, `AMPLIHACK_AGENT_BINARY`, `AMPLIHACK_NONINTERACTIVE`, `--subprocess-safe`, and non-TTY stdin each suppress'
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

# --- the list finds the retired lines and nothing else ---------------------
# The lines as b5a13757's parent had them, verbatim, one per entry above.
cat > "$samples/stale.txt" <<'EOF'
    /// Subprocess-safe skip: any of NONINTERACTIVE / AGENT_BINARY (non-empty)
- [Startup Self-Update Prompt — Subprocess-Safe Skip](startup-update-prompt-subprocess-safe.md) — startup self-update prompt skips automatically in CI, delegated agents, non-TTY stdin, with `--subprocess-safe`, or when `AMPLIHACK_NONINTERACTIVE` / `AMPLIHACK_AGENT_BINARY` is set; emits a single skip-line to stderr (issue [#625](https://github.com/rysweet/amplihack-rs/issues/625)).
> `AMPLIHACK_AGENT_BINARY` / `CI` is set, see [Startup Self-Update Prompt —
the subprocess-safe skip signals (`CI`, `AMPLIHACK_AGENT_BINARY`,
- [Startup Self-Update Prompt — Subprocess-Safe Skip](../features/startup-update-prompt-subprocess-safe.md) — How `CI`, `AMPLIHACK_AGENT_BINARY`, `AMPLIHACK_NONINTERACTIVE`, `--subprocess-safe`, and non-TTY stdin each suppress the `Update now? [y/N] (5s timeout):` prompt
EOF
# Correct sentences: the three the regex rejected, then each retired line as
# b5a13757 corrected it, then a table and a code line that name both.
cat > "$samples/fine.txt" <<'EOF'
`amplihack recipe run` exports `AMPLIHACK_AGENT_BINARY` and `AMPLIHACK_NONINTERACTIVE=1` to the runner.
Before #1525 the skip fired on `AMPLIHACK_NONINTERACTIVE` or `AMPLIHACK_AGENT_BINARY`; now only the former counts.
// Every step inherits AMPLIHACK_AGENT_BINARY and AMPLIHACK_NONINTERACTIVE from recipe run.
    /// Subprocess-safe skip: `AMPLIHACK_NONINTERACTIVE` (non-empty) / `CI`
    /// (non-empty) / `--subprocess-safe` argv token. `AMPLIHACK_AGENT_BINARY`
- [Startup Self-Update Prompt — Subprocess-Safe Skip](startup-update-prompt-subprocess-safe.md) — startup self-update prompt skips automatically in CI, delegated agents, non-TTY stdin, with `--subprocess-safe`, or when `AMPLIHACK_NONINTERACTIVE` is set; emits a single skip-line to stderr (issue [#625](https://github.com/rysweet/amplihack-rs/issues/625)). `AMPLIHACK_AGENT_BINARY` names the agent CLI and does not suppress it (issue [#1525](https://github.com/rysweet/amplihack-rs/issues/1525)).
> `--subprocess-safe`, or when `AMPLIHACK_NONINTERACTIVE` / `CI` is set, see
> `AMPLIHACK_AGENT_BINARY` is not one of these signals (issue
the subprocess-safe skip signals (`CI`, `AMPLIHACK_NONINTERACTIVE`,
- [Startup Self-Update Prompt — Subprocess-Safe Skip](../features/startup-update-prompt-subprocess-safe.md) — How `CI`, `AMPLIHACK_NONINTERACTIVE`, `--subprocess-safe`, and non-TTY stdin each suppress the `Update now? [y/N] (5s timeout):` prompt, and why `AMPLIHACK_AGENT_BINARY` does not (#1525)
| `AMPLIHACK_NONINTERACTIVE` | If `=1` → triggers subprocess-safe context. |
| `AMPLIHACK_AGENT_BINARY` | **No effect** on subprocess-safe (#1525). |
    let s = "printf '%s' \"$AMPLIHACK_AGENT_BINARY|$AMPLIHACK_NONINTERACTIVE\"";
EOF
missed=""
for phrase in "${retired[@]}"; do
  grep -qF -- "$phrase" "$samples/stale.txt" || missed+="    $phrase"$'\n'
done
stale_lines="$(retired_in "$samples/stale.txt" | wc -l | tr -d ' ')"
if [ -z "$missed" ] && [ "$stale_lines" -eq "${#retired[@]}" ]; then
  pass "the list finds each of the ${#retired[@]} retired lines as b5a13757's parent had them"
else
  fail "the retired list does not match the old text (found $stale_lines of ${#retired[@]} lines); entries not found:
${missed%$'\n'}"
fi
false_hits="$(retired_in "$samples/fine.txt")"
if [ -z "$false_hits" ]; then
  pass "the list passes over correct sentences naming both variables"
else
  fail "the retired list flagged text that is correct:
$false_hits"
fi

# --- shipped text ---------------------------------------------------------
shipped=()
while IFS= read -r file; do
  shipped+=("$file")
done < <(git ls-files '*.md' 'crates/*.rs' 'bins/*.rs')
[ "${#shipped[@]}" -gt 0 ] || { echo "  FAIL  no shipped files found (is this a git checkout?)"; exit 1; }
offenders="$(retired_in "${shipped[@]}")"
if [ -z "$offenders" ]; then
  pass "none of the ${#retired[@]} sentences b5a13757 corrected is back (${#shipped[@]} files)"
else
  fail "a sentence b5a13757 corrected is back. AMPLIHACK_AGENT_BINARY names an agent CLI and has not skipped the update check or made a launch subprocess-safe since #1525; AMPLIHACK_NONINTERACTIVE, CI, --subprocess-safe and non-TTY stdin do:
$offenders"
fi

if [ "$fails" -ne 0 ]; then
  echo "issue #1525 AMPLIHACK_AGENT_BINARY is not a skip signal: $fails failure(s)"
  exit 1
fi
echo "issue #1525 AMPLIHACK_AGENT_BINARY is not a skip signal: all checks passed"
