#!/usr/bin/env bash
# autodrive_platform_facts.sh — the merge round's platform facts for one pull
# request, measured here so that no agent decides them (#1517, #1518).
#
# Usage: bash autodrive_platform_facts.sh [pr-number]
#   Run from inside the repository; gh resolves {owner}/{repo} from it. With
#   no usable number, the current branch's pull request is used.
#
# Prints ONE JSON line, always, and exits 0:
#   {"pr","state","is_draft","mergeable","merge_state","review_decision",
#    "head_sha","base_ref","unresolved_threads",
#    "required_approvals","approval_status","approval_source"}
# Every value is a fixed token, digits, a hex SHA, or a branch name of
# [A-Za-z0-9._/-]. Anything else is reported as unreadable, never passed on.
#
# Criterion 6, reviews and approvals (#1518)
# ------------------------------------------
# approval_status is MET, NOT_MET, PENDING or UNREADABLE, and approval_source
# says what decided it:
#   review-decision  GitHub's reviewDecision: APPROVED is MET;
#                    CHANGES_REQUESTED and REVIEW_REQUIRED are NOT_MET.
#   required-count   reviewDecision is empty and the required approval count
#                    is known: 0 is MET, more than 0 is NOT_MET. A count of 0
#                    is known only when no review rule applies (see below).
#   merge-state      reviewDecision is empty, the count cannot be read, and
#                    GitHub's own mergeStateStatus is CLEAN, HAS_HOOKS or
#                    UNSTABLE, so no required review blocks the merge: MET.
#                    required_approvals stays empty: the count was not read,
#                    and approval_source says what decided the status instead.
#   other-blockers   reviewDecision is empty, the count cannot be read, and
#                    mergeStateStatus is BLOCKED, DIRTY or BEHIND while another
#                    blocker an agent can clear is present: a check in the
#                    rollup that has not passed (or no checks at all), a
#                    conflict (mergeable CONFLICTING, or DIRTY), a branch
#                    BEHIND its base, or an unresolved review thread (a rule
#                    can require conversations to be resolved, and an agent
#                    answers and resolves threads). GitHub folds every rule
#                    into that one state, so whether a review is also missing
#                    cannot be read until those clear: PENDING. The merge
#                    round clears the other blockers first and reads the
#                    approval again; it does not route PENDING to a person.
#                    An unreadable thread count is not a thread, so it alone
#                    never makes PENDING.
#   unreadable       none of the above: UNREADABLE. BLOCKED with every check
#                    passed, no conflict, the branch up to date and no
#                    unresolved review thread leaves only the rules a person
#                    satisfies, a required review among them.
# A check has passed when its conclusion (a check run) or state (a status
# context) is SUCCESS, NEUTRAL or SKIPPED; anything else, an unfinished run
# included, has not.
# An empty reviewDecision does not mean that no review is required. GitHub
# publishes a decision only when a rule on the base branch requires at least
# one approving review. At a required count of 0 the field stays empty even
# while a code-owner, last-push or required-reviewer rule keeps the merge
# blocked (Mergify, "GitHub Rulesets Compatibility",
# https://docs.mergify.com/merge-queue/github-rulesets/). A count of 0 that
# comes with such a rule is therefore unknown, not 0, and the merge state
# decides: BLOCKED gives UNREADABLE, never MET.
#
# required_approvals is the highest of two counts:
#   classic   required_approving_review_count from
#             branches/<base>/protection/required_pull_request_reviews, or 0
#             when branches/<base> reports "protected": false. Unknown when
#             the count is 0 and require_code_owner_reviews or
#             require_last_push_approval is true.
#   rulesets  the highest required_approving_review_count among the
#             pull_request rules in rules/branches/<base> (read access is
#             enough), or 0 when no such rule applies. Unknown when that
#             highest count is 0 and a pull_request rule sets
#             require_code_owner_review or require_last_push_approval, or
#             lists required_reviewers.
# A count above 0 is kept whatever else the rule requires: GitHub publishes a
# decision for it, and an empty decision beside it is NOT_MET.
# A failed classic read, a 404 included, leaves the classic count unknown.
# The response text is not parsed. GitHub answers 404 when the branch has no
# review rule, and also when the token has no admin rights on the repository
# (measured on rysweet/amplihack-rs with push but not admin: the protection
# endpoint returns 404 "Not Found" while branches/main reports
# "protected": true). Reading that 404 as zero would pass repositories that
# do require approvals. When only one count can be read and it is above zero,
# it is reported as a lower bound and the status is NOT_MET.

set -uo pipefail
export GIT_PAGER=cat GH_PAGER=cat PAGER=cat LESS=FRX

digits() { case "${1:-}" in '' | *[!0-9]*) return 1 ;; esac; }
# The highest of one number per line. Fails on no lines or on any line that is
# not a number, so an error page from a paginated read is never a count.
maxof() { awk 'NF==0{next} /^[0-9]+$/{if(!n||$1+0>m)m=$1+0;n++;next} {bad=1} END{if(bad||!n) exit 1; print m}'; }
oneof() { # oneof <value> <default> <allowed>...: the value if allowed, else the default
  local v="$1" d="$2" a
  shift 2
  for a in "$@"; do [ "$v" = "$a" ] && { printf '%s' "$v"; return 0; }; done
  printf '%s' "$d"
}

PR="${1:-}"
case "$PR" in '' | *[!0-9]*) PR="$(gh pr view --json number --jq .number 2>/dev/null || printf '')" ;; esac
digits "$PR" || PR=""

# One read of the pull request. gh's own jq joins the fields with `|`, so no
# other JSON reader is needed. `|` is not whitespace, so an empty field (an
# empty reviewDecision) stays in place instead of collapsing; the base ref
# is last, so a `|` inside it lands in BASE and fails the check below.
# CHECKS sums up the rollup as pass, not-passed or none (see the header).
VIEW_OK="false"
STATE=""; DRAFT=""; MERGEABLE=""; MSTATE=""; DECISION=""; HEAD=""; CHECKS=""; BASE=""
VIEW_FIELDS="state,isDraft,mergeable,mergeStateStatus,reviewDecision,headRefOid,statusCheckRollup,baseRefName"
VIEW_JQ='[.state, (.isDraft | tostring), .mergeable, .mergeStateStatus, (.reviewDecision // ""), .headRefOid,
  ((.statusCheckRollup // []) as $c | [$c[] | ((.conclusion // .state // "") | tostring | ascii_upcase)
    | select(. != "SUCCESS" and . != "NEUTRAL" and . != "SKIPPED")] as $open
    | if ($c | length) == 0 then "none" elif ($open | length) == 0 then "pass" else "not-passed" end),
  .baseRefName] | map(. // "" | tostring) | join("|")'
if [ -n "$PR" ] && LINE="$(gh pr view "$PR" --json "$VIEW_FIELDS" --jq "$VIEW_JQ" 2>/dev/null)"; then
  IFS='|' read -r STATE DRAFT MERGEABLE MSTATE DECISION HEAD CHECKS BASE <<<"$LINE"
  [ -n "$STATE" ] && [ -n "$HEAD" ] && VIEW_OK="true"
fi
CHECKS="$(oneof "$CHECKS" unreadable pass not-passed none)"
STATE="$(oneof "$STATE" UNKNOWN OPEN CLOSED MERGED)"
DRAFT="$(oneof "$DRAFT" unknown true false)"
MERGEABLE="$(oneof "$MERGEABLE" UNKNOWN MERGEABLE CONFLICTING)"
MSTATE="$(oneof "$MSTATE" UNKNOWN BEHIND BLOCKED CLEAN DIRTY DRAFT HAS_HOOKS UNSTABLE)"
case "$DECISION" in '' | APPROVED | CHANGES_REQUESTED | REVIEW_REQUIRED) ;; *) DECISION="UNKNOWN" ;; esac
case "${#HEAD}:${HEAD}" in 40:*[!0123456789abcdef]* | 64:*[!0123456789abcdef]*) HEAD="" ;; 40:* | 64:*) ;; *) HEAD="" ;; esac
case "$BASE" in '' | -* | /* | *..* | *//* | */ | *[!A-Za-z0-9._/-]*) BASE="" ;; esac

# Review threads. PAGINATED: `first:100` alone truncates, so a PR whose only
# unresolved thread is on page two would report 0. Per-page counts are summed;
# a page that is not a number makes the whole count unreadable.
PAGES="$(gh api graphql --paginate -F pr="${PR:-0}" -F owner='{owner}' -F name='{repo}' -f query='
  query($owner:String!,$name:String!,$pr:Int!,$endCursor:String){repository(owner:$owner,name:$name){
    pullRequest(number:$pr){reviewThreads(first:100,after:$endCursor){
      pageInfo{hasNextPage endCursor}
      nodes{isResolved isOutdated}}}}}' \
  --jq '[.data.repository.pullRequest.reviewThreads.nodes[]|select(.isResolved==false and .isOutdated==false)]|length' 2>/dev/null)"
THREADS="$(printf '%s\n' "$PAGES" | awk 'NF==0{next} /^[0-9]+$/{s+=$1;n++;next} {bad=1} END{if(bad||!n) exit 1; print s}')" || THREADS=""
[ -n "$THREADS" ] || THREADS="unreadable"

# Required approvals: classic protection, then rulesets (see the header).
# A count of 0 made unknown by a code-owner, last-push or required-reviewer
# rule is left empty and named in *_RULE; it is never recorded as 0.
CLASSIC=""; RULES=""; CLASSIC_RULE=""; RULES_RULE=""
if [ -n "$BASE" ]; then
  V="$(gh api "repos/{owner}/{repo}/branches/${BASE}/protection/required_pull_request_reviews" \
    --jq 'if ((.required_approving_review_count // 0) == 0) and (.require_code_owner_reviews == true or .require_last_push_approval == true)
          then "REVIEW_RULE" else (.required_approving_review_count // 0) end' 2>/dev/null)" || V=""
  if [ "$V" = "REVIEW_RULE" ]; then
    CLASSIC_RULE="code-owner-or-last-push"
  elif digits "$V"; then
    CLASSIC="$V"
  else
    V="$(gh api "repos/{owner}/{repo}/branches/${BASE}" --jq '.protected' 2>/dev/null)" && [ "$V" = "false" ] && CLASSIC=0
  fi
  # Per page: the page's highest count, then REVIEW_RULE when one of its
  # pull_request rules requires a code-owner, last-push or listed reviewer.
  if V="$(gh api --paginate "repos/{owner}/{repo}/rules/branches/${BASE}" \
       --jq '[.[] | select(.type == "pull_request") | (.parameters // {})] as $r
             | ([$r[] | .required_approving_review_count // 0] | max // 0),
               (if any($r[]; .require_code_owner_review == true or .require_last_push_approval == true
                             or ((.required_reviewers // []) | length) > 0) then "REVIEW_RULE" else empty end)' 2>/dev/null)" \
     && N="$(printf '%s\n' "$V" | grep -vx 'REVIEW_RULE' | maxof)"; then
    if [ "$N" = "0" ] && printf '%s\n' "$V" | grep -qx 'REVIEW_RULE'; then
      RULES_RULE="code-owner-last-push-or-reviewers"
    else
      RULES="$N"
    fi
  fi
fi
REQ=""
if [ -n "$CLASSIC" ] && [ -n "$RULES" ]; then
  REQ="$CLASSIC"; [ "$RULES" -gt "$REQ" ] && REQ="$RULES"
elif [ -n "$CLASSIC" ] && [ "$CLASSIC" -gt 0 ]; then
  REQ="$CLASSIC"
elif [ -n "$RULES" ] && [ "$RULES" -gt 0 ]; then
  REQ="$RULES"
fi

APPROVAL="UNREADABLE"; SOURCE="unreadable"
if [ "$VIEW_OK" = "true" ]; then
  case "$DECISION" in
    APPROVED) APPROVAL="MET"; SOURCE="review-decision" ;;
    CHANGES_REQUESTED | REVIEW_REQUIRED) APPROVAL="NOT_MET"; SOURCE="review-decision" ;;
    '')
      if [ -n "$REQ" ]; then
        SOURCE="required-count"
        if [ "$REQ" = "0" ]; then APPROVAL="MET"; else APPROVAL="NOT_MET"; fi
      else
        # Another blocker an agent can clear (see other-blockers above).
        OTHER="false"
        [ "$CHECKS" = "pass" ] || OTHER="true"
        [ "$MERGEABLE" = "CONFLICTING" ] && OTHER="true"
        case "$THREADS" in 0 | unreadable) ;; *) OTHER="true" ;; esac
        case "$MSTATE" in
          CLEAN | HAS_HOOKS | UNSTABLE) APPROVAL="MET"; SOURCE="merge-state" ;;
          DIRTY | BEHIND) APPROVAL="PENDING"; SOURCE="other-blockers" ;;
          BLOCKED) [ "$OTHER" = "false" ] || { APPROVAL="PENDING"; SOURCE="other-blockers"; } ;;
        esac
      fi
      ;;
  esac
fi
SHOWN_C="${CLASSIC:-unreadable}"; [ -z "$CLASSIC_RULE" ] || SHOWN_C="unknown(${CLASSIC_RULE})"
SHOWN_R="${RULES:-unreadable}"; [ -z "$RULES_RULE" ] || SHOWN_R="unknown(${RULES_RULE})"
echo "INFO: approval: status=${APPROVAL} source=${SOURCE} required=${REQ:-unknown} classic=${SHOWN_C}" \
  "rulesets=${SHOWN_R} review_decision=${DECISION:-<empty>} merge_state=${MSTATE} mergeable=${MERGEABLE} checks=${CHECKS} threads=${THREADS}" >&2

printf '{"pr":"%s","state":"%s","is_draft":"%s","mergeable":"%s","merge_state":"%s","review_decision":"%s","head_sha":"%s","base_ref":"%s","unresolved_threads":"%s","required_approvals":"%s","approval_status":"%s","approval_source":"%s"}\n' \
  "$PR" "$STATE" "$DRAFT" "$MERGEABLE" "$MSTATE" "$DECISION" "$HEAD" "$BASE" "$THREADS" "$REQ" "$APPROVAL" "$SOURCE"
