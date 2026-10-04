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
# approval_status is MET, NOT_MET or UNREADABLE, and approval_source says
# what decided it:
#   review-decision  GitHub's reviewDecision: APPROVED is MET;
#                    CHANGES_REQUESTED and REVIEW_REQUIRED are NOT_MET.
#   required-count   reviewDecision is empty and the required approval count
#                    is known: 0 is MET, more than 0 is NOT_MET.
#   merge-state      reviewDecision is empty, the count cannot be read, and
#                    GitHub's own mergeStateStatus is CLEAN, HAS_HOOKS or
#                    UNSTABLE, so no required review blocks the merge: MET.
#                    required_approvals stays empty: the count was not read,
#                    and approval_source says what decided the status instead.
#   unreadable       none of the above: UNREADABLE.
# An empty reviewDecision is not a missing approval. GitHub leaves it empty
# when the base branch requires no review.
#
# required_approvals is the highest of two counts:
#   classic   required_approving_review_count from
#             branches/<base>/protection/required_pull_request_reviews, or 0
#             when branches/<base> reports "protected": false.
#   rulesets  the highest required_approving_review_count among the
#             pull_request rules in rules/branches/<base> (read access is
#             enough), or 0 when no such rule applies.
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
VIEW_OK="false"
STATE=""; DRAFT=""; MERGEABLE=""; MSTATE=""; DECISION=""; HEAD=""; BASE=""
if [ -n "$PR" ] && LINE="$(gh pr view "$PR" --json state,isDraft,mergeable,mergeStateStatus,reviewDecision,headRefOid,baseRefName \
  --jq '[.state, (.isDraft | tostring), .mergeable, .mergeStateStatus, (.reviewDecision // ""), .headRefOid, .baseRefName] | map(. // "" | tostring) | join("|")' 2>/dev/null)"; then
  IFS='|' read -r STATE DRAFT MERGEABLE MSTATE DECISION HEAD BASE <<<"$LINE"
  [ -n "$STATE" ] && [ -n "$HEAD" ] && VIEW_OK="true"
fi
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
CLASSIC=""; RULES=""
if [ -n "$BASE" ]; then
  V="$(gh api "repos/{owner}/{repo}/branches/${BASE}/protection/required_pull_request_reviews" \
    --jq '.required_approving_review_count // 0' 2>/dev/null)" && digits "$V" && CLASSIC="$V"
  if [ -z "$CLASSIC" ]; then
    V="$(gh api "repos/{owner}/{repo}/branches/${BASE}" --jq '.protected' 2>/dev/null)" && [ "$V" = "false" ] && CLASSIC=0
  fi
  V="$(gh api --paginate "repos/{owner}/{repo}/rules/branches/${BASE}" \
    --jq '[.[] | select(.type == "pull_request") | (.parameters.required_approving_review_count // 0)] | max // 0' 2>/dev/null)" \
    && V="$(printf '%s\n' "$V" | maxof)" && RULES="$V"
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
        case "$MSTATE" in CLEAN | HAS_HOOKS | UNSTABLE) APPROVAL="MET"; SOURCE="merge-state" ;; esac
      fi
      ;;
  esac
fi
echo "INFO: approval: status=${APPROVAL} source=${SOURCE} required=${REQ:-unknown} classic=${CLASSIC:-unreadable} rulesets=${RULES:-unreadable} review_decision=${DECISION:-<empty>} merge_state=${MSTATE}" >&2

printf '{"pr":"%s","state":"%s","is_draft":"%s","mergeable":"%s","merge_state":"%s","review_decision":"%s","head_sha":"%s","base_ref":"%s","unresolved_threads":"%s","required_approvals":"%s","approval_status":"%s","approval_source":"%s"}\n' \
  "$PR" "$STATE" "$DRAFT" "$MERGEABLE" "$MSTATE" "$DECISION" "$HEAD" "$BASE" "$THREADS" "$REQ" "$APPROVAL" "$SOURCE"
