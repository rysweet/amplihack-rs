#!/usr/bin/env bash
# Outside-in scenario for issue #3220: a directory of installed skills and
# agents.
#
# Asserted from outside, against the shipped binary:
#
#   A. `amplihack install --local <this checkout>` into a fresh HOME writes the
#      skills and agents index.
#   B. `amplihack directory find <word>` prints the entry the word should
#      match, as a tab-separated line of type, name, description and path, and
#      the printed path is an existing file.
#   C. Every path that `amplihack directory list` prints is an existing file.
#   D. A word that matches no entry prints nothing on standard output and
#      exits 0.
#   E. Neither subcommand writes anything under HOME.
#
# HOME and AMPLIHACK_HOME are two different fresh folders, so the scenario
# also shows that the command finds the index where the installer wrote it,
# whatever AMPLIHACK_HOME holds. Every command runs under `env -i`, with
# stand-ins for the tools the installer probes on a private PATH. Everything
# is created under one mktemp folder that the EXIT trap removes.
#
# Where it runs: CI's "Install Smoke Test" job (.github/workflows/ci.yml) runs
# this script against the amplihack binary that job installs.
#
# Run by hand:
#   AMPLIHACK_3220_QA_BIN=/abs/path/to/amplihack \
#     bash tests/gadugi/run-issue-3220-directory-scenario.sh
# On success it prints ALL_CASES_PASSED and exits 0; otherwise
# SCENARIO_FAILED and exit 1.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd -P)"

die() {
  echo "FAIL: $*"
  echo "SCENARIO_FAILED"
  exit 1
}

BIN="${AMPLIHACK_3220_QA_BIN:-}"
[ -n "$BIN" ] || die "set AMPLIHACK_3220_QA_BIN to the amplihack binary under test"
case "$BIN" in
  /*) ;;
  *) die "AMPLIHACK_3220_QA_BIN must be an absolute path (got '$BIN')" ;;
esac
[ -f "$BIN" ] && [ -x "$BIN" ] || die "AMPLIHACK_3220_QA_BIN is not an executable file: $BIN"
echo "binary under test: $BIN"

root="$(mktemp -d "${TMPDIR:-/tmp}/issue-3220-qa.XXXXXX")" ||
  die "cannot create a temporary directory"
root="$(cd "$root" && pwd -P)"
chmod 700 "$root"
trap 'rm -rf -- "${root:?}"' EXIT

qa_home="$(mktemp -d "$root/home.XXXXXX")"
qa_amplihack_home="$(mktemp -d "$root/amplihack-home.XXXXXX")"
bin="$root/bin"
mkdir -p "$bin" "$root/tmp" "$root/work"

# Stand-ins for the tools the installer probes. Padded past the installer's
# minimum binary size, as the neighbouring scenarios do.
write_stub() {
  cat > "$bin/$1"
  printf '# %1200s\n' '' >> "$bin/$1"
  chmod 700 "$bin/$1"
}
write_stub claude <<'STUB'
#!/bin/sh
case "$*" in
  *--version*) echo '2.1.319 (Claude Code)' ;;
  *plugin*) echo '{}' ;;
esac
exit 0
STUB
write_stub node <<'STUB'
#!/bin/sh
echo v24.1.0
STUB
write_stub amplihack-hooks <<'STUB'
#!/bin/sh
echo '{}'
STUB
write_stub recipe-runner-rs <<'STUB'
#!/bin/sh
printf '%s\n' '{"schema_version":1,"version":"fixture","capabilities":["codex_exec","durable_recovery_v1"]}'
STUB

CHILD_ENV=(
  "PATH=$bin:/usr/bin:/bin"
  "HOME=$qa_home"
  "AMPLIHACK_HOME=$qa_amplihack_home"
  "TERM=dumb"
  "LANG=C.UTF-8"
  "TMPDIR=$root/tmp"
  "AMPLIHACK_AGENT_BINARY=claude"
  "AMPLIHACK_NONINTERACTIVE=1"
  "AMPLIHACK_NO_UPDATE_CHECK=1"
  "AMPLIHACK_NO_FRESHNESS_CHECK=1"
  "AMPLIHACK_SKIP_MMDC=1"
  "AMPLIHACK_ROTATE_AUTOSTART=0"
  "AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH=$bin/amplihack-hooks"
  "RECIPE_RUNNER_RS_PATH=$bin/recipe-runner-rs"
  "IS_SANDBOX=1"
  "GIT_CONFIG_NOSYSTEM=1"
  "GIT_CONFIG_GLOBAL=/dev/null"
  "GIT_TERMINAL_PROMPT=0"
)

amp() {
  (cd "$root/work" && exec env -i "${CHILD_ENV[@]}" "$BIN" "$@") </dev/null
}

# Every path under HOME with its size and modification time, sorted.
snapshot() {
  find "$qa_home" -printf '%P %s %T@\n' | LC_ALL=C sort
}

# --- A. install writes the index ---------------------------------------------
if ! amp install --local "$REPO_ROOT" >"$root/install.log" 2>&1; then
  tail -40 "$root/install.log" | sed 's/^/    install: /'
  die "amplihack install --local failed"
fi
index="$qa_home/.amplihack/.claude/install/skills-and-agents-index.tsv"
[ -s "$index" ] || die "install wrote no index at the installer's staging folder"
echo "PASS A: install wrote an index of $(wc -l < "$index") entries"

before="$(snapshot)"

# --- B. find prints the matching entry, and its path is a file ---------------
amp directory find skill-and-agents-directory >"$root/find.out" 2>"$root/find.err" ||
  die "directory find exited non-zero: $(cat "$root/find.err")"
line="$(awk -F'\t' '$1 == "skill" && $2 == "skill-and-agents-directory"' "$root/find.out")"
[ -n "$line" ] || { cat "$root/find.out"; die "find printed no line for the directory skill"; }
fields="$(printf '%s\n' "$line" | awk -F'\t' '{ print NF }')"
[ "$fields" = 4 ] || die "a find line has $fields fields, expected 4"
path="$(printf '%s\n' "$line" | cut -f4)"
case "$path" in
  /*) ;;
  *) die "the printed path is not absolute: $path" ;;
esac
[ -f "$path" ] || die "the printed path is not a file: $path"
head -n 5 "$path" | grep -q '^name: skill-and-agents-directory$' ||
  die "the printed path does not hold the directory skill: $path"
echo "PASS B: find printed the directory skill, and its path is the skill's SKILL.md"

# An agent, found by a word from its description, in any letter case.
amp directory find ARCHITECTURE >"$root/agent.out" 2>/dev/null ||
  die "directory find for an agent exited non-zero"
agent_path="$(awk -F'\t' '$1 == "agent" { print $4; exit }' "$root/agent.out")"
[ -n "$agent_path" ] || die "find ARCHITECTURE printed no agent"
[ -f "$agent_path" ] || die "the printed agent path is not a file: $agent_path"
echo "PASS B: find printed an agent for an upper-case word, and its path is a file"

# --- C. every listed path exists ---------------------------------------------
amp directory list >"$root/list.out" 2>/dev/null || die "directory list exited non-zero"
listed="$(wc -l < "$root/list.out")"
[ "$listed" = "$(wc -l < "$index")" ] || die "list printed $listed lines; the index has $(wc -l < "$index")"
missing=0
while IFS=$'\t' read -r _type _name _description listed_path; do
  [ -f "$listed_path" ] || { echo "    missing: $listed_path"; missing=$((missing + 1)); }
done < "$root/list.out"
[ "$missing" = 0 ] || die "$missing listed paths are not files"
echo "PASS C: every one of $listed listed paths is a file"

# --- D. a word that matches nothing ------------------------------------------
amp directory find zqxjv-no-such-entry >"$root/none.out" 2>"$root/none.err" ||
  die "directory find for an unmatched word exited non-zero"
[ ! -s "$root/none.out" ] || die "find printed output for a word that matches no entry"
[ -s "$root/none.err" ] || die "find said nothing on stderr for a word that matches no entry"
echo "PASS D: an unmatched word printed nothing and exited 0"

# --- E. the command wrote nothing --------------------------------------------
after="$(snapshot)"
if [ "$before" != "$after" ]; then
  # No truncating stage: under pipefail an early-exit reader such as head
  # breaks the producer (issue #1434). awk reads all input and prints 20 lines;
  # diff exits 1 on a difference, which set -e would turn into an exit before die.
  diff <(printf "%s\n" "$before") <(printf "%s\n" "$after") | awk 'NR <= 20 { print "    " $0 }' || true
  die "directory find or list changed files under HOME"
fi
echo "PASS E: directory find and list wrote nothing under HOME"

echo "ALL_CASES_PASSED"
