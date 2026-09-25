#!/usr/bin/env bash
# test-gh-compat-graphql-blocked.sh — issue #1484.
#
# Contract under test (amplifier-bundle/tools/workflow_gh_compat.sh, reached
# through a `gh` launcher first on PATH, the way the recipe runner installs it):
#
#   1. Where GraphQL works, the real gh's output and exit status pass through
#      unchanged and no REST call is made.
#   2. When the real gh fails with the Claude Code GraphQL block (HTTP 403), the
#      call is replayed over REST (`gh api repos/...`) in the shape callers
#      parse, and the block is remembered: the next call goes straight to REST.
#   3. pr view/list/create/checks/ready, issue view/list/create, label create,
#      `api graphql` (viewer permission) and `auth status` each issue the
#      expected REST request.
#   4. /search is refused by the proxy; issue search falls back to matching
#      the repo's issues client-side.
#   5. Subcommands that are not GraphQL clients are exec'd untouched.
#   6. workflow_gh_retry.sh classifies the block as permanent, not a rate limit
#      (a rate limit waits for a reset that never comes).
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
printf '%s\n' "$*" >> "$STUB_LOG"
if [ "$1" = "api" ]; then
  method=GET; path=""; input=""
  shift
  while [ $# -gt 0 ]; do
    case "$1" in
      -X) method="$2"; shift 2 ;;
      -H) shift 2 ;;
      -f|-F) shift 2 ;;
      --input) input="$2"; shift 2 ;;
      *) path="$1"; shift ;;
    esac
  done
  [ -n "$input" ] && printf 'BODY %s\n' "$(jq -c . "$input")" >> "$STUB_LOG"
  if [ "$path" = graphql ]; then
    if [ "${STUB_GRAPHQL_OK:-0}" = 1 ]; then echo '{"data":{"viewer":{"login":"bot"}}}'; exit 0; fi
    echo "HTTP 403: GitHub GraphQL is not available from Claude Code sessions; use the REST API (gh api repos/{owner}/{repo}/...). (https://api.github.com/graphql)" >&2; exit 1
  fi
  pr='{"number":42,"node_id":"PR_1","title":"feat: widget","body":"Fixes #7","state":"open","draft":true,"merged_at":null,"html_url":"https://github.com/o/r/pull/42","created_at":"2026-01-01T00:00:00Z","user":{"login":"bot"},"labels":[],"mergeable":true,"mergeable_state":"clean","head":{"ref":"feat","sha":"abc123","repo":{"name":"r","full_name":"o/r","owner":{"login":"o"}}},"base":{"ref":"main","sha":"def456","repo":{"full_name":"o/r"}}}'
  case "$method $path" in
    "GET repos/o/r/pulls/42") printf '%s\n' "$pr" ;;
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
        *) printf '{"check_runs":[{"name":"Test","status":"completed","conclusion":"success","details_url":"u"}]}\n' ;;
      esac ;;
    "GET repos/o/r/commits/abc123/status"*) printf '{"statuses":[]}\n' ;;
    "POST repos/o/r/pulls/42/ccr/ready_for_review") printf '{}\n' ;;
    "POST repos/o/r/issues/42/comments") printf '{"html_url":"https://github.com/o/r/pull/42#issuecomment-1"}\n' ;;
    "GET repos/o/r/pulls?"*"head=o%3Afix%2Ba%26b"*) printf '[]\n' ;;
    "POST repos/o/r/labels") printf '{"message":"Validation Failed"}'; echo "gh: Validation Failed (HTTP 422)" >&2; exit 1 ;;
    "GET user") printf '{"login":"bot"}\n' ;;
    "PUT repos/o/r/pulls/42/merge") printf '{"merged":true}\n' ;;
    "GET repos/o/r/pulls?state=closed"*)
      # Paged: per_page=3; page 1 holds one merged PR of three, page 2 two of three.
      pg="${path##*page=}"
      jq -nc --argjson p "$pg" '[range(0;3) | {number: ($p * 10 + .), state: "closed", title: "t", html_url: "u", head: {ref: "b", sha: "s", repo: null}, base: {ref: "main", sha: "m", repo: null},
        merged_at: (if ($p == 1 and . == 0) or ($p == 2 and . > 0) then "2026-01-01T00:00:00Z" else null end)}]' ;;
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
  echo "progress: 1 pending" >&2
  i=0; while [ ! -e "$STUB_STREAM" ] && [ "$i" -lt 50 ]; do sleep 0.1; i=$((i + 1)); done
  [ -e "$STUB_STREAM" ] && { echo "done"; exit 0; }
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
logged_prefix "api " && fail pass-through "REST was called although GraphQL works"
[ ! -e "$AMPLIHACK_GH_COMPAT_STATE" ] || fail pass-through "block state recorded although GraphQL works"
ok "GraphQL available: real gh output passes through, no REST call"

# 2. First blocked call: detected, replayed over REST, remembered.
reset_log
out="$(gh pr view 42 --json headRefName -q .headRefName)"
[ "$out" = "feat" ] || fail detect "pr view -q .headRefName gave '$out'"
logged "pr view 42 --json headRefName -q .headRefName" || fail detect "real gh was not tried first"
logged "api -X GET repos/o/r/pulls/42" || fail detect "no REST GET repos/o/r/pulls/42"
[ -e "$AMPLIHACK_GH_COMPAT_STATE" ] || fail detect "block not remembered"
ok "403 detected once, pr view served by GET repos/o/r/pulls/42"

reset_log
json="$(gh pr view https://github.com/o/r/pull/42 --json number,state,isDraft,headRefOid,isCrossRepository,closingIssuesReferences)"
logged_prefix "pr view" && fail remembered "real gh retried after the block was recorded"
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

# 8. label create on an existing label fails the way gh does.
rc=0; gh label create workflow:default --color 0366d6 >/dev/null 2>&1 || rc=$?
[ "$rc" = 1 ] || fail label "existing label should exit 1, got $rc"
ok "label create: 422 maps to gh's 'already exists' failure"

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
rm -f "$AMPLIHACK_GH_COMPAT_STATE"
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
  logged_prefix "pr view" && fail state "a fresh marker should skip the probe"
  touch -t 200001010000 "$d/graphql-blocked"
  reset_log
  gh pr view 42 --json number >/dev/null 2>&1
  logged "pr view 42 --json number" || fail state "an expired marker must re-probe the real gh"
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
rm -f "$AMPLIHACK_GH_COMPAT_STATE"
go="${WORK}/stream.go"; errlog="${WORK}/stream.err"; : > "$errlog"
STUB_GRAPHQL_OK=1 STUB_STREAM="$go" gh pr checks 42 --watch >/dev/null 2>"$errlog" &
bg=$!
i=0; while ! grep -q progress "$errlog" && [ "$i" -lt 40 ]; do sleep 0.1; i=$((i + 1)); done
grep -q progress "$errlog" || { kill "$bg" 2>/dev/null; fail stream "stderr held back until exit"; }
touch "$go"
rc=0; wait "$bg" || rc=$?
[ "$rc" = 0 ] || fail stream "streamed call exited $rc"
ok "pass-through stderr is streamed, not buffered until exit"

echo "PASS: ${PASS} checks"
