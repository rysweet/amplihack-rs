#!/usr/bin/env bash
# test-issue-2219-approval-no-review-required.sh — criterion 6 when no review
# is required, and the split of UNREADABLE (issue #2219, AC1 to AC3).
#
# WHAT IS PINNED HERE
#
# AC1 (regression lock, not a fix). A readable required count of 0 — classic
#     protection reporting 0, or `branches/<base>` reporting "protected": false
#     — beside a rulesets read that returned no `pull_request` rule is MET
#     through `required-count`, and that count wins BEFORE the merge state is
#     consulted. A rulesets read that FAILED is not the same thing: the second
#     half of the condition ("no ruleset has a pull_request rule") was never
#     established, so a classic 0 beside an unreadable rulesets read stays
#     unreadable. #2219's first reading of the gap — that REQ is left empty
#     when CLASSIC reads 0 and RULES is empty — describes a failed read, so
#     widening REQ there would report MET for a branch whose rulesets were
#     never read. This file locks that it does not happen.
#
# AC2 (characterisation). A non-admin 404 from the protection endpoint with
#     `branches/<base>` reporting "protected": true, an empty reviewDecision
#     and a merge state of CLEAN, HAS_HOOKS or UNSTABLE is MET through
#     `merge-state`, with `required_approvals` left empty: the count was not
#     read, and reporting it as 0 would pass repositories that do require
#     approvals.
#
# AC3 (the change). The single `UNREADABLE`/`unreadable` bucket splits three
#     ways, decided in the tool so that no agent decides it (#1517, #1518):
#
#       BLOCKED, nothing an agent can clear outstanding, and a review rule
#       NAMED by either read (code-owner, last-push or listed reviewers at a
#       count of 0)                      -> NOT_MET  / review-rule
#       `gh pr view` failed, or reviewDecision is a token outside the
#       allow-list, or the merge state is UNKNOWN while the count is also
#       unreadable                       -> UNREADABLE / facts-unreadable
#       anything else that reaches the bucket (BLOCKED with no rule named,
#       DRAFT)                           -> UNREADABLE / unreadable
#
#     The tool also prints `approval_blocker`, the blocker id the round is to
#     report for anything but MET — one per source, decided here so that no
#     agent maps a source to a blocker itself (#1517, #1518). It is empty
#     exactly when the status is MET.
#
#     P1: no case this change touches becomes MET. `review-rule` is confined
#     to BLOCKED, so CLEAN/HAS_HOOKS/UNSTABLE still wins, and an agent-clearable
#     blocker still wins over a named review rule (PENDING, not a person).
#
# HOW. autodrive_platform_facts.sh runs unmodified against a fake `gh` that
# returns canned POST-`--jq` strings per endpoint, keyed by environment
# variables, and IGNORES the `--jq` expression it is handed. Consequence: the
# tool's own VIEW_JQ expression is NOT exercised here; that stays covered by
# the `pf_expect` harness in test-auto-drive-to-merge.sh, which feeds a real
# jq real GitHub-shaped JSON. Nothing here uses jq, and nothing here waits:
# every assertion is an exit status or a string comparison.
#
# Usage: bash amplifier-bundle/recipes/tests/test-issue-2219-approval-no-review-required.sh
# Exit codes: 0 = pass, 1 = fail, 2 = test harness error.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
TOOL="${REPO_ROOT}/amplifier-bundle/tools/autodrive_platform_facts.sh"

harness_error() { echo "HARNESS-ERROR: $*" >&2; exit 2; }
[ -f "$TOOL" ] || harness_error "tool not found: $TOOL"

PASS_COUNT=0
FAIL_COUNT=0
pass() { PASS_COUNT=$((PASS_COUNT + 1)); echo "  PASS[$1]: $2"; }
fail() { FAIL_COUNT=$((FAIL_COUNT + 1)); echo "  FAIL[$1]: $2" >&2; }

echo "=== issue #2219: criterion 6 when no review is required ==="

WORK="$(mktemp -d "${TMPDIR:-/tmp}/issue-2219-approval.XXXXXX")" || harness_error "mktemp -d failed"
WORK="$(cd "$WORK" && pwd -P)"
cleanup() { [ -n "${WORK:-}" ] && [ "${WORK}" != "/" ] && rm -rf -- "${WORK}"; }
trap cleanup EXIT

TEST_HOME="${WORK}/home"; mkdir -p "${TEST_HOME}"
mkdir -p "${WORK}/ghcfg" "${WORK}/xdg"
BIN="${WORK}/bin"; mkdir -p "${BIN}"
chmod 0700 "${WORK}" "${BIN}"
SYSPATH="/usr/bin:/bin"
HEAD_SHA="0123456789abcdef0123456789abcdef01234567"

# --- the fake gh -----------------------------------------------------------
# One canned answer per endpoint, already in the shape the tool's own `--jq`
# would have produced. The `--jq` argument is deliberately ignored, so no jq
# is needed and no jq expression is under test here. An endpoint this test
# never set up exits 64 naming itself, so a silent empty answer can never be
# mistaken for a measured value.
cat > "${BIN}/gh" <<'GH'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "${GH_CALLS:-/dev/null}"
http_fail() { printf 'gh: %s (HTTP 404)\n' "${1:-Not Found}" >&2; exit 1; }
case "$1 ${2:-}" in
  "pr view")
    case " $* " in *" --json number "*) printf '%s\n' "${GH_PR_NUMBER:-42}"; exit 0 ;; esac
    [ "${GH_VIEW:-}" = fail ] && { echo "gh: could not read the pull request" >&2; exit 1; }
    printf '%s\n' "${GH_VIEW:-}"; exit 0 ;;
  "api graphql")
    [ "${GH_THREADS:-0}" = fail ] && { echo "gh: graphql read failed" >&2; exit 1; }
    printf '%s\n' "${GH_THREADS:-0}"
    [ "${GH_THREADS_PARTIAL:-false}" = true ] && { echo "gh: later thread page refused" >&2; exit 1; }
    exit 0 ;;
esac
if [ "${1:-}" = api ]; then
  p=""
  for a in "$@"; do case "$a" in repos/*) p="$a" ;; esac; done
  case "$p" in
    */protection/required_pull_request_reviews)
      [ "${GH_CLASSIC:-fail}" = fail ] && http_fail "Not Found"
      printf '%s\n' "${GH_CLASSIC}"; exit 0 ;;
    */rules/branches/*)
      [ "${GH_RULES-0}" = fail ] && http_fail "Not Found"
      printf '%s\n' "${GH_RULES-0}"
      [ "${GH_RULES_PARTIAL:-false}" = true ] && { echo "gh: later rules page refused" >&2; exit 1; }
      exit 0 ;;
    */branches/*)
      [ "${GH_PROTECTED:-true}" = fail ] && http_fail "Branch not found"
      printf '%s\n' "${GH_PROTECTED:-true}"; exit 0 ;;
  esac
fi
printf 'fake gh: unimplemented endpoint: %s\n' "$*" >&2
exit 64
GH
chmod +x "${BIN}/gh"

# view_line <merge_state> <decision> [checks] [mergeable] [state] [draft] [base]
# The eight `|`-joined fields the tool's VIEW_JQ produces, in its order.
view_line() {
  printf '%s|%s|%s|%s|%s|%s|%s|%s' \
    "${5:-OPEN}" "${6:-false}" "${4:-MERGEABLE}" "$1" "$2" "${HEAD_SHA}" "${3:-pass}" "${7:-main}"
}

N=0
OUT=""; RC=0; ERRF=""
run_pf() { # run_pf <GH_VAR=value>...: the tool for PR 42 with nothing but the fake gh
  N=$((N + 1)); ERRF="${WORK}/pf-${N}.err"
  OUT="$(env -i HOME="${TEST_HOME}" PATH="${BIN}:${SYSPATH}" \
    GH_CONFIG_DIR="${WORK}/ghcfg" XDG_CONFIG_HOME="${WORK}/xdg" \
    GH_CALLS="${WORK}/pf-${N}.calls" "$@" bash "${TOOL}" 42 2>"${ERRF}")"
  RC=$?
}

# jf <field>: the value of one string field of the one-line flat JSON record.
jf() { printf '%s' "${OUT}" | sed -n "s/.*\"$1\":\"\\([^\"]*\\)\".*/\\1/p"; }

errtail() { tr '\n' '|' < "${ERRF}" 2>/dev/null; }

MET_SEEN=""   # labels that reported MET, for the P1 roll-up
SPLIT_LABELS="" # labels the UNREADABLE split touches

# blocker_for <status> <source>: the id the round must report, or empty for
# MET. Written out here rather than read from the tool, so the test states
# the contract instead of echoing whatever the tool printed.
blocker_for() {
  case "$1" in
    MET) printf '' ;;
    PENDING) printf 'approval-pending-other-blockers' ;;
    *) case "$2" in
         facts-unreadable) printf 'approval-facts-unreadable' ;;
         unreadable) printf 'approval-state-unreadable' ;;
         *) printf 'required-approval-missing' ;;
       esac ;;
  esac
}

expect() { # expect <label> <status> <source> <required> <why> [field=value]...
  local label="$1" status="$2" source="$3" req="$4" why="$5" bad="" kv got want
  shift 5
  [ "${RC}" = 0 ] || bad="${bad} rc=${RC}"
  [ "$(printf '%s\n' "${OUT}" | grep -c .)" = 1 ] || bad="${bad} not-one-line"
  case "${OUT}" in '{"pr":"'*'"approval_source":"'*'"}') ;; *) bad="${bad} shape" ;; esac
  case "$(errtail)" in *"unimplemented endpoint"*) bad="${bad} fake-gh-endpoint-missing" ;; esac
  got="$(jf approval_status)"
  [ "${got}" = "${status}" ] || bad="${bad} approval_status=${got}(want ${status})"
  [ "${got}" != "MET" ] || MET_SEEN="${MET_SEEN} ${label}"
  got="$(jf approval_source)"
  [ "${got}" = "${source}" ] || bad="${bad} approval_source=${got}(want ${source})"
  got="$(jf required_approvals)"
  [ "${got}" = "${req}" ] || bad="${bad} required_approvals=${got}(want ${req})"
  # The blocker id travels with the facts, so the round reports a measurement.
  want="$(blocker_for "${status}" "${source}")"
  case " $* " in
    *" approval_blocker="*) ;; # the case states its own expectation below
    *) got="$(jf approval_blocker)"
       [ "${got}" = "${want}" ] || bad="${bad} approval_blocker=${got}(want ${want})" ;;
  esac
  for kv in "$@"; do
    got="$(jf "${kv%%=*}")"
    [ "${got}" = "${kv#*=}" ] || bad="${bad} ${kv%%=*}=${got}(want ${kv#*=})"
  done
  if [ -z "${bad}" ]; then pass "${label}" "${why}"
  else fail "${label}" "${why} --${bad} | ${OUT} | $(errtail)"; fi
}

# ---------------------------------------------------------------------------
# AC1 — a readable count of 0 is MET; an unreadable rulesets read is not.
# ---------------------------------------------------------------------------
run_pf GH_CLASSIC=0 GH_RULES=0 GH_VIEW="$(view_line CLEAN '')"
expect AC1-classic-0-no-rule MET required-count 0 \
  "classic protection requires 0 approvals and no ruleset pull_request rule applies: criterion 6 is met" \
  pr=42 state=OPEN head_sha="${HEAD_SHA}" base_ref=main review_decision= unresolved_threads=0

run_pf GH_CLASSIC=0 GH_RULES=0 GH_VIEW="$(view_line BLOCKED '')"
expect AC1-classic-0-outranks-merge-state MET required-count 0 \
  "a READABLE count of 0 is read before the merge state: BLOCKED does not make a known 0 unknown"

run_pf GH_CLASSIC=fail GH_PROTECTED=false GH_RULES=0 GH_VIEW="$(view_line CLEAN '')"
expect AC1-unprotected-no-rule MET required-count 0 \
  "an unprotected base branch with no ruleset pull_request rule requires 0 approvals"

run_pf GH_CLASSIC=0 GH_RULES=fail GH_VIEW="$(view_line BLOCKED '')"
expect AC1-rulesets-unreadable-stays-unreadable UNREADABLE unreadable "" \
  "a classic 0 beside a FAILED rulesets read is not 'no ruleset requires a review': the count stays unknown"
SPLIT_LABELS="${SPLIT_LABELS} AC1-rulesets-unreadable-stays-unreadable"

# A successful first page says nothing about pages that could not be read.
run_pf GH_CLASSIC=0 GH_RULES=0 GH_RULES_PARTIAL=true GH_VIEW="$(view_line BLOCKED '')"
expect PAGINATION-rules-partial UNREADABLE unreadable "" \
  "a failed later rules page cannot turn a partial zero into no required reviews"

run_pf GH_CLASSIC=0 GH_RULES=0 GH_THREADS=0 GH_THREADS_PARTIAL=true GH_VIEW="$(view_line CLEAN '')"
expect PAGINATION-threads-partial MET required-count 0 \
  "a failed later conversation page makes the thread count unreadable, preserving independent approval evidence" \
  unresolved_threads=unreadable

run_pf GH_CLASSIC=0 GH_RULES=0 GH_THREADS="$(printf '0\n2')" GH_VIEW="$(view_line CLEAN '')"
expect PAGINATION-threads-complete MET required-count 0 \
  "complete conversation pages include unresolved threads from later pages" unresolved_threads=2

run_pf GH_CLASSIC=0 GH_RULES=fail GH_VIEW="$(view_line CLEAN '')"
expect AC1-rulesets-unreadable-clean MET merge-state "" \
  "with the rulesets unreadable, CLEAN is still GitHub saying no required review blocks the merge"

# ---------------------------------------------------------------------------
# AC2 — the non-admin 404 characterisation. Already true; locked here.
# ---------------------------------------------------------------------------
for state in CLEAN HAS_HOOKS UNSTABLE; do
  run_pf GH_CLASSIC=fail GH_PROTECTED=true GH_RULES=0 GH_VIEW="$(view_line "${state}" '')"
  expect "AC2-404-protected-${state}" MET merge-state "" \
    "a non-admin 404 with protected:true and ${state} is met through the merge state; the unread count stays empty, never 0" \
    merge_state="${state}"
done

run_pf GH_CLASSIC=fail GH_PROTECTED=true GH_RULES=0 GH_VIEW="$(view_line CLEAN APPROVED)"
expect AC2-approved-outranks-everything MET review-decision "" \
  "an APPROVED decision is met whatever the counts say"

# ---------------------------------------------------------------------------
# AC3 — the UNREADABLE split.
# ---------------------------------------------------------------------------
# A named review rule, BLOCKED, and nothing an agent can clear: a person, and
# the blocker says so by name (required-approval-missing), not "unreadable".
run_pf GH_CLASSIC=REVIEW_RULE GH_RULES=0 GH_VIEW="$(view_line BLOCKED '')"
expect AC3-blocked-classic-review-rule NOT_MET review-rule "" \
  "classic protection names a code-owner or last-push rule at a count of 0 and the PR is BLOCKED with nothing else outstanding: a review is required"
SPLIT_LABELS="${SPLIT_LABELS} AC3-blocked-classic-review-rule"

run_pf GH_CLASSIC=fail GH_PROTECTED=true GH_RULES="$(printf '0\nREVIEW_RULE')" GH_VIEW="$(view_line BLOCKED '')"
expect AC3-blocked-ruleset-review-rule NOT_MET review-rule "" \
  "a ruleset names a code-owner, last-push or required-reviewer rule at a count of 0: BLOCKED means that review is missing"
SPLIT_LABELS="${SPLIT_LABELS} AC3-blocked-ruleset-review-rule"

if grep -q 'source=review-rule' "${ERRF}" 2>/dev/null; then
  pass AC3-info-names-source "the INFO line names approval_source=review-rule, so the round can cite what decided it"
else
  fail AC3-info-names-source "stderr does not name the new source: $(errtail)"
fi

run_pf GH_CLASSIC=fail GH_PROTECTED=true GH_RULES=0 GH_VIEW="$(view_line BLOCKED '')"
expect AC3-blocked-no-rule-named UNREADABLE unreadable "" \
  "BLOCKED with no review rule named by either read is still a person's job, and still says so as 'unreadable'"
SPLIT_LABELS="${SPLIT_LABELS} AC3-blocked-no-rule-named"

run_pf GH_VIEW=fail GH_CLASSIC=0 GH_RULES=0
expect AC3-view-unreadable UNREADABLE facts-unreadable "" \
  "a failed 'gh pr view' is a failed MEASUREMENT: the round re-reads it, it does not wake a person" \
  state=UNKNOWN head_sha= base_ref= merge_state=UNKNOWN
SPLIT_LABELS="${SPLIT_LABELS} AC3-view-unreadable"

run_pf GH_CLASSIC=fail GH_PROTECTED=true GH_RULES=0 GH_VIEW="$(view_line UNKNOWN '')"
expect AC3-merge-state-unknown UNREADABLE facts-unreadable "" \
  "a merge state GitHub has not computed, beside an unreadable count, is a failed measurement"
SPLIT_LABELS="${SPLIT_LABELS} AC3-merge-state-unknown"

# The shape PR #999 (mistt-repo/Product-Management) stopped on. Its round-3
# platform facts, still in that run's merge-ready-round-3.log, were
# merge_state=UNKNOWN, mergeable=UNKNOWN, an empty reviewDecision and no
# readable count, read straight after a push, before GitHub had computed the
# merge state (a later read in the same round showed CLEAN/MERGEABLE). On
# origin/main the UNKNOWN merge state matched no arm, so the tool kept its
# default UNREADABLE/unreadable, the round reported approval-state-unreadable
# and the loop stopped with "needs a person". The CLEAN shape the issue
# describes was round 1 of the same run, and was already MET/merge-state.
run_pf GH_CLASSIC=fail GH_PROTECTED=true GH_RULES=0 GH_VIEW="$(view_line UNKNOWN '' pass UNKNOWN)"
expect AC3-issue-999-round-3 UNREADABLE facts-unreadable "" \
  "PR #999's round-3 facts (merge state and mergeable both UNKNOWN right after a push) are a failed measurement the next round re-reads, not a person's job" \
  merge_state=UNKNOWN mergeable=UNKNOWN
SPLIT_LABELS="${SPLIT_LABELS} AC3-issue-999-round-3"

run_pf GH_CLASSIC=2 GH_RULES=0 GH_VIEW="$(view_line UNKNOWN '')"
expect AC3-unknown-state-known-count NOT_MET required-count 2 \
  "an UNKNOWN merge state does not hide a count that WAS read: 2 required approvals and no decision is not met"

run_pf GH_CLASSIC=fail GH_PROTECTED=true GH_RULES=0 GH_VIEW="$(view_line CLEAN SOMETHING_NEW)"
expect AC3-decision-outside-allowlist UNREADABLE facts-unreadable "" \
  "a reviewDecision outside the allow-list is a field this tool could not read, whatever the merge state says" \
  review_decision=UNKNOWN
SPLIT_LABELS="${SPLIT_LABELS} AC3-decision-outside-allowlist"

run_pf GH_CLASSIC=2 GH_RULES=0 GH_VIEW="$(view_line CLEAN SOMETHING_NEW)"
expect AC3-decision-outside-allowlist-outranks-count UNREADABLE facts-unreadable 2 \
  "an unreadable decision is unconditional: a readable count does not make the unread field readable, and the count it DID read is still reported" \
  review_decision=UNKNOWN
SPLIT_LABELS="${SPLIT_LABELS} AC3-decision-outside-allowlist-outranks-count"

run_pf GH_CLASSIC=fail GH_PROTECTED=true GH_RULES=0 GH_VIEW="$(view_line DRAFT '' pass MERGEABLE OPEN true)"
expect AC3-draft-stays-unreadable UNREADABLE unreadable "" \
  "a draft pull request is not a failed measurement: it keeps the plain unreadable bucket" \
  is_draft=true merge_state=DRAFT
SPLIT_LABELS="${SPLIT_LABELS} AC3-draft-stays-unreadable"

# review-rule is confined to BLOCKED, and yields to every blocker an agent clears.
run_pf GH_CLASSIC=REVIEW_RULE GH_RULES=0 GH_VIEW="$(view_line CLEAN '')"
expect AC3-review-rule-clean-is-met MET merge-state "" \
  "a named review rule beside CLEAN is GitHub saying it does not block: the merge state still wins"

run_pf GH_CLASSIC=REVIEW_RULE GH_RULES=0 GH_VIEW="$(view_line BLOCKED '' not-passed)"
expect AC3-review-rule-yields-to-check PENDING other-blockers "" \
  "a check that has not passed is cleared by an agent first; BLOCKED cannot yet say a review is missing"

run_pf GH_CLASSIC=REVIEW_RULE GH_RULES=0 GH_THREADS=1 GH_VIEW="$(view_line BLOCKED '')"
expect AC3-review-rule-yields-to-thread PENDING other-blockers "" \
  "an unresolved review thread is an agent's job; it outranks a named review rule" unresolved_threads=1

run_pf GH_CLASSIC=REVIEW_RULE GH_RULES=0 GH_VIEW="$(view_line DIRTY '' pass CONFLICTING)"
expect AC3-review-rule-yields-to-conflict PENDING other-blockers "" \
  "a conflict is resolved before a person is asked for a review" mergeable=CONFLICTING

run_pf GH_CLASSIC=REVIEW_RULE GH_RULES=0 GH_VIEW="$(view_line BEHIND '')"
expect AC3-review-rule-yields-to-behind PENDING other-blockers "" \
  "a branch behind its base is synced before a person is asked for a review"

run_pf GH_CLASSIC=REVIEW_RULE GH_RULES=0 GH_THREADS=unreadable GH_VIEW="$(view_line BLOCKED '')"
expect AC3-unreadable-threads-not-a-thread NOT_MET review-rule "" \
  "an unreadable thread count is not a thread: the named review rule still decides" unresolved_threads=unreadable

run_pf GH_CLASSIC=REVIEW_RULE GH_RULES=0 GH_VIEW="$(view_line BLOCKED CHANGES_REQUESTED)"
expect AC3-changes-requested-keeps-its-source NOT_MET review-decision "" \
  "GitHub's own decision outranks the rule names: requested changes are reported as the decision" \
  approval_blocker=changes-requested

# ---------------------------------------------------------------------------
# P1 — nothing the split touches became MET.
# ---------------------------------------------------------------------------
bad=""
for label in ${SPLIT_LABELS}; do
  case " ${MET_SEEN} " in *" ${label} "*) bad="${bad} ${label}" ;; esac
done
if [ -z "${bad}" ]; then
  pass P1-no-new-met "no case the UNREADABLE split reclassifies reports MET:${SPLIT_LABELS}"
else
  fail P1-no-new-met "the split made these MET:${bad}"
fi

# Every status and source printed is from the published vocabulary.
run_pf GH_CLASSIC=REVIEW_RULE GH_RULES=0 GH_VIEW="$(view_line BLOCKED '')"
case "$(jf approval_status)" in
  MET | NOT_MET | PENDING | UNREADABLE) pass VOCAB-status "approval_status stays one of MET, NOT_MET, PENDING, UNREADABLE" ;;
  *) fail VOCAB-status "approval_status is '$(jf approval_status)'" ;;
esac
case "$(jf approval_source)" in
  review-decision | required-count | merge-state | other-blockers | review-rule | facts-unreadable | unreadable)
    pass VOCAB-source "approval_source stays in the published vocabulary, which review-rule and facts-unreadable join" ;;
  *) fail VOCAB-source "approval_source is '$(jf approval_source)'" ;;
esac
case "$(jf approval_blocker)" in
  changes-requested | required-approval-missing | approval-pending-other-blockers \
    | approval-facts-unreadable | approval-state-unreadable)
    pass VOCAB-blocker "approval_blocker is one of the five ids the merge round knows" ;;
  *) fail VOCAB-blocker "approval_blocker is '$(jf approval_blocker)'" ;;
esac
run_pf GH_CLASSIC=0 GH_RULES=0 GH_VIEW="$(view_line CLEAN '')"
if [ -z "$(jf approval_blocker)" ]; then
  pass VOCAB-blocker-empty-on-met "a met approval names no blocker, so the round has nothing to report"
else
  fail VOCAB-blocker-empty-on-met "approval_blocker is '$(jf approval_blocker)' on a MET approval"
fi

echo ""
echo "passed: ${PASS_COUNT}  failed: ${FAIL_COUNT}"
[ "${FAIL_COUNT}" -eq 0 ] || exit 1
exit 0
