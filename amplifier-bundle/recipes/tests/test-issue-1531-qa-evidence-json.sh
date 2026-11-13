#!/usr/bin/env bash
# test-issue-1531-qa-evidence-json.sh — regression test for #1531.
#
# The merge loop's QA record once spliced the last lines of the repository
# test command's log and the scenario file list into JSON built with printf.
# A runner that prints ANSI colour put raw ESC bytes into the record, the
# record was invalid JSON, and the merge round misread a passing QA run.
#
# Since #2569 the record is qa-inputs.json, written beside
# AUTODRIVE_QA_EVIDENCE by amplifier-bundle/tools/autodrive_qa_evidence.sh,
# which step-02a-qa-inputs of autodrive-merge-evidence.yaml runs. This runs
# that tool, unmodified, on a scratch Rust repository. It checks that
# qa-inputs.json is valid strict-UTF-8 JSON, and that its stdout matches the
# file, when:
#   - the declared command (AUTODRIVE_QA_COMMAND) prints colour, cursor and
#     erase sequences, quotes, backslashes, tabs, multibyte text and control
#     bytes: declared_qa.log_tail keeps the runner's words, with quotes and
#     backslashes kept and tabs and line ends turned into spaces;
#   - tracked scenario files in the scenario directory
#     (AUTODRIVE_QA_SCENARIO_DIR) and files changed since the merge base have
#     names holding quotes, backslashes, tabs and newlines: each is one exact
#     entry of existing_scenarios or changed_files.
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
# field_of <dotted.path>: one field of the JSON on stdin, e.g. declared_qa.log_tail.
field_of() {
    python3 -c '
import json, sys
v = json.loads(sys.stdin.buffer.read().decode("utf-8"))
for k in sys.argv[1].split("."):
    v = v[k]
sys.stdout.write(v if isinstance(v, str) else json.dumps(v))' "$1"
}
# has_entry <array-field> <exact name>: the JSON on stdin lists the name exactly once.
has_entry() {
    python3 -c '
import json, sys
v = json.loads(sys.stdin.buffer.read().decode("utf-8"))[sys.argv[1]]
sys.exit(0 if v.count(sys.argv[2]) == 1 else 1)' "$1" "$2"
}

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

# record <log-file> [NAME=value]... — the tool's stdout record for a repository
# whose declared command prints <log-file>. The tool's stderr goes to
# <log-file>.err and qa-inputs.json to <log-file>.d/. Only the last line of
# stdout is the record; take it with a shell substring, not `| tail -n 1`
# (issue #1434: no early-exit pipeline stages). RC is the tool's exit status.
RC=0
record() {
    local log="$1" stdout
    shift
    mkdir -p "$log.d"
    set +e
    stdout="$(env -i HOME="$WORK" TMPDIR="$WORK" PATH="/usr/bin:/bin" REPO_PATH="$REPO" \
        AUTODRIVE_ROUND_LABEL=round-1 AUTODRIVE_QA_COMMAND="cat $log" \
        AUTODRIVE_QA_EVIDENCE="$log.d/qa-evidence.json" "$@" \
        bash "$TOOL" 2>"$log.err")"
    RC=$?
    set -e
    printf '%s' "${stdout##*$'\n'}"
}
# file_matches <log-file> <record>: qa-inputs.json is valid JSON equal to the record.
file_matches() {
    local f="$1.d/qa-inputs.json"
    [[ -f "$f" ]] && valid_json < "$f" && [[ "$(cat "$f")" == "$2" ]]
}

# check_log <id> <description> <printf-format for the log> [text the summary must keep]...
check_log() {
    local id="$1" desc="$2" fmt="$3" log="$WORK/$1.log" out summary want ok=1
    shift 3
    # shellcheck disable=SC2059
    printf "$fmt" > "$log"
    out="$(record "$log")"
    if [[ "$RC" -ne 0 ]] || ! printf '%s' "$out" | valid_json; then
        fail "$id" "$desc: exit $RC or the record is not valid JSON: $(printf '%q' "${out:0:200}") | stderr: $(tail -n 3 "$log.err" | tr '\n' '|')"
        return 0
    fi
    if ! file_matches "$log" "$out"; then
        ok=0
        fail "$id" "$desc: qa-inputs.json is missing, invalid or differs from stdout"
    fi
    summary="$(printf '%s' "$out" | field_of declared_qa.log_tail)"
    for want in "$@"; do
        if [[ "$summary" != *"$want"* ]]; then
            ok=0
            fail "$id" "$desc: declared_qa.log_tail lost '$want' (got '$summary')"
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

echo "== #1531: declared_qa.log_tail is valid JSON and keeps the runner's words =="
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
check_log quotes "double quotes are kept" \
    'assert "a" == "b" failed\n' 'assert "a" == "b" failed'
check_log backslash "backslashes are kept" \
    'path C:\\Users\\ci\\src and \\u00zz\n' 'C:\Users\ci\src' '\u00zz'
check_log tabs "tabs and CRs become spaces" \
    'name\tstatus\r\npytest\tPASS\n' "name status" "pytest PASS"
check_log utf8 "a 300-byte cut that would split a multibyte character" \
    "x$(printf 'é%.0s' $(seq 1 200))\\n"
check_log control "other control bytes" \
    'bell\a here \001\002 raw\n' "bell here" "raw"
check_log plain "an ordinary log" \
    'test result: ok. 12 passed; 0 failed; 0 ignored\n' "test result: ok. 12 passed; 0 failed; 0 ignored"

echo "== #1531: existing_scenarios and changed_files are valid JSON for awkward file names =="
# Every awkward name, plus an ordinary one. Each must come back as one exact entry.
NAMES=('say "hi".yaml' 'back\slash.yaml' "$(printf 'tab\tname').yaml" "$(printf 'new\nline').yaml" 'plain.yaml')
SDIR="qa/scenarios"
mkdir -p "$REPO/$SDIR" "$REPO/changed"
for n in "${NAMES[@]}"; do : > "$REPO/$SDIR/$n"; : > "$REPO/changed/$n"; done
# A second commit holding them all, with origin/main at the first, so the
# merge base is the first commit and every file is changed in the "PR".
( cd "$REPO" && base="$(git rev-parse HEAD)" && git update-ref refs/remotes/origin/main "$base" \
  && git add -A \
  && c="$(GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@example.invalid GIT_COMMITTER_NAME=t \
          GIT_COMMITTER_EMAIL=t@example.invalid git commit-tree "$(git write-tree)" -p "$base" -m names)" \
  && git update-ref HEAD "$c" ) >/dev/null 2>&1 \
  || harness_error "could not commit the awkward file names"
printf 'ok\n' > "$WORK/scen.log"
out="$(record "$WORK/scen.log" AUTODRIVE_QA_SCENARIO_DIR="$SDIR" AUTODRIVE_QA_BASE=main)"
if [[ "$RC" -eq 0 ]] && printf '%s' "$out" | valid_json && file_matches "$WORK/scen.log" "$out"; then
    pass names-json "quotes, backslashes, tabs and newlines in file names: stdout and qa-inputs.json are valid JSON"
else
    fail names-json "exit $RC, or the record is invalid or differs from qa-inputs.json: $out | stderr: $(tail -n 3 "$WORK/scen.log.err" | tr '\n' '|')"
fi
got="$(printf '%s' "$out" | field_of scenario_dir 2>/dev/null || echo '<invalid>')"
if [[ "$got" == "$SDIR" ]]; then
    pass scenario-dir "scenario_dir is the AUTODRIVE_QA_SCENARIO_DIR given"
else
    fail scenario-dir "scenario_dir=$got, want $SDIR"
fi
for n in "${NAMES[@]}"; do
    label="$(printf '%q' "$n")"
    if printf '%s' "$out" | has_entry existing_scenarios "$SDIR/$n" 2>/dev/null; then
        pass scenarios "existing_scenarios lists $label exactly once"
    else
        fail scenarios "existing_scenarios lacks $label as one entry: $out"
    fi
    if printf '%s' "$out" | has_entry changed_files "changed/$n" 2>/dev/null; then
        pass changed "changed_files lists $label exactly once"
    else
        fail changed "changed_files lacks $label as one entry: $out"
    fi
done
# The second commit adds every name twice: once under the scenario directory, once under changed/.
for pair in "existing_scenarios:${#NAMES[@]}" "changed_files:$((2 * ${#NAMES[@]}))"; do
    f="${pair%%:*}"; want="${pair#*:}"
    got="$(printf '%s' "$out" | python3 -c 'import json,sys; print(len(json.loads(sys.stdin.buffer.read().decode("utf-8"))[sys.argv[1]]))' "$f" 2>/dev/null || echo '<invalid>')"
    if [[ "$got" == "$want" ]]; then
        pass "$f-count" "$f has $want entries: no name was split or merged"
    else
        fail "$f-count" "$f has $got entries, want $want"
    fi
done

echo
echo "Summary: ${PASS_COUNT} passed, ${FAIL_COUNT} failed"
[ "$FAIL_COUNT" -eq 0 ] || exit 1
exit 0
