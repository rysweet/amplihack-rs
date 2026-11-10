#!/bin/bash -p
# autodrive_merge_gate.sh — the final, evidence-gated merge for auto-drive-to-merge.
#
# NO SILENT MERGE. Every criterion is re-verified HERE, in the run that merges,
# against ONE head SHA, and the evidence is written down before anything is
# merged. A criterion that cannot be read is a FAILURE, never a pass: an
# unreadable CI status means "we do not know", and that does not merge.
#
# The merge argv is a FIXED literal list built in this file, pinned element by
# element before execution. It takes no flags from any caller, so there is no
# argument through which a branch-protection bypass could be threaded. A bypass
# flag and a hook-skipping commit flag are NEVER used anywhere in this workflow;
# see docs/reference/auto-drive-to-merge.md#two-absolute-prohibitions.
#
#   Exit 0  — merged in this run and confirmed MERGED, or ALREADY merged.
#   Exit 1  — at least one criterion failed or was unreadable. Nothing merged.
#   Exit 79 — terminal policy refusal from a child. Surfaced, never retried.

set -uo pipefail
# #1551: -p ignores exported functions and BASH_ENV; drop gh/git/jq functions anyway.
builtin unset -f gh git jq

AUTODRIVE_EXIT_POLICY_REFUSAL=79
PR=""; REPO="."; ROUND_RECORD=""; QA_EVIDENCE=""; STATE_DIR=""; DRY_RUN="false"
MERGE_SUBJECT=""; MERGE_BODY_FILE=""; MERGE_BODY_DIGEST=""; SHA12=""; BODY_RC=0  # squash message (#2056), set in 5b

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
# This gate's own directory, found before the cd below because BASH_SOURCE may be
# relative. Every helper it sources or runs comes from here, and from nowhere else.
GATE_HOME="$(cd "$(dirname "${BASH_SOURCE[0]}")" 2>/dev/null && pwd -P)"
# gh resolves {owner}/{repo} from the working directory, so every read below
# must run inside the repository the PR belongs to.
cd "$REPO" 2>/dev/null || { echo "ERROR: --repo '${REPO}' is not a directory; refusing to read PR state from an unknown working directory." >&2; exit 2; }
# Decide whether a state dir was given BEFORE the fallback below: an empty
# --state-dir must not turn the world-writable TMPDIR into crusty evidence.
if [ -n "$STATE_DIR" ]; then STATE_DIR_GIVEN="true"; else STATE_DIR_GIVEN="false"; fi
# The evidence bundle (section 8) still needs somewhere to go. The gate only creates
# a missing directory, under umask 077; it never changes the mode of an existing one.
STATE_DIR="${STATE_DIR:-${TMPDIR:-/tmp}}"
( umask 077 && mkdir -p -- "$STATE_DIR" ) || exit 2
AMPLIHACK_BIN="${AMPLIHACK_BIN:-amplihack}"
export GIT_PAGER=cat GH_PAGER=cat PAGER=cat LESS=FRX

BLOCKERS=()
EVIDENCE=()
note()  { EVIDENCE+=("$1"); echo "  evidence: $1" >&2; }
block() { BLOCKERS+=("$1"); echo "  BLOCKER: $1" >&2; }

# `--require-field` selects the LAST JSON object carrying the field, not the first
# of any shape (#1337, PR #1347): first-wins is fail-OPEN on a quoted example.
field() { printf '%s' "${1:-}" | "$AMPLIHACK_BIN" orch helper extract-json --require-field "$2" \
          | "$AMPLIHACK_BIN" orch helper extract-field --field "$2" --default "$3"; }

# --- 0. Already merged? A resumed run must never redo merged work. ----------
STATE_JSON="$(gh pr view "$PR" --json state,mergedAt,isDraft,mergeable,mergeStateStatus,reviewDecision,headRefOid,title,body,url,baseRefName 2>/dev/null)"
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
# Counted by autodrive_platform_facts.sh beside this gate: the same PAGINATED
# reviewThreads reader the merge round uses. Unreadable blocks; outdated counts.
THREADS=""
[ -n "$GATE_HOME" ] && [ -f "${GATE_HOME}/autodrive_platform_facts.sh" ] \
  && THREADS="$(field "$(bash "${GATE_HOME}/autodrive_platform_facts.sh" "$PR" 2>/dev/null)" unresolved_threads "")"
case "$THREADS" in
  '' | *[!0-9]*) block "review-thread state is unreadable; an unreadable criterion is a failure, not a pass" ;;
  0) note "review_threads_unresolved=0" ;;
  *) block "${THREADS} unresolved review thread(s)" ;;
esac

# --- 5. CI on THIS head SHA -------------------------------------------------
# `gh pr checks` exits non-zero when checks are failing or pending, and also when
# it cannot read them. All three block; only a readable, all-green rollup passes.
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

# --- 5b. The squash commit message, read in THIS run (#2056) ---------------
# `gh pr merge --squash` with no message flags leaves GitHub to concatenate the
# BRANCH'S commits, so a stale trailer or a `Fixes #N` from a branch this one is
# stacked on lands in the base. Subject and body come from the same STATE_JSON that
# bound every other criterion to this head SHA, read with jq, never with field().
# Composing and writing are autodrive_merge_message.sh's; the decision is here.
if [ "$STATE_DIR_GIVEN" != "true" ] || [ -z "$GATE_HOME" ] || [ ! -f "${GATE_HOME}/autodrive_merge_message.sh" ] \
   || ! . "${GATE_HOME}/autodrive_merge_message.sh" || ! . "${GATE_HOME}/autodrive_state.sh" \
   || ! autodrive_private "$STATE_DIR" || ! SHA12="$(autodrive_merge_sha12 "$HEAD_SHA")"; then
  block "the squash commit message for #${PR} cannot be composed and written privately (no explicit --state-dir, an empty GATE_HOME, autodrive_merge_message.sh missing beside the merge gate, autodrive_state.sh missing or unloadable, a state dir that is not private to this user, or a head SHA whose first 12 characters are not lowercase hex); the message is never left for the platform to compose from the branch's commits"
elif ! MERGE_SUBJECT="$(autodrive_merge_subject "$STATE_JSON" "$PR")"; then
  block "pull request #${PR} title is unreadable (absent, null, not a string, blank, or over 1024 characters); a criterion we cannot read never merges"
else
  MERGE_BODY_FILE="${STATE_DIR}/merge-message-${PR}-${SHA12}.txt"; MERGE_BODY_DIGEST="$(autodrive_merge_body_file "$STATE_JSON" "$MERGE_BODY_FILE")"; BODY_RC=$?
  case "$BODY_RC" in
    0) note "squash_message merge_subject=\"${MERGE_SUBJECT}\" body_file=${MERGE_BODY_FILE} body_bytes=$(wc -c < "$MERGE_BODY_FILE" | tr -d '[:space:]') body_sha256=${MERGE_BODY_DIGEST}" ;;
    1) MERGE_BODY_FILE=""; MERGE_BODY_DIGEST=""; block "the body of pull request #${PR} is unreadable (no body key, not a string or null, or over 65536 bytes); a criterion we cannot read never merges" ;;
    *) MERGE_BODY_FILE=""; MERGE_BODY_DIGEST=""; block "the squash message file for #${PR} could not be written under ${STATE_DIR}, or what was written could not be digested (the directory is unwritable, something at merge-message-${PR}-${SHA12}.txt could not be replaced, or neither sha256sum nor shasum is installed); the message is never left for the platform to compose from the branch's commits" ;;
  esac
fi

# --- 6. qa-team scenario evidence ------------------------------------------
# The qa evidence and the round record are copied ONCE into a private temporary
# directory (#1517 D5); sections 6, 6c and 7 read only those copies.
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
  # qa_reason names the first failing check (#1517); only token characters kept.
  note "qa_reason=$(field "$QA_RAW" qa_reason '' | tr -cd 'a-z-')"
  [ "$QA_STATUS" = "PASS" ] || block "qa-team scenarios did not pass in this run (qa_status=${QA_STATUS})"
  # Existence + PASS is not enough: an evidence file left behind by an earlier
  # round describes a tree that is no longer what would be merged.
  if [ -z "$QA_SHA" ]; then
    block "the qa-team evidence records no head_sha; evidence that is not bound to a SHA never merges"
  elif [ -n "$HEAD_SHA" ] && [ "$QA_SHA" != "$HEAD_SHA" ]; then
    block "the qa-team evidence was captured against ${QA_SHA} but the head is now ${HEAD_SHA}; evidence must bind to the SHA being merged"
  fi
  # Gadugi need: from this gate's own baseRefName and headRefOid (#1763), never the evidence; failure: required.
  GADUGI_STATUS="$(field "$QA_RAW" gadugi_status MISSING)"
  GADUGI_COUNT="$(field "$QA_RAW" gadugi_scenario_count "")"
  GADUGI_REQ="$(AUTODRIVE_QA_BASE="$(field "$STATE_JSON" baseRefName "")"; AUTODRIVE_QA_HEAD="$HEAD_SHA"
    export AUTODRIVE_QA_BASE AUTODRIVE_QA_HEAD
    . "${GATE_HOME:-/nonexistent}/autodrive_trust.sh" 2>/dev/null && autodrive_gadugi_required .)" || GADUGI_REQ="true"
  note "gadugi_required=${GADUGI_REQ} gadugi_status=${GADUGI_STATUS} gadugi_scenario_count=${GADUGI_COUNT:-<none>} gadugi_scenario_dir=$(field "$QA_RAW" gadugi_scenario_dir '')"
  if [ "${GADUGI_REQ}:${GADUGI_STATUS}" != "false:NOT_REQUIRED" ]; then
    [ "$GADUGI_STATUS" = "PASS" ] || block "gadugi-test scenarios were not validated and run to a pass in this run (gadugi_status=${GADUGI_STATUS})"
    case "$GADUGI_COUNT" in
      ''|*[!0-9]*|0*) block "gadugi_scenario_count='${GADUGI_COUNT}' is not a positive integer; zero scenarios is not a qa-team pass" ;;
    esac
  fi
fi

# --- 6b. Criterion 3: crusty loop DONE, CLEAN or MINOR_ONLY with follow-ups filed (#1979)
# The crusty loop meets the quality-audit criterion: the `crusty-loop` marker in
# phases.tsv (written only after the loop reports DONE) and a final CLEAN or
# MINOR_ONLY verdict in crusty-latest.json; MINOR_ONLY also needs a follow-up issue
# filed for every minor finding (autodrive_followups.sh gate). Read only from a
# state dir given explicitly, private to this user, and only from regular files.
# Since #1517 the last loop-written record must also match crusty-records.tsv, the
# manifest autodrive_loop.sh writes before any agent runs, so autodrive_crusty_final
# rejects an injected, edited or archived record. block_unless_private <what>
# <name>...: blocks each named file in STATE_DIR that is a symlink, or exists and
# is not a private regular file; an absent file passes, and the reader decides.
block_unless_private() {
  local what="$1" f p rc=0
  shift
  for f in "$@"; do
    [ -n "$f" ] || continue
    p="${STATE_DIR}/${f}"
    if [ -L "$p" ] || { [ -e "$p" ] && { [ ! -f "$p" ] || ! autodrive_private "$p"; }; }; then
      block "${p} is not private to this user (a symlink, not a regular file, not owned by this user, or group/world-writable); ${what} there is not evidence"
      rc=1
    fi
  done
  return "$rc"
}
if [ "$STATE_DIR_GIVEN" != "true" ]; then
  block "no --state-dir was given, so the crusty loop's DONE/CLEAN state cannot be read; crusty evidence is never taken from the TMPDIR fallback"
elif [ -z "$GATE_HOME" ] || [ ! -f "${GATE_HOME}/autodrive_state.sh" ]; then
  block "autodrive_state.sh is missing beside the merge gate (${GATE_HOME:-<unknown>}); the crusty-loop marker cannot be read"
elif ! . "${GATE_HOME}/autodrive_state.sh"; then
  block "autodrive_state.sh beside the merge gate could not be loaded; the crusty-loop marker cannot be read"
elif ! autodrive_private "$STATE_DIR"; then
  block "state dir ${STATE_DIR} is not private to this user (not owned by this user, a symlink, or group/world-writable); crusty state there is not evidence"
elif block_unless_private "crusty state" phases.tsv crusty-latest.json crusty-records.tsv; then
  if ! autodrive_phase_done "$STATE_DIR" "crusty-loop"; then
    block "the crusty-loop phase is not recorded as done in ${STATE_DIR}; criterion 3 needs this run's crusty loop to have ended DONE"
  else
    CRUSTY_VERDICT="MISSING"
    [ -f "${STATE_DIR}/crusty-latest.json" ] && CRUSTY_VERDICT="$(field "$(cat "${STATE_DIR}/crusty-latest.json")" crusty_verdict MISSING)"
    note "crusty_phase_done=true crusty_verdict=$(printf '%s' "$CRUSTY_VERDICT" | tr -cd 'A-Za-z_')"
    case "$CRUSTY_VERDICT" in
      CLEAN|MINOR_ONLY)
        FU_NONE="ERROR: followups-unfiled: autodrive_followups.sh gate did not run beside the merge gate (#1979)"
        FU="$(bash "${GATE_HOME}/autodrive_followups.sh" gate --state-dir "$STATE_DIR" --pr "$PR")" && note "$FU" || block "${FU:-$FU_NONE}" ;;
      *) block "the crusty loop's final crusty_verdict is not CLEAN in ${STATE_DIR}/crusty-latest.json; criterion 3 is not met" ;;
    esac
    # The record the manifest's last row names must be private too; its name is
    # validated before any path is built from it.
    CRUSTY_ROW="$(autodrive_crusty_manifest_row "$STATE_DIR")" || CRUSTY_ROW=""
    block_unless_private "crusty state" "${CRUSTY_ROW%% *}" || true
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

# --- 6c. The qa evidence chain (#1517 D5) ----------------------------------
# Agents run after the evidence step as the same user, so the qa evidence is trusted
# only through merge-ready-records.tsv -> round record -> qa_evidence_sha -> head_sha.
TRUST_OK="false"; CRUSTY_REVIEWED="${CRUSTY_REVIEWED:-}"
if [ "$STATE_DIR_GIVEN" = "true" ]; then
  if [ -z "$GATE_HOME" ] || [ ! -f "${GATE_HOME}/autodrive_trust.sh" ] || [ ! -f "${GATE_HOME}/autodrive_state.sh" ] \
     || ! . "${GATE_HOME}/autodrive_state.sh" || ! . "${GATE_HOME}/autodrive_trust.sh"; then
    block "autodrive_trust.sh or autodrive_state.sh is missing beside the merge gate (${GATE_HOME:-<unknown>}) or could not be loaded; the qa evidence chain and the commits after the clean crusty round cannot be checked"
  else
    TRUST_OK="true"
    MR_ROW="$(autodrive_manifest_row "$STATE_DIR" merge-ready)" || MR_ROW=""
    block_unless_private "the qa evidence chain" merge-ready-records.tsv "${MR_ROW%% *}" merge-ready-latest.json qa-evidence.json || true
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
# description or evidence change. A range that cannot be read blocks; never skipped.
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
# --match-head-commit makes GitHub itself refuse the merge if the head moved after
# the evidence above was captured, which is why every criterion binds to HEAD_SHA.
# The self-check is POSITIONAL: with a subject that may hold spaces, comparing two
# IFS-joined arrays asserts nothing. #1551: `pr` and `merge` stay at elements 0 and 1.
# The message file is re-checked HERE for privacy AND for the bytes 5b wrote: the state
# dir holds files written by this same user (#1517 D5), whom mode and owner cannot stop.
MERGE_ARGV=(pr merge "$PR" --squash --delete-branch --match-head-commit "$HEAD_SHA" --subject "$MERGE_SUBJECT" --body-file "$MERGE_BODY_FILE")
if [ "${#MERGE_ARGV[@]}" -ne 11 ] || [ "${MERGE_ARGV[0]}" != "pr" ] || [ "${MERGE_ARGV[1]}" != "merge" ] || [ "${MERGE_ARGV[2]}" != "$PR" ] \
   || [ "${MERGE_ARGV[3]}" != "--squash" ] || [ "${MERGE_ARGV[4]}" != "--delete-branch" ] || [ "${MERGE_ARGV[5]}" != "--match-head-commit" ] \
   || [ "${MERGE_ARGV[6]}" != "$HEAD_SHA" ] || [ "${MERGE_ARGV[7]}" != "--subject" ] || [ "${MERGE_ARGV[8]}" != "$MERGE_SUBJECT" ] \
   || [ "${MERGE_ARGV[9]}" != "--body-file" ] || [ "${MERGE_ARGV[10]}" != "$MERGE_BODY_FILE" ] || [ -z "$MERGE_SUBJECT" ] || ! autodrive_merge_body_intact "$MERGE_BODY_FILE" "$MERGE_BODY_DIGEST"; then
  echo "ERROR: refusing to merge #${PR}. Either the merge argv is not the fixed 11-element list (gh pr merge <pr> --squash --delete-branch --match-head-commit <sha> --subject <subject> --body-file <file>), or the squash subject is empty, or the message file is no longer a regular file private to this user holding exactly the bytes composed in this run." >&2
  exit 1
fi
if [ "$DRY_RUN" = "true" ]; then
  echo "AUTO_DRIVE_MERGE: DRY_RUN — would run: gh ${MERGE_ARGV[*]}" >&2
  printf '{"merge_result":"DRY_RUN","pr":"%s","head_sha":"%s","evidence_bundle":"%s"}\n' "$PR" "$HEAD_SHA" "$BUNDLE"
  exit 0
fi

echo "AUTO_DRIVE_MERGE: merging PR #${PR} at ${HEAD_SHA} — gh ${MERGE_ARGV[*]}" >&2
# Capture the status of `gh` ITSELF. Inside `if ! gh ...; then`, `$?` is the
# status of the NEGATION, always 0 there, which would make exit 79 dead code.
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
