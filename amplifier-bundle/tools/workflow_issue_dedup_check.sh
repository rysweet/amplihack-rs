#!/usr/bin/env bash
# workflow_issue_dedup_check.sh — has a tracking issue already been opened for
# THIS task? (issue #1420)
#
# The failure this exists to prevent: a `default-workflow` run through
# `smart-orchestrator` died when the Claude account hit a usage limit. The
# relaunch did not resume — it re-ran `workflow-prep` from step 1 and created a
# SECOND GitHub issue for the same task, 27 minutes after the first and with a
# byte-identical title, then a second worktree, a second branch and a second
# pull request.
#
#   issue #110 (04:24)  ->  branch ci/dev-infrastructure          ->  PR #116
#   issue #112 (04:51)  ->  branch feat/issue-112-build-devel...  ->  PR #118
#
# `workflow-prep.yaml` DID have a de-duplication guard. It could not fire,
# because of how it asked the question:
#
#   SEARCH_Q="${ISSUE_TITLE:0:100}"
#   gh issue list --state open --search "$SEARCH_Q" ...
#
# The title was the verbatim task description, which began with a filesystem
# path, so the first hundred characters were:
#
#   Build development infrastructure for the Jamestown repository at ~/src/mistt/jamestown (Gi
#
# Three defects in one string: it is cut mid-token (`(Gi`), it carries an
# unbalanced `(`, and it contains a path whose slashes GitHub's search index
# tokenises differently from the stored title. Against the live repository, with
# the issue open, that query returned zero rows while `Build development
# infrastructure Jamestown` returned both issues. `2>/dev/null || echo ''`
# then swallowed the empty answer, control fell through to `gh issue create`,
# and nothing in the log recorded that a duplicate check had run at all.
#
# HOW THE QUESTION IS ASKED INSTEAD
#
# 1. A DETERMINISTIC TASK KEY, not prose. `--print-key` derives a stable
#    fingerprint of the task description (whitespace-collapsed, lowercased,
#    SHA-256, first 16 hex). workflow-prep writes it into the issue body as
#    `<!-- amplihack-task-key: HEX -->`. A relaunch of the same task derives the
#    same key and finds its own issue by exact match — no tokenisation, no
#    punctuation, no truncation, and it works for a title that is entirely
#    filesystem paths.
#
# 2. A PLAIN LIST, not the search index. The primary lookup is
#    `gh issue list --state all --limit N`, filtered locally. GitHub's search
#    index lags reality by seconds to minutes; a relaunch races inside exactly
#    that window, and the whole point is to see an issue created moments ago.
#    The search index is used only as a widening FALLBACK when the recent list
#    holds no candidate, and even then the decision is made locally by the same
#    comparison — never by trusting `.[0]`.
#
# 3. TITLES COMPARED AFTER NORMALISATION, not as a query. Both sides are
#    lowercased and reduced to alphanumeric words before comparison, so a path,
#    a bracket or a mid-token cut cannot change the answer. This is what finds
#    an issue created BEFORE the task key existed — issue #110 in the report
#    above has no key in its body and must still be found.
#
# 4. ALL STATES, not open only. An issue closed between the two runs was
#    invisible to the old guard. A closed issue counts only if it closed inside
#    a recent window (default 72h), which is what keeps a task legitimately
#    repeated months later from being folded into a long-dead issue.
#
# 5. THE OUTCOME IS ALWAYS LOGGED, in both directions. The silent fall-through
#    to creation is what made the original incident take a while to attribute,
#    so "no existing issue matches" is printed just as loudly as a match.
#
# WHY NO EXIT CODE FROM THIS HELPER EVER MEANS "STOP"
#
# #1268 is the local precedent for what a brittle gate costs, and
# workflow_identity_preflight.sh (#1290) is the reference shape. The outcomes
# are kept strictly apart, but note the asymmetry with those helpers: refusing
# to create an issue because GitHub could not be reached would strand the run
# with nothing to track, which is worse than the duplicate. So the strongest
# thing this helper can say is "reuse this URL"; everything else — no match, an
# unreadable answer, no `gh`, no `jq`, a rate limit, a 403 — resolves to
# "create the issue", and the caller proceeds.
#
#   an existing issue matches   -> exit 0, URL on stdout, reason on stderr
#   nothing matches             -> exit 3, empty stdout, INFO on stderr
#   the check could not be made -> exit 4, empty stdout, WARNING on stderr
#   there is nothing to check   -> exit 5, empty stdout, INFO on stderr
#
# Rate limiting is classified BEFORE any 403 is read as a denial: GitHub answers
# both "slow down" and "you may not" with 403, and neither can become a verdict
# here anyway — it only changes the warning an operator reads.
#
# This helper is deliberately self-contained: it sources nothing. A check whose
# job is to keep a run from duplicating itself must not acquire new ways to fail.
#
# Usage:
#   workflow_issue_dedup_check.sh --task-description TEXT [--title TITLE]
#                                 [--repo-path DIR] [--repo OWNER/NAME]
#   workflow_issue_dedup_check.sh --print-key --task-description TEXT
#
# Environment:
#   AMPLIHACK_SKIP_ISSUE_DEDUP_CHECK          non-empty -> skip entirely
#   AMPLIHACK_ISSUE_DEDUP_TIMEOUT             seconds per API call (default 45)
#   AMPLIHACK_ISSUE_DEDUP_LIMIT               issues listed, newest first (default 100)
#   AMPLIHACK_ISSUE_DEDUP_CLOSED_WINDOW_HOURS closed-issue window (default 72)
#   AMPLIHACK_ISSUE_DEDUP_NO_SEARCH_FALLBACK  non-empty -> skip the index pass
#
# Exit codes: 0 found / 3 none / 4 could-not-check / 5 nothing-to-check.
# None of them means "stop the run".

# No `set -e`: this helper must always reach a structured answer rather than
# dying mid-probe and leaving the caller to guess what it meant.
set -uo pipefail
export GH_PAGER=cat PAGER=cat GIT_PAGER=cat LESS=FRX

TASK_DESC=""
TITLE=""
REPO_ARG="${REPO_PATH:-.}"
REPO_SLUG=""
PRINT_KEY=""

while [ "$#" -gt 0 ]; do
  case "$1" in
    --task-description) TASK_DESC="${2:-}"; shift 2 ;;
    --title) TITLE="${2:-}"; shift 2 ;;
    --repo-path) REPO_ARG="${2:-}"; shift 2 ;;
    --repo) REPO_SLUG="${2:-}"; shift 2 ;;
    --print-key) PRINT_KEY=1; shift ;;
    -h|--help) sed -n '1,105p' "$0" >&2; exit 0 ;;
    *) shift ;;
  esac
done
[ -n "$TASK_DESC" ] || TASK_DESC="${TASK_DESCRIPTION:-}"
[ -n "$TITLE" ] || TITLE="$TASK_DESC"

TIMEOUT_S="${AMPLIHACK_ISSUE_DEDUP_TIMEOUT:-45}"
LIST_LIMIT="${AMPLIHACK_ISSUE_DEDUP_LIMIT:-100}"
CLOSED_WINDOW_H="${AMPLIHACK_ISSUE_DEDUP_CLOSED_WINDOW_HOURS:-72}"
case "$TIMEOUT_S" in ''|*[!0-9]*) TIMEOUT_S=45 ;; esac
case "$LIST_LIMIT" in ''|*[!0-9]*) LIST_LIMIT=100 ;; esac
case "$CLOSED_WINDOW_H" in ''|*[!0-9]*) CLOSED_WINDOW_H=72 ;; esac

note() { printf '%s\n' "$*" >&2; }

# Nothing in this file ever prints a token value; this is the belt to that pair
# of braces, applied to every provider message before it reaches a log.
redact() {
  sed -E \
    -e 's#gh[pousr]_[A-Za-z0-9_]{8,}#<redacted-token>#g' \
    -e 's#github_pat_[A-Za-z0-9_]{8,}#<redacted-token>#g' \
    -e 's#https?://[^[:space:]/]*@#https://<redacted>@#g' \
    -e 's#[Bb]earer[[:space:]]+[A-Za-z0-9._~+/=-]{20,}#Bearer <redacted-token>#g'
}

# skip <reason> — nothing to check here. The caller creates the issue.
skip() {
  note "INFO: issue de-duplication check skipped: $1 — a new tracking issue will be created."
  exit 5
}

# unknown <reason> — the check itself could not be made. Warn, never gate.
unknown() {
  note "WARNING: issue de-duplication check could not be completed ($1) — creating a new tracking issue without a duplicate check. This is advisory: an un-runnable check must not stop work (issue #1268)."
  exit 4
}

# ---------------------------------------------------------------------------
# The task key. Same input text -> same key, on any machine, in any run.
# ---------------------------------------------------------------------------
#
# Normalisation is deliberately aggressive: a relaunch may re-wrap or re-indent
# the task description without changing the task, and a key that changed with
# whitespace would be a key that never matches.

task_key() {
  local norm digest
  norm="$(printf '%s' "$TASK_DESC" | tr '[:upper:]' '[:lower:]' | tr -s '[:space:]' ' ' \
    | sed -e 's/^ *//' -e 's/ *$//')"
  [ -n "$norm" ] || return 1
  if command -v sha256sum >/dev/null 2>&1; then
    digest="$(printf '%s' "$norm" | sha256sum 2>/dev/null | cut -d' ' -f1)"
  elif command -v shasum >/dev/null 2>&1; then
    digest="$(printf '%s' "$norm" | shasum -a 256 2>/dev/null | cut -d' ' -f1)"
  else
    return 1
  fi
  case "$digest" in ''|*[!0-9a-fA-F]*) return 1 ;; esac
  printf '%s' "${digest:0:16}"
}

if [ -n "$PRINT_KEY" ]; then
  # A key that cannot be derived is not an error: the caller simply writes an
  # issue body without the marker, and title matching still applies.
  task_key || true
  exit 0
fi

# ---------------------------------------------------------------------------
# 1. Is there anything to check?
# ---------------------------------------------------------------------------

[ -n "${AMPLIHACK_SKIP_ISSUE_DEDUP_CHECK:-}" ] && skip "AMPLIHACK_SKIP_ISSUE_DEDUP_CHECK is set"
[ -n "${TASK_DESC//[[:space:]]/}" ] || skip "empty-task-description"
[ -n "${TITLE//[[:space:]]/}" ] || skip "empty-title"

cd "$REPO_ARG" 2>/dev/null || unknown "repo-path-unreadable"
git rev-parse --is-inside-work-tree >/dev/null 2>&1 || skip "not-a-git-repository"

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
  # Azure DevOps and everything else track work items through their own APIs and
  # are not this check's business.
  case "$HOST" in
    github.com|*.ghe.com|*.githubenterprise.com) ;;
    *) [ "$HOST" = "${GH_HOST:-}" ] || skip "non-github-remote:${HOST}" ;;
  esac
  REPO_SLUG="${OWNER}/${NAME}"
fi

command -v gh >/dev/null 2>&1 || unknown "gh-not-installed"
command -v jq >/dev/null 2>&1 || unknown "jq-not-installed"

KEY="$(task_key || true)"

# ---------------------------------------------------------------------------
# 2. Ask GitHub which issues exist. The primary read does not touch the search
#    index; the index cannot be trusted to have caught up with a run that
#    started minutes ago, which is precisely the case being detected.
# ---------------------------------------------------------------------------

FIELDS="number,url,title,state,body,closedAt,createdAt"
ERRFILE="$(mktemp "${TMPDIR:-/tmp}/issue-dedup-check.XXXXXX")" || unknown "cannot-create-tempfile"
# shellcheck disable=SC2329  # invoked by the EXIT trap below.
cleanup() { rm -f "$ERRFILE"; }
trap cleanup EXIT

gh_issue_list() { # gh_issue_list <extra args...> -> JSON array on stdout
  if command -v timeout >/dev/null 2>&1; then
    timeout "$TIMEOUT_S" gh issue list --repo "$REPO_SLUG" --state all \
      --json "$FIELDS" "$@" 2>>"$ERRFILE"
  else
    gh issue list --repo "$REPO_SLUG" --state all --json "$FIELDS" "$@" 2>>"$ERRFILE"
  fi
}

# classify_failure <rc> — never returns; every path is `unknown`. No failure
# classification here can produce a match, because an unreadable answer is not
# evidence that an issue exists, and not evidence that it does not.
classify_failure() {
  local rc="$1" errtext
  errtext="$(head -c 2000 "$ERRFILE" 2>/dev/null | redact | tr '\n' ' ')"
  [ -n "$errtext" ] || errtext="(no error output; gh exited ${rc})"
  if [ "$rc" -eq 124 ] || [ "$rc" -eq 137 ]; then
    unknown "api-call-timed-out-after-${TIMEOUT_S}s"
  fi
  # FIRST, because GitHub answers both "slow down" and "you may not" with 403.
  if printf '%s' "$errtext" | grep -Eiq 'rate limit|secondary rate|abuse detection|retry-after|x-ratelimit'; then
    unknown "rate-limited"
  fi
  if printf '%s' "$errtext" | grep -Eiq 'no such host|dial tcp|connection (refused|reset)|timed? ?out|tls handshake|network is unreachable|temporary failure|EOF|HTTP 5[0-9][0-9]|(^|[^0-9])(500|502|503|504)([^0-9]|$)|proxy'; then
    unknown "network-or-server-error"
  fi
  if printf '%s' "$errtext" | grep -Eiq 'not authoriz|unauthoriz|forbidden|denied|bad credentials|requires authentication|not logged in|could not resolve|(^|[^0-9])(401|403|404)([^0-9]|$)'; then
    unknown "not-authorised-to-list-issues"
  fi
  unknown "unrecognised-api-error: ${errtext}"
}

CUTOFF="$(date -u -d "-${CLOSED_WINDOW_H} hours" +%Y-%m-%dT%H:%M:%SZ 2>/dev/null \
  || date -u -v-"${CLOSED_WINDOW_H}"H +%Y-%m-%dT%H:%M:%SZ 2>/dev/null || true)"
[ -n "$CUTOFF" ] || CUTOFF="0000-01-01T00:00:00Z"

# ---------------------------------------------------------------------------
# 3. Which of them is this task? Decided locally, by the same comparison for
#    every candidate list, so a search-index pass can only widen the pool of
#    candidates — never decide the answer.
# ---------------------------------------------------------------------------
#
# `norm` reduces a title to lowercase alphanumeric words. Two titles that differ
# only by punctuation, a path separator or a truncation point compare equal.
# A prefix match is accepted only from 40 normalised characters up, so a short
# title cannot swallow an unrelated longer one; below that, equality is required.

# The issue list arrives on STDIN, never in argv. Linux caps a SINGLE argument at
# 128 KiB (MAX_ARG_STRLEN), and a hundred issues with their bodies is comfortably
# past that — `--argjson issues "$JSON"` dies with "Argument list too long" on
# any repository of real size, which would have made this whole check a
# permanent `unknown` exactly where it matters most. `printf` is a shell builtin,
# so nothing is exec'd with the payload.
select_match() { # <issues-json on stdin> -> "url<TAB>number<TAB>state<TAB>via" or empty
  jq -r \
    --arg key "$KEY" \
    --arg title "$TITLE" \
    --arg cutoff "$CUTOFF" '
      def norm: (. // "") | ascii_downcase | gsub("[^a-z0-9]+"; " ")
                | sub("^ +"; "") | sub(" +$"; "");
      ($title | norm) as $t
      | map(select((.state // "") != "CLOSED" or ((.closedAt // "") >= $cutoff)))
      | map(
          . + {via:
            (if ($key != "" and ((.body // "") | contains("amplihack-task-key: " + $key)))
             then "task-key"
             elif ($t != "" and ((.title | norm) as $c
                   | $c == $t
                     or (($c | length) >= 40 and ($t | startswith($c)))
                     or (($t | length) >= 40 and ($c | startswith($t)))))
             then "title"
             else "" end)})
      | map(select(.via != ""))
      | sort_by([(if .via == "task-key" then 0 else 1 end),
                 (if (.state // "") == "OPEN" then 0 else 1 end),
                 (.number // 0)])
      | if length == 0 then "" else
          (.[0] | [(.url // ""), ((.number // "") | tostring), (.state // "?"), .via]
                | @tsv)
        end
    ' 2>>"$ERRFILE"
}

report_match() { # report_match <tsv>
  local url num state via
  IFS=$'\t' read -r url num state via <<<"$1"
  [ -n "$url" ] || return 1
  note "INFO: issue de-duplication check: issue #${num} (${state}) in ${REPO_SLUG} already tracks this task, matched by ${via}: ${url}"
  note "INFO: reusing it instead of opening a duplicate (issue #1420). Set AMPLIHACK_SKIP_ISSUE_DEDUP_CHECK=1 to force a new issue."
  printf '%s\n' "$url"
  exit 0
}

RC=0
LIST_JSON="$(gh_issue_list --limit "$LIST_LIMIT")" || RC=$?
[ "$RC" -eq 0 ] || classify_failure "$RC"
# Plain `-n`, deliberately. `${LIST_JSON//[[:space:]]/}` is the idiomatic
# "is this blank" test and it is quadratic in bash: on a real 300 KB issue
# listing it does not finish inside a minute, which turns a check meant to save
# a duplicate into a check that stalls every run. Anything that is not a JSON
# array is caught by the `type` probe on the next line anyway.
[ -n "$LIST_JSON" ] || LIST_JSON="[]"
printf '%s' "$LIST_JSON" | jq -e 'type == "array"' >/dev/null 2>&1 || unknown "issue-list-unreadable"

MATCH="$(printf '%s' "$LIST_JSON" | select_match)"
[ -n "$MATCH" ] && report_match "$MATCH"

# ---------------------------------------------------------------------------
# 4. Nothing in the recent list. Widen once through the search index, with a
#    query made of ordinary words — which is the query shape that DID find the
#    issue in the original report, where the raw 100-character prefix did not.
# ---------------------------------------------------------------------------

if [ -z "${AMPLIHACK_ISSUE_DEDUP_NO_SEARCH_FALLBACK:-}" ]; then
  SEARCH_Q="$(printf '%s' "$TITLE" | tr -c '[:alnum:]' ' ' | tr -s ' ' '\n' \
    | awk 'length($0) >= 3 { print } NR > 400 { exit }' | head -n 8 | tr '\n' ' ' \
    | sed -e 's/ *$//')"
  if [ -n "$SEARCH_Q" ]; then
    RC=0
    SEARCH_JSON="$(gh_issue_list --limit 30 --search "${SEARCH_Q} in:title")" || RC=$?
    if [ "$RC" -eq 0 ] && [ -n "$SEARCH_JSON" ]; then
      MATCH="$(printf '%s' "$SEARCH_JSON" | select_match)"
      [ -n "$MATCH" ] && report_match "$MATCH"
    elif [ "$RC" -ne 0 ]; then
      # The primary read already answered. A failing widening pass downgrades
      # confidence; it does not turn "none" into "cannot check".
      note "WARNING: issue de-duplication check: the search-index widening pass failed (gh exited ${RC}); relying on the direct issue listing alone."
    fi
  fi
fi

# ---------------------------------------------------------------------------
# 5. No match. Say so out loud — the silent fall-through to `gh issue create` is
#    what made the original duplicate take a while to attribute (issue #1420).
# ---------------------------------------------------------------------------

note "INFO: issue de-duplication check: no open issue, and no issue closed within ${CLOSED_WINDOW_H}h, matches this task in ${REPO_SLUG} (checked ${LIST_LIMIT} recent issues by task key '${KEY:-<unavailable>}' and by normalised title). Creating a new tracking issue."
exit 3
