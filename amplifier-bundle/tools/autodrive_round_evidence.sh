#!/usr/bin/env bash
# autodrive_round_evidence.sh — the measured, deterministic steps of one merge
# round of auto-drive-to-merge (issue #1517, D4 and D5).
#
# These were step bodies of autodrive-merge-round.yaml until that recipe reached
# its brick budget (PR #1520 review); they moved here unchanged. The recipe's
# step-00-tools-dir finds this file once per round, with the tools beside it.
#
#   autodrive_round_evidence.sh crusty-range
#       step-00b-crusty-range. Does crusty re-review the current head before
#       this round assesses anything? One JSON line:
#       {"rereview":"true|false","range":"...","first_unreviewed_sha":"...","base_sha":"..."}
#
#   autodrive_round_evidence.sh qa-evidence-sha <file>
#       step-00d-qa-evidence-hash. The hash of the qa evidence file as
#       autodrive_qa_evidence_sha computes it, or nothing when the file is not
#       a regular file.
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
#   autodrive_round_evidence.sh findings [downgrade] < blockers.json
#       step-03 too: the recurring-findings signal. The `id` of each blocker
#       in the JSON array on stdin, one per line, and measured-evidence-disagrees
#       when a downgrade was given, written private to
#       AUTODRIVE_ROUND_RECORD.findings. Prints the number of lines.
#
#   autodrive_round_evidence.sh round-record
#       step-05-write-round-record: writes AUTODRIVE_ROUND_RECORD, private, as
#       one JSON line starting with {"merge_ready_verdict":", and prints it.
#
# Step outputs are read from the environment the way recipe-runner exports
# them: "${QA_EVIDENCE:-${RECIPE_VAR_qa_evidence:-}}" and the like (see
# docs/reference/auto-drive-to-merge.md, "How a step output reaches bash").
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
    '') status="DONE_CLEAN" ;;
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

measured_downgrade() {
  local qa ci conflict threads crusty approval qh qnow d=""
  qa="$(token "$(field "${QA_EVIDENCE:-${RECIPE_VAR_qa_evidence:-}}" qa_status MISSING)")"
  ci="$(token "$(field "${CI_EVIDENCE:-${RECIPE_VAR_ci_evidence:-}}" ci_status MISSING)")"
  conflict="$(token "$(field "${MERGE_SYNC:-${RECIPE_VAR_merge_sync:-}}" conflict unknown)")"
  threads="$(token "$(field "${PLATFORM_FACTS:-${RECIPE_VAR_platform_facts:-}}" unresolved_threads unreadable)")"
  crusty="$(field "${CRUSTY_EVIDENCE:-${RECIPE_VAR_crusty_evidence:-}}" crusty_status MISSING)"
  approval="$(field "${PLATFORM_FACTS:-${RECIPE_VAR_platform_facts:-}}" approval_status MISSING)"
  case "$crusty" in DONE_CLEAN|ABSENT|NOT_CLEAN|UNTRUSTED|UNREVIEWED_COMMITS|MISSING) ;; *) crusty="OTHER" ;; esac
  case "$approval" in MET|NOT_MET|PENDING|UNREADABLE|MISSING) ;; *) approval="OTHER" ;; esac
  # D5: the qa evidence must still be the file step-00d hashed before any agent ran.
  qh="$(hex_or_empty "$(field "${QA_EVIDENCE_HASH:-${RECIPE_VAR_qa_evidence_hash:-}}" qa_evidence_sha '')")"
  qnow="$(qa_evidence_sha "${AUTODRIVE_QA_EVIDENCE:-}")"
  [ "$qa" = "PASS" ] || d="${d}qa_status=${qa} "
  [ "$ci" = "GREEN" ] || d="${d}ci_status=${ci} "
  [ "$conflict" = "false" ] || d="${d}conflict=${conflict} "
  [ "$threads" = "0" ] || d="${d}unresolved_threads=${threads} "
  [ "$crusty" = "DONE_CLEAN" ] || d="${d}crusty_status=${crusty} "
  [ "$approval" = "MET" ] || d="${d}approval_status=${approval} "
  { [ -n "$qh" ] && [ "$qh" = "$qnow" ]; } || d="${d}qa_evidence=modified "
  printf '%s\n' "${d:-none}"
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

round_record() {
  local record="${AUTODRIVE_ROUND_RECORD:-}" head="" verdict count qa ci_signal ci_status qh
  [ -n "$record" ] || { echo "ERROR: autodrive_round_record is required; the loop driver reads the round record from a file, never from stdout." >&2; return 1; }
  if (cd "${REPO_PATH:-.}" 2>/dev/null && git rev-parse --is-inside-work-tree >/dev/null 2>&1); then
    head="$(cd "${REPO_PATH:-.}" && git rev-parse HEAD 2>/dev/null || printf '')"
  else
    echo "[skip] not a git repo; the head sha is left empty so the merge gate refuses rather than merging unbound evidence" >&2
  fi
  verdict="$(field "${MERGE_READY_VERDICT:-${RECIPE_VAR_merge_ready_verdict:-}}" merge_ready_verdict NOT_MERGE_READY)"
  case "$verdict" in MERGE_READY|NOT_MERGE_READY) ;; *) verdict="NOT_MERGE_READY" ;; esac
  count="$(field "${MERGE_READY_VERDICT:-${RECIPE_VAR_merge_ready_verdict:-}}" blocker_count 0)"
  case "$count" in '' | *[!0-9]*) count=0 ;; esac
  qa="$(field "${QA_EVIDENCE:-${RECIPE_VAR_qa_evidence:-}}" qa_status MISSING)"
  ci_signal="$(field "${CI_EVIDENCE:-${RECIPE_VAR_ci_evidence:-}}" ci_signal unreadable)"
  ci_status="$(field "${CI_EVIDENCE:-${RECIPE_VAR_ci_evidence:-}}" ci_status MISSING)"
  # step-00d's hash, whole hex or "": the gate trusts the qa evidence only at this hash (#1517 D5).
  qh="$(hex_or_empty "$(field "${QA_EVIDENCE_HASH:-${RECIPE_VAR_qa_evidence_hash:-}}" qa_evidence_sha '')")"
  # The record and its findings file are private: the state dir must stay private.
  ( umask 077
    [ -f "${record}.findings" ] || : > "${record}.findings"
    rm -f -- "$record"
    printf '{"merge_ready_verdict":"%s","blocker_count":%s,"head_sha":"%s","round_label":"%s","test_signal":"%s",' \
      "$verdict" "$count" "$head" "${AUTODRIVE_ROUND_LABEL:-round}" "$qa" > "$record"
    printf '"ci_signal":"%s","qa_status":"%s","ci_status":"%s","qa_evidence_sha":"%s"}\n' \
      "$ci_signal" "$qa" "$ci_status" "$qh" >> "$record" ) || return 1
  echo "INFO: round record written to ${record}" >&2
  cat -- "$record"
}

case "${1:-}" in
  crusty-range) crusty_range ;;
  qa-evidence-sha) qa_evidence_sha "${2:-}" ;;
  crusty-evidence) crusty_evidence ;;
  measured-downgrade) measured_downgrade ;;
  findings) round_findings "${2:-}" ;;
  round-record) round_record ;;
  *) echo "ERROR: autodrive_round_evidence.sh: unknown subcommand '${1:-}' (crusty-range," \
       "qa-evidence-sha <file>, crusty-evidence, measured-downgrade, findings [downgrade], round-record)" >&2
     exit 2 ;;
esac
