#!/usr/bin/env bash
# Outside-in QA for auto-drive follow-up issues (#2915). Drives
# autodrive_followups.sh as the crusty round and the merge loop do (classify,
# then collect, then file) against a stand-in gh that behaves like a
# repository: `gh issue create --label X` fails as real gh does while label X
# is missing, and `gh label create` adds it. HOME, TMPDIR, the state dir and
# gh's config are fixtures with 0700 roots; no real login or network is used.
#
# Usage: bash tests/qa/autodrive-followups.sh label|long|refused|edge|finalize
#   label    a repository without the follow-up label: it is made first, the
#            issue carries it, and the title is the finding with no task id
#   long     a finding longer than 200 characters, with a short one beside it:
#            each title is the finding's own whole words
#   refused  the label cannot be made: the step fails loudly with gh's own
#            message and followup-create-failed, and records nothing as filed
#   edge     awkward findings: mentions and references, bidirectional
#            overrides, two-byte characters across the 256 cut, a path with
#            no space, and an empty summary; each title stays readable
#   finalize the crusty loop's own end (autodrive_crusty_finalize.sh, as the
#            re-review calls it): a refused label leaves the phase incomplete
#            with gh's message, and a rerun once the label can be made files
#            the issue and records the crusty loop complete
set -uo pipefail
scenario=${1:?expected label, long, refused, edge or finalize}
case "$scenario" in label|long|refused|edge|finalize) ;; *) echo "unknown scenario: $scenario" >&2; exit 2 ;; esac
repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
tool="$repo_root/amplifier-bundle/tools/autodrive_followups.sh"
fixture=$(mktemp -d "${TMPDIR:-/tmp}/autodrive-followups-qa.XXXXXX") || exit 1
trap 'rm -rf -- "$fixture"' EXIT
chmod 700 "$fixture"
mkdir -m 700 "$fixture/home" "$fixture/tmp" "$fixture/state" "$fixture/gh-config" "$fixture/bin" "$fixture/repo"
state="$fixture/state"; log="$fixture/repo"
: > "$log/labels"; : > "$log/gh.log"

cat > "$fixture/bin/gh" <<'GH'
#!/usr/bin/env bash
# Stand-in gh: one repository o/r with PR #7, its labels in $QA_REPO/labels.
set -u
r="${QA_REPO:?}"
( IFS=$'\x1f'; printf '%s\n' "$*" ) >> "$r/gh.log"
args=("$@"); title=""; body=""; labels=()
for ((i=0; i<${#args[@]}; i++)); do
  case "${args[$i]}" in
    --title) title="${args[$((i+1))]}" ;;
    --body-file) body="${args[$((i+1))]}" ;;
    --label) labels+=("${args[$((i+1))]}") ;;
  esac
done
case "${1:-} ${2:-}" in
  "pr view")
    jq -cn --arg b "${QA_PR_BODY:-}" '{url:"https://github.com/o/r/pull/7", body:$b}' ;;
  "label create")
    if [ -n "${QA_LABEL_REFUSE:-}" ]; then
      echo "HTTP 403: Resource not accessible by integration (https://api.github.com/repos/o/r/labels)" >&2; exit 1
    fi
    grep -qxF -- "$3" "$r/labels" || printf '%s\n' "$3" >> "$r/labels" ;;
  "issue create")
    for l in "${labels[@]}"; do
      grep -qxF -- "$l" "$r/labels" || { echo "could not add label: '$l' not found" >&2; exit 1; }
    done
    n=$(( $(grep -c . "$r/created" 2>/dev/null || true) + 1 ))
    echo x >> "$r/created"
    printf '%s' "$title" > "$r/title-$n.txt"
    printf '%s\n' "${labels[@]}" > "$r/labels-$n.txt"
    cp -- "$body" "$r/body-$n.md"
    printf 'https://github.com/o/r/issues/%s\n' "$((100 + n))" ;;
  "issue comment")
    printf 'https://github.com/o/r/issues/%s#issuecomment-1\n' "${3:-0}" ;;
esac
exit 0
GH
chmod 700 "$fixture/bin/gh"

# Every run of the tool: fixtures first on PATH, no token, no inherited state.
run_tool() {
  env -u GH_TOKEN -u GITHUB_TOKEN -u GH_HOST -u BASH_ENV -u GIT_DIR -u GIT_WORK_TREE -u AUTODRIVE_DRY_RUN \
    HOME="$fixture/home" TMPDIR="$fixture/tmp" GH_CONFIG_DIR="$fixture/gh-config" \
    PATH="$fixture/bin:$PATH" QA_REPO="$log" GIT_CONFIG_NOSYSTEM=1 \
    "$@" bash "$tool" "${tool_args[@]}"
}

fails=0
check() { # check <description> <command...>
  local what=$1; shift
  if "$@"; then echo "PASS: $what"; else echo "FAIL: $what"; fails=$((fails + 1)); fi
}

# One crusty round as the loop leaves it: classify on the reviewer's concerns,
# then the MINOR_ONLY record, crusty-latest.json and the manifest row.
crusty_round() { # crusty_round <concerns JSON>
  local rec="$state/crusty-round-1.json" out sha hash
  tool_args=(classify --record "$rec" --state-dir "$state")
  out=$(printf '%s' "$1" | run_tool env 2>"$fixture/classify.err"); echo "classify: $out"
  [ "$out" = MINOR_ONLY ] || { echo "FAIL: classify did not decide MINOR_ONLY"; cat "$fixture/classify.err"; exit 1; }
  sha=$(printf 'a%.0s' {1..40})
  printf '{"crusty_verdict":"MINOR_ONLY","concern_count":%s,"commits_this_round":0,"head_sha":"%s","reviewed_head_sha":"%s","round_label":"round-1","test_signal":"","ci_signal":""}\n' \
    "$(printf '%s' "$1" | jq length)" "$sha" "$sha" > "$rec"
  cp -- "$rec" "$state/crusty-latest.json"
  hash=$(cd "$state" && git hash-object --no-filters -- crusty-round-1.json)
  printf 'round-1\tcrusty-round-1.json\t%s\n' "$hash" > "$state/crusty-records.tsv"
  tool_args=(collect --state-dir "$state")
  run_tool env 2>&1 || { echo "FAIL: collect"; exit 1; }
}

file_followups() { # file_followups [env assignments...]; sets rc, out, err
  tool_args=(file --state-dir "$state" --pr 7)
  out=$(run_tool env QA_PR_BODY="Fixes #2915." "$@" 2>"$fixture/file.err"); rc=$?
  err=$(cat "$fixture/file.err")
  printf 'file exit %s\nstdout: %s\nstderr:\n%s\n' "$rc" "$out" "$err"
  echo "gh calls:"; tr '\037' ' ' < "$log/gh.log" | cut -c 1-160 | sed 's/^/  gh /'
}

# whole_words <summary> <title>: the title is the summary, or the summary cut
# just before a space; nothing is cut inside a word.
whole_words() {
  [ "$2" = "$1" ] && return 0
  [ "${1:0:${#2}}" = "$2" ] && [ "${1:${#2}:1}" = " " ]
}
no_task_id() { case "$1" in *2915*|Follow-up*) return 1 ;; esac; }
chars() { printf '%s' "$1" | jq -Rrs length; } # characters (codepoints), not bytes

case "$scenario" in
  label)
    summary="Rename the helper that reads the last crusty round so that its name says what it returns"
    crusty_round "$(jq -cn --arg s "$summary" '[{id:"naming-1",severity:"minor",summary:$s,evidence:"autodrive_followups_file.sh crusty_round"}]')"
    check "the repository starts without the follow-up label" test ! -s "$log/labels"
    file_followups
    title=$(cat "$log/title-1.txt" 2>/dev/null)
    echo "title: $title"; echo "body:"; sed 's/^/  | /' "$log/body-1.md" 2>/dev/null
    first_label=$(grep -n $'^label\x1fcreate\x1ffollow-up' "$log/gh.log" | head -n 1 | cut -d: -f1)
    first_issue=$(grep -n $'^issue\x1fcreate' "$log/gh.log" | head -n 1 | cut -d: -f1)
    check "file exits 0 and reports followups-ok 1" test "$rc:$out" = "0:followups-ok 1"
    check "gh label create follow-up runs before the first issue" test -n "$first_label" -a -n "$first_issue" -a "${first_label:-0}" -lt "${first_issue:-0}"
    check "the step says the label is present" grep -q "the follow-up label is present in o/r" "$fixture/file.err"
    check "the repository now has the follow-up label" grep -qxF follow-up "$log/labels"
    check "the issue carries exactly the follow-up label" test "$(cat "$log/labels-1.txt")" = follow-up
    check "the title is the finding in plain words" test "$title" = "$summary"
    check "the title has no task id" no_task_id "$title"
    check "the body names the task" grep -qxF "Task: #2915" "$log/body-1.md"
    check "the body names the PR" grep -qF "PR #7" "$log/body-1.md"
    check "the body names the crusty round" grep -qxF "Crusty round: round-1" "$log/body-1.md"
    tool_args=(check --state-dir "$state" --pr 7)
    check "check finds every follow-up filed" run_tool env ;;
  long)
    long="The helper that reads the last crusty round from the manifest repeats the same file and link tests that the collect command already makes, so a reader has to look in both places to learn which rule applies when the record is missing or cannot be read at filing time"
    short="Name the label colour constant after what it is for"
    echo "long finding: ${#long} characters"
    crusty_round "$(jq -cn --arg a "$long" --arg b "$short" '[{id:"naming-2",severity:"minor",summary:$a,evidence:"e1"},{id:"style-3",severity:"minor",summary:$b,evidence:"e2"}]')"
    printf 'follow-up\n' > "$log/labels"
    file_followups
    t1=$(cat "$log/title-1.txt" 2>/dev/null); t2=$(cat "$log/title-2.txt" 2>/dev/null)
    printf 'title 1 (%s characters): %s\ntitle 2: %s\n' "${#t1}" "$t1" "$t2"
    check "file exits 0 and reports followups-ok 2" test "$rc:$out" = "0:followups-ok 2"
    check "two issues, each with the follow-up label" test "$(cat "$log"/labels-1.txt "$log"/labels-2.txt 2>/dev/null)" = $'follow-up\nfollow-up'
    check "the long title is within GitHub's 256 characters" test "${#t1}" -le 256 -a "${#t1}" -gt 0
    check "the long title keeps whole words (no word cut part-way)" whole_words "$long" "$t1"
    check "the long title has no task id" no_task_id "$t1"
    check "the short title is the whole finding" test "$t2" = "$short" ;;
  refused)
    crusty_round '[{"id":"naming-4","severity":"minor","summary":"Say which round the body names","evidence":"e"}]'
    file_followups QA_LABEL_REFUSE=1
    check "file fails loudly with exit 1" test "$rc" = 1
    check "gh's own message for the label reaches the log" grep -qF "Resource not accessible by integration" "$fixture/file.err"
    check "gh's own message for the create reaches the log" grep -qF "could not add label: 'follow-up' not found" "$fixture/file.err"
    check "followups.error names followup-create-failed" test "$(cat "$state/followups.error" 2>/dev/null)" = followup-create-failed
    check "no issue is recorded as filed" test ! -s "$state/followups.tsv"
    tool_args=(gate --state-dir "$state" --pr 7)
    gate=$(run_tool env 2>/dev/null); echo "gate: $gate"
    check "the gate reports the follow-ups as not filed" grep -q "follow-up issues not filed" <<<"$gate" ;;
  edge)
    wide=$(printf 'café %.0s' {1..60}); wide=${wide% }
    path=$(printf 'amplifier-bundle/tools/%.0s' {1..14}); path=${path:0:300}
    echo "two-byte finding: $(chars "$wide") characters; path with no space: ${#path} characters"
    crusty_round "$(jq -cn --arg w "$wide" --arg t "$path" '[
      {id:"refs-1",severity:"minor",summary:"Point @octocat at #42 and GH-7 on github.com/o/r",evidence:"e"},
      {id:"bidi-2",severity:"minor",summary:"Keep the words \u202ein reading order\u202c here",evidence:"e"},
      {id:"wide-3",severity:"minor",summary:$w,evidence:"e"},
      {id:"word-4",severity:"minor",summary:$t,evidence:"e"},
      {id:"empty-5",severity:"minor",summary:"",evidence:"e"}]')"
    file_followups
    for i in 1 2 3 4 5; do
      printf 'title %s (%s characters): %s\n' "$i" "$(chars "$(cat "$log/title-$i.txt" 2>/dev/null)")" "$(cat "$log/title-$i.txt" 2>/dev/null)"
    done
    t1=$(cat "$log/title-1.txt" 2>/dev/null); t2=$(cat "$log/title-2.txt" 2>/dev/null); t3=$(cat "$log/title-3.txt" 2>/dev/null)
    t4=$(cat "$log/title-4.txt" 2>/dev/null); t5=$(cat "$log/title-5.txt" 2>/dev/null)
    zw=$'\xe2\x80\x8b'
    check "file exits 0 and reports followups-ok 5" test "$rc:$out" = "0:followups-ok 5"
    check "five issues, each with exactly the follow-up label" \
      test "$(cat "$log"/labels-[1-5].txt 2>/dev/null | sort | uniq -c | awk '{print $1, $2}')" = "5 follow-up"
    check "no mention or reference in a title links anywhere" test -z "$(grep -lE '@[a-z]|#[0-9]|GH-[0-9]|github\.com/[a-z]' "$log"/title-[1-5].txt)"
    check "the reference title reads as the finding once the breaks are removed" \
      test "${t1//$zw/}" = "Point @octocat at #42 and GH-7 on github.com/o/r"
    check "no title carries a bidirectional override or isolate" \
      test -z "$(grep -lP '[\x{202A}-\x{202E}\x{2066}-\x{2069}]' "$log"/title-[1-5].txt)"
    check "the override title is the finding's own words" test "$t2" = "Keep the words in reading order here"
    check "the two-byte title is within 256 characters" test "$(chars "$t3")" -le 256
    check "the two-byte title is valid UTF-8 (no character split)" iconv -f UTF-8 -t UTF-8 "$log/title-3.txt" -o /dev/null
    check "the two-byte title keeps whole words" whole_words "$wide" "$t3"
    check "a path with no space in it is cut at 256 characters" test "$t4" = "${path:0:256}"
    check "an empty finding is filed under its id, not an empty title" test "$t5" = "Minor crusty finding empty-5"
    check "no title has a task id" test -z "$(grep -lE '2915|^Follow-up' "$log"/title-[1-5].txt)" ;;
  finalize)
    crusty_round '[{"id":"naming-6","severity":"minor","summary":"Name the round reader after the record it reads","evidence":"e"}]'
    printf 'Fixes #2915.\n' > "$fixture/task.md"
    tool="$repo_root/amplifier-bundle/tools/autodrive_crusty_finalize.sh"
    tool_args=("$state" 7 "" "$fixture/task.md")
    run_tool env QA_LABEL_REFUSE=1 >"$fixture/fin1.out" 2>&1; rc=$?
    echo "finalize with the label refused: exit $rc"; sed 's/^/  | /' "$fixture/fin1.out"
    check "the refused run exits 1" test "$rc" = 1
    check "gh's own message for the label reaches the log" grep -qF "Resource not accessible by integration" "$fixture/fin1.out"
    check "the phase is reported incomplete" grep -qF "followups-unfiled: filing failed; phase remains incomplete" "$fixture/fin1.out"
    check "the crusty loop is not recorded complete" test ! -s "$state/phases.tsv"
    check "no issue was filed" test ! -e "$log/created"
    run_tool env >"$fixture/fin2.out" 2>&1; rc=$?
    echo "finalize once the label can be made: exit $rc"; sed 's/^/  | /' "$fixture/fin2.out"
    title=$(cat "$log/title-1.txt" 2>/dev/null)
    check "the rerun exits 0" test "$rc" = 0
    check "exactly one issue was filed over both runs" test "$(grep -c . "$log/created" 2>/dev/null)" = 1
    check "the issue carries exactly the follow-up label" test "$(cat "$log/labels-1.txt" 2>/dev/null)" = follow-up
    check "the title is the finding in plain words" test "$title" = "Name the round reader after the record it reads"
    check "the body names the task from the task text" grep -qxF "Task: #2915" "$log/body-1.md"
    check "the crusty loop is recorded complete" grep -q $'^crusty-loop\t' "$state/phases.tsv"
    check "followups.error is cleared" test ! -e "$state/followups.error"
    tool="$repo_root/amplifier-bundle/tools/autodrive_followups.sh"; tool_args=(gate --state-dir "$state" --pr 7)
    gate=$(run_tool env 2>/dev/null); grc=$?; echo "gate (exit $grc): $gate"
    check "the merge gate's follow-up check passes" test "$grc" = 0 ;;
esac
echo "scenario $scenario: $fails failed"
[ "$fails" -eq 0 ]
