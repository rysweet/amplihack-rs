#!/usr/bin/env bash
# Issue #1525 — migrate.sh `detect_cli` must agree with the Rust resolver
# (crates/amplihack-utils/src/agent_binary.rs), which is authoritative:
#
#   * a session marker outranks any launcher_context.json, and the marker list
#     is agent_binary::SESSION_MARKERS, in the same order;
#   * a launcher context counts only while fresh (24h); one without a
#     timestamp is stale;
#   * the walk-up stops at a .git boundary and at a world-writable directory;
#   * an unusable file (empty, not JSON, wrong shape, unknown launcher) is
#     walked past, but named on stderr with the reason;
#   * the value is trimmed, not stripped: inner whitespace and control
#     characters reject it.
#
# Before #1525, detect_cli read launcher_context.json ahead of any session
# evidence and had no staleness bound. Inside Claude Code, a days-old
# `launcher: copilot` file made migrate resume `copilot --resume <id>`.
#
# The shell's one extra layer, the parent process chain, is stubbed out here
# except where it is under test.

set -uo pipefail

SCRIPT="amplifier-bundle/skills/migrate/scripts/migrate.sh"
RESOLVER="crates/amplihack-utils/src/agent_binary.rs"
[ -f "$SCRIPT" ] || { echo "missing $SCRIPT (run from repo root)"; exit 1; }
[ -f "$RESOLVER" ] || { echo "missing $RESOLVER (run from repo root)"; exit 1; }
command -v jq >/dev/null || { echo "  FAIL  jq is required by detect_cli's launcher-context layer"; exit 1; }

fails=0
pass() { printf '  ok    %s\n' "$1"; }
fail() { printf '  FAIL  %s\n' "$1"; fails=$((fails + 1)); }

eval "$(awk '/^log_warn\(\)/' "$SCRIPT")"
eval "$(awk '/^_?detect_cli[a-z_]*\(\) \{/,/^\}/' "$SCRIPT")"
declare -F detect_cli >/dev/null || { echo "  FAIL  detect_cli not defined"; exit 1; }

# --- the marker list must be SESSION_MARKERS, entry for entry -------------
rust_markers="$(awk '/pub const SESSION_MARKERS/,/^\];/' "$RESOLVER" \
  | sed -n 's/^ *("\([A-Z_]*\)", "\([a-z]*\)"),.*/\1:\2/p')"
shell_markers="$(awk '/local -a session_markers=\(/,/^ *\)$/' "$SCRIPT" \
  | sed -n 's/^ *\([A-Z_]*:[a-z]*\) *$/\1/p')"
if [ -n "$rust_markers" ] && [ "$rust_markers" = "$shell_markers" ]; then
  pass "shell marker list is SESSION_MARKERS ($(printf '%s\n' "$rust_markers" | wc -l | tr -d ' ') entries, same order)"
else
  fail "shell marker list differs from SESSION_MARKERS:
rust:
$rust_markers
shell:
$shell_markers"
fi

root="$(mktemp -d)"
trap 'chmod -R u+rwx "$root" 2>/dev/null; rm -rf "$root"' EXIT
now="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
old="$(date -u -d '3 days ago' +%Y-%m-%dT%H:%M:%SZ)"

# fixture <dir> <body>: a project dir (with a .git boundary unless <dir> ends
# in /nogit) holding <body> as its launcher context ("" for an empty file,
# "-" for none).
fixture() {
  local dir="${1%/nogit}" body="$2"
  mkdir -p "$dir"
  [ "$dir" = "$1" ] && mkdir -p "$dir/.git"
  if [ "$body" != "-" ]; then
    mkdir -p "$dir/.claude/runtime"
    printf '%s' "$body" > "$dir/.claude/runtime/launcher_context.json"
  fi
}

# run <dir> [VAR=value ...]: detect_cli from <dir> with no markers, no agent
# binary variables and no agent CLI in the process chain, plus the given
# variables. Sets $out and $err.
run() {
  local dir="$1"; shift
  local errfile="$root/stderr"
  out="$(
    exec 2>"$errfile"
    cd "$dir" || exit 1
    for entry in $shell_markers; do unset "${entry%%:*}"; done
    unset AMPLIHACK_AGENT_BINARY AMPLIHACK_AGENT_BINARY_SOURCE
    ps() { :; }
    for assignment in "$@"; do export "${assignment?}"; done
    detect_cli
  )"
  err="$(cat "$errfile")"
}

expect() {
  local desc="$1" want="$2"
  if [ "$out" = "$want" ]; then pass "$desc -> $out"; else fail "$desc: expected $want, got $out (stderr: $err)"; fi
}

expect_warning() {
  local desc="$1" want="$2"
  case "$err" in
    *"$want"*) pass "$desc" ;;
    *) fail "$desc: stderr lacks '$want': $err" ;;
  esac
}

# --- the marker layer ranks above a fresh file ----------------------------
fixture "$root/m" "{\"launcher\":\"copilot\",\"timestamp\":\"$now\"}"
run "$root/m" CLAUDE_CODE_ENTRYPOINT=cli
expect "a Claude marker beats a fresh copilot file" claude
for entry in $shell_markers; do
  run "$root/m" "${entry%%:*}=1"
  expect "marker ${entry%%:*} alone" "${entry##*:}"
done
run "$root/m" CLAUDECODE=
expect "an empty marker is no marker" copilot

# --- freshness ------------------------------------------------------------
fixture "$root/fresh" "{\"launcher\":\"claude\",\"timestamp\":\"$now\"}"
run "$root/fresh"
expect "a fresh file answers" claude
fixture "$root/stale" "{\"launcher\":\"claude\",\"timestamp\":\"$old\"}"
run "$root/stale"
expect "a three-day-old file is ignored" copilot
[ -z "$err" ] && pass "a stale file is not reported as broken" || fail "stale file warned: $err"
fixture "$root/notime" '{"launcher":"claude"}'
run "$root/notime"
expect "a file without a timestamp is stale" copilot

# --- unusable files are named, and walked past ----------------------------
fixture "$root/empty" ""
run "$root/empty"
expect "an empty file falls through" copilot
expect_warning "an empty file is named with its reason" \
  "ignored $root/empty/.claude/runtime/launcher_context.json: it is empty."
fixture "$root/badjson" '{"launcher": "claude"'
run "$root/badjson"
expect "invalid JSON falls through" copilot
expect_warning "invalid JSON is named with its reason" "it is not valid JSON."
fixture "$root/shape" '{"launcher": 5}'
run "$root/shape"
expect_warning "a wrong shape is named with its reason" "it is JSON but not a launcher context"
fixture "$root/vim" "{\"launcher\":\"vim\",\"timestamp\":\"$now\"}"
run "$root/vim"
expect_warning "an unknown launcher is named" "it does not name amplifier, claude, codex or copilot"
fixture "$root/ctl" "{\"launcher\":\"claude\\n\",\"timestamp\":\"$now\"}"
run "$root/ctl"
expect "a launcher with a control character is rejected, as in Rust" copilot

fixture "$root/walk" "{\"launcher\":\"codex\",\"timestamp\":\"$now\"}"
fixture "$root/walk/sub/nogit" ""
run "$root/walk/sub"
expect "a bad file in a subdirectory lets the project's file answer" codex
expect_warning "...and the bad file is still named" \
  "ignored $root/walk/sub/.claude/runtime/launcher_context.json: it is empty."

# --- walk-up boundaries ---------------------------------------------------
fixture "$root/above" "{\"launcher\":\"claude\",\"timestamp\":\"$now\"}"
rm -rf "$root/above/.git"
fixture "$root/above/repo" "-"
run "$root/above/repo"
expect "the walk-up stops at a .git boundary" copilot
fixture "$root/shared/nogit" "{\"launcher\":\"claude\",\"timestamp\":\"$now\"}"
chmod 1777 "$root/shared"
fixture "$root/shared/work/nogit" "-"
chmod 700 "$root/shared/work"
run "$root/shared/work"
expect "the walk-up stops at a world-writable directory" copilot

# --- the environment variable is trimmed, not stripped --------------------
run "$root/fresh" "AMPLIHACK_AGENT_BINARY=  Codex  "
expect "surrounding spaces are trimmed" codex
run "$root/fresh" "AMPLIHACK_AGENT_BINARY=co dex"
expect "inner whitespace rejects the value, as in Rust" claude

# --- the process chain still ranks above the file -------------------------
out="$(
  cd "$root/m" || exit 1
  for entry in $shell_markers; do unset "${entry%%:*}"; done
  unset AMPLIHACK_AGENT_BINARY AMPLIHACK_AGENT_BINARY_SOURCE
  ps() { case "$*" in *comm=*) echo claude ;; *) echo 1 ;; esac; }
  detect_cli
)"
err=""
expect "a claude ancestor beats a fresh copilot file" claude

if [ "$fails" -ne 0 ]; then
  echo "issue #1525 detect_cli parity: $fails failure(s)"
  exit 1
fi
echo "issue #1525 detect_cli parity: all checks passed"
