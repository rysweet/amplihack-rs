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
# Decide whether a state dir was given BEFORE the fallback below: an empty
# --state-dir must not turn the world-writable TMPDIR into crusty evidence.
if [ -n "$STATE_DIR" ]; then STATE_DIR_GIVEN="true"; else STATE_DIR_GIVEN="false"; fi
# The evidence bundle (section 8) still needs somewhere to go. The gate only
# creates a missing directory, under umask 077; it never changes the mode of
# an existing one, since sections 6b and 6c judge exactly that.
STATE_DIR="${STATE_DIR:-${TMPDIR:-/tmp}}"
( umask 077 && mkdir -p -- "$STATE_DIR" ) || exit 2
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
STATE_JSON="$(gh pr view "$PR" --json state,mergedAt,isDraft,mergeable,mergeStateStatus,reviewDecision,headRefOid,url,baseRefName 2>/dev/null)"
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
# The qa evidence and the round record are copied ONCE into a private
# temporary directory (#1517 D5). Section 6c checks the copies against the
# loop's manifest, and sections 6 and 7 read only the copies, so the files
# that were verified are the files that decide the merge.
GATE_TMP="$(mktemp -d "${TMPDIR:-/tmp}/autodrive-gate.XXXXXX" 2>/dev/null)" || GATE_TMP=""
REC_COPY=""; QA_COPY=""
if [ -z "$GATE_TMP" ] || [ ! -d "$GATE_TMP" ]; then
  GATE_TMP=""
  block "a private temporary directory could not be created, so the qa evidence and the round record cannot be copied and checked"
else
  trap 'rm -rf -- "$GATE_TMP"' EXIT
  if [ -n "$ROUND_RECORD" ] && [ -f "$ROUND_RECORD" ] && cat -- "$ROUND_RECORD" > "${GATE_TMP}/record.json" 2>/dev/null; then
    REC_COPY="${GATE_TMP}/record.json"
  fi
  if [ -n "$QA_EVIDENCE" ] && [ -f "$QA_EVIDENCE" ] && cat -- "$QA_EVIDENCE" > "${GATE_TMP}/qa-evidence.json" 2>/dev/null; then
    QA_COPY="${GATE_TMP}/qa-evidence.json"
  fi
fi
if [ -z "$QA_EVIDENCE" ] || [ ! -f "$QA_EVIDENCE" ]; then
  block "no qa-team scenario evidence file was produced in this run"
else
  QA_RAW="$(cat "${QA_COPY:-/dev/null}")"
  QA_STATUS="$(field "$QA_RAW" qa_status MISSING)"
  QA_SHA="$(field "$QA_RAW" head_sha "")"
  note "qa_status=${QA_STATUS} qa_head_sha=${QA_SHA:-<none>} ($(field "$QA_RAW" qa_command ''))"
  # qa_reason names the first failing check (issue #1517); only its token
  # characters are kept, so evidence text cannot reach the bundle as prose.
  note "qa_reason=$(field "$QA_RAW" qa_reason '' | tr -cd 'a-z-')"
  [ "$QA_STATUS" = "PASS" ] || block "qa-team scenarios did not pass in this run (qa_status=${QA_STATUS})"
  # Existence + PASS is not enough: an evidence file left behind by an earlier
  # round describes a tree that is no longer what would be merged. Every
  # criterion binds to ONE head SHA, and this one is no exception.
  if [ -z "$QA_SHA" ]; then
    block "the qa-team evidence records no head_sha; evidence that is not bound to a SHA never merges"
  elif [ -n "$HEAD_SHA" ] && [ "$QA_SHA" != "$HEAD_SHA" ]; then
    block "the qa-team evidence was captured against ${QA_SHA} but the head is now ${HEAD_SHA}; evidence must bind to the SHA being merged"
  fi
  # qa_status=PASS already requires gadugi; it is re-read here so evidence
  # written before gadugi was measured (no gadugi fields) never merges.
  GADUGI_STATUS="$(field "$QA_RAW" gadugi_status MISSING)"
  GADUGI_COUNT="$(field "$QA_RAW" gadugi_scenario_count "")"
  note "gadugi_status=${GADUGI_STATUS} gadugi_scenario_count=${GADUGI_COUNT:-<none>} gadugi_scenario_dir=$(field "$QA_RAW" gadugi_scenario_dir '')"
  [ "$GADUGI_STATUS" = "PASS" ] || block "gadugi-test scenarios were not validated and run to a pass in this run (gadugi_status=${GADUGI_STATUS})"
  case "$GADUGI_COUNT" in
    ''|*[!0-9]*|0*) block "gadugi_scenario_count='${GADUGI_COUNT}' is not a positive integer; zero scenarios is not a qa-team pass" ;;
  esac
fi

# --- 6b. Criterion 3: the crusty loop of this run ended DONE and CLEAN -------
# Under auto-drive the quality-audit criterion is met by the crusty loop: the
# `crusty-loop` marker in phases.tsv (written only after the loop reports DONE)
# and a final CLEAN verdict in crusty-latest.json. These files are evidence, so
# they are read only from a state dir that was given explicitly, is owned by
# this user and writable by nobody else, and only when they are regular files
# rather than symlinks. The state helper is sourced from beside this gate and
# from nowhere a pull request could populate. The writers make the directory
# and every file private whatever the caller's umask (autodrive_state.sh,
# PRIVATE STATE), so on a host whose umask is 0002 this check still passes for
# state the workflow wrote itself.
#
# Since #1517 the last loop-written round record must also check out against
# crusty-records.tsv, the manifest autodrive_loop.sh writes before any agent
# runs: autodrive_crusty_final rejects an injected, edited or archived record.
# These checks only add to the ones above; none of them replaces one.
GATE_HOME="$(cd "$(dirname "${BASH_SOURCE[0]}")" 2>/dev/null && pwd -P)"
autodrive_private() { # autodrive_private <path>: owned here, not a symlink, no group/world write
  local loose
  [ -L "$1" ] && return 1
  [ -O "$1" ] || return 1
  loose="$(find "$1" -maxdepth 0 \( -perm -0020 -o -perm -0002 \) -print 2>/dev/null)" || return 1
  [ -z "$loose" ]
}
if [ "$STATE_DIR_GIVEN" != "true" ]; then
  block "no --state-dir was given, so the crusty loop's DONE/CLEAN state cannot be read; crusty evidence is never taken from the TMPDIR fallback"
elif [ -z "$GATE_HOME" ] || [ ! -f "${GATE_HOME}/autodrive_state.sh" ]; then
  block "autodrive_state.sh is missing beside the merge gate (${GATE_HOME:-<unknown>}); the crusty-loop marker cannot be read"
elif ! autodrive_private "$STATE_DIR"; then
  block "state dir ${STATE_DIR} is not private to this user (not owned by this user, a symlink, or group/world-writable); crusty state there is not evidence"
else
  CRUSTY_OK="true"
  for f in phases.tsv crusty-latest.json crusty-records.tsv; do
    if [ -L "${STATE_DIR}/${f}" ] || { [ -e "${STATE_DIR}/${f}" ] && { [ ! -f "${STATE_DIR}/${f}" ] || ! autodrive_private "${STATE_DIR}/${f}"; }; }; then
      block "${STATE_DIR}/${f} is not private to this user (a symlink, not a regular file, not owned by this user, or group/world-writable); crusty state there is not evidence"
      CRUSTY_OK="false"
    fi
  done
  if [ "$CRUSTY_OK" = "true" ]; then
    # shellcheck source=/dev/null
    if ! . "${GATE_HOME}/autodrive_state.sh"; then
      block "autodrive_state.sh beside the merge gate could not be loaded; the crusty-loop marker cannot be read"
    elif ! autodrive_phase_done "$STATE_DIR" "crusty-loop"; then
      block "the crusty-loop phase is not recorded as done in ${STATE_DIR}; criterion 3 needs this run's crusty loop to have ended DONE"
    else
      CRUSTY_VERDICT="MISSING"
      [ -f "${STATE_DIR}/crusty-latest.json" ] && CRUSTY_VERDICT="$(field "$(cat "${STATE_DIR}/crusty-latest.json")" crusty_verdict MISSING)"
      note "crusty_phase_done=true crusty_verdict=$(printf '%s' "$CRUSTY_VERDICT" | tr -cd 'A-Za-z_')"
      [ "$CRUSTY_VERDICT" = "CLEAN" ] || block "the crusty loop's final crusty_verdict is not CLEAN in ${STATE_DIR}/crusty-latest.json; criterion 3 is not met"
      # The record the manifest's last row names must be private too. Its
      # name is validated before any path is built from it.
      CRUSTY_ROW="$(autodrive_crusty_manifest_row "$STATE_DIR")" || CRUSTY_ROW=""
      CRUSTY_RECORD="${CRUSTY_ROW%% *}"
      if [ -n "$CRUSTY_RECORD" ] && { [ -e "${STATE_DIR}/${CRUSTY_RECORD}" ] || [ -L "${STATE_DIR}/${CRUSTY_RECORD}" ]; } \
         && ! autodrive_private "${STATE_DIR}/${CRUSTY_RECORD}"; then
        block "${STATE_DIR}/${CRUSTY_RECORD} is not private to this user (a symlink, not owned by this user, or group/world-writable); crusty state there is not evidence"
      fi
      if CRUSTY_FINAL="$(autodrive_crusty_final "$STATE_DIR")"; then
        note "crusty_reviewed_head_sha=$(printf '%s' "$CRUSTY_FINAL" | tr -cd '0-9a-f')"
        CRUSTY_REVIEWED="$CRUSTY_FINAL"
      else
        case "$CRUSTY_FINAL" in
          crusty-loop-not-done|crusty-manifest-missing|crusty-record-missing|crusty-record-modified|crusty-not-clean|crusty-head-sha-empty) ;;
          *) CRUSTY_FINAL="crusty-other" ;;
        esac
        block "crusty records in ${STATE_DIR} are not loop-written evidence (${CRUSTY_FINAL}); criterion 3 is not met"
      fi
    fi
  fi
fi

# --- 6c. The qa evidence chain (#1517 D5) ----------------------------------
# Agents run after the evidence step as the same user, so the qa evidence is
# trusted only through merge-ready-records.tsv -> round record -> qa_evidence_sha
# -> qa-evidence.json -> head_sha, checked on the section 6 copies. The helpers
# come from beside this gate only. Without --state-dir, 6b has already blocked.
TRUST_OK="false"; CRUSTY_REVIEWED="${CRUSTY_REVIEWED:-}"
if [ "$STATE_DIR_GIVEN" = "true" ]; then
  if [ -z "$GATE_HOME" ] || [ ! -f "${GATE_HOME}/autodrive_trust.sh" ] || [ ! -f "${GATE_HOME}/autodrive_state.sh" ] \
     || ! . "${GATE_HOME}/autodrive_state.sh" || ! . "${GATE_HOME}/autodrive_trust.sh"; then
    block "autodrive_trust.sh or autodrive_state.sh is missing beside the merge gate (${GATE_HOME:-<unknown>}) or could not be loaded; the qa evidence chain and the commits after the clean crusty round cannot be checked"
  else
    TRUST_OK="true"
    MR_ROW="$(autodrive_manifest_row "$STATE_DIR" merge-ready)" || MR_ROW=""
    for f in merge-ready-records.tsv "${MR_ROW%% *}" merge-ready-latest.json qa-evidence.json; do
      [ -n "$f" ] || continue
      if [ -L "${STATE_DIR}/${f}" ] || { [ -e "${STATE_DIR}/${f}" ] && { [ ! -f "${STATE_DIR}/${f}" ] || ! autodrive_private "${STATE_DIR}/${f}"; }; }; then
        block "${STATE_DIR}/${f} is not private to this user (a symlink, not a regular file, not owned by this user, or group/world-writable); the qa evidence chain there is not evidence"
      fi
    done
    QA_TRUST="$(autodrive_qa_trusted "$STATE_DIR" "$REC_COPY" "$QA_COPY" "$HEAD_SHA")" || true
    case "$QA_TRUST" in
      ok) note "qa_evidence_chain=ok" ;;
      qa-manifest-missing|qa-record-modified|qa-evidence-modified|qa-evidence-stale)
        block "the qa evidence is not bound to a loop-written round record for ${HEAD_SHA} (${QA_TRUST}); criterion 1 is not met" ;;
      *) block "the qa evidence is not bound to a loop-written round record for ${HEAD_SHA} (qa-other); criterion 1 is not met" ;;
    esac
  fi
fi

# --- 6d. Commits after the clean crusty round (#1517 D4) --------------------
# Every commit from the reviewed head to HEAD_SHA must be a base merge or a
# description or evidence change. The base SHA is fetched now, never read from
# a local ref, and a range that cannot be read blocks; it is never skipped.
if [ -n "$CRUSTY_REVIEWED" ] && [ "$TRUST_OK" = "true" ]; then
  if ! autodrive_is_sha "$HEAD_SHA" \
     || ! env -u GIT_DIR -u GIT_WORK_TREE GIT_NO_REPLACE_OBJECTS=1 git cat-file -e "${HEAD_SHA}^{commit}" 2>/dev/null; then
    block "head ${HEAD_SHA:-<none>} is not in the local clone, so the commits after the clean crusty round cannot be read; criterion 3 is not met"
  else
    BASE_REF="$(field "$STATE_JSON" baseRefName "")"
    BASE_SHA="$(autodrive_base_sha "$PWD" "$BASE_REF")" || BASE_SHA=""
    note "base_ref=$(printf '%s' "$BASE_REF" | tr -cd 'A-Za-z0-9._/-') base_sha=${BASE_SHA:-<none>}"
    RANGE="$(autodrive_crusty_range "$PWD" "$CRUSTY_REVIEWED" "$HEAD_SHA" "$BASE_SHA")" || true
    FIRST="${RANGE#crusty-unreviewed-commits:}"
    case "$RANGE" in
      ok) note "crusty_range=ok" ;;
      crusty-unreviewed-commits:*)
        if autodrive_is_sha "$FIRST"; then
          block "commit ${FIRST} after the clean crusty round is not a base merge or a description or evidence change; criterion 3 is not met"
        else
          block "the commits after the clean crusty round cannot be read (crusty-range-other); criterion 3 is not met"
        fi ;;
      crusty-range-unreadable) block "the commits after the clean crusty round cannot be read; criterion 3 is not met" ;;
      *) block "the commits after the clean crusty round cannot be read (crusty-range-other); criterion 3 is not met" ;;
    esac
  fi
fi

# --- 7. The merge-ready round's own structured verdict ---------------------
# Read from the private copy that section 6c checked against the manifest.
if [ -z "$ROUND_RECORD" ] || [ ! -f "$ROUND_RECORD" ]; then
  block "no merge-ready round record was produced in this run"
else
  MR_RAW="$(cat "${REC_COPY:-/dev/null}")"
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
( umask 077; rm -f -- "$BUNDLE"
  { printf 'auto-drive-to-merge merge gate — PR #%s @ %s\n' "$PR" "$HEAD_SHA"
    printf 'captured: %s\n\nEVIDENCE\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    printf '  - %s\n' "${EVIDENCE[@]}"
    printf '\nBLOCKERS\n'
    if [ "${#BLOCKERS[@]}" -eq 0 ]; then printf '  (none)\n'; else printf '  - %s\n' "${BLOCKERS[@]}"; fi
  } > "$BUNDLE" )
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
