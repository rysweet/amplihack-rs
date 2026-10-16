#!/usr/bin/env bash
# test-issue-1531-qa-evidence-json.sh — regression test for #1531.
#
# The merge loop's QA evidence record splices the last lines of the repository
# test command's log (qa_summary) and the scenario file list (qa_scenarios)
# into a JSON record built with printf. A runner that prints ANSI colour
# (gadugi-test does) put raw ESC bytes into qa_summary, the record was invalid
# JSON, and the merge round read a passing QA run as qa_status=MISSING.
#
# The record is built by amplifier-bundle/tools/autodrive_qa_evidence.sh, which
# step-02-qa-team-scenarios of autodrive-merge-evidence.yaml runs. This runs
# that tool, unmodified, on a scratch Rust repository whose test command
# (AUTODRIVE_QA_COMMAND) prints logs with colour, cursor and erase sequences,
# quotes, backslashes, tabs, multibyte text and control bytes, and over awkward
# scenario file names. It checks that every record is valid strict-UTF-8 JSON
# and that the runner's own words survive the sanitising.
#
# Usage: bash amplifier-bundle/recipes/tests/test-issue-1531-qa-evidence-json.sh
# Exit codes: 0 = pass, 1 = fail, 2 = test harness error.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
TOOL="${REPO_ROOT}/amplifier-bundle/tools/autodrive_qa_evidence.sh"

harness_error() { echo "HARNESS-ERROR: $*" >&2; exit 2; }

[[ -f "$TOOL" ]] || harness_error "tool not found: $TOOL"
# python3 decodes strictly; jq accepts invalid UTF-8, so it cannot stand in for
# the real consumer (`amplihack orch helper extract-json` rejects it).
command -v python3 >/dev/null 2>&1 || harness_error "python3 is required to validate the JSON strictly"
valid_json() { python3 -c 'import json,sys; json.loads(sys.stdin.buffer.read().decode("utf-8"))' 2>/dev/null; }
field_of() { python3 -c 'import json,sys; print(json.loads(sys.stdin.buffer.read().decode("utf-8"))[sys.argv[1]])' "$1"; }

PASS_COUNT=0
FAIL_COUNT=0
pass() { PASS_COUNT=$((PASS_COUNT + 1)); echo "  PASS[$1]: $2"; }
fail() { FAIL_COUNT=$((FAIL_COUNT + 1)); echo "  FAIL[$1]: $2" >&2; }

WORK="$(mktemp -d "${TMPDIR:-/tmp}/issue-1531.XXXXXX")"
WORK="$(cd "$WORK" && pwd -P)"
trap 'rm -rf "$WORK"' EXIT

# A scratch Rust repository with one commit, written via plumbing (no hooks).
REPO="$WORK/repo"
mkdir -p "$REPO"
: > "$REPO/Cargo.toml"
( cd "$REPO" && git init -q . && git add -A \
  && c="$(GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@example.invalid GIT_COMMITTER_NAME=t \
          GIT_COMMITTER_EMAIL=t@example.invalid git commit-tree "$(git write-tree)" -m init)" \
  && git update-ref HEAD "$c" ) >/dev/null 2>&1 \
  || harness_error "could not create a scratch git repo"

# record <log-file> — the tool's qa record for a repository whose test command
# prints <log-file>. The tool's stderr goes to <log-file>.err.
record() {
    env -i HOME="$WORK" TMPDIR="$WORK" PATH="/usr/bin:/bin" REPO_PATH="$REPO" \
        AUTODRIVE_ROUND_LABEL=round-1 AUTODRIVE_QA_COMMAND="cat $1" \
        bash "$TOOL" 2>"$1.err" | tail -n 1
}

# check_log <id> <description> <printf-format for the log> [text the summary must keep]...
check_log() {
    local id="$1" desc="$2" fmt="$3" log="$WORK/$1.log" out summary want ok=1
    shift 3
    # shellcheck disable=SC2059
    printf "$fmt" > "$log"
    out="$(record "$log")"
    if ! printf '%s' "$out" | valid_json; then
        fail "$id" "$desc: record is not valid JSON: $(printf '%q' "${out:0:200}") | stderr: $(tail -n 3 "$log.err" | tr '\n' '|')"
        return 0
    fi
    summary="$(printf '%s' "$out" | field_of qa_summary)"
    for want in "$@"; do
        if [[ "$summary" != *"$want"* ]]; then
            ok=0
            fail "$id" "$desc: qa_summary lost '$want' (got '$summary')"
        fi
    done
    # A partly stripped CSI sequence leaves its tail behind, e.g. `[34m` or `[2K`.
    if [[ "$fmt" == *'\033['* && "$summary" =~ \[[0-9:\;?\>]+[A-Za-z] ]]; then
        ok=0
        fail "$id" "$desc: an escape sequence was only partly stripped (got '$summary')"
    fi
    if [[ "$ok" -eq 1 ]]; then pass "$id" "$desc"; fi
    return 0
}

echo "== #1531: qa_summary is valid JSON and keeps the runner's words =="
check_log colour "ANSI colour (gadugi-test output)" \
    '\033[34mdebug\033[39m: \033[2m Tests \033[22m \033[1m\033[32m 3 passed\033[39m\033[22m\n\033[32m✓\033[39m all scenarios passed\n' \
    "debug:" "3 passed" "all scenarios passed"
check_log cursor "cursor-up and erase-line before uppercase text" \
    '\033[1A\033[2KFAILED: 3 TESTS\n' "FAILED: 3 TESTS"
check_log erase "erase-line directly before a word" \
    '\033[2KTEST RESULT: ok. 5 passed\n' "TEST RESULT: ok. 5 passed"
check_log column "cursor-to-column before a path" \
    '\033[1G\033[2KPASS src/a.test.ts (12 tests)\n' "PASS src/a.test.ts (12 tests)"
check_log bare "parameterless erase-to-end and reset (ESC[K, ESC[m)" \
    'A\033[KB\033[mC done\n' "ABC done"
check_log truecolour "colon-separated and private-parameter sequences" \
    '\033[38:2:255:0:0mred\033[0m \033[>4;1mkeys\033[0m done\n' "red" "keys" "done"
check_log quotes "double quotes" \
    'assert "a" == "b" failed\n' "assert a == b failed"
check_log backslash "backslashes become '/'" \
    'path C:\\Users\\ci\\src and \\u00zz\n' "C:/Users/ci/src" "/u00zz"
check_log tabs "tabs and CRs become spaces" \
    'name\tstatus\r\npytest\tPASS\n' "name status" "pytest PASS"
check_log utf8 "a 300-byte cut that would split a multibyte character" \
    "x$(printf 'é%.0s' $(seq 1 200))\\n"
check_log control "other control bytes" \
    'bell\a here \001\002 raw\n' "bell here" "raw"
check_log plain "an ordinary log" \
    'test result: ok. 12 passed; 0 failed; 0 ignored\n' "test result: ok. 12 passed; 0 failed; 0 ignored"

echo "== #1531: qa_scenarios is valid JSON for awkward file names =="
mkdir -p "$REPO/tests/parity/scenarios"
touch "$REPO/tests/parity/scenarios/say \"hi\".yaml" "$REPO/tests/parity/scenarios/back\\slash.yaml" \
      "$REPO/tests/parity/scenarios/$(printf 'tab\tname').yaml" "$REPO/tests/parity/scenarios/plain.yaml"
printf 'ok\n' > "$WORK/scen.log"
out="$(record "$WORK/scen.log")"
if printf '%s' "$out" | valid_json; then
    pass scenarios "quotes, backslashes and tabs in scenario file names"
else
    fail scenarios "record is not valid JSON: $out | stderr: $(tail -n 3 "$WORK/scen.log.err" | tr '\n' '|')"
fi
if [[ "$out" == *"tests/parity/scenarios/plain.yaml"* ]]; then
    pass scenarios-text "ordinary scenario names are listed unchanged"
else
    fail scenarios-text "plain.yaml is missing from qa_scenarios: $out"
fi

echo
echo "Summary: ${PASS_COUNT} passed, ${FAIL_COUNT} failed"
[ "$FAIL_COUNT" -eq 0 ] || exit 1
exit 0
