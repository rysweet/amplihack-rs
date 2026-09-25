#!/usr/bin/env bash
# Issue #1481 — migrate.sh `detect_cli` must treat an AMPLIHACK_AGENT_BINARY
# that a parent tagged `AMPLIHACK_AGENT_BINARY_SOURCE=default:<binary>` as the
# guess it is, and fall through to the next layer, exactly as the Rust resolver
# (`agent_binary::is_default_guess`) does. Any other value is an instruction.
#
# The fall-through is made observable with a launcher_context.json naming
# `codex`: a skipped env value answers `codex`, an honoured one answers itself.

set -uo pipefail

SCRIPT="amplifier-bundle/skills/migrate/scripts/migrate.sh"
[ -f "$SCRIPT" ] || { echo "missing $SCRIPT (run from repo root)"; exit 1; }
command -v jq >/dev/null || { echo "  FAIL  jq is required by detect_cli's launcher-context layer"; exit 1; }

fails=0
pass() { printf '  ok    %s\n' "$1"; }
fail() { printf '  FAIL  %s\n' "$1"; fails=$((fails + 1)); }

# detect_cli is defined after the script's library-mode short-circuit, so it
# is lifted out on its own. It reads only the environment, the cwd and ps.
detect_cli_src="$(awk '/^detect_cli\(\) \{/,/^\}/' "$SCRIPT")"
eval "$detect_cli_src"
if ! declare -F detect_cli >/dev/null; then
  echo "  FAIL  detect_cli not defined"
  exit 1
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/.git" "$work/.claude/runtime"
printf '{"launcher":"codex"}' > "$work/.claude/runtime/launcher_context.json"

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
