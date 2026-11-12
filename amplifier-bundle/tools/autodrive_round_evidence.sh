#!/usr/bin/env bash
# autodrive_round_evidence.sh — the measured, deterministic steps of one merge
# round of auto-drive-to-merge (issue #1517, D4 and D5).
#
# These were step bodies of autodrive-merge-round.yaml until that recipe reached
# its brick budget (PR #1520 review); they moved here unchanged. The recipe's
# step-00-tools-dir finds this file once per round, with the tools beside it.
# crusty-range left the round in the next review: its caller now runs a crusty
# loop when it answers "true", and that loop must not start inside a round.
#
#   autodrive_round_evidence.sh crusty-range
#       Run by autodrive_crusty_rereview.sh before each merge round, from the
#       merge-ready loop's shell, never by a round step: does crusty re-review
#       the current head before the next round assesses anything? One JSON line:
#       {"rereview":"true|false","range":"...","first_unreviewed_sha":"...","base_sha":"..."}
#
#   autodrive_round_evidence.sh qa-evidence-sha <file>
#       step-00d-qa-evidence-hash. The hash of the qa evidence file as
#       autodrive_qa_evidence_sha computes it, or nothing when the file is not
#       a regular file.
#
#   autodrive_round_evidence.sh qa-evidence-hash <file>
#       step-00d-qa-evidence-hash itself: qa-evidence-sha as the step's one JSON
#       line, {"qa_evidence_sha":"..."}, empty unless whole 40- or 64-digit
#       lowercase hex. Moved here from the step at its brick budget (#1561).
#
#   autodrive_round_evidence.sh crusty-evidence
#       step-01b-crusty-evidence, criterion 3: autodrive_crusty_final (the
#       merge gate runs it too), then the range from the reviewed head to HEAD.
#       One JSON line:
#       {"crusty_status":"...","crusty_reason":"...","crusty_reviewed_head_sha":"...","crusty_first_unreviewed_sha":"..."}
#
#   autodrive_round_evidence.sh measured-downgrade
#       step-03-extract-merge-ready-verdict: every measurement that disagrees
#       with MERGE_READY, as `name=value ` tokens on one line, or `none` when
#       every one agrees. Missing evidence disagrees; it never passes.
#
#   autodrive_round_evidence.sh merge-ready-verdict <verdict> <source> < blockers.json
#       step-03 again, after it has parsed the review: applies measured-downgrade
#       to <verdict> (MERGE_READY or NOT_MERGE_READY) and writes the findings,
#       then prints the step's one JSON line:
#       {"merge_ready_verdict":"...","verdict_source":"...","blocker_count":N,"downgrade_reason":"..."}
#       It moved here from the step when the recipe reached its brick budget
#       (#1561); the review itself is still parsed in the step.
#
#   autodrive_round_evidence.sh findings [downgrade] < blockers.json
#       step-03 too: the recurring-findings signal. The `id` of each blocker
#       in the JSON array on stdin, one per line, and measured-evidence-disagrees
#       when a downgrade was given, written private to
#       AUTODRIVE_ROUND_RECORD.findings. Prints the number of lines.
#
#   autodrive_round_evidence.sh round-record
#       step-05-write-round-record: writes AUTODRIVE_ROUND_RECORD, private, as
#       one JSON line starting with {"merge_ready_verdict":", and prints it.
#       Carries held_by, which no gate reads: every value in it is a
#       measurement this tool took, so the record stays hash-bound evidence.
#
# Step outputs are read only as recipe-runner's RECIPE_VAR_<key> export:
# "${RECIPE_VAR_qa_evidence:-}" and the like. A bare QA_EVIDENCE, CI_EVIDENCE
# or MERGE_READY_VERDICT inherited from whatever launched the runner is never
# read (#1561; docs/reference/auto-drive-to-merge.md, "Step outputs ignore
# inherited bare variables").
# AUTODRIVE_STATE_DIR, AUTODRIVE_QA_EVIDENCE, AUTODRIVE_ROUND_RECORD,
# AUTODRIVE_ROUND_LABEL, PR_NUMBER and REPO_PATH come from there too.
# crusty-range, qa-evidence-sha and crusty-evidence print only fixed tokens and
# hex SHAs, and measured-downgrade keeps only [A-Za-z0-9_.:/-] of each value,
# so nothing from the branch under review reaches step-04's prompt through
# them as prose. round-record writes the step outputs' values as step-05 did.
#
# Executed, never sourced. autodrive_state.sh and autodrive_trust.sh are sourced
# from beside this file and from nowhere else. When they are missing, each
# subcommand reports what the step reported when it could not find them: no
# re-review, an empty hash, crusty UNTRUSTED, or qa_evidence=modified. None of
# those can merge.
#
# Exit 0 with the output above; exit 1 when round-record has no record path;
# exit 2 for an unknown subcommand.

set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" 2>/dev/null && pwd -P)"
HELPERS="false"
if [ -n "$HERE" ] && [ -f "$HERE/autodrive_state.sh" ] && [ -f "$HERE/autodrive_trust.sh" ]; then
  # shellcheck source=/dev/null
  . "$HERE/autodrive_state.sh" && . "$HERE/autodrive_trust.sh" && HELPERS="true"
fi

# The PR number from the environment, digits only, or nothing; `gh pr view`
# with no number reads the current branch's PR.
pr_base_ref() {
  local pr="${PR_NUMBER:-}"
  case "$pr" in *[!0-9]*) pr="" ;; esac
  gh pr view ${pr:+"$pr"} --json baseRefName --jq .baseRefName 2>/dev/null || printf ''
}

# field <json> <name> <default>: the canonical structured read
# (docs/reference/structured-verdict-parsing.md), the last JSON object that
# carries the field.
field() {
  printf '%s' "${1:-}" | amplihack orch helper extract-json --require-field "$2" \
    | amplihack orch helper extract-field --field "$2" --default "$3"
}

# token <value>: the value with every byte outside [A-Za-z0-9_.:/-] removed.
token() { printf '%s' "${1:-}" | LC_ALL=C tr -cd 'A-Za-z0-9_.:/-'; }

# hex_or_empty <value>: the value when it is 40 or 64 lowercase hex, else nothing.
hex_or_empty() {
  case "${#1}:${1}" in
    40:*[!0123456789abcdef]* | 64:*[!0123456789abcdef]*) ;;
    40:* | 64:*) printf '%s' "$1" ;;
  esac
}

crusty_range() {
  local no='{"rereview":"false","range":"not-checked","first_unreviewed_sha":"","base_sha":""}'
  local dir="${AUTODRIVE_STATE_DIR:-}" out
  if [ -z "$dir" ]; then
    echo "WARNING: no autodrive_state_dir was passed to this round; the range is not checked here." >&2
    printf '%s\n' "$no"; return 0
  fi
  if [ "$HELPERS" != "true" ]; then
    echo "WARNING: autodrive_state.sh and autodrive_trust.sh not found together beside autodrive_round_evidence.sh; the range is not checked here and step-01b reports it." >&2
    printf '%s\n' "$no"; return 0
  fi
  out="$(autodrive_rereview_decision "$dir" "$PWD" "$(pr_base_ref)")" || out=""
  case "$out" in
    '{"rereview":"'*) ;;
    *) echo "WARNING: autodrive_rereview_decision gave no decision; crusty is not re-run here." >&2; out="$no" ;;
  esac
  echo "INFO: crusty range at round start: $(printf '%s' "$out" | tr -cd 'A-Za-z0-9_:,{}"-')" >&2
  printf '%s\n' "$out"
}

qa_evidence_sha() {
  local sha=""
  if [ "$HELPERS" != "true" ]; then
    echo "WARNING: autodrive_trust.sh not found beside autodrive_round_evidence.sh; the qa evidence cannot be hashed." >&2
    return 0
  fi
  sha="$(autodrive_qa_evidence_sha "${1:-}")" || sha=""
  [ -z "$sha" ] || printf '%s\n' "$sha"
}

qa_evidence_hash() {
  local q
  q="$(hex_or_empty "$(qa_evidence_sha "${1:-}")")"
  [ -n "$q" ] || echo "WARNING: the qa evidence is not a regular file; qa_evidence_sha is empty, so this round cannot reach MERGE_READY." >&2
  printf '{"qa_evidence_sha":"%s"}\n' "$q"
}

crusty_evidence() {
  local dir="${AUTODRIVE_STATE_DIR:-}" token="crusty-loop-not-done" sha="" first="" status out head_now range
  if [ -z "$dir" ]; then
    echo "WARNING: no autodrive_state_dir was passed to this round; crusty evidence is ABSENT." >&2
  elif [ "$HELPERS" != "true" ]; then
    echo "WARNING: autodrive_state.sh and autodrive_trust.sh not found beside autodrive_round_evidence.sh; criterion 3 cannot be checked." >&2
    token="crusty-other"
  elif out="$(autodrive_crusty_final "$dir")"; then
    sha="$out"; token=""
    head_now="$(git rev-parse --verify --quiet HEAD 2>/dev/null || printf '')"
    # The base sync may have added a merge since crusty reviewed the head.
    range="$(autodrive_crusty_range "$PWD" "$sha" "$head_now" "$(autodrive_base_sha "$PWD" "$(pr_base_ref)")")"
    case "$range" in
      ok) ;;
      crusty-range-unreadable) token="crusty-range-unreadable" ;;
      crusty-unreviewed-commits:*) token="crusty-unreviewed-commits"; first="${range#*:}" ;;
      *) token="crusty-other" ;;
    esac
  else
    token="${out:-crusty-other}"
  fi
  case "$token" in
    '') status="DONE_CLEAN" ;; # a CLEAN or MINOR_ONLY final record (#1979)
    crusty-loop-not-done) status="ABSENT" ;;
    crusty-not-clean) status="NOT_CLEAN" ;;
    crusty-manifest-missing|crusty-record-missing|crusty-record-modified|crusty-head-sha-empty) status="UNTRUSTED" ;;
    crusty-unreviewed-commits|crusty-range-unreadable) status="UNREVIEWED_COMMITS" ;;
    *) status="UNTRUSTED"; token="crusty-other" ;;
  esac
  sha="$(hex_or_empty "$sha")"; first="$(hex_or_empty "$first")"
  [ -n "$sha" ] || [ "$status" != "DONE_CLEAN" ] || { status="UNTRUSTED"; token="crusty-other"; }
  [ -n "$first" ] || [ "$token" != "crusty-unreviewed-commits" ] || token="crusty-range-unreadable"
  echo "INFO: crusty evidence: status=${status} reason=${token:-none} reviewed_head_sha=${sha:-none} first_unreviewed_sha=${first:-none}" >&2
  printf '{"crusty_status":"%s","crusty_reason":"%s","crusty_reviewed_head_sha":"%s","crusty_first_unreviewed_sha":"%s"}\n' \
    "$status" "$token" "$sha" "$first"
}

# _measured_read: read the six measurements measured_downgrade compares and
# the two qa-evidence hashes, into MD_QA, MD_CI, MD_CONFLICT, MD_THREADS,
# MD_CRUSTY, MD_APPROVAL, MD_QH and MD_QNOW. One source of truth for the
# VALUES, so measured_downgrade and measured_held_by can never name the same
# condition with different values (#2219). It does NOT unify the set of
# conditions: each of those two functions still writes its own comparison per
# measurement, and a criterion added to one and not the other would give a
# stop line that contradicts the verdict beside it. Nothing in the code
# prevents that, so test-issue-2219-stop-line.sh drives both over a matrix,
# one condition at a time, and asserts they agree on which held the round.
# Every value passes token(), and the crusty and approval vocabularies stay
# closed, so nothing an agent wrote reaches a caller as prose.
MD_QA=""; MD_CI=""; MD_CONFLICT=""; MD_THREADS=""; MD_CRUSTY=""; MD_APPROVAL=""; MD_QH=""; MD_QNOW=""
_measured_read() {
  MD_QA="$(token "$(field "${RECIPE_VAR_qa_evidence:-}" qa_status MISSING)")"
  MD_CI="$(token "$(field "${RECIPE_VAR_ci_evidence:-}" ci_status MISSING)")"
  MD_CONFLICT="$(token "$(field "${RECIPE_VAR_merge_sync:-}" conflict unknown)")"
  MD_THREADS="$(token "$(field "${RECIPE_VAR_platform_facts:-}" unresolved_threads unreadable)")"
  MD_CRUSTY="$(field "${RECIPE_VAR_crusty_evidence:-}" crusty_status MISSING)"
  MD_APPROVAL="$(field "${RECIPE_VAR_platform_facts:-}" approval_status MISSING)"
  case "$MD_CRUSTY" in DONE_CLEAN|ABSENT|NOT_CLEAN|UNTRUSTED|UNREVIEWED_COMMITS|MISSING) ;; *) MD_CRUSTY="OTHER" ;; esac
  case "$MD_APPROVAL" in MET|NOT_MET|PENDING|UNREADABLE|MISSING) ;; *) MD_APPROVAL="OTHER" ;; esac
  # D5: the qa evidence must still be the file step-00d hashed before any agent ran.
  MD_QH="$(hex_or_empty "$(field "${RECIPE_VAR_qa_evidence_hash:-}" qa_evidence_sha '')")"
  MD_QNOW="$(qa_evidence_sha "${AUTODRIVE_QA_EVIDENCE:-}")"
}

measured_downgrade() {
  local d=""
  _measured_read
  [ "$MD_QA" = "PASS" ] || d="${d}qa_status=${MD_QA} "
  [ "$MD_CI" = "GREEN" ] || d="${d}ci_status=${MD_CI} "
  [ "$MD_CONFLICT" = "false" ] || d="${d}conflict=${MD_CONFLICT} "
  [ "$MD_THREADS" = "0" ] || d="${d}unresolved_threads=${MD_THREADS} "
  [ "$MD_CRUSTY" = "DONE_CLEAN" ] || d="${d}crusty_status=${MD_CRUSTY} "
  [ "$MD_APPROVAL" = "MET" ] || d="${d}approval_status=${MD_APPROVAL} "
  { [ -n "$MD_QH" ] && [ "$MD_QH" = "$MD_QNOW" ]; } || d="${d}qa_evidence=modified "
  printf '%s\n' "${d:-none}"
}

# measured_held_by: the same disagreements as measured_downgrade, as the
# condition lines the stop prints, joined by `|` (issue #2219). round_record
# is the only caller; there is no subcommand for it. Criterion
# numbers follow merge-ready/SKILL.md: qa 1, quality-audit 3, checks 4,
# approval 6, conflict 7, threads 10. The qa-evidence hash carries no
# criterion number: it is not a criterion, it is the evidence itself failing.
# Emitted in measured_downgrade's order, not in criterion order, so the two
# read the same way side by side.
#
# Prints `none` when every measurement agrees, as measured_downgrade does, so
# that an empty answer means this function could not run and is not mistaken
# for "nothing held it"; round_record maps the two apart.
#
# Derived only from measurements, never from the model's prose, so adding it
# to the round record leaves the record hash-bound evidence. No gate reads it.
measured_held_by() {
  local h=""
  _measured_read
  [ "$MD_QA" = "PASS" ] || h="${h}${h:+|}criterion 1: qa_status=${MD_QA}"
  [ "$MD_CI" = "GREEN" ] || h="${h}${h:+|}criterion 4: ci_status=${MD_CI}"
  [ "$MD_CONFLICT" = "false" ] || h="${h}${h:+|}criterion 7: conflict=${MD_CONFLICT}"
  [ "$MD_THREADS" = "0" ] || h="${h}${h:+|}criterion 10: unresolved_threads=${MD_THREADS}"
  [ "$MD_CRUSTY" = "DONE_CLEAN" ] || h="${h}${h:+|}criterion 3: crusty_status=${MD_CRUSTY}"
  [ "$MD_APPROVAL" = "MET" ] || h="${h}${h:+|}criterion 6: approval_status=${MD_APPROVAL} ($(_approval_detail))"
  { [ -n "$MD_QH" ] && [ "$MD_QH" = "$MD_QNOW" ]; } || h="${h}${h:+|}qa_evidence=modified"
  printf '%s\n' "${h:-none}"
}

# _approval_detail: the three platform facts behind a criterion 6 status that
# is not MET, as `source=…, required=…, merge_state=…` (issue #2219, Wanted 3).
# UNREADABLE alone is ambiguous: approval_source `facts-unreadable` means the
# measurement failed and the next round reads it again, `unreadable` means a
# rule only a person satisfies holds the merge. The stop line has to say
# which without anyone opening the round files. Read only when criterion 6
# holds the round, so an all-green round costs no extra read. Each value is
# from a closed vocabulary (the one autodrive_platform_facts.sh writes) or
# digits; anything else prints as OTHER, and an empty or missing count as
# unknown, so no agent-written byte reaches the line and it stays well inside
# the 120-byte cut autodrive_escalate.sh applies.
_approval_detail() {
  local src req ms
  src="$(field "${RECIPE_VAR_platform_facts:-}" approval_source unreadable)"
  req="$(field "${RECIPE_VAR_platform_facts:-}" required_approvals '')"
  ms="$(field "${RECIPE_VAR_platform_facts:-}" merge_state unknown)"
  case "$src" in
    review-decision|required-count|merge-state|other-blockers|review-rule|facts-unreadable|unreadable) ;;
    *) src="OTHER" ;;
  esac
  case "$req" in '') req="unknown" ;; *[!0-9]*) req="OTHER" ;; *) req="${req:0:6}" ;; esac
  case "$ms" in
    UNKNOWN|BEHIND|BLOCKED|CLEAN|DIRTY|DRAFT|HAS_HOOKS|UNSTABLE|unknown) ;;
    *) ms="OTHER" ;;
  esac
  printf 'source=%s, required=%s, merge_state=%s' "$src" "$req" "$ms"
}

round_findings() {
  local record="${AUTODRIVE_ROUND_RECORD:-}" blockers count=0
  blockers="$(cat)"
  if [ -n "$record" ]; then
    ( umask 077 # the state dir must stay private
      : > "${record}.findings"
      if ! command -v jq >/dev/null 2>&1; then
        echo "WARNING: jq unavailable; blocker ids cannot be extracted for the recurring-findings signal." >&2
      elif ! printf '%s' "$blockers" | jq -r '.[]?.id // empty' 2>/dev/null > "${record}.findings"; then
        echo "WARNING: blocker ids could not be parsed; the recurring-findings signal is incomplete this round." >&2
        : > "${record}.findings"
      fi
      [ -z "${1:-}" ] || printf 'measured-evidence-disagrees\n' >> "${record}.findings" )
    [ -f "${record}.findings" ] && count="$(grep -c . "${record}.findings" 2>/dev/null || true)"
  fi
  case "$count" in '' | *[!0-9]*) count=0 ;; esac
  printf '%s\n' "$count"
}

merge_ready_verdict() {
  local verdict="${1:-NOT_MERGE_READY}" source="${2:-missing_verdict}" blockers d downgrade count
  case "$verdict" in MERGE_READY|NOT_MERGE_READY) ;; *) verdict="NOT_MERGE_READY"; source="unparseable_verdict" ;; esac
  source="$(token "$source")"
  blockers="$(cat)"
  case "$blockers" in \[*\]) ;; *) blockers='[]' ;; esac
  # Measured evidence outranks the model: measured_downgrade names every
  # measurement that disagrees (qa, CI, conflict, threads, crusty, approval,
  # and the qa evidence hash step-00d took), or prints `none`. Missing
  # evidence, or no answer, disagrees.
  d="$(measured_downgrade)" || d=""
  case "$d" in
    none) downgrade="" ;;
    '') echo "WARNING: no measured evidence could be compared; it disagrees." >&2; downgrade="measured_evidence=unreadable " ;;
    *) downgrade="$d" ;;
  esac
  if [ -n "$downgrade" ] && [ "$verdict" = "MERGE_READY" ]; then
    echo "WARNING: downgrading MERGE_READY to NOT_MERGE_READY — measured evidence disagrees: ${downgrade}" >&2
    verdict="NOT_MERGE_READY"; source="evidence_downgrade"
  fi
  # The recurring-findings signal for the loop evaluator: blocker ids, and
  # measured-evidence-disagrees on a downgrade, in the record's .findings.
  count="$(printf '%s' "$blockers" | round_findings "$downgrade")"
  case "$count" in '' | *[!0-9]*) count=0 ;; esac
  echo "INFO: merge-ready ${AUTODRIVE_ROUND_LABEL:-round}: verdict=${verdict} (source=${source}) blockers=${count}" >&2
  printf '{"merge_ready_verdict":"%s","verdict_source":"%s","blocker_count":%s,"downgrade_reason":"%s"}\n' \
    "$verdict" "$source" "$count" "$(printf '%s' "$downgrade" | tr -d '"')"
}

round_record() {
  local record="${AUTODRIVE_ROUND_RECORD:-}" head="" verdict count qa ci_signal ci_status qh held label
  [ -n "$record" ] || { echo "ERROR: autodrive_round_record is required; the loop driver reads the round record from a file, never from stdout." >&2; return 1; }
  if (cd "${REPO_PATH:-.}" 2>/dev/null && git rev-parse --is-inside-work-tree >/dev/null 2>&1); then
    head="$(cd "${REPO_PATH:-.}" && git rev-parse HEAD 2>/dev/null || printf '')"
  else
    echo "[skip] not a git repo; the head sha is left empty so the merge gate refuses rather than merging unbound evidence" >&2
  fi
  verdict="$(field "${RECIPE_VAR_merge_ready_verdict:-}" merge_ready_verdict NOT_MERGE_READY)"
  case "$verdict" in MERGE_READY|NOT_MERGE_READY) ;; *) verdict="NOT_MERGE_READY" ;; esac
  count="$(field "${RECIPE_VAR_merge_ready_verdict:-}" blocker_count 0)"
  case "$count" in '' | *[!0-9]*) count=0 ;; esac
  qa="$(token "$(field "${RECIPE_VAR_qa_evidence:-}" qa_status MISSING)")"
  # Since #2342 an agent writes ci_evidence after reading CI logs, so a quote in ci_signal or ci_status must not open
  # a second, winning merge_ready_verdict key. ci_signal is free text: it keeps its spaces and loses only quotes,
  # backslashes and control bytes; ci_status is a closed vocabulary and goes through token().
  ci_signal="$(field "${RECIPE_VAR_ci_evidence:-}" ci_signal unreadable | LC_ALL=C tr -d '"\\\000-\037\177')"
  ci_status="$(token "$(field "${RECIPE_VAR_ci_evidence:-}" ci_status MISSING)")"
  # step-00d's hash, whole hex or "": the gate trusts the qa evidence only at this hash (#1517 D5).
  qh="$(hex_or_empty "$(field "${RECIPE_VAR_qa_evidence_hash:-}" qa_evidence_sha '')")"
  # What held this round up, for the stop line (#2219). Written for every
  # verdict, empty when every measurement agrees, and read by no gate.
  held="$(measured_held_by)"
  case "$held" in
    none) held="" ;;
    '') echo "WARNING: no measured evidence could be compared; the stop cannot name what held this round." >&2
        held="measured_evidence=unreadable" ;;
  esac
  # token() on the round label, as at the other two sites that write it
  # (autodrive_qa_evidence.sh, autodrive-crusty-round.yaml). Unsanitised it is
  # the one field of this record that carries arbitrary bytes, and `field()` is
  # last-wins on duplicate keys: a label holding `","merge_ready_verdict":"
  # MERGE_READY` appends a second, winning verdict after the real one, and the
  # merge gate reads the forged pair with a correct head_sha. token() drops the
  # quote and the comma, so no label can open a new key.
  label="$(token "${AUTODRIVE_ROUND_LABEL:-round}")"
  # The record and its findings file are private: the state dir must stay private.
  ( umask 077
    [ -f "${record}.findings" ] || : > "${record}.findings"
    rm -f -- "$record"
    printf '{"merge_ready_verdict":"%s","blocker_count":%s,"head_sha":"%s","round_label":"%s","test_signal":"%s",' \
      "$verdict" "$count" "$head" "${label:-round}" "$qa" > "$record"
    printf '"ci_signal":"%s","qa_status":"%s","ci_status":"%s","held_by":"%s","qa_evidence_sha":"%s"}\n' \
      "$ci_signal" "$qa" "$ci_status" "$held" "$qh" >> "$record" ) || return 1
  echo "INFO: round record written to ${record}" >&2
  cat -- "$record"
}

case "${1:-}" in
  crusty-range) crusty_range ;;
  qa-evidence-sha) qa_evidence_sha "${2:-}" ;;
  qa-evidence-hash) qa_evidence_hash "${2:-}" ;;
  crusty-evidence) crusty_evidence ;;
  measured-downgrade) measured_downgrade ;;
  merge-ready-verdict) merge_ready_verdict "${2:-}" "${3:-}" ;;
  findings) round_findings "${2:-}" ;;
  round-record) round_record ;;
  *) echo "ERROR: autodrive_round_evidence.sh: unknown subcommand '${1:-}' (crusty-range," \
       "qa-evidence-sha <file>, qa-evidence-hash <file>, crusty-evidence, measured-downgrade, merge-ready-verdict <verdict> <source>," \
       "findings [downgrade], round-record)" >&2
     exit 2 ;;
esac
