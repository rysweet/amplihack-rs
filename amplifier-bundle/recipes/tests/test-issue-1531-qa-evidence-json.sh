#!/usr/bin/env bash
# test-issue-1531-qa-evidence-json.sh — regression test for #1531.
#
# autodrive-merge-evidence.yaml splices the last lines of the qa-team runner's
# log (qa_summary) and the scenario file list (qa_scenarios) into a JSON record
# built with printf. A runner that prints ANSI colour (gadugi-test does) put raw
# ESC bytes into qa_summary, the record was invalid JSON, and the merge round
# read a passing QA run as qa_status=MISSING.
#
# This takes the recipe's own json_text sanitiser, SUMMARY and SCEN assignments
# and record printf (extracted from the YAML, not copied), runs them over logs
# with colour, cursor and erase sequences, quotes, backslashes, tabs, multibyte
# text and control bytes, and over awkward scenario file names. It checks that
# every record is valid strict-UTF-8 JSON and that the runner's own words
# survive the sanitising.
#
# Usage: bash amplifier-bundle/recipes/tests/test-issue-1531-qa-evidence-json.sh
# Exit codes: 0 = pass, 1 = fail, 2 = test harness error.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
RECIPE="${REPO_ROOT}/amplifier-bundle/recipes/autodrive-merge-evidence.yaml"

harness_error() { echo "HARNESS-ERROR: $*" >&2; exit 2; }

[[ -f "$RECIPE" ]] || harness_error "recipe not found: $RECIPE"
# python3 decodes strictly; jq accepts invalid UTF-8, so it cannot stand in for
# the real consumer (`amplihack orch helper extract-json` rejects it).
command -v python3 >/dev/null 2>&1 || harness_error "python3 is required to validate the JSON strictly"
valid_json() { python3 -c 'import json,sys; json.loads(sys.stdin.buffer.read().decode("utf-8"))' 2>/dev/null; }

# one_line <description> <ERE> — the single recipe line matching <ERE>, unindented.
one_line() {
    local found
    found="$(grep -E "$2" "$RECIPE" | sed 's/^[[:space:]]*//')"
    [[ -n "$found" && "$(printf '%s\n' "$found" | wc -l)" -eq 1 ]] \
        || harness_error "expected exactly one $1 line in $RECIPE"
    printf '%s' "$found"
}
JSON_TEXT_DEF="$(one_line "json_text()" '^[[:space:]]*json_text\(\) *\{')"
SUMMARY_LINE="$(one_line 'SUMMARY="$(tail ... "$LOG" ...)"' '^[[:space:]]*SUMMARY="\$\(tail -n [0-9]+ "\$LOG"')"
SCEN_LINE="$(one_line 'SCEN="$(ls tests/parity/scenarios/* ...)"' '^[[:space:]]*SCEN="\$\(ls tests/parity/scenarios/')"
# The record: from `OUT="$(printf` through the first line not ending in a backslash.
OUT_STMT="$(awk '/^[[:space:]]*OUT="\$\(printf /{f=1} f{print; if ($0 !~ /\\$/) exit}' "$RECIPE")"
[[ -n "$OUT_STMT" ]] || harness_error "record printf (OUT=\"\$(printf ...)\") not found in $RECIPE"
eval "$JSON_TEXT_DEF"

PASS_COUNT=0
FAIL_COUNT=0
pass() { PASS_COUNT=$((PASS_COUNT + 1)); echo "  PASS[$1]: $2"; }
fail() { FAIL_COUNT=$((FAIL_COUNT + 1)); echo "  FAIL[$1]: $2" >&2; }

WORK="$(mktemp -d "${TMPDIR:-/tmp}/issue-1531.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT

# record <log-file> [scenario-dir] — the recipe's qa record for a run whose log
# is <log-file>, with SCEN listed from <scenario-dir> when given.
record() {
    local LOG="$1" STATUS=PASS TYPE=rust-cli CMD="cargo test" RC=0 SCEN="" SUMMARY="" OUT=""
    local HEAD_SHA=0123456789abcdef0123456789abcdef01234567 AUTODRIVE_ROUND_LABEL=round-1
    if [[ -n "${2:-}" ]]; then SCEN="$(cd "$2" && eval "$SCEN_LINE" && printf '%s' "$SCEN")"; fi
    eval "$SUMMARY_LINE"
    eval "$OUT_STMT"
    printf '%s' "$OUT"
}

summary_of() { python3 -c 'import json,sys; print(json.loads(sys.stdin.buffer.read().decode("utf-8"))["qa_summary"])'; }

# check_log <id> <description> <printf-format for the log> [text the summary must keep]...
check_log() {
    local id="$1" desc="$2" fmt="$3" log="$WORK/$1.log" out summary want ok=1
    shift 3
    # shellcheck disable=SC2059
    printf "$fmt" > "$log"
    out="$(record "$log")"
    if ! printf '%s' "$out" | valid_json; then
        fail "$id" "$desc: record is not valid JSON: $(printf '%q' "${out:0:200}")"
        return 0
    fi
    summary="$(printf '%s' "$out" | summary_of)"
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
SC="$WORK/repo"
mkdir -p "$SC/tests/parity/scenarios"
touch "$SC/tests/parity/scenarios/say \"hi\".yaml" "$SC/tests/parity/scenarios/back\\slash.yaml" \
      "$SC/tests/parity/scenarios/$(printf 'tab\tname').yaml" "$SC/tests/parity/scenarios/plain.yaml"
printf 'ok\n' > "$WORK/scen.log"
out="$(record "$WORK/scen.log" "$SC")"
if printf '%s' "$out" | valid_json; then
    pass scenarios "quotes, backslashes and tabs in scenario file names"
else
    fail scenarios "record is not valid JSON: $out"
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
