#!/usr/bin/env bash
# test-issue-2333-graphql-unavailable.sh — acceptance A1 for issue #2333.
#
# With every GraphQL-backed read unavailable and REST answering, a run on a
# clean, green PR reaches merge. The assertion is that the merge happened.
#
# Subjects
#   amplifier-bundle/tools/autodrive_rest.sh   (rest_get, rest_json)
#   amplifier-bundle/tools/autodrive_merge.sh  (_merge_pr_json, _merge_checks_rest)
#
# Contract: docs/reference/autodrive-rest-reads.md.
#
# Every `gh` call goes to a fake on PATH that serves fixture files. No
# network, no sleep, no wall-clock window, and nothing here polls.
#
# Sections
#   1. Static contract of autodrive_rest.sh (exists, defines only functions,
#      sourcing has no other effect, no sleep or retry loop).
#   2. S2: the closed endpoint form; a refused endpoint makes no gh call.
#   3. rest_get exit codes, and its side files.
#   4. A1: GraphQL unavailable, REST answering, a clean green PR merges.
#   5. S15: a half-answer is not an answer.
#   6. The PR read before the merge: unreadable is waiting, never not-open.
#   7. The checks read: pending before failing; a 404 is unreadable, never
#      an empty list.
#   8. S10: reasons carry no check names, URLs or gh text from this path.
# The subjects are sourced from paths computed at run time.
# shellcheck disable=SC1090
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
REPO_ROOT="$(cd "${HERE}/../../.." && pwd -P)"
TOOLS="${REPO_ROOT}/amplifier-bundle/tools"
REST="${TOOLS}/autodrive_rest.sh"
MERGE="${TOOLS}/autodrive_merge.sh"

command -v jq >/dev/null 2>&1 || { echo "HARNESS-ERROR: jq is required" >&2; exit 2; }

PASS_COUNT=0
FAIL_COUNT=0
pass() { PASS_COUNT=$((PASS_COUNT + 1)); echo "  PASS[$1]: $2"; }
fail() { FAIL_COUNT=$((FAIL_COUNT + 1)); echo "  FAIL[$1]: $2" >&2; }
check() { # check <id> <description> <command...>
  local id="$1" d="$2"; shift 2
  if "$@"; then pass "$id" "$d"; else fail "$id" "$d"; fi
}

WORK="$(mktemp -d)" || { echo "HARNESS-ERROR: mktemp -d failed" >&2; exit 2; }
trap 'rm -rf "${WORK}"' EXIT
chmod 700 "${WORK}"
BIN="${WORK}/bin"; mkdir -p "${BIN}"
F="${WORK}/fix"; mkdir -p "${F}"
SHA="aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"

# ── The fake gh ─────────────────────────────────────────────────────────────
# Every call is logged to $F/gh.log. GraphQL-backed reads (`pr view`,
# `pr checks`, `api graphql`) fail with GitHub's secondary-limit message when
# $GQL_DOWN is set; the merge mutation always answers, as on the recorded
# incidents, every one of which was a failed read.
cat > "${BIN}/gh" <<'GH'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "${F:?}/gh.log"
jqf=""; prev=""
for a in "$@"; do [ "$prev" = "--jq" ] && jqf="$a"; prev="$a"; done
emit() { if [ -n "$jqf" ]; then printf '%s' "$1" | jq -r "$jqf"; else printf '%s\n' "$1"; fi; }
limit() { echo "You have exceeded a secondary rate limit. Please wait a few minutes before you try again." >&2; exit 1; }
serve() { # serve <fixture base>
  local b="${F}/$1"
  [ -f "${b}.rc" ] && [ "$(cat "${b}.rc")" != 0 ] && {
    cat "${b}.err" >&2 2>/dev/null || echo "gh: request failed" >&2
    exit "$(cat "${b}.rc")"
  }
  [ -f "${b}.json" ] || { echo "gh: no fixture for $1" >&2; exit 1; }
  emit "$(cat "${b}.json")"
}
case "$1 ${2:-}" in
  "pr view")   [ -n "${GQL_DOWN:-}" ] && limit; serve pr-view ;;
  "pr checks") [ -n "${GQL_DOWN:-}" ] && limit; serve pr-checks ;;
  "pr merge")
    cat > "${F}/merge-stdin" 2>/dev/null
    rc="$(cat "${F}/merge-rc" 2>/dev/null || echo 0)"
    # GitHub now reads the PR merged, through either route.
    if [ "$rc" = 0 ]; then
      [ -f "${F}/rest-pr-after.json" ] && cp "${F}/rest-pr-after.json" "${F}/rest-pr.json"
      [ -f "${F}/pr-view-after.json" ] && cp "${F}/pr-view-after.json" "${F}/pr-view.json"
    fi
    exit "$rc" ;;
  "api graphql") [ -n "${GQL_DOWN:-}" ] && limit; serve graphql ;;
  "api "*)
    ep="$2"
    case "$ep" in
      */check-runs*) serve rest-check-runs ;;
      */status*)     serve rest-status ;;
      repos/*/pulls/*) serve rest-pr ;;
      *) echo "gh: unexpected endpoint ${ep}" >&2; exit 1 ;;
    esac ;;
  *) echo "gh: unexpected call: $*" >&2; exit 1 ;;
esac
GH
chmod 755 "${BIN}/gh"
export PATH="${BIN}:${PATH}"
export F
export AMPLIHACK_HOME="${REPO_ROOT}"

reset_fixtures() { # reset_fixtures [head sha]
  local head="${1:-$SHA}"
  rm -f "${F}"/gh.log "${F}"/*.json "${F}"/*.rc "${F}"/*.err "${F}"/pr-checks "${F}"/merge-stdin
  : > "${F}/gh.log"
  printf '{"state":"OPEN","title":"Fix the thing (#2333)","body":"Task: 2333\\n\\nFixes #2333","mergeStateStatus":"CLEAN","headRefOid":"%s"}\n' "$head" > "${F}/pr-view.json"
  printf '{"number":7,"state":"open","merged":false,"title":"Fix the thing (#2333)","body":"Task: 2333\\n\\nFixes #2333","mergeable_state":"clean","head":{"sha":"%s"}}\n' "$head" > "${F}/rest-pr.json"
  printf '{"number":7,"state":"closed","merged":true,"title":"Fix the thing (#2333)","body":"Task: 2333\\n\\nFixes #2333","mergeable_state":"clean","head":{"sha":"%s"}}\n' "$head" > "${F}/rest-pr-after.json"
  printf '{"state":"MERGED","title":"Fix the thing (#2333)","body":"Task: 2333\\n\\nFixes #2333","mergeStateStatus":"CLEAN","headRefOid":"%s"}\n' "$head" > "${F}/pr-view-after.json"
  printf '{"total_count":1,"check_runs":[{"name":"ci","status":"completed","conclusion":"success"}]}\n' > "${F}/rest-check-runs.json"
  printf '{"state":"success","total_count":0,"statuses":[]}\n' > "${F}/rest-status.json"
  printf '0' > "${F}/merge-rc"
}

gh_log() { cat "${F}/gh.log" 2>/dev/null; }
merged_once() { [ "$(gh_log | grep -c '^pr merge ')" -eq 1 ]; }
no_merge_request() { [ "$(gh_log | grep -c '^pr merge ')" -eq 0 ]; }

run_merge() { # run_merge <pr> <sha>; sets OUT and RC
  OUT="$( set +u; . "${MERGE}" && autodrive_merge "$1" "$2" )"
  RC=$?
  [ -n "$OUT" ] || { echo "  (the tool printed no JSON line; rc=${RC})" >&2; RC=99; }
}

result_is() { [ "$(printf '%s' "$OUT" | jq -r '.result // empty')" = "$1" ]; }
reason() { printf '%s' "$OUT" | jq -r '.reason // empty'; }

echo "== 1. autodrive_rest.sh: static contract =="
check 1.1 "the helper exists" test -f "${REST}"
check 1.2 "it is not a symbolic link" test ! -L "${REST}"
if [ -f "${REST}" ]; then
  check 1.3 "it defines rest_get" grep -qE '^[[:space:]]*rest_get\(\)' "${REST}"
  check 1.4 "it defines rest_json" grep -qE '^[[:space:]]*rest_json\(\)' "${REST}"
  # Sourcing must have no other effect: no global umask, no export, no command.
  body="$(grep -vE '^[[:space:]]*(#|$)' "${REST}")"
  check 1.5 "sourcing sets no global umask" bash -c '! grep -qE "^umask " <<< "$1"' _ "$body"
  check 1.6 "sourcing exports nothing" bash -c '! grep -qE "^export " <<< "$1"' _ "$body"
  check 1.7 "no sleep anywhere" bash -c '! grep -qE "(^|[^a-z_])sleep[[:space:]]" "$1"' _ "${REST}"
  check 1.8 "no retry or wait loop" bash -c '! grep -qE "(^|[[:space:]])(until|while true|read -t)[[:space:]]" "$1"' _ "${REST}"
  # Sourcing it twice in a subshell must leave the environment alone.
  before="$(env | sort | md5sum)"
  after="$( set +u; . "${REST}"; . "${REST}"; env | sort | md5sum )"
  check 1.9 "sourcing changes no environment variable" test "$before" = "$after"
fi

# Sections 2-8 assert on behaviour, and a missing subject would let their
# negative assertions pass with nothing having run. Stop here instead.
if [ ! -f "${REST}" ] || [ ! -f "${MERGE}" ]; then
  echo
  echo "STOP: the subjects of sections 2-8 are not on this tree:"
  [ -f "${REST}" ]  || echo "  missing ${REST}"
  [ -f "${MERGE}" ] || echo "  missing ${MERGE}"
  echo "PASS: ${PASS_COUNT}  FAIL: $((FAIL_COUNT + 1))"
  exit 1
fi

echo "== 2. S2: the endpoint must match a closed form =="
  for bad in \
    "https://attacker.example/x" \
    "//attacker.example/x" \
    "/repos/o/r/pulls/7" \
    "-X POST repos/o/r/pulls/7" \
    "user@attacker.example/repos/o/r/pulls/7" \
    "repos/o/r/../../../user" \
    "repos/o/r/pulls/7 --method POST" \
    "graphql"
  do
    reset_fixtures
    rc=0
    ( set +u; . "${REST}"; rest_get "${WORK}/out" 0 "$bad" ) >/dev/null 2>&1 || rc=$?
    check "2.$bad" "refused: ${bad}" bash -c '[ "$1" -eq 1 ] && [ ! -s "$2" ]' _ "$rc" "${F}/gh.log"
  done
reset_fixtures
rc=0
( set +u; . "${REST}"; rest_get "${WORK}/ok" 0 "repos/o/r/pulls/7" ) >/dev/null 2>&1 || rc=$?
check 2.ok "the permitted form is accepted" test "$rc" -eq 0

echo "== 3. rest_get exit codes and side files =="
  reset_fixtures
  printf '1' > "${F}/rest-pr.rc"; printf 'gh: HTTP 404: Not Found (https://api.github.com/repos/o/r/pulls/7)\n' > "${F}/rest-pr.err"
  rc=0; ( set +u; . "${REST}"; rest_get "${WORK}/o404" 0 "repos/o/r/pulls/7" ) >/dev/null 2>&1 || rc=$?
  check 3.1 "a 404 is exit 4" test "$rc" -eq 4

  reset_fixtures
  printf '1' > "${F}/rest-pr.rc"; printf 'gh: HTTP 502: Bad Gateway\n' > "${F}/rest-pr.err"
  rc=0; ( set +u; . "${REST}"; rest_get "${WORK}/o502" 0 "repos/o/r/pulls/7" ) >/dev/null 2>&1 || rc=$?
  check 3.2 "any other failure is exit 1" test "$rc" -eq 1

  reset_fixtures
  printf '79' > "${F}/rest-pr.rc"; printf 'refused\n' > "${F}/rest-pr.err"
  rc=0; ( set +u; . "${REST}"; rest_get "${WORK}/o79" 0 "repos/o/r/pulls/7" ) >/dev/null 2>&1 || rc=$?
  check 3.3 "exit 79 is passed through unchanged" test "$rc" -eq 79

  reset_fixtures
  out="${WORK}/sides"
  ( set +u; . "${REST}"; rest_get "$out" 0 "repos/o/r/pulls/7" ) >/dev/null 2>&1
  check 3.4 "the side files are removed" bash -c '[ ! -e "$1.raw" ] && [ ! -e "$1.err" ]' _ "$out"
  check 3.5 "the answer is one JSON array" bash -c 'jq -e "type == \"array\"" "$1" >/dev/null' _ "$out"

  # S13: an <out> that is already a symbolic link is refused.
  reset_fixtures
  ln -sf "${WORK}/target-of-the-link" "${WORK}/link-out"
  rc=0; ( set +u; . "${REST}"; rest_get "${WORK}/link-out" 0 "repos/o/r/pulls/7" ) >/dev/null 2>&1 || rc=$?
  check 3.6 "a symbolic link as <out> is refused" bash -c '[ "$1" -ne 0 ] && [ ! -e "$2" ]' _ "$rc" "${WORK}/target-of-the-link"

  reset_fixtures
  ( set +u; . "${REST}"; rest_json "${WORK}/filtered" 0 "repos/o/r/pulls/7" '.[0].head.sha' ) >/dev/null 2>&1
  check 3.7 "rest_json applies the filter" bash -c 'grep -q "$2" "$1"' _ "${WORK}/filtered" "$SHA"

echo "== 4. A1: GraphQL unavailable, REST answering, a clean green PR merges =="
reset_fixtures
GQL_DOWN=1 run_merge 7 "$SHA"
check 4.1 "the tool exits 0" test "${RC:-1}" -eq 0
check 4.2 "it reports the PR merged" result_is merged
check 4.3 "the merge request was made exactly once" merged_once
check 4.4 "the merge named the assessed head" bash -c 'grep -q -- "--match-head-commit $1" "$2"' _ "$SHA" "${F}/gh.log"
check 4.5 "it is still a squash merge" grep -q -- '--squash' "${F}/gh.log"
check 4.6 "a GraphQL read was attempted and failed, not skipped" grep -qE '^pr view' "${F}/gh.log"
check 4.7 "the PR was read through REST instead" grep -qE '^api repos/[^ ]*/pulls/7' "${F}/gh.log"
check 4.8 "the checks were read through REST instead" grep -qE '^api repos/[^ ]*/commits/[^ ]*/(check-runs|status)' "${F}/gh.log"
check 4.9 "nothing waited: no second read of either route" bash -c '[ "$(grep -c "check-runs" "$1")" -le 1 ]' _ "${F}/gh.log"

echo "   (the same PR with GraphQL answering is unchanged)"
reset_fixtures
printf 'ci\tpass\t1s\thttps://example.invalid\n' > "${F}/pr-checks.json"
run_merge 7 "$SHA"
check 4.10 "GraphQL answering still merges" bash -c '[ "$1" -eq 0 ]' _ "${RC:-1}"
check 4.11 "and makes no REST read it did not need" bash -c '! grep -qE "^api repos/[^ ]*/commits/" "$1"' _ "${F}/gh.log"

echo "== 5. S15: a half-answer is not an answer =="
reset_fixtures
printf '1' > "${F}/rest-status.rc"; printf 'gh: HTTP 502: Bad Gateway\n' > "${F}/rest-status.err"
GQL_DOWN=1 run_merge 7 "$SHA"
check 5.1 "an unanswered combined status is checks-pending" test "${RC:-0}" -eq 7
check 5.2 "and reports it as waiting" result_is checks-pending
check 5.3 "no merge request is made on a half-answer" no_merge_request

reset_fixtures
printf '1' > "${F}/rest-check-runs.rc"; printf 'gh: HTTP 502: Bad Gateway\n' > "${F}/rest-check-runs.err"
GQL_DOWN=1 run_merge 7 "$SHA"
check 5.4 "an unanswered check-runs read is checks-pending" test "${RC:-0}" -eq 7
check 5.5 "no merge request is made then either" no_merge_request

echo "== 6. The PR read before the merge: unreadable is waiting, not not-open =="
reset_fixtures
printf '1' > "${F}/rest-pr.rc"; printf 'gh: HTTP 502: Bad Gateway\n' > "${F}/rest-pr.err"
GQL_DOWN=1 run_merge 7 "$SHA"
check 6.1 "an unreadable PR read is checks-pending, a wait" test "${RC:-0}" -eq 7
check 6.2 "never not-open" bash -c '! printf "%s" "$1" | jq -e ".result == \"not-open\"" >/dev/null' _ "$OUT"
check 6.3 "the reason names an unreadable read" bash -c 'printf "%s" "$1" | grep -qi "could not be read\|unreadable"' _ "$(reason)"
check 6.4 "the reason never asserts a state the run did not observe" bash -c '! printf "%s" "$1" | grep -q "state .unreadable."' _ "$(reason)"
check 6.5 "no merge request is made" no_merge_request

echo "   (a PR that really is closed is still not-open)"
reset_fixtures
printf '{"number":7,"state":"closed","merged":false,"title":"t","body":"b","mergeable_state":"clean","head":{"sha":"%s"}}\n' "$SHA" > "${F}/rest-pr.json"
GQL_DOWN=1 run_merge 7 "$SHA"
check 6.6 "a closed PR is not-open, exit 5" bash -c '[ "$1" -eq 5 ]' _ "${RC:-0}"
check 6.7 "and says so" result_is not-open

echo "   (a PR already merged at the expected head is merged, with no request)"
reset_fixtures
cp "${F}/rest-pr-after.json" "${F}/rest-pr.json"
GQL_DOWN=1 run_merge 7 "$SHA"
check 6.8 "already merged at the expected head is merged" bash -c '[ "$1" -eq 0 ]' _ "${RC:-1}"
check 6.9 "with no merge request of its own" no_merge_request

echo "== 7. The checks read: pending before failing; 404 is unreadable =="
reset_fixtures
printf '{"total_count":2,"check_runs":[{"name":"ci","status":"in_progress","conclusion":null},{"name":"lint","status":"completed","conclusion":"failure"}]}\n' > "${F}/rest-check-runs.json"
GQL_DOWN=1 run_merge 7 "$SHA"
check 7.1 "pending is tested before failing" test "${RC:-0}" -eq 7
check 7.2 "and reported as checks-pending" result_is checks-pending

reset_fixtures
printf '{"total_count":1,"check_runs":[{"name":"ci","status":"completed","conclusion":"failure"}]}\n' > "${F}/rest-check-runs.json"
GQL_DOWN=1 run_merge 7 "$SHA"
check 7.3 "nothing pending and a failure is checks-not-green" test "${RC:-0}" -eq 3
check 7.4 "and no merge request" no_merge_request

reset_fixtures
printf '{"state":"failure","total_count":1,"statuses":[{"context":"legacy","state":"failure"}]}\n' > "${F}/rest-status.json"
GQL_DOWN=1 run_merge 7 "$SHA"
check 7.5 "a failing combined status also blocks" test "${RC:-0}" -eq 3

reset_fixtures
printf '1' > "${F}/rest-check-runs.rc"; printf 'gh: HTTP 404: Not Found\n' > "${F}/rest-check-runs.err"
GQL_DOWN=1 run_merge 7 "$SHA"
check 7.6 "a 404 on a checks endpoint is unreadable, never an empty list" test "${RC:-0}" -eq 7
check 7.7 "an unreadable check is never 'nothing is failing'" no_merge_request

reset_fixtures
printf '{"total_count":0,"check_runs":[]}\n' > "${F}/rest-check-runs.json"
printf '{"state":"pending","total_count":0,"statuses":[]}\n' > "${F}/rest-status.json"
GQL_DOWN=1 run_merge 7 "$SHA"
check 7.8 "a PR with no checks at all goes on to GitHub's own decision" bash -c '[ "$1" -eq 0 ] || [ "$1" -eq 7 ]' _ "${RC:-1}"

echo "== 8. S10: reasons carry no check names, URLs or gh text from this path =="
reset_fixtures
printf '{"total_count":1,"check_runs":[{"name":"deploy-to-$(id)-prod","status":"in_progress","conclusion":null}]}\n' > "${F}/rest-check-runs.json"
GQL_DOWN=1 run_merge 7 "$SHA"
why="$(reason)"
# Assert the path first, so the rest of the section is not checking the
# wording of some other outcome.
check 8.0 "a pending check at the head is checks-pending" bash -c '[ "$1" -eq 7 ]' _ "${RC:-0}"
check 8.1 "the reason carries no check name from the REST path" bash -c '! printf "%s" "$1" | grep -q "deploy-to"' _ "$why"
check 8.2 "nor a URL" bash -c '! printf "%s" "$1" | grep -q "://"' _ "$why"
check 8.3 "nor gh's own message" bash -c '! printf "%s" "$1" | grep -qi "rate limit\|HTTP [0-9]"' _ "$why"
check 8.4 "the reason is one non-empty line" bash -c '[ -n "$1" ] && [ "$(printf "%s" "$1" | wc -l)" -eq 0 ]' _ "$why"
check 8.5 "and is bounded" bash -c '[ "${#1}" -le 300 ]' _ "$why"

echo
echo "PASS: ${PASS_COUNT}  FAIL: ${FAIL_COUNT}"
[ "${FAIL_COUNT}" -eq 0 ]
