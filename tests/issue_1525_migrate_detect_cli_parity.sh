#!/usr/bin/env bash
# Issue #1525 — migrate.sh `detect_cli` must agree with the Rust resolver
# (crates/amplihack-utils/src/agent_binary.rs), which is authoritative.
#
# detect_cli keeps three layers in shell: AMPLIHACK_AGENT_BINARY (its tag rule
# is pinned by tests/issue_1481_migrate_detect_cli_default_tag.sh), the session
# markers, and the parent process chain, which the resolver does not have.
# Everything below them -- launcher_context.json and the copilot default -- it
# asks `amplihack agent-binary`, the resolver itself.
#
# The crusty review of #1490 found that layer re-implemented in about 235
# lines of bash (an RFC 3339 parser, the trust checks, BSD-portable stand-ins
# for GNU tools), kept equal to the Rust copy by a 415-line suite. There is now
# one copy, so this suite checks only what the shell still owns:
#
#   * the shell's marker list is agent_binary::SESSION_MARKERS, in order;
#   * a marker, and the parent process chain, answer before amplihack is asked;
#   * otherwise detect_cli takes the first word of `amplihack agent-binary`,
#     run from the cwd, and leaves its stderr -- where the resolver names each
#     launcher context it could not use -- on the user's terminal;
#   * without amplihack, or when it fails or names no agent CLI, a warning
#     says no launcher context was read, and the answer is copilot.
#
# A stand-in `amplihack` plays the resolver here, because this CI job does not
# build the binary. bins/amplihack/tests/issue_1525_migrate_detect_cli_uses_the_resolver.rs
# runs detect_cli against the real one.

set -uo pipefail

SCRIPT="amplifier-bundle/skills/migrate/scripts/migrate.sh"
RESOLVER="crates/amplihack-utils/src/agent_binary.rs"
[ -f "$SCRIPT" ] || { echo "missing $SCRIPT (run from repo root)"; exit 1; }
[ -f "$RESOLVER" ] || { echo "missing $RESOLVER (run from repo root)"; exit 1; }

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

# --- the launcher-context layer is not re-implemented ---------------------
# The resolver's walk-up, freshness bound and file checks live in Rust. A
# shell copy of any of them is the drift this suite used to exist to catch.
copies="$(awk '/^detect_cli\(\) \{/,/^\}/' "$SCRIPT" \
  | grep -v '^ *#' | grep -nE 'launcher_context\.json|jq |date |find |readlink' || true)"
if [ -z "$copies" ]; then
  pass "detect_cli reads no launcher context itself"
else
  fail "detect_cli reads launcher contexts in shell again; ask amplihack agent-binary:
$copies"
fi

root="$(mktemp -d)"
trap 'rm -rf "$root"' EXIT
work="$root/work"
mkdir -p "$work/.git" "$root/empty"

# stand_in <stdout> <exit> [stderr]: an `amplihack` on PATH that records its
# arguments and cwd in $root/called, prints <stdout> and [stderr], and exits
# with <exit>. The texts are kept in files, so no quoting reaches the stub.
stand_in() {
  mkdir -p "$root/bin" "$root/stub"
  printf '%s' "$1" > "$root/stub/stdout"
  printf '%s' "$2" > "$root/stub/exit"
  printf '%s' "${3:-}" > "$root/stub/stderr"
  cat > "$root/bin/amplihack" <<STUB
#!/bin/sh
printf '%s|%s\n' "\$*" "\$(pwd -P)" > "$root/called"
[ -s "$root/stub/stderr" ] && { cat "$root/stub/stderr"; echo; } >&2
[ -s "$root/stub/stdout" ] && { cat "$root/stub/stdout"; echo; }
exit "\$(cat "$root/stub/exit")"
STUB
  chmod +x "$root/bin/amplihack"
}

# run <path> [VAR=value ...]: detect_cli from $work with no markers, no agent
# binary variables and no agent CLI in the process chain, plus the given
# variables, with <path> as PATH. Sets $out, $err and $called.
run() {
  local path="$1"; shift
  local errfile="$root/stderr"
  rm -f "$root/called"
  out="$(
    exec 2>"$errfile"
    cd "$work" || exit 1
    for entry in $shell_markers; do unset "${entry%%:*}"; done
    unset AMPLIHACK_AGENT_BINARY AMPLIHACK_AGENT_BINARY_SOURCE
    ps() { :; }
    PATH="$path"
    for assignment in "$@"; do export "${assignment?}"; done
    detect_cli
  )"
  err="$(cat "$errfile")"
  called="$(cat "$root/called" 2>/dev/null || true)"
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

expect_not_asked() {
  if [ -z "$called" ]; then pass "$1: amplihack not asked"; else fail "$1: amplihack was asked ($called)"; fi
}

host_path="$root/bin:$PATH"

# --- markers and the process chain answer first ---------------------------
stand_in "codex (launcher_context)" 0
for entry in $shell_markers; do
  run "$host_path" "${entry%%:*}=1"
  expect "marker ${entry%%:*} alone" "${entry##*:}"
  expect_not_asked "marker ${entry%%:*}"
done
run "$host_path" CLAUDECODE=
expect "an empty marker is no marker" codex

out="$(
  cd "$work" || exit 1
  for entry in $shell_markers; do unset "${entry%%:*}"; done
  unset AMPLIHACK_AGENT_BINARY AMPLIHACK_AGENT_BINARY_SOURCE
  rm -f "$root/called"
  # A Claude Code process directly above this shell.
  ps() { case "$*" in (*comm=*) echo claude ;; (*) echo 1 ;; esac; }
  PATH="$host_path"
  detect_cli 2>/dev/null
)"
called="$(cat "$root/called" 2>/dev/null || true)"
expect "a claude parent process answers" claude
expect_not_asked "a claude parent process"

# --- the rest is the resolver's ---------------------------------------------
stand_in "codex (launcher_context)" 0 \
  "amplihack: ignored $work/.claude/runtime/launcher_context.json: it is empty. Fix or delete it."
run "$host_path"
expect "the resolver's launcher-context answer is taken" codex
if [ "$called" = "agent-binary|$(cd "$work" && pwd -P)" ]; then
  pass "amplihack agent-binary is asked from the cwd"
else
  fail "amplihack was asked as '$called', not 'agent-binary|$work'"
fi
expect_warning "the resolver's stderr reaches the user" \
  "amplihack: ignored $work/.claude/runtime/launcher_context.json: it is empty."

stand_in "copilot (default)" 0 \
  "amplihack: resolved the agent binary to 'copilot' (no AMPLIHACK_AGENT_BINARY or agent session marker was found)."
run "$host_path"
expect "the resolver's default is taken" copilot
expect_warning "the resolver's default notice reaches the user" "resolved the agent binary to 'copilot'"

# --- and without a usable resolver, copilot, said aloud --------------------
run "$root/empty"
expect "no amplihack on PATH" copilot
expect_warning "no amplihack on PATH is named" "amplihack not found; no launcher context was read"

# An amplihack from before `agent-binary` existed: clap refuses the
# subcommand and exits 2.
stand_in "" 2 "error: unrecognized subcommand 'agent-binary'"
run "$host_path"
expect "amplihack without the subcommand" copilot
expect_warning "a failed amplihack agent-binary is named" \
  "amplihack agent-binary did not name an agent CLI; no launcher context was read"

stand_in "Installing amplihack..." 0
run "$host_path"
expect "output that names no agent CLI" copilot
expect_warning "output that names no agent CLI is named" \
  "amplihack agent-binary did not name an agent CLI"

if [ "$fails" -ne 0 ]; then
  echo "issue #1525 detect_cli parity: $fails failure(s)"
  exit 1
fi
echo "issue #1525 detect_cli parity: all checks passed"
