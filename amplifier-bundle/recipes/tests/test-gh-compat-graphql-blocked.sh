#!/usr/bin/env bash
# test-gh-compat-graphql-blocked.sh — issue #1484.
#
# Contract under test (amplifier-bundle/tools/workflow_gh_compat.sh, reached
# through a `gh` launcher first on PATH, the way the recipe runner installs it):
#
#   1. Each call aimed at a GraphQL client first learns whether GraphQL works
#      on its host: one bounded `gh api graphql '{viewer{login}}'` probe, whose
#      answer is remembered per host (blocked for the TTL, works / could not
#      tell for a few minutes). Where it works, the real gh is exec'd: output,
#      exit status and signals are gh's and no REST call is made.
#   2. Where the probe gets the Claude Code GraphQL block (HTTP 403), the call
#      is served over REST (`gh api repos/...`) in the shape callers parse, and
#      later calls go straight to REST. After an inconclusive probe, a read
#      that gh finds blocked is replayed; a write is never replayed.
#   3. pr view/list/create/checks/ready, issue view/list/create, label list,
#      `api graphql` (viewer permission) and `auth status` each issue the
#      expected REST request.
#   4. /search is refused by the proxy; issue search falls back to matching
#      the repo's issues client-side.
#   5. Subcommands that are not GraphQL clients, -h/--help and `label create`
#      are exec'd untouched, without a probe.
#   6. workflow_gh_retry.sh classifies the block as permanent, not a rate limit
#      (a rate limit waits for a reset that never comes).
#   7. --json fields are validated as gh does and never read as a silent null;
#      sub-lists read every page; results over 128 KiB come back whole;
#      closedByPullRequestsReferences is derived; unsupported flags fail.
#   8. Another host (a positional repo/issue/PR URL, HOST/OWNER/REPO,
#      --hostname, GHE remote, GH_HOST) is never answered from github.com; a
#      URL inside a body, title or search is data and selects no host.
#
# Never touches the network: the "real" gh is a stub that records its argv.
# Usage: bash amplifier-bundle/recipes/tests/test-gh-compat-graphql-blocked.sh
# Exit codes: 0 = pass, 1 = fail, 2 = harness error.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
COMPAT="${REPO_ROOT}/amplifier-bundle/tools/workflow_gh_compat.sh"
RETRY="${REPO_ROOT}/amplifier-bundle/tools/workflow_gh_retry.sh"

for f in "$COMPAT" "$RETRY"; do
  [ -f "$f" ] || { echo "HARNESS-ERROR: $f not found" >&2; exit 2; }
done
command -v jq >/dev/null 2>&1 || { echo "HARNESS-ERROR: jq required" >&2; exit 2; }
command -v git >/dev/null 2>&1 || { echo "HARNESS-ERROR: git required" >&2; exit 2; }

WORK="$(mktemp -d -t gh-compat-XXXXXX)"
trap 'rm -rf "${WORK}"' EXIT

# --- the "real" gh: GraphQL blocked unless STUB_GRAPHQL_OK=1 -----------------
mkdir -p "${WORK}/real" "${WORK}/launcher"
export STUB_LOG="${WORK}/gh.log"
cat > "${WORK}/real/gh" <<'STUB'
#!/usr/bin/env bash
# `gh GROUP VERB --json` alone lists the fields, offline (a newer gh's lists).
if [ $# = 3 ] && [ "$3" = --json ]; then
  echo 'Specify one or more comma-separated fields for `--json`:' >&2
  case "$1 $2" in
    "pr view"|"pr list") f="additions assignees author autoMergeRequest baseRefName baseRefOid body changedFiles closed closedAt closingIssuesReferences comments commits createdAt deletions files fullDatabaseId headRefName headRefOid headRepository headRepositoryOwner id isCrossRepository isDraft labels latestReviews maintainerCanModify mergeCommit mergeStateStatus mergeable mergedAt mergedBy milestone number potentialMergeCommit projectCards projectItems reactionGroups reviewDecision reviewRequests reviews state statusCheckRollup title updatedAt url" ;;
    "issue view"|"issue list") f="assignees author body closed closedAt closedByPullRequestsReferences comments createdAt id isPinned labels milestone number projectCards projectItems reactionGroups state stateReason title updatedAt url" ;;
    "pr checks") f="bucket completedAt description event link name startedAt state workflow" ;;
    "label list") f="color createdAt description id isDefault name updatedAt url" ;;
  esac
  # gh 2.63 (what cloud sessions have) predates these two.
  [ "${STUB_OLD_GH:-0}" = 1 ] && f="$(printf '%s' "$f" | tr ' ' '\n' | grep -v -e closingIssuesReferences -e closedByPullRequestsReferences | tr '\n' ' ')"
  for x in $f; do echo "  $x" >&2; done
  exit 1
fi
printf '%s\n' "$*" >> "$STUB_LOG"
if [ "$1" = "api" ]; then
  method=GET; path=""; input=""; paginate=0; host=github.com
  shift
  while [ $# -gt 0 ]; do
    case "$1" in
      -X) method="$2"; shift 2 ;;
      -H) shift 2 ;;
      -f|-F) shift 2 ;;
      --input) input="$2"; shift 2 ;;
      --paginate) paginate=1; shift ;;
      --hostname) host="$2"; shift 2 ;;
      *) path="$1"; shift ;;
    esac
  done
  [ -n "$input" ] && printf 'BODY %s\n' "$(jq -c . "$input")" >> "$STUB_LOG"
  if [ "$path" = graphql ]; then
    # Probe edge cases: a one-off 502, a rate limit, a slow endpoint, a GHE host.
    if [ -n "${STUB_PROBE_502_ONCE:-}" ] && [ ! -e "$STUB_PROBE_502_ONCE" ]; then
      : > "$STUB_PROBE_502_ONCE"; echo "HTTP 502: Bad Gateway (https://api.github.com/graphql)" >&2; exit 1
    fi
    [ "${STUB_PROBE_RATELIMIT:-0}" = 1 ] && { echo "GraphQL: API rate limit exceeded for user ID 1." >&2; exit 1; }
    [ -n "${STUB_PROBE_SLEEP:-}" ] && sleep "$STUB_PROBE_SLEEP"
    [ "$host" != github.com ] && [ "${STUB_GHE_OK:-0}" = 1 ] && { echo '{"data":{"viewer":{"login":"ghe"}}}'; exit 0; }
    if [ "${STUB_GRAPHQL_OK:-0}" = 1 ]; then echo '{"data":{"viewer":{"login":"bot"}}}'; exit 0; fi
    echo "HTTP 403: GitHub GraphQL is not available from Claude Code sessions; use the REST API (gh api repos/{owner}/{repo}/...). (https://api.github.com/graphql)" >&2; exit 1
  fi
  pr='{"number":42,"node_id":"PR_1","title":"feat: widget","body":"Fixes #7","state":"open","draft":true,"merged_at":null,"html_url":"https://github.com/o/r/pull/42","created_at":"2026-01-01T00:00:00Z","user":{"login":"bot"},"labels":[],"mergeable":true,"mergeable_state":"clean","head":{"ref":"feat","sha":"abc123","repo":{"name":"r","full_name":"o/r","owner":{"login":"o"}}},"base":{"ref":"main","sha":"def456","repo":{"full_name":"o/r"}}}'
  # Fork PR (head in fork/r) and odd head branch names, for --delete-branch.
  [ "${STUB_FORK_PR:-0}" = 1 ] && pr="$(printf '%s' "$pr" | jq -c '.head.repo = {name: "r", full_name: "fork/r", owner: {login: "fork"}} | .head.ref = "release-1"')"
  [ -n "${STUB_HEAD:-}" ] && pr="$(printf '%s' "$pr" | jq -c --arg h "$STUB_HEAD" '.head.ref = $h')"
  # Results past Linux's 128 KiB per-argument limit (MAX_ARG_STRLEN).
  big() { jq -nc --argjson n "$1" '[range(0; $n) | {id: ., node_id: "C\(.)", user: {login: "a"}, body: ("x" * 60000)}]'; }
  case "$method $path" in
    "GET repos/o/r/issues/5/comments"*) [ "${STUB_BIG:-0}" = 1 ] && { big 3; exit 0; } ;;
    "GET repos/o/r/issues/42/comments"*) [ "${STUB_BIG:-0}" = 1 ] && { big 3; exit 0; } ;;
    "GET repos/o/r/pulls/42/files"*) [ "${STUB_BADJSON:-0}" = 1 ] && { echo '[{"filename": trunc'; exit 0; } ;;
  esac
  case "$method $path" in
    "GET repos/o/r/pulls/42")
      if [ -n "${STUB_BASE:-}" ]; then printf '%s\n' "$pr" | jq -c --arg b "$STUB_BASE" '.base.ref = $b | .base.repo.default_branch = "main"'
      else printf '%s\n' "$pr"; fi ;;
    # --paginate prints each page's JSON in turn.
    "GET repos/o/r/pulls/42/files"*)
      printf '[{"filename":"a.rs","additions":1,"deletions":0}]\n'
      [ "$paginate" = 1 ] && printf '[{"filename":"b.rs","additions":2,"deletions":1}]\n' ;;
    "GET repos/o/r/issues/5") printf '{"number":5,"title":"Fix the flaky widget","body":"","state":"open","html_url":"https://github.com/o/r/issues/5","user":{"login":"bot"},"labels":[]}\n' ;;
    "GET repos/o/r/issues/42/comments"*) printf '[{"id":76,"user":{"login":"bot"},"body":"older"},{"id":77,"user":{"login":"bot"},"body":"old"},{"id":78,"user":{"login":"else"},"body":"theirs"}]\n' ;;
    "PATCH repos/o/r/issues/comments/77") printf '{"html_url":"https://github.com/o/r/pull/42#issuecomment-77"}\n' ;;
    "GET repos/o/r/milestones"*) printf '[{"number":3,"title":"v1"}]\n' ;;
    "PATCH repos/o/r/issues/43") printf '{}\n' ;;
    "GET repos/o/r/issues/5/comments"*) printf '[{"node_id":"C1","user":{"login":"a"},"body":"x"},{"node_id":"C2","user":{"login":"b"},"body":"y"}]\n' ;;
    "GET repos/o/r/issues/7/timeline"*)
      xr() { printf '{"event":"cross-referenced","source":{"type":"issue","issue":{"number":%s,"node_id":"PR_%s","html_url":"https://github.com/%s/pull/%s","state":"%s","body":"%s","pull_request":{"merged_at":%s},"repository":{"full_name":"%s","name":"%s","owner":{"login":"%s"}}}}}' \
        "$1" "$1" "$2" "$1" "$3" "$4" "$5" "$2" "${2#*/}" "${2%/*}"; }
      printf '[%s,%s,%s]\n' "$(xr 42 o/r open 'Fixes #7' null)" "$(xr 43 o/r closed 'Fixes #7' null)" "$(xr 44 o/r open 'see #7' null)"
      [ "$paginate" = 1 ] && printf '[%s,%s,%s,%s]\n' "$(xr 45 o/r closed 'Closes #7' '"2026-01-01T00:00:00Z"')" "$(xr 9 o2/r2 open 'Fixes o/r#7' null)" "$(xr 10 o2/r2 open 'Fixes #7' null)" "$(xr 46 o/r open 'Fixes #7' null)" ;;
    "GET repos/o/r/labels?"*) printf '[{"id":2,"node_id":"L2","name":"bug","color":"f00","description":"Something broken","default":true},{"id":1,"node_id":"L1","name":"docs","color":"0f0","description":null,"default":false}]\n' ;;
    "GET repos/o/r/actions/runs?head_sha=abc123"*)
      [ "${STUB_NO_ACTIONS:-0}" = 1 ] && { echo "gh: Resource not accessible by integration (HTTP 403)" >&2; exit 1; }
      printf '{"workflow_runs":[{"id":99,"name":"CI","event":"pull_request"}]}\n' ;;
    "GET repos/o/r/pulls/42/reviews"*) printf '[{"user":{"login":"a"},"state":"CHANGES_REQUESTED","submitted_at":"2026-01-01T00:00:00Z"},{"user":{"login":"b"},"state":"APPROVED","submitted_at":"2026-01-02T00:00:00Z"},{"user":{"login":"b"},"state":"COMMENTED","submitted_at":"2026-01-03T00:00:00Z"}]\n' ;;
    "GET repos/o/r/pulls?"*"head=o%3Afeat"*) printf '[%s]\n' "$pr" ;;
    "POST repos/o/r/pulls")
      if [ "${STUB_PR_EXISTS:-0}" = 1 ]; then
        printf '{"message":"Validation Failed","errors":[{"message":"A pull request already exists for o:feat."}]}'
        echo "gh: Validation Failed (HTTP 422)" >&2; exit 1
      fi
      printf '%s\n' "$pr" | jq -c '.number = 43 | .html_url = "https://github.com/o/r/pull/43"' ;;
    "GET repos/o/r") printf '{"full_name":"o/r","default_branch":"main","permissions":{"admin":false,"maintain":false,"push":true,"triage":true,"pull":true}}\n' ;;
    "POST repos/o/r/issues") printf '{"number":8,"html_url":"https://github.com/o/r/issues/8"}\n' ;;
    "GET repos/o/r/issues/7") printf '{"number":7,"title":"Widget","body":"","state":"open","html_url":"https://github.com/o/r/issues/7","user":{"login":"bot"},"labels":[]}\n' ;;
    "GET search/"*) printf '{"message":"This GitHub API path is not available"}'; echo "gh: This GitHub API path is not available: sessions are bound to their configured repositories. (HTTP 403)" >&2; exit 1 ;;
    "GET repos/o/r/issues?"*) printf '[{"number":5,"title":"Fix the flaky widget","body":"","state":"open","html_url":"https://github.com/o/r/issues/5","user":{"login":"bot"},"labels":[]},{"number":6,"title":"Fix the flaky widget","pull_request":{},"state":"open","html_url":"https://github.com/o/r/pull/6","user":{"login":"bot"},"labels":[]}]\n' ;;
    "GET repos/o/r/commits/abc123/check-runs"*)
      case "${STUB_CHECKS:-pass}" in
        pending) printf '{"check_runs":[{"name":"Test","status":"in_progress","conclusion":null,"details_url":"u"},{"name":"Lint","status":"completed","conclusion":"success","details_url":"u"}]}\n' ;;
        fail) printf '{"check_runs":[{"name":"Test","status":"completed","conclusion":"failure","details_url":"u"}]}\n' ;;
        actions) printf '{"check_runs":[{"name":"Test","status":"completed","conclusion":"success","details_url":"https://github.com/o/r/actions/runs/99/job/1","output":{"title":"All green"}}]}\n' ;;
        *) printf '{"check_runs":[{"name":"Test","status":"completed","conclusion":"success","details_url":"u"}]}\n' ;;
      esac ;;
    "GET repos/o/r/commits/abc123/status"*) printf '{"statuses":[]}\n' ;;
    "POST repos/o/r/pulls/42/ccr/ready_for_review") printf '{}\n' ;;
    "POST repos/o/r/issues/42/comments") printf '{"html_url":"https://github.com/o/r/pull/42#issuecomment-1"}\n' ;;
    "GET repos/o/r/pulls?"*"head=o%3Afix%2Ba%26b"*) printf '[]\n' ;;
    "POST repos/o/r/labels") printf '{"message":"Validation Failed"}'; echo "gh: Validation Failed (HTTP 422)" >&2; exit 1 ;;
    "GET user") printf '{"login":"bot"}\n' ;;
    "PUT repos/o/r/pulls/42/merge") printf '{"merged":true}\n' ;;
    # Open PRs for the client-side search fallback: #51 first, then draft #50.
    "GET repos/o/r/pulls?state=open&per_page="*)
      printf '[%s,%s,%s]\n' \
        '{"number":51,"state":"open","draft":false,"title":"t","user":{"login":"bot"},"labels":[{"name":"x"},{"name":"y"}],"assignees":[],"head":{"ref":"b"},"base":{"ref":"main"}}' \
        '{"number":50,"state":"open","draft":true,"title":"t","user":{"login":"bot"},"labels":[{"name":"x"}],"assignees":[{"login":"a"}],"head":{"ref":"b"},"base":{"ref":"main"}}' \
        '{"number":52,"state":"open","draft":false,"title":"t","user":{"login":"other"},"labels":[],"assignees":[],"head":{"ref":"b"},"base":{"ref":"main"}}' ;;
    "GET repos/o/r/pulls?state=closed"*)
      # Paged: per_page=3; page 1 holds one merged PR of three, page 2 two of three.
      pg="${path##*page=}"
      jq -nc --argjson p "$pg" '[range(0;3) | {number: ($p * 10 + .), state: "closed", title: "t", html_url: "u", user: {login: "bot"}, head: {ref: "b", sha: "s", repo: null}, base: {ref: "main", sha: "m", repo: null},
        merged_at: (if ($p == 1 and . == 0) or ($p == 2 and . > 0) then "2026-01-01T00:00:00Z" else null end)}]' ;;
    # Any other PR: into the default branch, except #46 (a release branch).
    "GET repos/"*"/pulls/"[0-9]*)
      n="${path##*/}"; b=main; [ "$n" = 46 ] && b=release; d=false; [ "$n" = 50 ] && d=true
      printf '{"number":%s,"draft":%s,"base":{"ref":"%s","repo":{"default_branch":"main"}}}\n' "$n" "$d" "$b" ;;
    *) printf '{"message":"Not Found"}'; echo "gh: Not Found (HTTP 404)" >&2; exit 1 ;;
  esac
  exit 0
fi
case "$1 ${2:-}" in
  "auth status") echo "  X The token in GH_TOKEN is invalid." >&2; exit 1 ;;
  "run list") echo "real-run-list"; exit 0 ;;
esac
# A long-running call (think `pr checks --watch`) that reports progress on
# stderr and only finishes once the test has seen that progress.
if [ "${STUB_STREAM:-}" != "" ]; then
  [ -n "${STUB_PIDFILE:-}" ] && echo $$ > "$STUB_PIDFILE"
  echo "progress: 1 pending" >&2
  i=0; while [ ! -e "$STUB_STREAM" ] && [ "$i" -lt 50 ]; do sleep 0.1; i=$((i + 1)); done
  [ -e "$STUB_STREAM" ] && { echo "done"; exit 0; }
  exit 3
fi
# An older gh refuses a field it does not know before it reaches GitHub.
if [ "${STUB_OLD_GH:-0}" = 1 ]; then
  case " $* " in *closingIssuesReferences*|*closedByPullRequestsReferences*)
    echo 'Unknown JSON field: "closingIssuesReferences"' >&2; exit 1 ;;
  esac
fi
# stderr edge cases for the pass-through mode (#1499).
if [ "${STUB_NOISY:-}" != "" ]; then
  i=0; while [ "$i" -lt "$STUB_NOISY" ]; do echo "noise line $i" >&2; i=$((i + 1)); done
  echo "noisy-done"; exit 0
fi
for a in "$@"; do case "$a" in -h|--help) echo "help for $1 $2"; exit 0 ;; esac; done
[ "${STUB_GH_502:-0}" = 1 ] && { echo "HTTP 502: Bad Gateway (comment may have been posted)" >&2; exit 1; }
[ "${STUB_ENV:-0}" = 1 ] && { echo "GHC_REPO=${GHC_REPO-unset}"; exit 0; }
[ "${STUB_GHE_OK:-0}" = 1 ] && [ "${GH_HOST:-}" = ghe.example.com ] && { echo "ghe-output: $*"; exit 0; }
if [ "${STUB_INTERLEAVE:-0}" = 1 ]; then
  echo err1 >&2; echo out1; echo err2 >&2; echo out2; exit 0
fi
if [ "${STUB_FORK:-0}" = 1 ]; then sleep 5 >/dev/null & echo "forked"; exit 0; fi
if [ "${STUB_WARN_BLOCK:-0}" = 1 ]; then
  echo "HTTP 403: GitHub GraphQL is not available from Claude Code sessions" >&2; echo "answered-anyway"; exit 0
fi
if [ "${STUB_PROMPT:-}" != "" ]; then
  printf 'Continue? ' >&2
  i=0; while [ ! -e "$STUB_PROMPT" ] && [ "$i" -lt 50 ]; do sleep 0.1; i=$((i + 1)); done
  [ -e "$STUB_PROMPT" ] && { echo "done"; exit 0; }
  exit 3
fi
# Like the real gh, a `--body-file -` call drains stdin before the request fails.
[ "${STUB_READS_STDIN:-0}" = 1 ] && cat >/dev/null
if [ "${STUB_GRAPHQL_OK:-0}" = 1 ]; then echo "real-gh-output: $*"; exit 0; fi
echo "HTTP 403: GitHub GraphQL is not available from Claude Code sessions; use the REST API (gh api repos/{owner}/{repo}/...). (https://api.github.com/graphql)" >&2
exit 1
STUB
chmod +x "${WORK}/real/gh"

# --- the launcher, as crates/.../recipe/run/gh_compat.rs writes it -----------
: > "${WORK}/launcher/.amplihack-gh-compat"
printf '#!/bin/sh\nexec bash %s "$@"\n' "'${COMPAT}'" > "${WORK}/launcher/gh"
chmod +x "${WORK}/launcher/gh"

# --- a checkout on branch feat with a github.com origin ----------------------
git init -q -b feat "${WORK}/repo"
git -C "${WORK}/repo" remote add origin https://github.com/o/r.git
cd "${WORK}/repo"

export PATH="${WORK}/launcher:${WORK}/real:${PATH}"
export TMPDIR="${WORK}"
export AMPLIHACK_GH_COMPAT_STATE="${WORK}/graphql-blocked"
export AMPLIHACK_GH_COMPAT_LOG="${WORK}/compat.log"
unset AMPLIHACK_GH_REST_ONLY GH_REPO AMPLIHACK_REAL_GH || true

PASS=0
fail() { echo "FAIL[$1]: $2" >&2; echo "--- gh.log ---" >&2; cat "$STUB_LOG" >&2 || true; exit 1; }
ok() { echo "  ok  $1"; PASS=$((PASS + 1)); }
reset_log() { : > "$STUB_LOG"; }
logged() { grep -Fqx -- "$1" "$STUB_LOG"; }
logged_prefix() { grep -Fq -- "$1" "$STUB_LOG"; }

echo "issue #1484: gh compatibility layer when GraphQL is blocked"

# 1. GraphQL works -> untouched pass-through, no REST, no state.
reset_log
out="$(STUB_GRAPHQL_OK=1 gh pr view 42 --json state)"
[ "$out" = "real-gh-output: pr view 42 --json state" ] || fail pass-through "got '$out'"
logged_prefix "api -X" && fail pass-through "REST was called although GraphQL works"
logged "api graphql -f query={viewer{login}}" || fail pass-through "GraphQL was not probed"
[ -e "${AMPLIHACK_GH_COMPAT_STATE}.ok" ] || fail pass-through "a working GraphQL was not recorded"
reset_log
STUB_GRAPHQL_OK=1 gh pr view 42 --json state >/dev/null
logged_prefix "api " && fail pass-through "a recorded working host was probed again"
rm -f "${AMPLIHACK_GH_COMPAT_STATE}.ok"
[ ! -e "$AMPLIHACK_GH_COMPAT_STATE" ] || fail pass-through "block state recorded although GraphQL works"
ok "GraphQL available: real gh output passes through, no REST call"

# 2. First blocked call: detected, replayed over REST, remembered.
reset_log
out="$(gh pr view 42 --json headRefName -q .headRefName)"
[ "$out" = "feat" ] || fail detect "pr view -q .headRefName gave '$out'"
logged "api graphql -f query={viewer{login}}" || fail detect "GraphQL was not probed first"
logged "api -X GET repos/o/r/pulls/42" || fail detect "no REST GET repos/o/r/pulls/42"
[ -e "$AMPLIHACK_GH_COMPAT_STATE" ] || fail detect "block not remembered"
ok "403 detected once, pr view served by GET repos/o/r/pulls/42"

reset_log
json="$(gh pr view https://github.com/o/r/pull/42 --json number,state,isDraft,headRefOid,isCrossRepository,closingIssuesReferences)"
logged_prefix "api graphql" && fail remembered "GraphQL re-probed after the block was recorded"
[ "$(printf '%s' "$json" | jq -c '[.number,.state,.isDraft,.headRefOid,.isCrossRepository,.closingIssuesReferences[0].number]')" = '[42,"OPEN",true,"abc123",false,7]' ] \
  || fail shape "pr view JSON shape: $json"
ok "later calls go straight to REST; pr view JSON keeps gh field names"

# 3. pr list by head branch.
reset_log
n="$(gh pr list --head feat --state all --json number,url --jq '.[0].number')"
[ "$n" = 42 ] || fail pr-list "got '$n'"
logged "api -X GET repos/o/r/pulls?state=all&head=o%3Afeat&per_page=30&page=1" || fail pr-list "unexpected REST path"
ok "pr list --head -> GET repos/o/r/pulls?head=o%3Afeat"

# 4. pr create --draft uses the current branch and the default base.
reset_log
url="$(gh pr create --draft --title "feat: widget" --body "Fixes #7")"
[ "$url" = "https://github.com/o/r/pull/43" ] || fail pr-create "stdout was '$url'"
logged 'BODY {"title":"feat: widget","body":"Fixes #7","head":"feat","base":"main","draft":true}' \
  || fail pr-create "POST body not as expected"
ok "pr create --draft -> POST repos/o/r/pulls, prints the PR URL"

# 4b. A create collision surfaces gh's own message, which publish's #1017
#     recovery path keys on; the HTTP status must survive the $(...) capture.
rc=0; msg="$(STUB_PR_EXISTS=1 gh pr create --draft --title t --body b 2>&1)" || rc=$?
[ "$rc" = 1 ] || fail pr-exists "exit $rc, want 1"
case "$msg" in *'a pull request for branch "feat" into branch "main" already exists'*) ;; *) fail pr-exists "message was: $msg" ;; esac
ok "pr create collision (422) -> gh's 'already exists' message, exit 1"

# 5. pr checks exit codes: 0 pass, 8 pending, 1 fail.
for case_ in "pass 0" "pending 8" "fail 1"; do
  set -- $case_
  rc=0; STUB_CHECKS="$1" gh pr checks 42 >/dev/null 2>&1 || rc=$?
  [ "$rc" = "$2" ] || fail pr-checks "checks=$1 exited $rc, want $2"
done
ok "pr checks: 0 all pass, 8 pending, 1 failed (gh's codes)"

# 6. pr ready uses the CCR REST route.
reset_log
gh pr ready 42 2>/dev/null || fail pr-ready "exited non-zero"
logged "api -X POST repos/o/r/pulls/42/ccr/ready_for_review" || fail pr-ready "no ccr/ready_for_review POST"
ok "pr ready -> POST .../pulls/42/ccr/ready_for_review"

# 7. issue view / create, and search falling back to a client-side match.
reset_log
[ "$(gh issue view 7 --json url --jq '.url // ""')" = "https://github.com/o/r/issues/7" ] || fail issue-view "wrong url"
url="$(gh issue create --title "Widget" --body "b" --label workflow:default 2>&1)"
[ "$url" = "https://github.com/o/r/issues/8" ] || fail issue-create "stdout+stderr was '$url' (callers capture 2>&1)"
logged 'BODY {"title":"Widget","body":"b","labels":["workflow:default"]}' || fail issue-create "POST body not as expected"
found="$(gh issue list --state open --search "flaky widget" --json url --jq '.[0].url // ""')"
[ "$found" = "https://github.com/o/r/issues/5" ] || fail issue-search "search fallback gave '$found' (PRs must be excluded)"
miss="$(gh issue list --state open --search "wid" --json url --jq '.[0].url // ""')"
[ -z "$miss" ] || fail issue-search "search fallback matched a word fragment: '$miss'"
ok "issue view/create over REST; blocked /search falls back to repo issues"

# 8. label create is REST in gh itself: handed to the real gh, not replayed.
reset_log; gh label create workflow:default --color 0366d6 >/dev/null 2>&1 || true
logged "label create workflow:default --color 0366d6" || fail label "label create not handed to the real gh"
logged_prefix "api " && fail label "label create was probed or replayed"
ok "label create goes to the real gh (REST there) untouched"

# 9. api graphql viewer permission (the identity preflight query).
resp="$(gh api graphql --hostname github.com -f owner=o -f name=r -f query='query($owner:String!,$name:String!){viewer{login} repository(owner:$owner,name:$name){nameWithOwner viewerPermission}}')"
[ "$(printf '%s' "$resp" | jq -r '.data.viewer.login + " " + .data.repository.viewerPermission')" = "bot WRITE" ] \
  || fail graphql "got $resp"
rc=0; gh api graphql -f query='{ rateLimit { remaining } }' >/dev/null 2>&1 || rc=$?
[ "$rc" != 0 ] || fail graphql "an untranslatable query must fail, not fake success"
# "reviewer" contains "viewer": a substring match would fake this one.
rc=0; gh api graphql -f query='{repository(owner:"o",name:"r"){pullRequest(number:42){reviewRequests(first:9){nodes{requestedReviewer{... on User{login}}}}}}}' >/dev/null 2>&1 || rc=$?
[ "$rc" != 0 ] || fail graphql "a reviewer query was answered with a bare viewer object"
[ "$(gh api graphql -f query='query { viewer { login } }' --jq .data.viewer.login)" = bot ] || fail graphql "--jq not applied to viewer answer"
ok "api graphql: viewerPermission answered from REST; other queries still fail"

# 10. auth status: gh rejects the proxy token, REST /user accepts it.
gh auth status >/dev/null 2>&1 || fail auth "auth status should pass when REST /user answers"
ok "auth status verified over REST"

# 11. Non-GraphQL subcommands are exec'd untouched.
[ "$(gh run list)" = "real-run-list" ] || fail exec "gh run list not passed through"
ok "other subcommands pass straight through"

# 12. The retry classifier treats the block as permanent.
blk="${WORK}/blk.stderr"
echo "HTTP 403: GitHub GraphQL is not available from Claude Code sessions" > "$blk"
# shellcheck source=/dev/null
class="$(. "$RETRY"; classify_gh_error "$blk")"
[ "$class" = other ] || fail classify "GraphQL block classified as '$class', must not be rate_limit"
ok "workflow_gh_retry.sh: GraphQL block is not a rate limit"

# 13. `--body-file -` on the call that first detects the block: the probe must
#     not swallow stdin, or the REST replay posts an empty body.
rm -f "$AMPLIHACK_GH_COMPAT_STATE"; reset_log
url="$(printf 'body from stdin' | STUB_READS_STDIN=1 gh pr comment 42 --body-file -)"
[ "$url" = "https://github.com/o/r/pull/42#issuecomment-1" ] || fail stdin "stdout was '$url'"
logged 'BODY {"body":"body from stdin"}' || fail stdin "stdin body lost between the probe and the REST replay"
ok "--body-file - survives the GraphQL probe"

# 14. Branch names are percent-encoded in REST queries.
reset_log
rc=0; gh pr view 'fix+a&b' --json number >/dev/null 2>&1 || rc=$?
[ "$rc" = 1 ] || fail uri "unknown branch should exit 1, got $rc"
logged "api -X GET repos/o/r/pulls?head=o%3Afix%2Ba%26b&state=all&per_page=30" || fail uri "head= not percent-encoded"
ok "branch names are percent-encoded in ?head="

# 15. Per-call scratch files are private and cleaned up.
leftover="$(find "$WORK" -maxdepth 1 -name 'ghc*' -print -quit)"
[ -z "$leftover" ] || fail cleanup "scratch left behind: $leftover"
ok "no gh-compat scratch files left in TMPDIR"

# 16. reviewDecision comes from each reviewer's latest decisive review, never a
#     constant "" that a merge gate would read as "nothing blocking".
[ "$(gh pr view 42 --json reviewDecision --jq .reviewDecision)" = CHANGES_REQUESTED ] || fail review "reviewDecision did not surface CHANGES_REQUESTED"
ok "pr view reviewDecision derived from REST reviews"

# 17. An issue URL names its own repository.
reset_log
gh issue view https://github.com/o2/r2/issues/7 --json url >/dev/null 2>&1 || true
logged "api -X GET repos/o2/r2/issues/7" || fail issue-url "issue URL resolved against the checkout's repo"
ok "issue URL targets its own repository"

# 18. --template cannot be honoured over REST: fail, do not emit JSON instead.
rc=0; gh pr view 42 --json title --template '{{.title}}' >/dev/null 2>&1 || rc=$?
[ "$rc" = 1 ] || fail template "--template exited $rc, want 1"
ok "--template fails instead of printing JSON"

# ---------------------------------------------------------------------------
# Review follow-ups (PR #1497 review, comment 5837593243).
# ---------------------------------------------------------------------------

# 19. step-03 falls back to local tracking ONLY where gh cannot reach GitHub
#     issues at all; a real failure on a working host stays fatal.
TRACK="${REPO_ROOT}/amplifier-bundle/tools/workflow_issue_tracking.sh"
# shellcheck source=/dev/null
unsupported() { ( . "$TRACK"; issue_create_host_unsupported "$1" "$2" ); }
unsupported 1 "HTTP 403: GitHub GraphQL is not available from Claude Code sessions; use the REST API" || fail step03 "GraphQL block not recognised"
unsupported 127 "timeout: failed to run command 'gh': No such file or directory" || fail step03 "missing gh not recognised"
unsupported 1 "gh-compat: 'gh issue pin' needs GitHub GraphQL, which this host blocks, and has no REST fallback" || fail step03 "compat refusal not recognised"
unsupported 1 "HTTP 403: Resource not accessible by integration" && fail step03 "a permission error on a working host must stay fatal"
unsupported 1 "GraphQL: Issues are disabled for this repo (createIssue)" && fail step03 "issues-disabled must stay fatal"
unsupported 1 "could not add label: 'x' not found" && fail step03 "a bad payload must stay fatal"
grep -Fq 'issue_create_host_unsupported "$GH_ISSUE_RC" "$GH_ISSUE_OUTPUT"' "${REPO_ROOT}/amplifier-bundle/recipes/workflow-prep.yaml" \
  || fail step03 "workflow-prep step-03 no longer gates its local fallback on issue_create_host_unsupported"
ok "step-03 local fallback limited to GraphQL-blocked / gh-unavailable hosts"

# 20. Attached and explicitly-empty flag values.
reset_log
gh issue view 7 -Ro2/r2 --json url >/dev/null 2>&1 || true
logged "api -X GET repos/o2/r2/issues/7" || fail flags "-Ro2/r2 ignored (fell back to the origin repo)"
reset_log
gh issue view 7 --repo=o2/r2 --json url >/dev/null 2>&1 || true
logged "api -X GET repos/o2/r2/issues/7" || fail flags "--repo=o2/r2 ignored"
reset_log
gh issue view 7 -R o2/r2 --json url >/dev/null 2>&1 || true
logged "api -X GET repos/o2/r2/issues/7" || fail flags "-R o2/r2 ignored"
reset_log
gh issue create --title T --body= --label x >/dev/null 2>&1 || fail flags "issue create --body= failed"
logged 'BODY {"title":"T","body":"","labels":["x"]}' || fail flags "--body= swallowed the next argument"
[ "$(gh api graphql -fquery='query { viewer { login } }' --jq=.data.viewer.login)" = bot ] || fail flags "attached -fquery=/--jq= not parsed"
ok "flags: -Ro/r, --repo=o/r, -R o/r and --body= (empty) parse like gh"

# 21. auth status: the real output and exit code stand unless GraphQL is blocked.
rm -f "$AMPLIHACK_GH_COMPAT_STATE" "${AMPLIHACK_GH_COMPAT_STATE}.ok"
rc=0; err="$(STUB_GRAPHQL_OK=1 gh auth status 2>&1 >/dev/null)" || rc=$?
[ "$rc" = 1 ] || fail auth "auth status on a working-GraphQL host exited $rc, want gh's 1"
case "$err" in *"The token in GH_TOKEN is invalid."*) ;; *) fail auth "real gh output discarded: '$err'" ;; esac
[ ! -e "$AMPLIHACK_GH_COMPAT_STATE" ] || fail auth "block recorded although GraphQL works"
out="$(gh auth status 2>/dev/null)" || fail auth "blocked host: auth status should pass when REST /user answers"
case "$out" in *"Logged in to github.com account bot"*) ;; *) fail auth "blocked host: got '$out'" ;; esac
[ -e "$AMPLIHACK_GH_COMPAT_STATE" ] || fail auth "a confirmed block was not recorded"
rc=0; gh auth status --hostname ghe.example.com >/dev/null 2>&1 || rc=$?
[ "$rc" = 1 ] || fail auth "another host must keep gh's own answer"
ok "auth status: substituted only when GraphQL is confirmed blocked"

# 22. The block marker: private per-user default, and it expires.
(
  unset AMPLIHACK_GH_COMPAT_STATE
  reset_log
  gh pr view 42 --json number >/dev/null 2>&1 || fail state "pr view failed"
  d="${TMPDIR}/amplihack-gh-compat-$(id -u)"
  [ -f "$d/graphql-blocked" ] || fail state "default marker not in the per-user dir"
  [ "$(ls -ld "$d" | cut -c1-10)" = drwx------ ] || fail state "per-user dir is not private: $(ls -ld "$d")"
  reset_log
  gh pr view 42 --json number >/dev/null 2>&1
  logged_prefix "api graphql" && fail state "a fresh marker should skip the probe"
  touch -t 200001010000 "$d/graphql-blocked"
  reset_log
  gh pr view 42 --json number >/dev/null 2>&1
  logged "api graphql -f query={viewer{login}}" || fail state "an expired marker must re-probe GraphQL"
  exit 0
) || exit 1
ok "block marker: private per-user dir by default, re-probed after the TTL"

# 23. pr merge without a method refuses, as non-interactive gh does.
reset_log
rc=0; msg="$(gh pr merge 42 2>&1)" || rc=$?
[ "$rc" = 1 ] || fail merge "pr merge without a method exited $rc"
case "$msg" in *"--merge, --rebase, or --squash required"*) ;; *) fail merge "message was: $msg" ;; esac
logged_prefix "api -X PUT" && fail merge "merged without a method"
gh pr merge 42 --squash 2>/dev/null || fail merge "pr merge --squash failed"
logged 'BODY {"merge_method":"squash"}' || fail merge "--squash not sent"
ok "pr merge: no method flag refuses; --squash merges by squash"

# 24. assignee is percent-encoded; --head OWNER:BRANCH keeps the fork owner.
reset_log
gh issue list --assignee 'a b&c' --json number >/dev/null 2>&1 || true
logged_prefix "assignee=a%20b%26c" || fail encode "assignee not percent-encoded"
reset_log
gh pr list --head fork:feat --json number >/dev/null 2>&1 || true
logged_prefix "head=fork%3Afeat" || fail head "--head fork:feat lost the fork owner"
ok "assignee percent-encoded; --head fork:branch keeps its owner"

# 25. Pagination: --limit counts what survives the filter (merged), across pages.
reset_log
nums="$(gh pr list --state merged --limit 3 --json number --jq '[.[].number] | join(",")')"
[ "$nums" = "10,21,22" ] || fail paging "merged --limit 3 gave '$nums'"
logged_prefix "pulls?state=closed&per_page=3&page=2" || fail paging "second page not read"
ok "pr list pages through REST until --limit items survive the filter"

# 26. Pass-through mode streams the real gh's stderr instead of holding it.
rm -f "$AMPLIHACK_GH_COMPAT_STATE" "${AMPLIHACK_GH_COMPAT_STATE}.ok"
go="${WORK}/stream.go"; errlog="${WORK}/stream.err"; : > "$errlog"
STUB_GRAPHQL_OK=1 STUB_STREAM="$go" gh pr checks 42 --watch >/dev/null 2>"$errlog" &
bg=$!
i=0; while ! grep -q progress "$errlog" && [ "$i" -lt 40 ]; do sleep 0.1; i=$((i + 1)); done
grep -q progress "$errlog" || { kill "$bg" 2>/dev/null; fail stream "stderr held back until exit"; }
touch "$go"
rc=0; wait "$bg" || rc=$?
[ "$rc" = 0 ] || fail stream "streamed call exited $rc"
ok "pass-through stderr is streamed, not buffered until exit"

# 27. --author @me in the /search fallback means the REST login, not a literal "@me".
: > "$AMPLIHACK_GH_COMPAT_STATE"   # back to a blocked host after 26's working one
reset_log
nums="$(gh issue list --author @me --limit 1 --json number --jq '[.[].number] | join(",")')"
[ "$nums" = 5 ] || fail author-me "--author @me gave '$nums'"
logged_prefix "issues?state=open&per_page=100&page=1" || fail author-me "search fallback read a short page"
nums="$(gh issue list --author someone-else --json number --jq 'length')"
[ "$nums" = 0 ] || fail author-me "--author someone-else matched $nums"
ok "search fallback resolves --author @me and reads full pages"

# ---------------------------------------------------------------------------
# Issue #1499: the low-severity leftovers of the #1497 reviews.
# ---------------------------------------------------------------------------

# 28. A stderr reader that exits early: the result is exactly the real gh's
#     (the shim adds no stage of its own that could die of SIGPIPE first).
rm -f "$AMPLIHACK_GH_COMPAT_STATE" "${AMPLIHACK_GH_COMPAT_STATE}.ok"
drc=0; dout="$(STUB_GRAPHQL_OK=1 STUB_NOISY=5000 "${WORK}/real/gh" pr view 42 2> >(head -1 >/dev/null))" || drc=$?
rc=0; out="$(STUB_GRAPHQL_OK=1 STUB_NOISY=5000 gh pr view 42 2> >(head -1 >/dev/null))" || rc=$?
[ "$rc" = "$drc" ] || fail stderr-closed "exit $rc when the stderr reader went away; the real gh alone exits $drc"
[ "$out" = "$dout" ] || fail stderr-closed "stdout '$out', the real gh alone gives '$dout'"
ok "stderr reader exiting early: same exit code and stdout as the real gh"

# 29. A child the real gh leaves holding stderr does not hold the shim.
start=$SECONDS
out="$(STUB_GRAPHQL_OK=1 STUB_FORK=1 gh pr view 42 2>/dev/null)" || fail fork "exited non-zero"
[ "$out" = forked ] || fail fork "stdout was '$out'"
[ $((SECONDS - start)) -lt 3 ] || fail fork "waited $((SECONDS - start))s for gh's background child"
ok "a background child holding stderr does not delay the shim"

# 30. A block-text line on a call that succeeds is shown, not swallowed.
rc=0; err="$(STUB_GRAPHQL_OK=1 STUB_WARN_BLOCK=1 gh pr view 42 2>&1 >/dev/null)" || rc=$?
[ "$rc" = 0 ] || fail block-line "exited $rc"
case "$err" in *"GraphQL is not available"*) ;; *) fail block-line "line swallowed on success: '$err'" ;; esac
[ ! -e "$AMPLIHACK_GH_COMPAT_STATE" ] || fail block-line "a successful call recorded a block"
ok "block text on a successful call reaches stderr"

# 31. A prompt without a trailing newline is passed on while gh waits.
go="${WORK}/prompt.go"; errlog="${WORK}/prompt.err"; : > "$errlog"
STUB_GRAPHQL_OK=1 STUB_PROMPT="$go" gh pr view 42 >/dev/null 2>"$errlog" &
bg=$!
i=0; while ! grep -q 'Continue?' "$errlog" && [ "$i" -lt 40 ]; do sleep 0.1; i=$((i + 1)); done
grep -q 'Continue?' "$errlog" || { kill "$bg" 2>/dev/null; fail prompt "partial-line prompt held back"; }
touch "$go"; rc=0; wait "$bg" || rc=$?
[ "$rc" = 0 ] || fail prompt "exited $rc"
ok "a partial-line prompt is passed on while gh waits"

# 32. --json fields: gh's own error for an unknown one; a loud failure for one
#     REST cannot answer; never null.
: > "$AMPLIHACK_GH_COMPAT_STATE"; reset_log
rc=0; err="$(gh pr view 42 --json number,authorAssociation 2>&1 >/dev/null)" || rc=$?
[ "$rc" = 1 ] || fail json-field "unknown field exited $rc"
case "$err" in *'Unknown JSON field: "authorAssociation"'*"Available fields:"*) ;; *) fail json-field "message was '$err'" ;; esac
logged_prefix "api -X" && fail json-field "REST was called for an invalid field list"
rc=0; err="$(gh issue view 7 --json projectItems 2>&1 >/dev/null)" || rc=$?
[ "$rc" = 1 ] || fail json-field "REST-less field exited $rc"
rc=0; err="$(gh issue list --json number,isPinned 2>&1 >/dev/null)" || rc=$?
[ "$rc" = 1 ] || fail json-field "REST-less field on issue list exited $rc"
rc=0; err="$(gh issue view 7 --json projectItems 2>&1 >/dev/null)" || rc=$?
[ "$rc" = 1 ] || fail json-field "REST-less field exited $rc"
case "$err" in *'"projectItems"'*"no REST equivalent"*) ;; *) fail json-field "message was '$err'" ;; esac
json="$(gh pr view 42 --json closed,mergedBy,reviewRequests,potentialMergeCommit,fullDatabaseId)"
printf '%s' "$json" | jq -e 'has("closed") and has("mergedBy") and (.reviewRequests | type == "array")' >/dev/null || fail json-field "known fields missing: $json"
n="$(STUB_OLD_GH=1 gh pr view 42 --json closingIssuesReferences --jq '.closingIssuesReferences | length')" || fail json-field "an older gh's list rejected closingIssuesReferences"
[ "$n" = 1 ] || fail json-field "closingIssuesReferences gave '$n'"
[ "$(gh pr view 42 --json number,state --jq '{number, state}')" = '{"number":42,"state":"OPEN"}' ] || fail json-field "--jq object output is not compact like gh's"
[ "$(gh pr view 42 --json number --jq '-.number')" = -42 ] || fail json-field "a --jq expression starting with '-' was read as a jq option"
# First call of a run, before any block is on record: the installed gh refuses
# the newer field itself, so the shim must check for the block up front.
rm -f "$AMPLIHACK_GH_COMPAT_STATE" "${AMPLIHACK_GH_COMPAT_STATE}.ok"
n="$(STUB_OLD_GH=1 gh pr view 42 --json closingIssuesReferences --jq '.closingIssuesReferences | length' 2>/dev/null)" || fail json-field "first call with a newer field failed on a blocked host"
[ "$n" = 1 ] || fail json-field "first call closingIssuesReferences gave '$n'"
rm -f "$AMPLIHACK_GH_COMPAT_STATE" "${AMPLIHACK_GH_COMPAT_STATE}.ok"
out="$(STUB_OLD_GH=1 STUB_GRAPHQL_OK=1 gh pr view 42 --json closingIssuesReferences 2>&1)" || true
[ ! -e "$AMPLIHACK_GH_COMPAT_STATE" ] || fail json-field "a working host was recorded as blocked"
: > "$AMPLIHACK_GH_COMPAT_STATE"
ok "--json: unknown fields rejected as gh does; GraphQL-only fields fail loudly"

# 33. issue list --json comments carries each issue's comments.
json="$(gh issue list --json number,comments)"
[ "$(printf '%s' "$json" | jq -c 'map([.number, (.comments | length)])')" = '[[5,2]]' ] || fail list-comments "got $json"
ok "issue list --json comments is filled in, not null"

# 34. Per-PR sub-lists read every page.
reset_log
n="$(gh pr view 42 --json files --jq '.files | length')"
[ "$n" = 2 ] || fail sublist "files gave $n, want both pages"
logged "api -X GET repos/o/r/pulls/42/files?per_page=100 --paginate" || fail sublist "files not paginated"
ok "per-PR sub-lists page past the first 100"

# 35. closedByPullRequestsReferences: open or merged PRs whose body closes the issue.
refs="$(gh issue view 7 --json closedByPullRequestsReferences --jq '[.closedByPullRequestsReferences[] | "\(.repository.owner.login)/\(.repository.name)#\(.number)"] | sort | join(",")')"
[ "$refs" = "o/r#42,o/r#45,o2/r2#9" ] || fail closed-by "got '$refs'"
[ "$(gh pr view 42 --json closingIssuesReferences --jq '.closingIssuesReferences[0] | "\(.repository.owner.login)/\(.repository.name)#\(.number)"')" = "o/r#7" ] \
  || fail closed-by "closingIssuesReferences lost its repository"
# A PR into a non-default branch closes nothing, so it lists no closing issues.
[ "$(STUB_BASE=release gh pr view 42 --json closingIssuesReferences --jq '.closingIssuesReferences | length')" = 0 ] \
  || fail closed-by "a PR into a non-default branch listed closing issues"
ok "closedByPullRequestsReferences derived from the cross-reference timeline"

# 36. pr checks: workflow and event from the Actions run, description from the output title.
json="$(STUB_CHECKS=actions gh pr checks 42 --json name,workflow,event,description)"
[ "$(printf '%s' "$json" | jq -c '.[0] | [.workflow, .event, .description]')" = '["CI","pull_request","All green"]' ] || fail checks-json "got $json"
# Unreadable Actions runs: a rollup's workflowName reads null (the merge gate's
# conclusions still answer); a call that asked for workflow fails.
[ "$(STUB_CHECKS=actions STUB_NO_ACTIONS=1 gh pr view 42 --json statusCheckRollup --jq '.statusCheckRollup[0] | [.conclusion, .workflowName]')" = '["SUCCESS",null]' ] \
  || fail checks-json "rollup with unreadable Actions runs"
rc=0; STUB_CHECKS=actions STUB_NO_ACTIONS=1 gh pr checks 42 --json name,workflow >/dev/null 2>&1 || rc=$?
[ "$rc" = 1 ] || fail checks-json "pr checks --json workflow with unreadable runs exited $rc"
reset_log; STUB_CHECKS=actions gh pr checks 42 >/dev/null 2>&1
logged_prefix "actions/runs" && fail checks-json "plain pr checks looked up workflow runs it does not print"
ok "pr checks --json workflow/event/description come from REST, not placeholders"

# 37. label list: --search on name or description; creation order by default,
#     --sort name on request; --limit cuts the ordered list.
[ "$(gh label list --search broken --json name,isDefault --jq '.')" = '[{"name":"bug","isDefault":true}]' ] || fail labels "search/isDefault wrong"
[ "$(gh label list --limit 1 --json name --jq '.[].name')" = docs ] || fail labels "default order is not creation order"
[ "$(gh label list --limit 1 --sort name --json name --jq '.[].name')" = bug ] || fail labels "--sort name ignored"
rc=0; gh label list --sort color >/dev/null 2>&1 || rc=$?
[ "$rc" = 1 ] || fail labels "--sort color accepted"
ok "label list: --search, gh's sort orders, --limit, isDefault"

# 38. pr list: every filter holds in the search fallback, and --limit counts
#     what survives --draft.
: > "$AMPLIHACK_GH_COMPAT_STATE"
pl() { gh pr list "$@" --json number --jq '[.[].number] | map(tostring) | join(",")'; }
[ "$(pl --label x --label y)" = 51 ] || fail pr-filters "--label x --label y must mean both"
[ "$(pl --author @me --draft)" = 50 ] || fail pr-filters "--draft ignored in the search fallback"
[ "$(pl --assignee a)" = 50 ] || fail pr-filters "--assignee ignored"
[ "$(pl --draft --limit 1)" = 50 ] || fail pr-filters "--draft applied after --limit"
[ "$(pl --author @me --state merged)" = 10 ] || fail pr-filters "--state merged let unmerged PRs through the search fallback"
reset_log; rc=0; err="$(gh issue list --milestone v1 --json number 2>&1 >/dev/null)" || rc=$?
[ "$rc" = 1 ] || fail pr-filters "an unsupported filter flag exited $rc instead of failing"
case "$err" in *"flag --milestone"*"no REST fallback"*) ;; *) fail pr-filters "message was '$err'" ;; esac
logged_prefix "api -X" && fail pr-filters "an unsupported flag still listed (widened) over REST"
ok "pr list filters (label AND, draft, assignee, merged) hold in the REST fallback"

# 39. view --comments without --json prints the comments, as gh does off a terminal.
out="$(gh issue view 5 --comments)" || fail view-comments "issue view --comments failed"
case "$out" in *"author:	a"*"--"*"x"*"author:	b"*"y"*) ;; *) fail view-comments "got '$out'" ;; esac
out="$(gh pr view 42 --comments)" || fail view-comments "pr view --comments failed"
case "$out" in *"status:	changes_requested"*"status:	approved"*) ;; *) fail view-comments "pr reviews missing: '$out'" ;; esac
ok "view --comments prints the comment list instead of dropping it"

# 40. Write side effects never fail silently.
reset_log; rc=0; gh issue close 99 --comment "closing" >/dev/null 2>&1 || rc=$?
[ "$rc" = 1 ] || fail writes "issue close with an unpostable --comment exited $rc"
logged_prefix "api -X PATCH" && fail writes "closed although the --comment failed"
reset_log; rc=0; gh pr edit 42 --remove-label "good first issue" >/dev/null 2>&1 || rc=$?
logged "api -X DELETE repos/o/r/issues/42/labels/good%20first%20issue" || fail writes "a label with spaces was split"
[ "$rc" = 1 ] || fail writes "a failed --remove-label exited $rc"
rc=0; out="$(gh pr create --title t --body b --reviewer someone 2>"${WORK}/create.err")" || rc=$?
[ "$rc" = 0 ] && [ "$out" = "https://github.com/o/r/pull/43" ] || fail writes "pr create: rc $rc, stdout '$out'"
grep -q "warning: could not request review from someone" "${WORK}/create.err" || fail writes "a failed reviewer request was silent"
ok "write side effects (comment, label removal, reviewers) fail or warn, never silently"

# 41. Flags the fallback implements instead of dropping: --milestone,
#     --edit-last, --patch, --fill / --fill-first. --web and --dry-run are refused.
reset_log
gh issue create --title T --body b --milestone v1 >/dev/null || fail flags2 "issue create --milestone failed"
logged 'BODY {"title":"T","body":"b","milestone":3}' || fail flags2 "--milestone not sent as its number"
reset_log; rc=0; gh issue create --title T --body b --milestone nope >/dev/null 2>&1 || rc=$?
[ "$rc" = 1 ] || fail flags2 "unknown milestone exited $rc"
logged_prefix "api -X POST" && fail flags2 "created an issue although the milestone is unknown"
reset_log
[ "$(gh pr comment 42 --edit-last --body new)" = "https://github.com/o/r/pull/42#issuecomment-77" ] || fail flags2 "--edit-last did not edit the viewer's last comment"
logged_prefix "api -X POST repos/o/r/issues/42/comments" && fail flags2 "--edit-last posted a new comment"
reset_log; gh pr diff 42 --patch >/dev/null
logged_prefix "Accept: application/vnd.github.v3.patch" || fail flags2 "--patch did not ask for a patch"
for fl in "pr view 42 --web" "pr create --title t --body b --dry-run"; do
  rc=0; reset_log; gh $fl >/dev/null 2>&1 || rc=$?
  [ "$rc" = 1 ] || fail flags2 "'$fl' exited $rc"
  logged_prefix "api -X POST" && fail flags2 "'$fl' created something"
done
gi() { git -c user.name=t -c user.email=t@t "$@"; }
gi commit -q --allow-empty -m base && gi update-ref refs/remotes/origin/main HEAD
gi commit -q --allow-empty -m "add a" -m "why a" && gi commit -q --allow-empty -m "add b"
reset_log; gh pr create --fill >/dev/null || fail flags2 "pr create --fill failed"
logged 'BODY {"title":"Feat","body":"- add a\n- add b","head":"feat","base":"main","draft":false}' || fail flags2 "--fill over two commits"
reset_log; gh pr create --fill-first >/dev/null || fail flags2 "pr create --fill-first failed"
logged 'BODY {"title":"add a","body":"why a","head":"feat","base":"main","draft":false}' || fail flags2 "--fill-first"
ok "--milestone, --edit-last, --patch and --fill are implemented; --web and --dry-run are refused"

# ---------------------------------------------------------------------------
# Independent crusty review of 8dcec6eb (PR comment 5840823326).
# ---------------------------------------------------------------------------

# 42. argjson-accumulator-arg-max-silent-empty: results past 128 KiB come back
#     whole, and a jq failure fails the call instead of printing nothing, rc 0.
: > "$AMPLIHACK_GH_COMPAT_STATE"
rc=0; out="$(STUB_BIG=1 gh issue list --json number,comments 2>/dev/null)" || rc=$?
[ "$rc" = 0 ] || fail arg-max "issue list with 180 KB of comments exited $rc"
[ "$(printf '%s' "$out" | jq -c 'map([.number, (.comments | length)])' 2>/dev/null)" = '[[5,3]]' ] || fail arg-max "issue list: ${out:0:80}"
rc=0; out="$(STUB_BIG=1 gh pr view 42 --json number,comments 2>/dev/null)" || rc=$?
[ "$rc" = 0 ] && [ "$(printf '%s' "$out" | jq '.comments | length' 2>/dev/null)" = 3 ] || fail arg-max "pr view with 180 KB of comments: rc $rc, '${out:0:80}'"
head -c 200000 /dev/zero | tr '\0' b > "${WORK}/big.body"
reset_log; rc=0; gh issue create --title T --body-file "${WORK}/big.body" >/dev/null 2>&1 || rc=$?
[ "$rc" = 0 ] || fail arg-max "issue create with a 200 KB body exited $rc"
[ "$(grep '^BODY ' "$STUB_LOG" | wc -c)" -gt 200000 ] || fail arg-max "a 200 KB body did not reach the POST"
rc=0; out="$(STUB_BADJSON=1 gh pr view 42 --json number,files 2>/dev/null)" || rc=$?
[ "$rc" != 0 ] || fail arg-max "unparseable REST JSON gave rc 0 and '$out'"
ok "results over 128 KiB come back whole; a jq failure fails the call"

# 43. delete-branch-wrong-ref: never the base repo's branch for a fork PR,
#     and the ref is percent-encoded.
reset_log; STUB_FORK_PR=1 gh pr merge 42 --squash --delete-branch >/dev/null 2>&1 || true
logged_prefix "api -X DELETE" && fail delete-branch "deleted a base-repo branch for a fork PR: $(grep DELETE "$STUB_LOG")"
reset_log; STUB_HEAD='feat/a#b' gh pr merge 42 --squash --delete-branch >/dev/null 2>&1 || true
logged "api -X DELETE repos/o/r/git/refs/heads/feat%2Fa%23b" || fail delete-branch "ref not encoded: $(grep DELETE "$STUB_LOG")"
ok "--delete-branch: skipped for fork PRs, ref percent-encoded"

# 44. passthrough-stderr-reordered-on-working-hosts: where GraphQL works, gh's
#     own stdout/stderr interleaving is kept (the real gh is exec'd).
rm -f "$AMPLIHACK_GH_COMPAT_STATE" "${AMPLIHACK_GH_COMPAT_STATE}.ok"
direct="$(STUB_GRAPHQL_OK=1 STUB_INTERLEAVE=1 "${WORK}/real/gh" pr view 42 2>&1 | paste -sd' ' -)"
for i in 1 2 3; do
  via="$(STUB_GRAPHQL_OK=1 STUB_INTERLEAVE=1 gh pr view 42 2>&1 | paste -sd' ' -)"
  [ "$via" = "$direct" ] || fail interleave "via shim '$via', direct '$direct'"
done
ok "working host: stdout/stderr interleaving is gh's own"

# 45. sigterm-orphans-real-gh: TERM to the shim reaches the real gh.
go="${WORK}/term.go"; pidf="${WORK}/term.pid"; rm -f "$go" "$pidf"
STUB_GRAPHQL_OK=1 STUB_STREAM="$go" STUB_PIDFILE="$pidf" gh pr checks 42 --watch >/dev/null 2>&1 &
bg=$!
i=0; while [ ! -s "$pidf" ] && [ "$i" -lt 40 ]; do sleep 0.1; i=$((i + 1)); done
[ -s "$pidf" ] || fail sigterm "stub never started"
kill -TERM "$bg"; wait "$bg" 2>/dev/null || true; sleep 0.3
if kill -0 "$(cat "$pidf")" 2>/dev/null; then kill "$(cat "$pidf")"; fail sigterm "real gh survived SIGTERM to the shim"; fi
ok "SIGTERM to the shim leaves no real gh behind"

# ---------------------------------------------------------------------------
# Independent crusty review round 2, of bc9da2dd (PR comment 5841113968).
# ---------------------------------------------------------------------------
fresh() { rm -f "$AMPLIHACK_GH_COMPAT_STATE"*; reset_log; }
probes() { grep -c '^api graphql' "$STUB_LOG" || true; }

# 46. inconclusive-probe-execs-into-block: a probe that gets a 502 on a blocked
#     host, and a stale ".ok" on a host that has become blocked, still end in
#     the REST answer.
fresh
out="$(STUB_PROBE_502_ONCE="${WORK}/p502" gh issue view 7 --json url --jq .url 2>/dev/null)" || fail inconclusive "502 probe: exit $?"
[ "$out" = "https://github.com/o/r/issues/7" ] || fail inconclusive "502 probe then block gave '$out'"
[ -e "$AMPLIHACK_GH_COMPAT_STATE" ] || fail inconclusive "the block seen by gh was not recorded"
# An .ok recorded 10 minutes ago (inside the 60 min TTL, past the 5 min recheck).
ago="$(date -d '10 minutes ago' +%Y%m%d%H%M 2>/dev/null || date -v-10M +%Y%m%d%H%M)"
fresh; : > "${AMPLIHACK_GH_COMPAT_STATE}.ok"; touch -t "$ago" "${AMPLIHACK_GH_COMPAT_STATE}.ok"
out="$(gh issue view 7 --json url --jq .url 2>/dev/null)" || fail inconclusive "stale .ok on a blocked host: exit $?"
[ "$out" = "https://github.com/o/r/issues/7" ] || fail inconclusive "stale .ok on a blocked host gave '$out'"
ok "an inconclusive probe or a stale .ok still ends in the REST answer"

# 47. probe-unbounded-and-uncached-when-inconclusive.
fresh
for i in 1 2 3; do STUB_GRAPHQL_OK=1 STUB_PROBE_RATELIMIT=1 gh pr view 42 >/dev/null 2>&1 || true; done
[ "$(probes)" = 1 ] || fail probe "a rate-limited probe was repeated: $(probes) probes for 3 calls"
fresh; start=$SECONDS
STUB_GRAPHQL_OK=1 STUB_PROBE_SLEEP=5 AMPLIHACK_GH_COMPAT_PROBE_TIMEOUT=1 gh pr view 42 >/dev/null 2>&1 || true
[ $((SECONDS - start)) -lt 4 ] || fail probe "a hung probe held the call for $((SECONDS - start))s"
fresh; gh label create x --color 000000 >/dev/null 2>&1 || true
[ "$(probes)" = 0 ] || fail probe "label create (REST in gh) was probed"
ok "the probe is bounded, an inconclusive answer is cached, label create is not probed"

# 48. help-flag-refused-on-blocked-host.
fresh; : > "$AMPLIHACK_GH_COMPAT_STATE"
for h in "pr view --help" "issue list -h"; do
  out="$(gh $h 2>&1)" || fail help "'gh $h' exited $?: $out"
  case "$out" in "help for ${h% *}") ;; *) fail help "'gh $h' gave '$out'" ;; esac
done
ok "-h/--help reaches gh's offline help on a blocked host"

# 49. host-qualified-repo-and-host-unkeyed-state.
fresh; : > "$AMPLIHACK_GH_COMPAT_STATE"
rc=0; err="$(gh issue view 5 -R ghe.example.com/o/r 2>&1)" || rc=$?
[ "$rc" != 0 ] || fail host "-R ghe.example.com/o/r answered from github.com"
logged_prefix "api -X GET repos/ghe.example.com" && fail host "HOST/OWNER/REPO sent as a github.com path"
case "$err" in *ghe.example.com*) ;; *) fail host "refusal does not name the host: '$err'" ;; esac
rm -f "${AMPLIHACK_GH_COMPAT_STATE}@"*; reset_log   # github.com stays blocked
out="$(GH_HOST=ghe.example.com STUB_GHE_OK=1 gh pr view 42 2>/dev/null)" || true
[ "$out" = "ghe-output: pr view 42" ] || fail host "github.com's block marker applied to ghe.example.com: '$out'"
logged "api graphql --hostname ghe.example.com -f query={viewer{login}}" || fail host "ghe.example.com was not probed itself"
git init -q -b feat "${WORK}/ghe" && git -C "${WORK}/ghe" remote add origin https://ghe.example.com/o/r.git
reset_log; rc=0; ( cd "${WORK}/ghe" && gh issue view 5 >/dev/null 2>&1 ) || rc=$?
logged_prefix "api -X GET repos/o/r" && fail host "a GHE origin was answered from github.com's o/r"
ok "HOST/OWNER/REPO and GHE remotes are not answered from github.com; answers are per host"

# 50. ghc-repo-env-leaks-into-exec.
fresh; : > "${AMPLIHACK_GH_COMPAT_STATE}.ok"
[ "$(STUB_GRAPHQL_OK=1 STUB_ENV=1 gh pr view 42)" = "GHC_REPO=unset" ] || fail env "GHC_REPO leaked into the real gh's environment"
ok "no gh-compat variables leak into the real gh"
fresh; : > "$AMPLIHACK_GH_COMPAT_STATE"

# ---------------------------------------------------------------------------
# Independent crusty review round 3, of 46d14022 (PR comment 5841382203).
# ---------------------------------------------------------------------------

# 52. url-target-after-boolean-flag-served-from-github-com: a GHE URL anywhere
#     in argv is never answered from github.com.
fresh; : > "$AMPLIHACK_GH_COMPAT_STATE"
rc=0; gh pr merge --squash https://ghe.example.com/o/r/pull/5 >/dev/null 2>&1 || rc=$?
[ "$rc" != 0 ] || fail ghe-url "pr merge of a GHE URL succeeded"
logged_prefix "api -X PUT" && fail ghe-url "a GHE PR URL was merged on github.com: $(grep PUT "$STUB_LOG")"
reset_log; gh issue view --comments https://ghe.example.com/o/r/issues/5 >/dev/null 2>&1 || true
logged_prefix "api -X GET repos/o/r" && fail ghe-url "a GHE issue URL was read from github.com"
ok "a GHE URL after a boolean flag is refused, not served from github.com"

# 53. api-graphql-hostname-answered-from-github-com.
fresh; : > "$AMPLIHACK_GH_COMPAT_STATE"
rc=0; out="$(gh api graphql --hostname ghe.example.com -f query='{viewer{login}}' 2>/dev/null)" || rc=$?
[ "$rc" != 0 ] || fail api-host "api graphql --hostname ghe.example.com answered: $out"
logged_prefix "api -X GET user" && fail api-host "github.com's /user answered for ghe.example.com"
ok "api graphql --hostname HOST is not answered with github.com's identity"

# 54. stale-ok-recheck-replays-non-block-failure: an old "works" answer is
#     re-checked BEFORE gh runs, and after an inconclusive probe a write that
#     meets the block is not replayed.
fresh; : > "${AMPLIHACK_GH_COMPAT_STATE}.ok"; touch -t "$ago" "${AMPLIHACK_GH_COMPAT_STATE}.ok"
out="$(STUB_GH_502=1 gh issue comment 42 --body hello 2>&1)" || fail no-replay "stale .ok, blocked host: exit $?: $out"
logged "issue comment 42 --body hello" && fail no-replay "gh ran on a stale .ok before GraphQL was re-checked"
[ "$(grep -c '^api -X POST repos/o/r/issues/42/comments' "$STUB_LOG")" = 1 ] || fail no-replay "comment not posted exactly once"
fresh; : > "${AMPLIHACK_GH_COMPAT_STATE}.unknown"
rc=0; err="$(gh issue comment 42 --body hello 2>&1)" || rc=$?
[ "$rc" != 0 ] || fail no-replay "a blocked write after an inconclusive probe was replayed"
logged_prefix "api -X POST" && fail no-replay "a write was replayed over REST"
case "$err" in *"run the command again"*) ;; *) fail no-replay "no hint: '$err'" ;; esac
[ -e "$AMPLIHACK_GH_COMPAT_STATE" ] || fail no-replay "the block gh met was not recorded"
fresh; : > "${AMPLIHACK_GH_COMPAT_STATE}.unknown"
[ "$(gh issue view 7 --json url --jq .url 2>/dev/null)" = "https://github.com/o/r/issues/7" ] || fail no-replay "a read after an inconclusive probe was not replayed"
ok "no write is replayed; a stale 'works' is re-checked before gh runs"

# 55. child-gh-ignores-sigint-and-wrong-signal-exit: where GraphQL works the
#     real gh IS the process (exec); after an inconclusive probe a signal is
#     passed on as itself and the shim dies by it.
fresh; : > "${AMPLIHACK_GH_COMPAT_STATE}.ok"
go="${WORK}/sig.go"; pidf="${WORK}/sig.pid"; rm -f "$go" "$pidf"
STUB_GRAPHQL_OK=1 STUB_STREAM="$go" STUB_PIDFILE="$pidf" gh pr checks 42 --watch >/dev/null 2>&1 &
bg=$!
i=0; while [ ! -s "$pidf" ] && [ "$i" -lt 40 ]; do sleep 0.1; i=$((i + 1)); done
[ "$(cat "$pidf" 2>/dev/null)" = "$bg" ] || { kill "$bg" 2>/dev/null; fail signals "the real gh is not the shim's own process (no exec)"; }
kill "$bg" 2>/dev/null; wait "$bg" 2>/dev/null || true
fresh; : > "${AMPLIHACK_GH_COMPAT_STATE}.unknown"; rm -f "$go" "$pidf"
STUB_GRAPHQL_OK=1 STUB_STREAM="$go" STUB_PIDFILE="$pidf" gh pr checks 42 --watch >/dev/null 2>&1 &
bg=$!
i=0; while [ ! -s "$pidf" ] && [ "$i" -lt 40 ]; do sleep 0.1; i=$((i + 1)); done
kill -HUP "$bg"; rc=0; { wait "$bg"; } 2>/dev/null || rc=$?
[ "$rc" = 129 ] || fail signals "SIGHUP to the shim gave exit $rc, want 129 (death by SIGHUP)"
sleep 0.3; kill -0 "$(cat "$pidf")" 2>/dev/null && { kill "$(cat "$pidf")"; fail signals "real gh survived SIGHUP to the shim"; }
ok "exec where GraphQL works; the received signal is passed on and re-raised otherwise"

# 56. probe-watcher-orphans-sleep: no stray sleep outlives a probe.
fresh; AMPLIHACK_GH_COMPAT_PROBE_TIMEOUT=37 STUB_GRAPHQL_OK=1 gh pr view 42 >/dev/null 2>&1 || true
sleep 0.2
[ "$(ps -eo args= | grep -c '^sleep 37$' || true)" = 0 ] || fail watcher "a probe left its watcher's sleep running"
ok "the probe's timeout watcher leaves no process behind"

# ---------------------------------------------------------------------------
# Independent crusty review round 4, of 76587c19 (PR comment 5841827808).
# ---------------------------------------------------------------------------

# 57. free-text-url-argument-selects-target-host: a body, title or search term
#     that begins with a URL is data. It selects no host, triggers no probe of
#     that host, and the call proceeds (the round-3 GHE URL targets in 52 stay
#     refused).
u="https://ghe.example.invalid/x"
fresh; : > "$AMPLIHACK_GH_COMPAT_STATE"
for args in "--body $u" "-b $u" "--title $u" "--body=$u"; do
  reset_log
  # shellcheck disable=SC2086  # word-split on purpose: flag and value.
  gh issue create -t T $args >/dev/null 2>&1 || true
  [ "$(grep -c '^api -X POST repos/o/r/issues ' "$STUB_LOG")" = 1 ] || fail free-text "'issue create $args' did not POST: $(cat "$STUB_LOG")"
  logged_prefix "api graphql --hostname" && fail free-text "'$args' probed a foreign host"
done
reset_log
gh issue list --search "$u" --json number >/dev/null 2>&1 || fail free-text "issue list --search '$u' failed"
logged_prefix "api graphql --hostname" && fail free-text "a search term probed a foreign host"
logged_prefix "api -X GET repos/o/r/issues" || fail free-text "issue list --search '$u' made no REST call"
# step-03's own call shape, with a task description that begins with a URL.
TASK_DESC="$u is broken"
ISSUE_BODY="$(printf '## Task Description\n%s\n' "$TASK_DESC")"
reset_log
out="$(gh issue create --title "$TASK_DESC" --body "$ISSUE_BODY" --label workflow:default 2>&1)" || fail free-text "step-03 create with a URL-leading task failed: $out"
[ "$out" = "https://github.com/o/r/issues/8" ] || fail free-text "step-03 create printed '$out'"
logged_prefix "api -X POST repos/o/r/issues" || fail free-text "step-03 create made no REST POST"
ok "a URL at the start of a body, title or search is data, not a target host"

# ---------------------------------------------------------------------------
# Independent crusty review round 5, of 532aede2 (PR comment 5842017601).
# ---------------------------------------------------------------------------

# 58. search-fallback-empty-query-matches-every-issue: in the client-side
#     fallback (the only one in cloud sessions), a query that leaves nothing to
#     match on matches nothing, URLs are matched as text or as this repo's
#     issue number, and unsupported qualifiers fail instead of being dropped.
fresh; : > "$AMPLIHACK_GH_COMPAT_STATE"
sl() { gh issue list --state open --search "$1" --json number --jq '[.[].number] | map(tostring) | join(",")'; }
[ "$(sl "https://ghe.example.com/o/r/pull/7/")" = "" ] || fail search "a URL-only query matched: $(sl "https://ghe.example.com/o/r/pull/7/")"
[ "$(sl "fix https://github.com/o/r/issues/99")" = "" ] || fail search "'fix <issue 99 URL>' matched another issue"
[ "$(sl "fix https://github.com/o/r/issues/5")" = 5 ] || fail search "'fix <issue 5 URL>' did not find #5"
[ "$(sl "widget is:closed")" = "" ] || fail search "is:closed was dropped"
[ "$(sl "--- ...")" = "" ] || fail search "a word-less query matched"
rc=0; gh issue list --search "widget sort:created-asc" --json number >/dev/null 2>&1 || rc=$?
[ "$rc" != 0 ] || fail search "an unsupported qualifier was silently dropped"
# step-03 with a task description that is only a PR URL: no tracker found,
# so a new issue is created.
TASK_DESC="https://ghe.example.com/o/r/pull/7/"; SEARCH_Q="${TASK_DESC:0:100}"; reset_log
FOUND_URL="$(gh issue list --state open --search "$SEARCH_Q" --json url --jq '.[0].url // ""' 2>/dev/null || echo '')"
[ -z "$FOUND_URL" ] || fail search "step-03 would adopt $FOUND_URL as its tracker"
[ "$(gh issue create --title "$TASK_DESC" --body b --label workflow:default 2>&1)" = "https://github.com/o/r/issues/8" ] || fail search "step-03 did not create an issue"
ok "search fallback: an empty residue matches nothing, URLs match as text or number, unknown qualifiers fail"

# 59. vspec-incomplete-free-text-selects-host: values of every gh value flag
#     (implemented or not) and prose after a URL never select a host.
fresh; : > "$AMPLIHACK_GH_COMPAT_STATE"
gh pr review 42 --comment --body "https://ghe.example.com/o/r/pull/7#issuecomment-1 already covers this" >/dev/null 2>&1 || true
gh pr merge 42 --squash --body-file https://ghe.example.com/o/r >/dev/null 2>&1 || true
gh pr view "https://ghe.example.com/o/r/pull/7 is related" >/dev/null 2>&1 || true
logged_prefix "api graphql --hostname" && fail vspec "a flag value or prose selected a foreign host: $(grep hostname "$STUB_LOG")"
ok "values of gh's value flags and prose after a URL select no host"

# 60. issue-reopen-accepts-reason-flag: gh's own error, no request.
fresh; : > "$AMPLIHACK_GH_COMPAT_STATE"
rc=0; err="$(gh issue reopen 5 -r completed 2>&1)" || rc=$?
[ "$rc" = 1 ] || fail reopen "issue reopen -r exited $rc"
case "$err" in *"unknown shorthand flag: 'r' in -r"*) ;; *) fail reopen "message was '$err'" ;; esac
case "$(gh issue reopen 5 --reason x 2>&1)" in *"unknown flag: --reason"*) ;; *) fail reopen "--reason not rejected like gh" ;; esac
logged_prefix "api -X PATCH" && fail reopen "reopen sent a request"
ok "issue reopen rejects -r/--reason as gh does"

# 51. stale-test-contract-header: the contract above describes the probe, not
#     the removed stderr follower or a first real-gh attempt.
hdr="$(sed -n '2,/^set -euo pipefail/p' "${SCRIPT_DIR}/$(basename "${BASH_SOURCE[0]}")")"
case "$hdr" in *"survives an early"*|*"When the real gh fails with"*) fail header "contract header describes removed behaviour" ;; esac
case "$hdr" in *"probe"*) ;; *) fail header "contract header does not describe the GraphQL probe" ;; esac
ok "the contract header matches the probe design"

echo "PASS: ${PASS} checks"
