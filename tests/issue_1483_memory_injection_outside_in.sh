#!/usr/bin/env bash
# Outside-in check for issue #1483: the UserPromptSubmit hook must inject only
# memories relevant to the prompt, once, with a computed score.
#
# It drives the real `amplihack-hooks` binary the way Claude Code does:
#   1. `session-stop` stores learnings from transcripts, the production path
#      (one `Agent <name>: ` copy per agent, sqlite backend, temp HOME);
#   2. `user-prompt-submit` receives prompts, and its JSON output is checked.
#
# Usage: tests/issue_1483_memory_injection_outside_in.sh [path/to/amplihack-hooks]
# (default: $AMPLIHACK_HOOKS, then target/release, then target/debug).
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

HOOKS="${1:-${AMPLIHACK_HOOKS:-}}"
if [ -z "${HOOKS}" ]; then
  for candidate in "${REPO_ROOT}/target/release/amplihack-hooks" "${REPO_ROOT}/target/debug/amplihack-hooks"; do
    if [ -x "${candidate}" ]; then
      HOOKS="${candidate}"
      break
    fi
  done
fi
if [ -z "${HOOKS}" ] || [ ! -x "${HOOKS}" ]; then
  echo "FAIL: no amplihack-hooks binary (pass a path or set AMPLIHACK_HOOKS)" >&2
  exit 1
fi
# The script works in a temp directory, so the binary's path must not be
# relative to the caller's.
HOOKS="$(cd "$(dirname "${HOOKS}")" && pwd)/$(basename "${HOOKS}")"

WORK="$(mktemp -d)"
trap 'rm -rf "${WORK}"' EXIT
export HOME="${WORK}/home"
mkdir -p "${HOME}"
export AMPLIHACK_MEMORY_BACKEND=sqlite
unset AMPLIHACK_GRAPH_DB_PATH AMPLIHACK_KUZU_DB_PATH
cd "${WORK}"

SESSION="issue-1483-outside-in"
PASS_COUNT=0
FAIL_COUNT=0
pass() { printf '  PASS: %s\n' "$1"; PASS_COUNT=$((PASS_COUNT + 1)); }
fail() { printf '  FAIL: %s\n' "$1"; FAIL_COUNT=$((FAIL_COUNT + 1)); }

# store <transcript file> <agent>: run session-stop on a transcript, storing
# its learning under <agent> (session-stop's explicit `agent_type`).
# A failed or crashed hook is reported with its stderr, never silently.
store() {
  local status=0
  printf '{"hook_event_name":"SessionStop","session_id":"%s","transcript_path":"%s","agent_type":"%s"}' \
    "${SESSION}" "$1" "$2" | "${HOOKS}" session-stop >/dev/null 2>"${WORK}/stderr" || status=$?
  if [ "${status}" -ne 0 ] || grep -q "failed to store" "${WORK}/stderr"; then
    echo "FAIL: session-stop for agent '$2' (exit ${status}):" >&2
    cat "${WORK}/stderr" >&2
    exit 1
  fi
}

# ask <prompt>: run user-prompt-submit and print its JSON output.
ask() {
  local status=0
  printf '{"hook_event_name":"UserPromptSubmit","session_id":"%s","prompt":"%s"}' \
    "${SESSION}" "$1" | "${HOOKS}" user-prompt-submit 2>"${WORK}/stderr" || status=$?
  if [ "${status}" -ne 0 ]; then
    echo "FAIL: user-prompt-submit for '$1' (exit ${status}):" >&2
    cat "${WORK}/stderr" >&2
    exit 1
  fi
}

# count <haystack> <needle>: occurrences of a fixed string.
count() {
  awk -v needle="$2" 'BEGIN { n = 0 } { line = $0; while ((i = index(line, needle)) > 0) { n++; line = substr(line, i + length(needle)) } } END { print n }' <<<"$1"
}

cat >"${WORK}/pong.jsonl" <<'EOF'
{"role":"user","content":"reply with just: pong"}
{"role":"assistant","content":"pong"}
EOF
cat >"${WORK}/auth.jsonl" <<'EOF'
{"role":"user","content":"why did the auth middleware reject expired tokens?"}
{"role":"assistant","content":"The auth middleware rejected expired tokens because the refresh ran after the check."}
EOF
cat >"${WORK}/bin.jsonl" <<'EOF'
{"role":"user","content":"why do the workers crash on startup?"}
{"role":"assistant","content":"The build copies files into the bin directory, and the workers die if it is missing."}
EOF

echo "storing learnings with ${HOOKS} session-stop"
for agent in general analyzer builder reviewer; do
  store "${WORK}/pong.jsonl" "${agent}"
done
for agent in analyzer builder; do
  store "${WORK}/auth.jsonl" "${agent}"
done
store "${WORK}/bin.jsonl" builder

echo "scenario 1: the #1483 smoke-test memory is not injected for an agent prompt"
out="$(ask "/analyze the builder agent output")"
if [ "$(count "${out}" "pong")" -eq 0 ]; then pass "no pong memory"; else fail "pong memory injected: ${out}"; fi
if [ "$(count "${out}" "Relevant Memory")" -eq 0 ]; then pass "no memory section"; else fail "memory section injected: ${out}"; fi
if [ "$(count "${out}" "relevance: 0.00")" -eq 0 ]; then pass "no zero-relevance line"; else fail "zero relevance: ${out}"; fi

echo "scenario 2: a relevant learning stored under two agents is injected once, scored"
out="$(ask "/analyze why the auth middleware rejected expired tokens")"
if [ "$(count "${out}" "## Relevant Memory")" -eq 1 ]; then pass "one memory section"; else fail "memory sections != 1: ${out}"; fi
if [ "$(count "${out}" "The auth middleware rejected expired tokens because")" -eq 1 ]; then pass "relevant memory printed once"; else fail "relevant memory not printed exactly once: ${out}"; fi
if [ "$(count "${out}" "Agent analyzer:")" -eq 0 ] && [ "$(count "${out}" "Agent builder:")" -eq 0 ]; then pass "agent prefix stripped"; else fail "agent prefix printed: ${out}"; fi
if [ "$(count "${out}" "(relevance: ")" -ge 1 ] && [ "$(count "${out}" "relevance: 0.00")" -eq 0 ]; then pass "computed relevance score"; else fail "no computed score: ${out}"; fi
if [ "$(count "${out}" "pong")" -eq 0 ]; then pass "unrelated pong memory left out"; else fail "pong injected: ${out}"; fi

echo "scenario 3: a path segment named like an agent is not an agent"
# Relevant to the auth memory, but it names no agent: `src/builder` is a
# path. The loose pre-#1483 pattern made `/builder ` an agent and injected.
out="$(ask "why the auth middleware in src/builder rejected expired tokens")"
if [ "$(count "${out}" "Relevant Memory")" -eq 0 ]; then pass "no memory without an agent"; else fail "memory injected: ${out}"; fi

echo "scenario 4: a German prompt's function words don't match an English memory"
out="$(ask "/fix ich bin nicht sicher, warum die Tests scheitern")"
if [ "$(count "${out}" "bin directory")" -eq 0 ]; then pass "no memory for a non-English prompt"; else fail "memory injected: ${out}"; fi

echo "scenario 5: an English prompt about that memory still gets it"
out="$(ask "/fix why the workers die when the bin directory is missing")"
if [ "$(count "${out}" "bin directory, and the workers die")" -eq 1 ]; then pass "relevant bin memory injected"; else fail "relevant memory missing: ${out}"; fi

echo
echo "passed: ${PASS_COUNT}, failed: ${FAIL_COUNT}"
[ "${FAIL_COUNT}" -eq 0 ]
