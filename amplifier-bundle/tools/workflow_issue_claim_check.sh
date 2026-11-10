#!/usr/bin/env bash
# workflow_issue_claim_check.sh — is this issue already claimed by a pull
# request? (issue #1361)
#
# The failure this exists to prevent: the workflow opens a SECOND pull request
# for an issue it is already working on. The second one merges; the first is
# never closed and sits open until it drifts into a conflicted state. A month
# later it presents as real unmerged work needing a rebase. It is not — its code
# is byte-identical to what shipped, and the only correct resolution of the
# conflict is "keep main", producing an empty commit. Nothing distinguishes that
# from genuine work when looking at the PR from the outside, so each one costs a
# full investigation to establish it is a duplicate.
#
#   #1084 -> #1126 opened 23:02, #1128 merged 00:05      (~63 min apart)
#   #1123 -> #1131 opened 01:03, #1132 merged 01:32      (~29 min apart)
#   #1277 -> #1283 #1284 #1286 #1288 #1289, five PRs in 103 minutes,
#           from five branches, after three siblings had already been closed
#
# Nothing in the workflow ever asked whether somebody — including an earlier
# instance of itself — was already working on the issue. The only de-duplication
# that existed was `workflow_pr_scope.sh`, whose primary key is
# (headRefName, baseRefName, same-repo). That is exactly the key a second run
# defeats: a new run derives a NEW branch, so the lookup correctly reports "no
# PR for this head" and a duplicate is opened. The claim is on the ISSUE, not on
# the branch, so the check has to be too.
#
# WHAT COUNTS AS A CLAIM
#
# Four signals, each of which is a PR *claiming* the issue rather than merely
# mentioning it. "Referencing #N" would be too loose — a PR body that says
# "similar to #1361" would refuse a run for no reason — so a passing mention is
# deliberately NOT a claim:
#
#   1. closingIssuesReferences contains N. This is GitHub's own resolved link,
#      populated from `Closes #N` / `Fixes #N` in the body. Authoritative.
#   2. The body contains a closing keyword followed by #N: close, closes,
#      closed, fix, fixes, fixed, resolve, resolves or resolved, in any case,
#      with an optional colon and any whitespace before the `#`. The keyword
#      must start the body or follow a non-word character, so `prefixes #N`
#      does not match. `Fixes #N`, `closes #N` and `Resolves: #N` are claims;
#      `Refs #N`, `see #N` and `Fixes #N0` are not. Each issue needs its own
#      keyword, as on GitHub, so `Closes #119, #N` does not claim N.
#
#      The reference may also name this repository in full, as GitHub accepts:
#      `Closes owner/repo#N` and `Fixes https://<host>/owner/repo/issues/N`,
#      where owner/repo is the slug being checked (case-insensitive). A
#      reference to another repository's issue N is not a claim on this one,
#      and if the slug is not plain `owner/repo` only the `#N` form is matched.
#
#      This signal is a FALLBACK for signal 1, not a parallel claim. It is
#      consulted only when GitHub could not have resolved the keywords itself:
#      closingIssuesReferences is empty AND the PR's base is not the default
#      branch (GitHub links keywords only on PRs that target the default
#      branch). When the default branch cannot be established, the first
#      condition alone applies. Fenced code blocks and inline code spans are
#      removed before matching, as GitHub ignores keywords inside them. Both
#      follow CommonMark closely enough for this purpose: a fence opens only
#      at the start of a line (up to three spaces of indent), and a code span
#      is a backtick run closed by a run of exactly the same length. A
#      mid-line ``` or ~~~, or an unmatched run, is ordinary text and is
#      matched as such. See strip_code in section 3.
#
#      Why so narrow: workflow-written PR bodies routinely quote keyword
#      examples, earlier PRs and test scenarios. Matched against the raw body
#      of every PR, the regex claimed issues GitHub had not linked on 4 of 100
#      recent rysweet/amplihack-rs PRs: #1718 (`Closes #1437` in a table cell),
#      #1688 ("runner fix #31" in prose), #1601 ("runner fixes #161" as a test
#      label) and #1428 (`(fix #3132)` in a quoted heredoc). In each, GitHub
#      had already resolved the real closing references. Where it has, its
#      answer stands and the body is not read.
#   3. The title contains "(#N)". This is the exact shape the workflow gives its
#      own PRs (`Update <scope> (#N)`, `fix(#N): ...`).
#   4. The head branch is `<prefix>/issue-<N>`, optionally followed by `-` or
#      `/` and more text: `feat/issue-1277-...`, `docs/issue-1277-...`,
#      `fix/issue-1277/...`. That is the only shape the workflow derives
#      (`${BRANCH_PREFIX:-feat}/issue-<N>-<slug>`). The prefix is exactly one
#      segment, and the match is case-sensitive.
#
# A bare number in a branch name is NOT a claim (#1707). The earlier rule
# accepted N anywhere between separators, so another tool's branch
# `codex/p5-1084-agent-preview-controls-2232`, whose `1084` is that tool's own
# task number, was reported as claiming issue #1084 and refused the run. Such a
# PR still claims the issue if it says so through signals 1 to 3. A branch the
# workflow did not derive, such as `fix/1084-...`, is likewise only a claim when
# its PR says so.
#
# The body is fetched for matching only. It is dropped from every PR before the
# result is reported, so body text never appears on stdout, in the ERROR block
# or in logs. What IS reported is which signals made each PR a claim:
# `via=closing-ref`, `via=body-keyword`, `via=title` or `via=branch`, comma
# separated when several match. Without it a body-only claim would show an
# unrelated-looking PR on an unrelated branch with no reason given.
#
# WHICH PULL REQUESTS ARE SEARCHED
#
# Open ones, and ones merged inside a recent window (default 24h). Both halves
# matter: in every confirmed case above, the winner was already open — and in
# the #1123 case it merged 29 minutes after the loser was opened, so a search
# restricted to open PRs would have missed a run starting half an hour later.
# The window is what keeps this from refusing legitimate follow-up work on an
# issue that was fixed and reopened months ago.
#
# The lookup is a plain `gh pr list`, NOT `gh pr list --search`. GitHub's search
# index lags behind reality by seconds to minutes; a race measured in minutes is
# exactly where that lag would hide the PR we are looking for.
#
# WHY IT IS NOT A GATE ON ITS OWN ABILITY TO RUN
#
# #1268 is the local precedent for what a brittle gate costs, and
# workflow_identity_preflight.sh (#1290) is the reference shape. Three outcomes,
# kept strictly apart:
#
#   the check ran and found a claim  -> exit 1, naming the PR and the remedy
#   the check could not be made      -> WARNING on stderr, exit 0
#   there is nothing to check        -> INFO on stderr, exit 0
#     (no git repo, no origin, a non-GitHub remote, no `gh`, no `jq`,
#      a non-numeric issue reference from local tracking)
#
# Note what is absent from the refusal path: an API error of ANY kind, including
# a 403. GitHub answers both "slow down" and "you may not" with 403, so this
# helper classifies rate limiting FIRST and then declines to turn any remaining
# failure into a verdict — an unreadable answer is not evidence that an issue is
# unclaimed, and it is not evidence that it is claimed either. Only a PR we
# actually saw stops a run.
#
# A MERGED PULL REQUEST CLAIMS THE ISSUE ONLY WHILE THE ISSUE IS NOT OPEN
# (issue #2238)
#
# A pull request whose body says `Closes #N` closes the issue when it merges,
# so "merged claim, closed issue" is the ordinary pair, and the #1123 race
# above is that pair seen inside the window. The pair that is NOT a race is a
# merged pull request that left the issue open: one that only referenced the
# issue ("Refs #N, which stays open until this lands"), or that closed a
# sibling issue while linking this one. Follow-up work on such an issue met a
# refusal naming a pull request that had already shipped, with no remedy short
# of a bypass. #2238 records two of them, from Product-Management lanes on
# another repository; the issue and pull request numbers there are that
# repository's, and every other number in this header is this one's.
#
# So when any claim in the window is merged, the issue's own state is read
# once, `gh issue view N --json state`, and if it reads OPEN every merged
# claim is dropped from both halves of the result. Open claims are untouched:
# an open pull request claims the issue whatever state the issue is in.
#
# A merged claim on our own head branch is dropped with the rest, since the
# rule is about the pull request having merged, not about whose it is. That
# changes what such a run prints. With no claim of ours left to report, a
# review or resume run standing on a merged pull request's head branch now
# takes the plain `issue_claim_check: ok issue=.. repo=..` line instead of
# `issue_claim_check: ok-own-pr issue=.. repo=.. pr=.. head=..`, and the #1448
# INFO line below is not emitted. The outcome and the exit code are the same
# either way; only the stdout token and the explanation differ.
#
# The issue's state is the whole discriminator. closedByPullRequestsReferences
# is deliberately not requested, although it looks like the direct answer:
# GitHub populates it with pull requests LINKED to close the issue, not with
# the one that did. In the case recorded in #2238 the issue was still open and
# that field already listed the merged pull request, so consulting it would
# re-refuse the exact case this rule exists to admit.
#
# The state is read at most once per run, after the claim list exists and only
# when a merged claim is in it, never in a loop and never with a retry. No
# grace period is applied to it. A pull request merged seconds ago may briefly
# leave the issue reading OPEN and have its claim dropped; covering that would
# mean a wall-clock window, and the recorded races were 29 and 63 minutes
# apart, both of them visible through the open half instead.
#
# Every way the read can fail leaves the merged claims counting, which is the
# behaviour from before #2238: a non-zero exit, a timeout, an empty answer, or
# an answer that is not a plain state word. An unreadable state is not evidence
# that an issue is open, and promoting it to "cannot check" would disable the
# #1361 guard on any API hiccup. There is no new outcome and no new exit code;
# the three outcomes above are unchanged.
#
# What this means in practice: an issue left open by a merged pull request is
# free for follow-up work, with no `allow_existing_pr=true` and no
# `AMPLIHACK_SKIP_ISSUE_CLAIM_CHECK=1`. When a refusal does happen it says
# which kind of claim it stands on in its first line — "open pull request #N
# is already working on issue #M", or "recently-merged pull request #N ..." —
# and whenever a merged claim was in the window the block also reports
# the issue state that was read, on an `issue state:` line beside the
# repository and branch. A refusal on a merged pull request is therefore
# legible as one without reading the claim list, and reopening the issue is
# the remedy when such a refusal is wrong. The verdict thus depends on
# open/closed state, which anyone with triage permission can change; the
# influence runs one way only, since it can drop a claim and never create one,
# and it is weaker than the two bypasses the helper already honours.
#
# WORKING ON THE PULL REQUEST IS NOT RACING IT (issue #1448)
#
# The claim is on the ISSUE, so every follow-up run against an issue that has a
# pull request — reviewing it, acting on review findings, fixing its CI, or
# resuming a run a usage limit killed after the PR was opened (#1390) — met the
# same refusal as a second implementation attempt. Two review runs died here
# having done no work, and the caller abandoned the workflow. For those tasks a
# pull request existing is the normal precondition, not a race.
#
# The evidence needed to tell the two apart was already in hand and unread: the
# branch checked out in the working directory. A second implementation attempt
# derives a NEW branch — that is the whole mechanism of #1361 — so it is never
# standing on the claiming PR's head. A review, a review-fix and a resume all
# are. So:
#
#   claim head == the branch checked out in --repo-path, same repository
#     -> this stream is continuing that pull request. Allow, and say so.
#   any other claim
#     -> a different branch is implementing this issue. Refuse, as before.
#
# Cross-repository claims are excluded from that allowance: a fork's PR may have
# `headRefName: main` while we sit on our own `main`, and equal names there are
# not the same branch.
#
# The allowance costs the #1361 guard nothing. It only ever applies here, at
# prep time, where no PR is being created; workflow_publish_pr.sh passes an
# explicit `--head` (the branch it is about to push), so at the moment a PR is
# actually opened this default is not consulted at all. A run that is allowed
# through prep on an existing branch and then derives a NEW branch anyway is
# still refused at publish, by the same check.
#
# ALLOW_EXISTING_PR is the escape hatch for the case the comparison cannot
# cover: the working directory is not on that branch (a fresh clone about to
# check out the PR, a review driven from a detached HEAD). It states an intent
# the check cannot infer. AMPLIHACK_SKIP_ISSUE_CLAIM_CHECK remains the blunter
# instrument that disables the check outright.
#
# What is deliberately NOT used: the task's classification. "This is a review,
# not an implementation" would be the most direct signal, and classification is
# exactly what has been unreliable here (#1435, #1437, #1402). A branch name is
# a fact; a classification is a guess.
#
# This helper is deliberately self-contained: it sources nothing. A check whose
# job is to keep a run from wasting itself must not acquire new ways to fail.
#
# Usage:
#   workflow_issue_claim_check.sh --issue N [--repo-path DIR] [--head BRANCH]
#                                 [--repo OWNER/NAME] [--allow-existing-pr]
#
#   --head BRANCH   a branch whose PR is OURS and therefore never a duplicate.
#                   Passed by the publish helper so the run's own PR (already
#                   handled by the head/base scoped lookup) is not mistaken for
#                   a competitor.
#
# Environment:
#   ALLOW_EXISTING_PR                         true/1/yes/on -> skip entirely
#   AMPLIHACK_ALLOW_EXISTING_PR               same, for callers outside a recipe
#   AMPLIHACK_SKIP_ISSUE_CLAIM_CHECK          non-empty -> skip entirely
#   AMPLIHACK_CLAIM_CHECK_TIMEOUT             seconds per API call (default 45)
#   AMPLIHACK_CLAIM_CHECK_MERGED_WINDOW_HOURS merged-PR window (default 24)
#   AMPLIHACK_CLAIM_CHECK_OPEN_LIMIT          open PRs fetched (default 100)
#   AMPLIHACK_CLAIM_CHECK_MERGED_LIMIT        merged PRs fetched (default 60)
#
# stdout is one machine-readable line (it becomes the recipe step's context
# value). Everything human-facing goes to stderr.
#
# Exit codes:
#   0  no claim / skipped / could-not-check
#   1  an existing pull request already claims this issue

# No `set -e`: this helper must always reach a structured answer rather than
# dying mid-probe and leaving the caller to guess what it meant.
set -uo pipefail
export GH_PAGER=cat PAGER=cat GIT_PAGER=cat LESS=FRX

ISSUE=""
REPO_ARG="${REPO_PATH:-.}"
OWN_HEAD=""
REPO_SLUG=""
ALLOW_EXISTING=""
SLUG_FROM_ORIGIN=""

while [ "$#" -gt 0 ]; do
  case "$1" in
    --issue) ISSUE="${2:-}"; shift 2 ;;
    --repo-path) REPO_ARG="${2:-}"; shift 2 ;;
    --head) OWN_HEAD="${2:-}"; shift 2 ;;
    --repo) REPO_SLUG="${2:-}"; shift 2 ;;
    --allow-existing-pr) ALLOW_EXISTING="true"; shift ;;
    -h|--help) sed -n '1,/^#   1  an existing pull request already claims this issue$/p' "$0" >&2; exit 0 ;;
    *) shift ;;
  esac
done
# No fallback to a bare ISSUE_NUMBER: an empty --issue means this run has no
# issue, and an inherited name would be a parent run's issue (#1684).

TIMEOUT_S="${AMPLIHACK_CLAIM_CHECK_TIMEOUT:-45}"
WINDOW_H="${AMPLIHACK_CLAIM_CHECK_MERGED_WINDOW_HOURS:-24}"
OPEN_LIMIT="${AMPLIHACK_CLAIM_CHECK_OPEN_LIMIT:-100}"
MERGED_LIMIT="${AMPLIHACK_CLAIM_CHECK_MERGED_LIMIT:-60}"
case "$TIMEOUT_S" in ''|*[!0-9]*) TIMEOUT_S=45 ;; esac
case "$WINDOW_H" in ''|*[!0-9]*) WINDOW_H=24 ;; esac
case "$OPEN_LIMIT" in ''|*[!0-9]*) OPEN_LIMIT=100 ;; esac
case "$MERGED_LIMIT" in ''|*[!0-9]*) MERGED_LIMIT=60 ;; esac

# The issue's own state (#2238), read at most once and only when a merged claim
# is in the window. Both are initialised here and expanded as ${VAR:-} at every
# use: under `set -u` an unbound expansion would exit 1 before any verdict
# reached stdout, and both callers read a bare exit 1 as FAILED_DUPLICATE_CLAIM.
ISSUE_STATE=""
ISSUE_STATE_READ=""

note() { printf '%s\n' "$*" >&2; }

# is_true <value> — an affirmative context/env value. `allow_existing_pr=false`
# must NOT read as "set"; a bare non-empty test would have made the flag
# impossible to turn off once a caller had learned to pass it.
is_true() {
  case "$(printf '%s' "${1:-}" | tr '[:upper:]' '[:lower:]')" in
    1|true|yes|on) return 0 ;;
    *) return 1 ;;
  esac
}

# Nothing in this file ever prints a token value; this is the belt to that pair
# of braces, applied to every provider message before it reaches a log.
redact() {
  sed -E \
    -e 's#gh[pousr]_[A-Za-z0-9_]{8,}#<redacted-token>#g' \
    -e 's#github_pat_[A-Za-z0-9_]{8,}#<redacted-token>#g' \
    -e 's#https?://[^[:space:]/]*@#https://<redacted>@#g' \
    -e 's#[Bb]earer[[:space:]]+[A-Za-z0-9._~+/=-]{20,}#Bearer <redacted-token>#g'
}

# skip <reason> — nothing to check here.
skip() {
  note "INFO: issue claim check skipped: $1"
  printf 'issue_claim_check: skipped reason=%s\n' "$1"
  exit 0
}

# unknown <reason> — the check itself could not be made. Warn, do not gate.
unknown() {
  note "WARNING: issue claim check could not be completed ($1) — continuing without a claim check. This is advisory: an un-runnable check must not stop work (issue #1268)."
  printf 'issue_claim_check: unknown reason=%s\n' "$1"
  exit 0
}

# ---------------------------------------------------------------------------
# 1. Is there anything to check?
# ---------------------------------------------------------------------------

[ -n "${AMPLIHACK_SKIP_ISSUE_CLAIM_CHECK:-}" ] && skip "AMPLIHACK_SKIP_ISSUE_CLAIM_CHECK is set"

# The caller has stated that an existing pull request is this run's precondition
# (#1448). Nothing left to ask GitHub about.
if is_true "$ALLOW_EXISTING" || is_true "${ALLOW_EXISTING_PR:-}" || is_true "${AMPLIHACK_ALLOW_EXISTING_PR:-}"; then
  skip "allow-existing-pr-requested"
fi

# Local tracking emits references like `local-issue-42`; there is no provider to
# ask, so there is nothing to check.
case "$ISSUE" in
  '') skip "no-issue-number" ;;
  *[!0-9]*) skip "issue-reference-not-numeric:${ISSUE}" ;;
esac

cd "$REPO_ARG" 2>/dev/null || unknown "repo-path-unreadable"
git rev-parse --is-inside-work-tree >/dev/null 2>&1 || skip "not-a-git-repository"

# The branch checked out here — the load-bearing fact of #1448, read inline.
# Empty on a detached HEAD, which simply means the comparison below cannot fire
# and the pre-#1448 behaviour stands.
CWD_HEAD="$(git symbolic-ref --quiet --short HEAD 2>/dev/null)" || CWD_HEAD=""

if [ -z "$REPO_SLUG" ]; then
  ORIGIN="$(git remote get-url origin 2>/dev/null)" || ORIGIN=""
  [ -n "$ORIGIN" ] || skip "no-origin-remote"
  # Both URL shapes git uses:
  #   scheme://[user@]host[:port]/owner/repo[.git]
  #   [user@]host:owner/repo[.git]                    (scp-like)
  if [ "${ORIGIN#*://}" != "$ORIGIN" ]; then
    REST="${ORIGIN#*://}"; REST="${REST##*@}"
    HOST="${REST%%/*}"; HOST="${HOST%%:*}"
    RPATH="${REST#*/}"
  else
    REST="${ORIGIN##*@}"
    HOST="${REST%%:*}"
    RPATH="${REST#*:}"
  fi
  HOST="$(printf '%s' "$HOST" | tr '[:upper:]' '[:lower:]')"
  RPATH="${RPATH%.git}"; RPATH="${RPATH%/}"
  OWNER="${RPATH%%/*}"
  NAME="${RPATH#*/}"; NAME="${NAME%%/*}"
  case "$OWNER" in ''|*[!A-Za-z0-9._-]*) skip "origin-not-parseable-as-owner/repo" ;; esac
  case "$NAME" in ''|*[!A-Za-z0-9._-]*) skip "origin-not-parseable-as-owner/repo" ;; esac
  # Azure DevOps and everything else have their own PR model and are not this
  # check's business.
  case "$HOST" in
    github.com|*.ghe.com|*.githubenterprise.com) ;;
    *) [ "$HOST" = "${GH_HOST:-}" ] || skip "non-github-remote:${HOST}" ;;
  esac
  REPO_SLUG="${OWNER}/${NAME}"
  SLUG_FROM_ORIGIN="true"
fi

command -v gh >/dev/null 2>&1 || unknown "gh-not-installed"
command -v jq >/dev/null 2>&1 || unknown "jq-not-installed"

# ---------------------------------------------------------------------------
# 2. Ask GitHub which pull requests exist. Two reads, no search index.
# ---------------------------------------------------------------------------

FIELDS="number,url,title,state,mergedAt,headRefName,isCrossRepository,closingIssuesReferences,baseRefName,body"
ERRFILE="$(mktemp "${TMPDIR:-/tmp}/issue-claim-check.XXXXXX")" || unknown "cannot-create-tempfile"
# shellcheck disable=SC2329  # invoked by the EXIT trap below.
cleanup() { rm -f "$ERRFILE"; }
trap cleanup EXIT

gh_list() { # gh_list <state> <limit> -> JSON array on stdout, rc from gh
  if command -v timeout >/dev/null 2>&1; then
    timeout "$TIMEOUT_S" gh pr list --repo "$REPO_SLUG" --state "$1" --limit "$2" --json "$FIELDS" 2>>"$ERRFILE"
  else
    gh pr list --repo "$REPO_SLUG" --state "$1" --limit "$2" --json "$FIELDS" 2>>"$ERRFILE"
  fi
}

# classify_failure <rc> — never returns; every path is `unknown`.
#
# Rate limiting is classified FIRST and named separately because GitHub answers
# both "slow down" and "you may not" with 403. It changes only the WARNING an
# operator reads: no failure classification here can produce a refusal, because
# an unreadable answer is not evidence about who is working on an issue.
classify_failure() {
  local rc="$1" errtext
  errtext="$(head -c 2000 "$ERRFILE" 2>/dev/null | redact | tr '\n' ' ')"
  [ -n "$errtext" ] || errtext="(no error output; gh exited ${rc})"
  if [ "$rc" -eq 124 ] || [ "$rc" -eq 137 ]; then
    unknown "api-call-timed-out-after-${TIMEOUT_S}s"
  fi
  if printf '%s' "$errtext" | grep -Eiq 'rate limit|secondary rate|abuse detection|retry-after|x-ratelimit'; then
    unknown "rate-limited"
  fi
  if printf '%s' "$errtext" | grep -Eiq 'no such host|dial tcp|connection (refused|reset)|timed? ?out|tls handshake|network is unreachable|temporary failure|EOF|HTTP 5[0-9][0-9]|(^|[^0-9])(500|502|503|504)([^0-9]|$)|proxy'; then
    unknown "network-or-server-error"
  fi
  if printf '%s' "$errtext" | grep -Eiq 'not authoriz|unauthoriz|forbidden|denied|bad credentials|requires authentication|not logged in|could not resolve|(^|[^0-9])(401|403|404)([^0-9]|$)'; then
    unknown "not-authorised-to-list-pull-requests"
  fi
  unknown "unrecognised-api-error: ${errtext}"
}

RC=0
OPEN_JSON="$(gh_list open "$OPEN_LIMIT")" || RC=$?
[ "$RC" -eq 0 ] || classify_failure "$RC"
RC=0
MERGED_JSON="$(gh_list merged "$MERGED_LIMIT")" || RC=$?
[ "$RC" -eq 0 ] || classify_failure "$RC"

# An empty answer reads as no pull requests. The test is a regex, not
# `${VAR//[[:space:]]/}`: with PR bodies included the listings run to hundreds
# of kilobytes, and bash's pattern substitution is quadratic in the number of
# whitespace characters, so that form took minutes on a real repository (#1707).
[[ "$OPEN_JSON" =~ [^[:space:]] ]] || OPEN_JSON="[]"
[[ "$MERGED_JSON" =~ [^[:space:]] ]] || MERGED_JSON="[]"

# The default branch decides whether the body signal applies (see header).
# A local read of origin/HEAD first, when the slug came from origin; otherwise
# one API call. Any failure leaves it empty, which only narrows the body signal
# to "closingIssuesReferences is empty". A reply that is not a plausible branch
# name is discarded rather than trusted.
DEFAULT_BRANCH=""
if [ "$SLUG_FROM_ORIGIN" = "true" ]; then
  DEFAULT_BRANCH="$(git symbolic-ref --quiet --short refs/remotes/origin/HEAD 2>/dev/null)" || DEFAULT_BRANCH=""
  DEFAULT_BRANCH="${DEFAULT_BRANCH#origin/}"
fi
if [ -z "$DEFAULT_BRANCH" ]; then
  if command -v timeout >/dev/null 2>&1; then
    DEFAULT_BRANCH="$(timeout "$TIMEOUT_S" gh repo view "$REPO_SLUG" --json defaultBranchRef --jq '.defaultBranchRef.name' 2>/dev/null)" || DEFAULT_BRANCH=""
  else
    DEFAULT_BRANCH="$(gh repo view "$REPO_SLUG" --json defaultBranchRef --jq '.defaultBranchRef.name' 2>/dev/null)" || DEFAULT_BRANCH=""
  fi
fi
# First line only, then no whitespace, by parameter expansion: a pipeline
# ending in an early-exit stage is the shape the #1434 guard bans.
DEFAULT_BRANCH="${DEFAULT_BRANCH%%$'\n'*}"
DEFAULT_BRANCH="${DEFAULT_BRANCH//[[:space:]]/}"
[[ "$DEFAULT_BRANCH" =~ ^[A-Za-z0-9._/-]+$ ]] || DEFAULT_BRANCH=""

CUTOFF="$(date -u -d "-${WINDOW_H} hours" +%Y-%m-%dT%H:%M:%SZ 2>/dev/null \
  || date -u -v-"${WINDOW_H}"H +%Y-%m-%dT%H:%M:%SZ 2>/dev/null || true)"
[ -n "$CUTOFF" ] || CUTOFF="0000-01-01T00:00:00Z"

# ---------------------------------------------------------------------------
# 3. Which of them claim this issue?
# ---------------------------------------------------------------------------
#
# The four signals, and why a bare number in a branch is not a claim (#1707),
# are described in the header. The body and branch patterns build a regex from
# $n, which is safe only because --issue has already been checked to be digits.
#
# The PR lists reach jq on stdin, not as --argjson arguments. With bodies
# included, 100 open PRs easily exceed the kernel's 128 KiB limit on a single
# argument; jq would then fail to start, the result would read as
# claim-filter-unreadable, and the guard would fail open without a word.
# `body` is deleted from each selected PR before the split below.
#
# strip_code removes fenced blocks first, then inline code spans
# (CommonMark 0.31, sections 4.5 and 6.1). Every pattern is bounded so that
# stray punctuation cannot swallow a real `Fixes #N` that follows it, which
# would fail open:
#
#   - A fence opens only at the start of a line, after at most three spaces,
#     and a paired fence closes only on a line of its own. A ``` or ~~~ in
#     the middle of a line is text, not a fence. Only a fence that does open
#     and never closes runs to the end of the body, as CommonMark specifies.
#   - A code span opens with a backtick run of any length and closes at the
#     next run of exactly that length; the lookarounds stop a run of two from
#     being read as two runs of one, so ``Fixes #N`` is removed whole. A run
#     with no partner is text. A span may wrap onto the next line but may not
#     cross a blank line, since a blank line ends the paragraph. `\r` is
#     allowed in the blank line because bodies are often CRLF.
#
# jq's regex engine (Oniguruma) supports the backreference and lookbehind
# used here.
#
# The claims are then split in two (#1448). `ours` are the claims this working
# directory is standing on: same head branch, same repository. They are this
# stream continuing that pull request — a review, a review-fix, a CI fix, a
# resume — and are not competitors. `others` are on a different branch, which is
# the shape of every incident in #1361, and they still refuse the run.

CLAIMS="$(
  printf '%s\n%s\n' "$OPEN_JSON" "$MERGED_JSON" | jq -sc \
    --arg issue "$ISSUE" \
    --arg own_head "$OWN_HEAD" \
    --arg cwd_head "$CWD_HEAD" \
    --arg cutoff "$CUTOFF" \
    --arg default "$DEFAULT_BRANCH" \
    --arg repo "$REPO_SLUG" '
      .[0] as $open
      | .[1] as $merged
      | (if ($repo | test("^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$"))
         then ($repo | gsub("\\."; "\\."))
         else "(?!)" end) as $repo_re
      | def strip_code:
        gsub("(^|\n)[ ]{0,3}```[^`\n]*\n(?:[\\s\\S]*?\n)?[ ]{0,3}```+[ \t\r]*(?=\n|$)"; "\n")
        | gsub("(^|\n)[ ]{0,3}~~~[^\n]*\n(?:[\\s\\S]*?\n)?[ ]{0,3}~~~+[ \t\r]*(?=\n|$)"; "\n")
        | gsub("(^|\n)[ ]{0,3}(```|~~~)[\\s\\S]*$"; "\n")
        | gsub("(?<!`)(`+)(?!`)(?:[^\n]|\n(?![ \t\r]*\n))*?(?<!`)\\1(?!`)"; " ");
      def body_applies:
        ((.closingIssuesReferences // []) | length == 0)
        and (($default == "") or ((.baseRefName // "") != $default));
      def claim_via($n):
        [ (if ((.closingIssuesReferences // []) | map((.number // -1) | tostring) | index($n) != null)
             then "closing-ref" else empty end),
          (if body_applies
              and ((.body // "") | strip_code
                   | test("(^|[^A-Za-z0-9_])(close[sd]?|fix(e[sd])?|resolve[sd]?):?[[:space:]]*"
                        + "(#|" + $repo_re + "#|https?://[^/[:space:]]+/" + $repo_re + "/issues/)"
                        + $n + "([^0-9]|$)"; "i"))
             then "body-keyword" else empty end),
          (if ((.title // "") | contains("(#" + $n + ")")) then "title" else empty end),
          (if ((.headRefName // "") | test("^[^/]+/issue-" + $n + "([/-]|$)")) then "branch" else empty end)
        ];
      def is_ours:
        ($cwd_head != "")
        and ((.headRefName // "") == $cwd_head)
        and (((.isCrossRepository // false) | not));
      [ ($open | map(. + {claim_source: "open"}))
        + ($merged | map(select((.mergedAt // "") >= $cutoff)) | map(. + {claim_source: "recently-merged"}))
        | .[]
        | . + {claim_via: claim_via($issue)}
        | select(.claim_via | length > 0)
        | select(($own_head == "") or ((.headRefName // "") != $own_head))
        | del(.body)
      ]
      | {ours: map(select(is_ours)), others: map(select(is_ours | not))}
    ' 2>>"$ERRFILE"
)" || CLAIMS=""

if [ -z "${CLAIMS//[[:space:]]/}" ]; then
  unknown "claim-filter-unreadable"
fi

# ---------------------------------------------------------------------------
# 3b. A merged pull request claims the issue only while the issue is not open
#     (issue #2238).
# ---------------------------------------------------------------------------
#
# Reached only when a merged claim survived section 3, so the ordinary run —
# no claims, or open ones only — makes no additional API call and behaves
# exactly as it did before. The call is made once here, never per claim and
# never in a loop, and it is not retried.

MERGED_CLAIMS="$(printf '%s' "$CLAIMS" | jq '
  [(.ours // []), (.others // [])] | add
  | map(select(.claim_source == "recently-merged")) | length' 2>/dev/null || echo "")"
case "$MERGED_CLAIMS" in ''|*[!0-9]*) MERGED_CLAIMS=0 ;; esac

if [ "$MERGED_CLAIMS" -gt 0 ]; then
  ISSUE_STATE_READ="true"
  # stderr to /dev/null, not to $ERRFILE: routing provider text from this call
  # into the shared error file would let it surface through the unrecognised-
  # api-error message, which is an outcome this filter must never reach.
  if command -v timeout >/dev/null 2>&1; then
    ISSUE_STATE="$(timeout "$TIMEOUT_S" gh issue view "$ISSUE" --repo "$REPO_SLUG" --json state --jq '.state' 2>/dev/null)" || ISSUE_STATE=""
  else
    ISSUE_STATE="$(gh issue view "$ISSUE" --repo "$REPO_SLUG" --json state --jq '.state' 2>/dev/null)" || ISSUE_STATE=""
  fi
  # Bounded first, then first line, then stripped, then allow-listed — by
  # parameter expansion only, because a pipeline ending in an early-exit stage
  # is the shape the #1434 guard bans.
  #
  # The bound is first because EVERY pattern-matching expansion below it is
  # quadratic in bash, `%%` included, and a reply served in place of a state
  # word is not length-bounded. Taking the first line first looks harmless and
  # is not: `${v%%$'\n'*}` on a single line with no newline in it tries the
  # pattern at every suffix position, measured here at 0.47s for 64 KiB, 7.2s
  # for 256 KiB and 79s for 300 KiB, inside a guard that both callers are
  # blocked on. Cutting to 64 bytes first is O(1) in practice and costs
  # nothing: a state word is four to six characters, so no well-formed reply
  # reaches the bound, and a malformed one only has to survive as far as the
  # allow-list to be rejected. This is the #1707 family of defect, found by
  # measurement in the #2238 security review rather than by reading.
  #
  # The allow-list is what makes this value printable: it admits a bare word
  # and nothing else, so no token shape, URL, escape sequence or control
  # character can reach a log.
  ISSUE_STATE="${ISSUE_STATE:0:64}"
  ISSUE_STATE="${ISSUE_STATE%%$'\n'*}"
  ISSUE_STATE="${ISSUE_STATE//[[:space:]]/}"
  [[ "$ISSUE_STATE" =~ ^[A-Za-z_]+$ ]] || ISSUE_STATE=""

  if [ "$(printf '%s' "$ISSUE_STATE" | tr '[:upper:]' '[:lower:]')" = "open" ]; then
    # The re-filter is validated structurally rather than for non-emptiness: a
    # jq program that succeeded but emitted null or a scalar would replace
    # CLAIMS with something `.others | length` cannot read, and the guard would
    # fail open through claim-filter-unreadable. This program emits nothing at
    # all unless its input is the two-array object section 3 builds.
    FILTERED="$(printf '%s' "$CLAIMS" | jq -c '
      if (type == "object") and ((.ours | type) == "array") and ((.others | type) == "array")
      then {ours:   (.ours   | map(select(.claim_source != "recently-merged"))),
            others: (.others | map(select(.claim_source != "recently-merged")))}
      else empty end' 2>/dev/null)" || FILTERED=""
    if [[ "$FILTERED" =~ [^[:space:]] ]]; then
      # Numbers only. Titles and bodies are attacker-controlled free text and
      # are never printed; `numbers` discards anything that is not one.
      DROPPED="$(printf '%s' "$CLAIMS" | jq -r '
        [(.ours // []), (.others // [])] | add
        | map(select(.claim_source == "recently-merged") | .number)
        | map(numbers | tostring) | join(", #")' 2>/dev/null || echo "")"
      CLAIMS="$FILTERED"
      if [ -n "$DROPPED" ]; then
        note "INFO: issue claim check: pull request #${DROPPED} merged recently, but issue #${ISSUE} is still open, so it is not a claim on the work that remains — a merged pull request claims an issue only while that issue is closed (issue #2238)."
      fi
    fi
  fi
fi

COUNT="$(printf '%s' "$CLAIMS" | jq '.others | length' 2>/dev/null || echo "")"
case "$COUNT" in ''|*[!0-9]*) unknown "claim-filter-unreadable" ;; esac
OWN_COUNT="$(printf '%s' "$CLAIMS" | jq '.ours | length' 2>/dev/null || echo "")"
case "$OWN_COUNT" in ''|*[!0-9]*) unknown "claim-filter-unreadable" ;; esac

if [ "$COUNT" -eq 0 ] && [ "$OWN_COUNT" -gt 0 ]; then
  OWN_URL="$(printf '%s' "$CLAIMS" | jq -r '.ours[0].url // ""')"
  OWN_NUM="$(printf '%s' "$CLAIMS" | jq -r '(.ours[0].number // "") | tostring')"
  note "INFO: issue claim check: pull request #${OWN_NUM} claims issue #${ISSUE}, and its head branch '${CWD_HEAD}' is the branch checked out here — this run is continuing that pull request, not racing it (issue #1448). ${OWN_URL}"
  printf 'issue_claim_check: ok-own-pr issue=%s repo=%s pr=%s head=%s\n' \
    "$ISSUE" "$REPO_SLUG" "${OWN_URL:-unknown}" "$CWD_HEAD"
  exit 0
fi

if [ "$COUNT" -eq 0 ]; then
  note "INFO: issue claim check: no open or recently-merged pull request claims issue #${ISSUE} in ${REPO_SLUG}."
  printf 'issue_claim_check: ok issue=%s repo=%s\n' "$ISSUE" "$REPO_SLUG"
  exit 0
fi

# ---------------------------------------------------------------------------
# 4. A claim exists. Stop, and say what to do instead.
# ---------------------------------------------------------------------------

FIRST_URL="$(printf '%s' "$CLAIMS" | jq -r '.others[0].url // ""')"
FIRST_HEAD="$(printf '%s' "$CLAIMS" | jq -r '.others[0].headRefName // ""')"
FIRST_NUM="$(printf '%s' "$CLAIMS" | jq -r '(.others[0].number // "") | tostring')"
# "open" or "recently-merged", generated in section 3 and never provider text.
FIRST_SOURCE="$(printf '%s' "$CLAIMS" | jq -r '.others[0].claim_source // ""')"

# Saying which kind of pull request the refusal stands on makes a merged claim
# legible as one from the first line, which is what the reports in #2238 were
# missing. Either part being unavailable falls back to the wording that was
# here before; the `already working on issue #N` clause is unchanged in both.
HEADLINE="ERROR: issue claim check failed — a pull request is already working on issue #${ISSUE}."
if [ -n "$FIRST_NUM" ] && [ -n "$FIRST_SOURCE" ]; then
  HEADLINE="ERROR: issue claim check failed — ${FIRST_SOURCE} pull request #${FIRST_NUM} is already working on issue #${ISSUE}."
fi

{
  echo "$HEADLINE"
  echo ""
  echo "  repository : ${REPO_SLUG}"
  echo "  issue      : #${ISSUE}"
  # Only when the lookup actually ran, so its absence means no merged claim was
  # in the window rather than a state that could not be read.
  if [ -n "${ISSUE_STATE_READ:-}" ]; then
    echo "  issue state: ${ISSUE_STATE:-(unreadable)}"
  fi
  echo "  branch here: ${CWD_HEAD:-(detached HEAD)}"
  echo "  claimed by :"
  printf '%s' "$CLAIMS" | jq -r '.others[] | "    #\(.number)  \(.claim_source)  via=\(.claim_via | join(","))  head=\(.headRefName // "?")  \(.url)"'
  echo ""
  echo "  Opening a second pull request here is the failure recorded in issue #1361."
  echo "  The second one merges, the first is never closed, and a month later it"
  echo "  presents as conflicted work that is byte-identical to what already shipped."
  echo ""
  echo "  To continue the EXISTING pull request instead of racing it, re-run the"
  echo "  workflow targeting it — the recipe accepts either:"
  if [ -n "$FIRST_NUM" ]; then echo "    pr_number=${FIRST_NUM}"; fi
  if [ -n "$FIRST_HEAD" ]; then echo "    existing_branch=${FIRST_HEAD}"; fi
  echo "  Both make step-04 reuse that branch rather than deriving a new one, and"
  echo "  this check steps aside when either is set."
  echo ""
  echo "  Reviewing, fixing review findings or CI, or resuming that pull request?"
  echo "  Check out its head branch here and this check allows the run (#1448):"
  if [ -n "$FIRST_HEAD" ]; then echo "    git checkout ${FIRST_HEAD}"; fi
  echo "  If the working directory cannot be on that branch, state the intent:"
  echo "    -c allow_existing_pr=true"
  echo ""
  echo "  If the existing pull request is genuinely abandoned, close it first."
  echo "  If this check is wrong for your setup, set AMPLIHACK_SKIP_ISSUE_CLAIM_CHECK=1."
} >&2

printf 'issue_claim_check: claimed issue=%s repo=%s pr=%s\n' "$ISSUE" "$REPO_SLUG" "${FIRST_URL:-unknown}"
exit 1
