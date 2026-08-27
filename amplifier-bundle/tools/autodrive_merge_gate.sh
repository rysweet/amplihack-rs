#!/usr/bin/env bash
# autodrive_merge_gate.sh — the final, evidence-gated merge for
# auto-drive-to-merge.
#
# NO SILENT MERGE. Every criterion is re-verified HERE, in the run that merges,
# against ONE head SHA, and the evidence is written down before anything is
# merged. A criterion that cannot be read is a FAILURE, never a pass: an
# unreadable CI status means "we do not know", and "we do not know" does not
# merge.
#
# The merge argv is a FIXED literal list built in this file. It takes no flags
# from any caller, so there is no argument through which a branch-protection
# bypass could be threaded — the prohibition is structural here, not advisory.
# A bypass flag and a hook-skipping commit flag are NEVER used anywhere in this
# workflow; see docs/reference/auto-drive-to-merge.md#two-absolute-prohibitions.
#
#   Exit 0  — merged in this run, and the platform confirms MERGED. Also 0 when
#             the PR was ALREADY merged (a resumed run never re-merges).
#   Exit 1  — at least one criterion failed or was unreadable. Nothing merged.
#   Exit 79 — terminal policy refusal from a child. Surfaced, never retried.

set -uo pipefail

AUTODRIVE_EXIT_POLICY_REFUSAL=79
PR=""; REPO="."; ROUND_RECORD=""; QA_EVIDENCE=""; STATE_DIR=""; DRY_RUN="false"

while [ $# -gt 0 ]; do
  case "$1" in
    --pr)            PR="${2:-}"; shift 2 ;;
    --repo)          REPO="${2:-}"; shift 2 ;;
    --round-record)  ROUND_RECORD="${2:-}"; shift 2 ;;
    --qa-evidence)   QA_EVIDENCE="${2:-}"; shift 2 ;;
    --state-dir)     STATE_DIR="${2:-}"; shift 2 ;;
    --dry-run)       DRY_RUN="true"; shift ;;
    *) echo "ERROR: autodrive_merge_gate.sh: unknown argument '$1'" >&2; exit 2 ;;
  esac
done
case "$PR" in ''|*[!0-9]*) echo "ERROR: --pr must be a positive integer (got '${PR}')" >&2; exit 2 ;; esac
# gh resolves {owner}/{repo} from the working directory, so every read below
# must run inside the repository the PR belongs to.
cd "$REPO" 2>/dev/null || { echo "ERROR: --repo '${REPO}' is not a directory; refusing to read PR state from an unknown working directory." >&2; exit 2; }
STATE_DIR="${STATE_DIR:-${TMPDIR:-/tmp}}"
mkdir -p "$STATE_DIR" || exit 2
AMPLIHACK_BIN="${AMPLIHACK_BIN:-amplihack}"
export GIT_PAGER=cat GH_PAGER=cat PAGER=cat LESS=FRX

BLOCKERS=()
EVIDENCE=()
note()  { EVIDENCE+=("$1"); echo "  evidence: $1" >&2; }
block() { BLOCKERS+=("$1"); echo "  BLOCKER: $1" >&2; }

# `--require-field` selects the LAST JSON object carrying the field rather than
# the first parseable object of any shape (issue #1337, PR #1347). First-wins is
# fail-OPEN for a verdict: a quoted example or an early draft object gets read
# instead of the real one.
field() { printf '%s' "${1:-}" | "$AMPLIHACK_BIN" orch helper extract-json --require-field "$2" \
          | "$AMPLIHACK_BIN" orch helper extract-field --field "$2" --default "$3"; }

# --- 0. Already merged? A resumed run must never redo merged work. ----------
STATE_JSON="$(gh pr view "$PR" --json state,mergedAt,isDraft,mergeable,mergeStateStatus,reviewDecision,headRefOid,url 2>/dev/null)"
if [ -z "$STATE_JSON" ]; then
  block "pull request #${PR} metadata is unreadable; an unreadable platform state never merges"
  printf '{"merge_result":"NOT_MERGED","pr":"%s","blockers":["pr metadata unreadable"]}\n' "$PR"
  exit 1
fi
PR_STATE="$(field "$STATE_JSON" state UNKNOWN)"
MERGED_AT="$(field "$STATE_JSON" mergedAt "")"
if [ "$PR_STATE" = "MERGED" ] || { [ -n "$MERGED_AT" ] && [ "$MERGED_AT" != "null" ]; }; then
  echo "AUTO_DRIVE_MERGE: ALREADY_MERGED — PR #${PR} is already merged; nothing to redo." >&2
  printf '{"merge_result":"ALREADY_MERGED","pr":"%s"}\n' "$PR"
  exit 0
fi
if [ "$PR_STATE" != "OPEN" ]; then
  block "pull request #${PR} is in state '${PR_STATE}', not OPEN"
fi

HEAD_SHA="$(field "$STATE_JSON" headRefOid "")"
case "$HEAD_SHA" in
  [0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]*) ;;
  *) block "head SHA for #${PR} is unreadable ('${HEAD_SHA}'); every criterion must bind to one SHA" ;;
esac
note "head_sha=${HEAD_SHA}"

# --- 1. Draft ---------------------------------------------------------------
[ "$(field "$STATE_JSON" isDraft false)" = "true" ] && block "#${PR} is a draft"

# --- 2. Merge conflicts / mergeability -------------------------------------
MERGEABLE="$(field "$STATE_JSON" mergeable UNKNOWN)"
MERGE_STATE="$(field "$STATE_JSON" mergeStateStatus UNKNOWN)"
note "mergeable=${MERGEABLE} mergeStateStatus=${MERGE_STATE}"
[ "$MERGEABLE" = "MERGEABLE" ] || block "mergeable='${MERGEABLE}' (CONFLICTING or UNKNOWN never merges)"
case "$MERGE_STATE" in
  CLEAN|HAS_HOOKS|UNSTABLE) ;;
  BEHIND) block "branch is BEHIND base; this repository requires strict up-to-date branches before merge" ;;
  *) block "mergeStateStatus='${MERGE_STATE}' is not a mergeable state" ;;
esac

# --- 3. Reviews -------------------------------------------------------------
REVIEW_DECISION="$(field "$STATE_JSON" reviewDecision "")"
note "reviewDecision=${REVIEW_DECISION:-<none>}"
[ "$REVIEW_DECISION" = "CHANGES_REQUESTED" ] && block "a review requests changes"

# --- 4. Review threads resolved --------------------------------------------
# PAGINATED. `reviewThreads(first:100)` without a `pageInfo` follow-up silently
# truncates: a PR with 101 threads whose only unresolved one is the last would
# report 0 unresolved and pass this gate. `--paginate` walks every page (gh
# supplies $endCursor), emits one count per page, and the counts are summed. A
# page that does not come back as a number makes the whole criterion
# unreadable, which is a blocker.
THREAD_PAGES="$(gh api graphql --paginate -F pr="$PR" -F owner='{owner}' -F name='{repo}' -f query='
  query($owner:String!,$name:String!,$pr:Int!,$endCursor:String){repository(owner:$owner,name:$name){
    pullRequest(number:$pr){reviewThreads(first:100,after:$endCursor){
      pageInfo{hasNextPage endCursor}
      nodes{isResolved isOutdated}}}}}' \
  --jq '[.data.repository.pullRequest.reviewThreads.nodes[] | select(.isResolved==false and .isOutdated==false)] | length' 2>/dev/null)"
THREADS="$(printf '%s\n' "$THREAD_PAGES" \
  | awk 'NF==0{next} /^[0-9]+$/{s+=$1;n++;next} {bad=1} END{if(bad||!n) exit 1; print s}')" || THREADS=""
if [ -z "$THREADS" ]; then
  block "review-thread state is unreadable; an unreadable criterion is a failure, not a pass"
elif [ "$THREADS" != "0" ]; then
  block "${THREADS} unresolved review thread(s)"
else
  note "review_threads_unresolved=0"
fi

# --- 5. CI on THIS head SHA -------------------------------------------------
# `gh pr checks` exits non-zero when checks are failing or pending, and also
# when it cannot read them. All three are blockers; only a readable, complete,
# all-green rollup passes.
CHECKS_JSON="$(gh pr checks "$PR" --json name,state,bucket,link 2>/dev/null)"
if ! command -v jq >/dev/null 2>&1; then
  block "jq is unavailable, so the CI rollup cannot be read; a criterion we cannot read is a failure"
elif [ -z "$CHECKS_JSON" ]; then
  block "CI status for #${PR} is unreadable; an unreadable CI status is a failure, not a pass"
else
  PENDING="$(printf '%s' "$CHECKS_JSON" | jq -r '[.[] | select(.bucket=="pending")] | length' 2>/dev/null)"
  FAILING="$(printf '%s' "$CHECKS_JSON" | jq -r '[.[] | select(.bucket=="fail" or .bucket=="cancel")] | length' 2>/dev/null)"
  TOTAL="$(printf '%s' "$CHECKS_JSON" | jq -r 'length' 2>/dev/null)"
  note "ci_checks total=${TOTAL} pending=${PENDING} failing=${FAILING}"
  if [ -z "$TOTAL" ] || [ "$TOTAL" = "0" ]; then
    block "no CI checks reported for #${PR}; zero checks is not a green build"
  fi
  [ "${PENDING:-x}" = "0" ] || block "${PENDING:-unreadable} CI check(s) still pending"
  [ "${FAILING:-x}" = "0" ] || block "${FAILING:-unreadable} CI check(s) failing or cancelled"
fi

# --- 6. qa-team scenario evidence ------------------------------------------
if [ -z "$QA_EVIDENCE" ] || [ ! -f "$QA_EVIDENCE" ]; then
  block "no qa-team scenario evidence file was produced in this run"
else
  QA_RAW="$(cat "$QA_EVIDENCE")"
  QA_STATUS="$(field "$QA_RAW" qa_status MISSING)"
  QA_SHA="$(field "$QA_RAW" head_sha "")"
  note "qa_status=${QA_STATUS} qa_head_sha=${QA_SHA:-<none>} ($(field "$QA_RAW" qa_command ''))"
  [ "$QA_STATUS" = "PASS" ] || block "qa-team scenarios did not pass in this run (qa_status=${QA_STATUS})"
  # Existence + PASS is not enough: an evidence file left behind by an earlier
  # round describes a tree that is no longer what would be merged. Every
  # criterion binds to ONE head SHA, and this one is no exception.
  if [ -z "$QA_SHA" ]; then
    block "the qa-team evidence records no head_sha; evidence that is not bound to a SHA never merges"
  elif [ -n "$HEAD_SHA" ] && [ "$QA_SHA" != "$HEAD_SHA" ]; then
    block "the qa-team evidence was captured against ${QA_SHA} but the head is now ${HEAD_SHA}; evidence must bind to the SHA being merged"
  fi
fi

# --- 7. The merge-ready round's own structured verdict ---------------------
if [ -z "$ROUND_RECORD" ] || [ ! -f "$ROUND_RECORD" ]; then
  block "no merge-ready round record was produced in this run"
else
  MR_RAW="$(cat "$ROUND_RECORD")"
  MR_VERDICT="$(field "$MR_RAW" merge_ready_verdict MISSING)"
  MR_SHA="$(field "$MR_RAW" head_sha "")"
  note "merge_ready_verdict=${MR_VERDICT} recorded_head_sha=${MR_SHA}"
  [ "$MR_VERDICT" = "MERGE_READY" ] || block "merge-ready verdict is '${MR_VERDICT}', not MERGE_READY"
  if [ -n "$HEAD_SHA" ] && [ "$MR_SHA" != "$HEAD_SHA" ]; then
    block "the merge-ready evidence was captured against ${MR_SHA:-<none>} but the head is now ${HEAD_SHA}; evidence must bind to the SHA being merged"
  fi
fi

# --- 8. Record the evidence bundle BEFORE any merge ------------------------
BUNDLE="${STATE_DIR}/merge-evidence-${PR}-${HEAD_SHA:0:12}.txt"
{ printf 'auto-drive-to-merge merge gate — PR #%s @ %s\n' "$PR" "$HEAD_SHA"
  printf 'captured: %s\n\nEVIDENCE\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  printf '  - %s\n' "${EVIDENCE[@]}"
  printf '\nBLOCKERS\n'
  if [ "${#BLOCKERS[@]}" -eq 0 ]; then printf '  (none)\n'; else printf '  - %s\n' "${BLOCKERS[@]}"; fi
} > "$BUNDLE"
echo "INFO: merge evidence written to ${BUNDLE}" >&2

if [ "${#BLOCKERS[@]}" -ne 0 ]; then
  echo "AUTO_DRIVE_MERGE: NOT_MERGED — ${#BLOCKERS[@]} blocker(s) for PR #${PR}. Nothing was merged." >&2
  printf '{"merge_result":"NOT_MERGED","pr":"%s","head_sha":"%s","blocker_count":%s,"evidence_bundle":"%s"}\n' \
    "$PR" "$HEAD_SHA" "${#BLOCKERS[@]}" "$BUNDLE"
  exit 1
fi

# --- 9. Merge — fixed argv, no caller-supplied flags ------------------------
# --match-head-commit makes GitHub itself refuse the merge if the head moved
# after the evidence above was captured. That closes the check-then-merge gap
# without a cooperative lease, and it is why every criterion binds to HEAD_SHA.
MERGE_ARGV=(pr merge "$PR" --squash --delete-branch --match-head-commit "$HEAD_SHA")
EXPECTED_ARGV=(pr merge "$PR" --squash --delete-branch --match-head-commit "$HEAD_SHA")
if [ "${MERGE_ARGV[*]}" != "${EXPECTED_ARGV[*]}" ]; then
  echo "ERROR: merge argv was modified; refusing. Expected: gh ${EXPECTED_ARGV[*]}" >&2
  exit 1
fi
if [ "$DRY_RUN" = "true" ]; then
  echo "AUTO_DRIVE_MERGE: DRY_RUN — would run: gh ${MERGE_ARGV[*]}" >&2
  printf '{"merge_result":"DRY_RUN","pr":"%s","head_sha":"%s","evidence_bundle":"%s"}\n' "$PR" "$HEAD_SHA" "$BUNDLE"
  exit 0
fi

echo "AUTO_DRIVE_MERGE: merging PR #${PR} at ${HEAD_SHA} — gh ${MERGE_ARGV[*]}" >&2
# Capture the status of `gh` ITSELF. Inside `if ! gh ...; then`, `$?` is the
# status of the NEGATION, which is always 0 on the failure branch — that would
# make the exit-79 test below dead code and report every failure as "exit 0".
gh "${MERGE_ARGV[@]}"
MERGE_RC=$?
if [ "$MERGE_RC" -ne 0 ]; then
  if [ "$MERGE_RC" = "$AUTODRIVE_EXIT_POLICY_REFUSAL" ]; then
    echo "ERROR: exit ${AUTODRIVE_EXIT_POLICY_REFUSAL} terminal policy refusal during merge. Final; not retried." >&2
    exit "$AUTODRIVE_EXIT_POLICY_REFUSAL"
  fi
  echo "AUTO_DRIVE_MERGE: NOT_MERGED — gh pr merge failed (exit ${MERGE_RC}). Fix the cause; the gate is not bypassed." >&2
  printf '{"merge_result":"NOT_MERGED","pr":"%s","head_sha":"%s","blocker_count":1,"evidence_bundle":"%s"}\n' "$PR" "$HEAD_SHA" "$BUNDLE"
  exit 1
fi

# --- 10. The platform must confirm it ---------------------------------------
FINAL="$(gh pr view "$PR" --json state,mergedAt,mergeCommit 2>/dev/null)"
if [ "$(field "$FINAL" state UNKNOWN)" != "MERGED" ]; then
  echo "AUTO_DRIVE_MERGE: NOT_MERGED — gh reported success but the platform does not confirm MERGED. Treating as not merged." >&2
  printf '{"merge_result":"NOT_MERGED","pr":"%s","head_sha":"%s","blocker_count":1,"evidence_bundle":"%s"}\n' "$PR" "$HEAD_SHA" "$BUNDLE"
  exit 1
fi
echo "AUTO_DRIVE_MERGE: MERGED — PR #${PR} at ${HEAD_SHA}." >&2
printf '{"merge_result":"MERGED","pr":"%s","head_sha":"%s","evidence_bundle":"%s"}\n' "$PR" "$HEAD_SHA" "$BUNDLE"
exit 0
