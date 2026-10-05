#!/usr/bin/env bash
# Issue #1481 — migrate.sh `detect_cli` must treat an AMPLIHACK_AGENT_BINARY
# that a parent tagged `AMPLIHACK_AGENT_BINARY_SOURCE=default:<binary>` as the
# guess it is, and fall through to the next layer, as the Rust resolver
# (`agent_binary::is_default_guess`) does. Any other value is an instruction.
# The other layers are covered by tests/issue_1525_migrate_detect_cli_parity.sh;
# these checks pin the tag rule.
#
# The fall-through is made observable with a stand-in `amplihack` whose
# `agent-binary` answers `codex`, as the resolver does from a fresh launcher
# context naming it: a skipped env value answers `codex`, an honoured one
# answers itself. Session markers and the parent process chain rank above it,
# so both are neutralised; otherwise, run inside Claude Code, every skipped
# value would answer `claude`.

set -uo pipefail

SCRIPT="amplifier-bundle/skills/migrate/scripts/migrate.sh"
[ -f "$SCRIPT" ] || { echo "missing $SCRIPT (run from repo root)"; exit 1; }

fails=0
pass() { printf '  ok    %s\n' "$1"; }
fail() { printf '  FAIL  %s\n' "$1"; fails=$((fails + 1)); }

# detect_cli and its helpers are defined after the script's library-mode
# short-circuit, so they are lifted out on their own, with log_warn. They read
# only the environment, ps, and what `amplihack agent-binary` prints.
eval "$(awk '/^log_warn\(\)/' "$SCRIPT")"
eval "$(awk '/^_?detect_cli[a-z_]*\(\) \{/,/^\}/' "$SCRIPT")"
if ! declare -F detect_cli >/dev/null; then
  echo "  FAIL  detect_cli not defined"
  exit 1
fi
# The parent process chain: report no agent CLI anywhere above this shell.
ps() { :; }
for marker in CLAUDECODE CLAUDE_CODE CLAUDE_CODE_SESSION_ID CLAUDE_PROJECT_DIR \
              CLAUDE_CODE_ENTRYPOINT COPILOT_CLI GITHUB_COPILOT GITHUB_COPILOT_AGENT \
              COPILOT_AGENT; do
  unset "$marker"
done

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/.git" "$work/bin"
printf '#!/bin/sh\necho "codex (launcher_context)"\n' > "$work/bin/amplihack"
chmod +x "$work/bin/amplihack"
PATH="$work/bin:$PATH"

# check <description> <expected> <binary> <tag|-unset->
check() {
  local desc="$1" expected="$2" binary="$3" tag="$4" got
  got="$(
    cd "$work" || exit 1
    export AMPLIHACK_AGENT_BINARY="$binary"
    if [ "$tag" = "-unset-" ]; then
      unset AMPLIHACK_AGENT_BINARY_SOURCE
    else
      export AMPLIHACK_AGENT_BINARY_SOURCE="$tag"
    fi
    detect_cli
  )"
  if [ "$got" = "$expected" ]; then
    pass "$desc -> $got"
  else
    fail "$desc: expected $expected, got $got"
  fi
}

check "untagged value is an instruction"            copilot copilot   -unset-
check "tag naming the value is a guess"             codex   copilot   "default:copilot"
check "value is normalised before the comparison"   codex   " Copilot " "default:copilot"
check "tag's surrounding whitespace is trimmed"     codex   copilot   " default:copilot "
check "tag naming another binary does not veto"     claude  claude    "default:copilot"
check "bare 'default' is not a guess"               copilot copilot   "default"
check "internal whitespace is not trimmed"          copilot copilot   "default: copilot"
check "tag comparison is case-sensitive"            copilot copilot   "DEFAULT:copilot"
check "empty tag is not a guess"                    copilot copilot   ""

if [ "$fails" -ne 0 ]; then
  echo "issue #1481 detect_cli default-tag: $fails failure(s)"
  exit 1
fi
echo "issue #1481 detect_cli default-tag: all checks passed"
