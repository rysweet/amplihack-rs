#!/usr/bin/env bash
# autodrive_light_round.sh - the minor-only rule of an auto-drive-light crusty
# round (issue #1802).
#
# Usage:
#   autodrive_light_round.sh defer --record <abs path> --pr <N>
#       --reviewed-head <sha> --repo <dir>        (concerns JSON array on stdin)
#
# Called by step-03 of autodrive-crusty-round.yaml, and only when the state
# directory, `dirname <record>`, holds a `light-mode` marker that is a regular
# file. Without that marker the round never calls it, and its output is the
# same as before #1802.
#
# The tool:
#   1. validates the marker against light-eligibility.json and escalation.json;
#   2. measures the change again with autodrive_light_eligibility.sh when the
#      reviewed head differs from the marker head, from the merge-base of the
#      reviewed head and the PR's current base tip (gh pr view baseRefOid), so
#      a merge of the base branch into the PR does not count upstream changes.
#      When that base cannot be read, fetched or checked, it measures from the
#      pinned base, which is stricter. It escalates in the loop (marker
#      removed, escalation.json written) when the change is no longer light;
#   3. works out each concern's effective severity, failing closed;
#   4. when every effective severity is minor (and there is at least one
#      concern), files or reuses the follow-up issue, posts only ids not yet
#      posted, links the issue from the PR once, writes <record>.concerns.tsv
#      and <record>.light.json, and truncates <record>.findings.
#
# Stdout is always one JSON line:
#   {"light_exit":"off"}                       no marker
#   {"light_exit":"none","reason":"<token>"}   marker present, rule not applied:
#       no-concerns not-minor-only marker-invalid marker-stale state-unsafe
#       re-measured-escalate follow-up-failed
#   {"light_exit":"minor-deferred","follow_up_issue":"N","deferred_ids":[...],"concern_count":K}
#
# Exit 0 on any decision, 2 on bad arguments (nothing on stdout), and 79 when
# gh exits 79: that code is final and is passed through, never retried.
#
# Agent text reaches gh only through --body-file. Every cell is sanitised
# (docs/reference/auto-drive-light.md#cell-sanitising). State files are written
# under umask 077 through mktemp and mv -f. Logs carry counts, ids, digits and
# fixed tokens only. Contract: docs/reference/auto-drive-light.md

set -uo pipefail
shopt -u patsub_replacement 2>/dev/null || true
export GIT_PAGER=cat GH_PAGER=cat PAGER=cat LESS=FRX
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE
umask 077

TOOLS="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
KEYWORDS='sec|auth|secret|token|credential|inject|xss|csrf|cve|vuln|unsafe|privilege|perm'
ZW=$'\xe2\x80\x8b'

usage() {
  echo "ERROR: autodrive_light_round: $1" >&2
  echo "usage: autodrive_light_round.sh defer --record <abs path> --pr <N> --reviewed-head <sha> --repo <dir>" >&2
  exit 2
}

say() { printf '%s\n' "$1"; exit 0; }
none() {
  echo "INFO: light round: rule not applied (reason=$1)" >&2
  say "{\"light_exit\":\"none\",\"reason\":\"$1\"}"
}

# --- arguments -----------------------------------------------------------------
[ "${1:-}" = defer ] || usage "the only command is 'defer'"
shift
RECORD=""; PR=""; REVIEWED=""; REPO=""
while [ $# -gt 0 ]; do
  case "$1" in
    --record|--pr|--reviewed-head|--repo) [ $# -ge 2 ] || usage "$1 needs a value" ;;
    *) usage "unknown option" ;;
  esac
  case "$1" in
    --record) RECORD="$2" ;;
    --pr) PR="$2" ;;
    --reviewed-head) REVIEWED="$2" ;;
    --repo) REPO="$2" ;;
  esac
  shift 2
done
[[ "$PR" =~ ^[0-9]{1,10}$ ]] || usage "--pr must be 1 to 10 digits"
[[ "$REVIEWED" =~ ^([0-9a-f]{40}|[0-9a-f]{64})$ ]] || usage "--reviewed-head must be a 40 or 64 character lowercase hex SHA"
case "$RECORD" in /*) ;; *) usage "--record must be an absolute path" ;; esac
case "/$RECORD/" in */../*|*/./*) usage "--record must not contain . or .. components" ;; esac
[[ "$RECORD" =~ [[:cntrl:]] ]] && usage "--record contains a control character"
[[ "${RECORD##*/}" =~ ^[A-Za-z0-9._-]+\.json$ ]] || usage "--record must name a .json round record"
[ -n "$REPO" ] && [ -d "$REPO" ] || usage "--repo must be a directory"

STATE="${RECORD%/*}"
REC_NAME="${RECORD##*/}"
CONCERNS="$(cat)"

# --- marker --------------------------------------------------------------------
MARKER_FILE="$STATE/light-mode"
ELIG_FILE="$STATE/light-eligibility.json"
ESC_FILE="$STATE/escalation.json"
if [ -L "$MARKER_FILE" ] || [ ! -f "$MARKER_FILE" ]; then
  say '{"light_exit":"off"}'
fi

# private <path>: a regular file or directory owned by this user, not a
# symlink, with no group or world write bit.
private() {
  local loose
  [ ! -L "$1" ] && [ -O "$1" ] || return 1
  loose="$(find "$1" -maxdepth 0 \( -perm -0020 -o -perm -0002 \) -print 2>/dev/null)" || return 1
  [ -z "$loose" ]
}

command -v jq >/dev/null 2>&1 || { echo "ERROR: light round: jq is not on PATH; the marker cannot be checked" >&2; none state-unsafe; }
if [ ! -d "$STATE" ] || ! private "$STATE"; then none state-unsafe; fi
private "$MARKER_FILE" || none state-unsafe
MHEAD="$(cat -- "$MARKER_FILE" 2>/dev/null)" || none marker-invalid
[[ "$MHEAD" =~ ^([0-9a-f]{40}|[0-9a-f]{64})$ ]] || none marker-invalid
{ [ -e "$ESC_FILE" ] || [ -L "$ESC_FILE" ]; } && none marker-invalid
{ [ -f "$ELIG_FILE" ] && [ ! -L "$ELIG_FILE" ]; } || none marker-invalid
private "$ELIG_FILE" || none state-unsafe
ELIG="$(cat -- "$ELIG_FILE")" || none marker-invalid
printf '%s' "$ELIG" | jq -e --arg h "$MHEAD" \
  'type == "object" and .decision == "light" and .head == $h and ((.base // "") | test("\\A([0-9a-f]{40}|[0-9a-f]{64})\\z"))' \
  >/dev/null 2>&1 || none marker-invalid
LBASE="$(printf '%s' "$ELIG" | jq -r '.base')"

# write_state <name>: stdin to <state>/<name> through mktemp and mv -f, so a
# symlink planted at the target is replaced, not followed.
write_state() {
  local target="$STATE/$1" tmp
  [ ! -d "$target" ] || return 1
  tmp="$(mktemp "$STATE/.$1.XXXXXX" 2>/dev/null)" || return 1
  if cat > "$tmp" && mv -f -- "$tmp" "$target"; then return 0; fi
  rm -f -- "$tmp"
  return 1
}

# --- gh helpers -------------------------------------------------------------------
# Loaded once, before the re-measurement, which reads the PR's base tip. A
# missing helper only matters where gh is needed: the re-measurement then uses
# the pinned base, and the follow-up step reports follow-up-failed.
PRIV="$(mktemp -d "${TMPDIR:-/tmp}/autodrive-light.XXXXXX")" || none state-unsafe
trap 'rm -rf -- "$PRIV"' EXIT
HELPER_MISSING=""
for helper in workflow_gh_retry.sh workflow_issue_tracking.sh; do
  if [ -f "$TOOLS/$helper" ]; then
    # shellcheck source=/dev/null
    . "$TOOLS/$helper"
  else
    HELPER_MISSING="$helper"
  fi
done

failed() {
  printf '%s\n' "$1" | write_state follow-up-failed \
    || echo "ERROR: light round: follow-up-failed could not be written" >&2
  echo "WARNING: light round: follow-up step failed (${1}); the concerns are addressed in this round instead" >&2
  none follow-up-failed
}

# gh_call <label> <gh args...>: through _gh_retry_core; exit 79 is final.
gh_call() {
  local out rc
  out="$(GH_RETRY_TMPDIR="$PRIV" _gh_retry_core "$@")"
  rc=$?
  if [ "$rc" -eq 79 ]; then
    echo "ERROR: light round: gh exited 79; that code is final and is not retried" >&2
    exit 79
  fi
  [ "$rc" -eq 0 ] || return "$rc"
  printf '%s' "$out"
}

# current_base: prints the merge-base of the reviewed head and the PR's current
# base tip, when that merge-base descends from the pinned base. Returns 1, with
# one INFO line, in every other case; the caller then keeps the pinned base.
current_base() {
  local js rc name oid mb
  [ -z "$HELPER_MISSING" ] || { echo "INFO: light round: $HELPER_MISSING is missing; measuring from the pinned base" >&2; return 1; }
  js="$(cd -- "$REPO" && gh_call "pr view" pr view "$PR" --json baseRefName,baseRefOid)"
  rc=$?
  [ "$rc" -ne 79 ] || exit 79
  [ "$rc" -eq 0 ] || { echo "INFO: light round: the PR's base could not be read; measuring from the pinned base" >&2; return 1; }
  name="$(printf '%s' "$js" | jq -r '.baseRefName // "" | strings' 2>/dev/null)"
  oid="$(printf '%s' "$js" | jq -r '.baseRefOid // "" | strings' 2>/dev/null)"
  if ! [[ "$oid" =~ ^([0-9a-f]{40}|[0-9a-f]{64})$ ]] || [ -z "$name" ] || [ "${name#-}" != "$name" ] \
    || ! git -C "$REPO" check-ref-format --branch "$name" >/dev/null 2>&1; then
    echo "INFO: light round: the PR's base name or tip is not valid; measuring from the pinned base" >&2
    return 1
  fi
  if ! git -C "$REPO" cat-file -e "${oid}^{commit}" 2>/dev/null; then
    GIT_TERMINAL_PROMPT=0 git -C "$REPO" fetch -q --no-tags origin "refs/heads/$name" </dev/null >/dev/null 2>&1 || true
    git -C "$REPO" cat-file -e "${oid}^{commit}" 2>/dev/null \
      || { echo "INFO: light round: the PR's base tip could not be fetched; measuring from the pinned base" >&2; return 1; }
  fi
  mb="$(git -C "$REPO" merge-base "$REVIEWED" "$oid" 2>/dev/null)"
  if ! [[ "$mb" =~ ^([0-9a-f]{40}|[0-9a-f]{64})$ ]] \
    || ! git -C "$REPO" merge-base --is-ancestor "$LBASE" "$mb" >/dev/null 2>&1; then
    echo "INFO: light round: the merge-base with the PR's base tip does not descend from the pinned base; measuring from the pinned base" >&2
    return 1
  fi
  printf '%s' "$mb"
}

# --- measure again when the head moved -------------------------------------------
if [ "$REVIEWED" != "$MHEAD" ]; then
  git -C "$REPO" merge-base --is-ancestor "$MHEAD" "$REVIEWED" >/dev/null 2>&1 || none marker-stale
  MBASE="$(current_base)"
  rc=$?
  [ "$rc" -ne 79 ] || exit 79
  [ "$rc" -eq 0 ] && [ -n "$MBASE" ] || MBASE="$LBASE"
  LIMITS=()
  for pair in lines:--max-lines new_modules:--max-new-modules public_api:--max-public-api \
              schema:--max-schema recipe_contract:--max-recipe-contract; do
    v="$(printf '%s' "$ELIG" | jq -r --arg k "${pair%%:*}" '.limits[$k] // empty | tostring' 2>/dev/null)"
    [[ "$v" =~ ^[0-9]{1,6}$ ]] && LIMITS+=("${pair#*:}" "$v")
  done
  ERR="$(mktemp "${TMPDIR:-/tmp}/autodrive-light-measure.XXXXXX")" || none state-unsafe
  OUT="$(bash "$TOOLS/autodrive_light_eligibility.sh" --repo "$REPO" --base "$MBASE" --head "$REVIEWED" ${LIMITS[@]+"${LIMITS[@]}"} 2>"$ERR")"
  RC=$?
  FAULT="$(sed -n 's/^AUTODRIVE_LIGHT_ELIGIBILITY=fault reason=\([a-z-]*\)$/\1/p' "$ERR" | head -n 1)"
  rm -f -- "$ERR"
  LINE1="${OUT%%$'\n'*}"; JSON="${OUT#*$'\n'}"
  if [ "$RC" -eq 0 ] && [ "$LINE1" = "AUTODRIVE_LIGHT_ELIGIBILITY=light reason=within-limits" ]; then
    if ! printf '%s\n' "$JSON" | write_state light-eligibility.json \
      || ! printf '%s\n' "$REVIEWED" | write_state light-mode; then
      none state-unsafe
    fi
    echo "INFO: light round: head moved; measured again and still light" >&2
    MHEAD="$REVIEWED"
  else
    if [ "$RC" -eq 0 ] && [[ "$LINE1" =~ ^AUTODRIVE_LIGHT_ELIGIBILITY=escalate\ reason=([a-z-]+)$ ]]; then
      WHY="in-loop-${BASH_REMATCH[1]}"
    else
      WHY="in-loop-fault-${FAULT:-unknown}"; JSON='null'
    fi
    printf '%s' "$JSON" | jq -e 'type == "object"' >/dev/null 2>&1 || JSON='null'
    rm -f -- "$MARKER_FILE"
    jq -nc --arg r "$WHY" --arg b "$MBASE" --arg h "$REVIEWED" --argjson m "$JSON" \
      '{reason: $r, base_sha: $b, head_sha: $h, measurements: $m}' | write_state escalation.json \
      || echo "ERROR: light round: escalation.json could not be written" >&2
    echo "WARNING: light round: head moved and the change is no longer light (${WHY}); light mode ends for this state directory" >&2
    none re-measured-escalate
  fi
fi

# --- effective severity ----------------------------------------------------------
printf '%s' "$CONCERNS" | jq -e 'type == "array"' >/dev/null 2>&1 || none not-minor-only
[ "$(printf '%s' "$CONCERNS" | jq 'length')" != 0 ] || none no-concerns

# Returning ids: resolved-concerns.txt and the .findings of earlier records.
# Light rounds empty their .findings, so a non-empty one is from a round whose
# step-04 addressed every concern.
RETURNING=""
for f in "$STATE/resolved-concerns.txt" "$STATE"/crusty-round-*.json.findings; do
  [ "$f" != "$RECORD.findings" ] && [ -f "$f" ] && [ ! -L "$f" ] || continue
  RETURNING="${RETURNING}$(cat -- "$f")"$'\n'
done

ROWS="$(printf '%s' "$CONCERNS" | jq -c --arg ret "$RETURNING" --arg kw "$KEYWORDS" '
  ($ret | split("\n") | map(select(length > 0))) as $r
  | def clean: (if type == "string" then . elif . == null then "" else tojson end)
      | gsub("[[:cntrl:]]"; " ") | .[0:200];
  map(
    (if type == "object" then . else {} end) as $c
    | ($c.id) as $id
    | (($id | type) == "string" and ($id | test("\\A[A-Za-z0-9._-]{1,64}\\z"))) as $okid
    | ($c.severity) as $sev
    | (($sev | type) == "string" and ($sev == "minor" or $sev == "major" or $sev == "blocking")) as $known
    | {
        id: (if $okid then $id else "" end),
        eff: (if ($known | not) or ($okid | not) then "major"
              elif $sev != "minor" then $sev
              elif any($r[]; . == $id) then "major"
              elif ($id | ascii_downcase | test($kw)) then "major"
              else "minor" end),
        summary: ($c.summary | clean),
        evidence: ($c.evidence | clean)
      })')" || none not-minor-only
COUNT="$(printf '%s' "$ROWS" | jq 'length')"
NON_MINOR="$(printf '%s' "$ROWS" | jq '[.[] | select(.eff != "minor")] | length')"
if [ "$NON_MINOR" != 0 ]; then
  echo "INFO: light round: ${NON_MINOR} of ${COUNT} concern(s) are not minor after the fail-closed rules" >&2
  none not-minor-only
fi
IDS_JSON="$(printf '%s' "$ROWS" | jq -c 'reduce .[].id as $x ([]; if index([$x]) then . else . + [$x] end)')"
IDS=()
while IFS= read -r l; do IDS+=("$l"); done < <(printf '%s' "$IDS_JSON" | jq -r '.[]')

# --- follow-up issue ---------------------------------------------------------------
[ -z "$HELPER_MISSING" ] || { echo "ERROR: light round: $HELPER_MISSING is missing from $TOOLS" >&2; none follow-up-failed; }

GH_REPO_NAME=""
repo_name() {
  [ -z "$GH_REPO_NAME" ] || return 0
  local url re
  url="$(cd -- "$REPO" && gh_call "pr view" pr view "$PR" --json url --jq .url)"
  [ $? -ne 79 ] || exit 79
  re="^https://github\\.com/([A-Za-z0-9-]+/[A-Za-z0-9._-]+)/pull/${PR}\$"
  [[ "$url" =~ $re ]] || return 1
  GH_REPO_NAME="${BASH_REMATCH[1]}"
}

cell() {
  local s
  s="$(sanitize_cli_output "$1")"
  s="${s//&/&amp;}"; s="${s//</&lt;}"; s="${s//>/&gt;}"
  s="${s//@/@$ZW}"
  s="$(printf '%s' "$s" | sed -E -e "s/#([0-9])/#${ZW}\\1/g" -e "s#(github\\.com)/#\\1/${ZW}#Ig")"
  s="${s//|/&#x7C;}"; s="${s//\`/&#x60;}"
  printf '%s' "$s"
}

# table <id>...: the Markdown table of the named concerns.
table() {
  local id
  printf '| id | severity | summary | evidence |\n| --- | --- | --- | --- |\n'
  for id in "$@"; do
    printf '%s' "$ROWS" | jq -r --arg id "$id" 'first(.[] | select(.id == $id)) | [.summary, .evidence] | join("\u001f")' \
      | while IFS=$'\x1f' read -r summary evidence; do
          printf '| %s | minor | %s | %s |\n' "$id" "$(cell "$summary")" "$(cell "$evidence")"
        done
  done
}

MARK="<!-- auto-drive-light:pr-${PR} -->"
ISSUE=""; CREATED=0
f="$STATE/follow-up-issue"
if [ -f "$f" ] && [ ! -L "$f" ]; then
  n="$(cat -- "$f")"
  if [[ "$n" =~ ^[0-9]{1,10}$ ]]; then ISSUE="$n"; else echo "WARNING: light round: the recorded follow-up issue number is not 1 to 10 digits; ignored" >&2; fi
fi

if [ -z "$ISSUE" ]; then
  repo_name || failed pr-repository
  ME="$(gh_call "api user" api user --jq .login)"
  rc=$?; [ "$rc" -ne 79 ] || exit 79; [ "$rc" -eq 0 ] || ME=""
  if [[ "$ME" =~ ^[A-Za-z0-9-]{1,39}$ ]]; then
    HITS="$(gh_call "issue list" issue list --repo "$GH_REPO_NAME" --state open --limit 20 \
      --search "auto-drive-light:pr-${PR} in:body" --json number,state,author,body)"
    rc=$?; [ "$rc" -ne 79 ] || exit 79; [ "$rc" -eq 0 ] || HITS='[]'
    n="$(printf '%s' "$HITS" | jq -r --arg me "$ME" --arg m "$MARK" '
      [ (if type == "array" then .[] else empty end)
        | select((.state // "") == "OPEN" and (.author.login // "") == $me
                 and ((.body // "") | type == "string" and (split($m) | length) == 2))
        | .number | tostring ] | first // empty' 2>/dev/null)"
    if [[ "$n" =~ ^[0-9]{1,10}$ ]]; then
      ISSUE="$n"
      echo "INFO: light round: reusing open follow-up issue #${ISSUE} found by its marker" >&2
      printf '%s\n' "$ISSUE" | write_state follow-up-issue || failed state-write
    fi
  else
    echo "WARNING: light round: the gh login could not be read; no existing follow-up issue is reused" >&2
  fi
fi

if [ -z "$ISSUE" ]; then
  repo_name || failed pr-repository
  { printf '%s\nMinor review concerns raised by crusty on PR #%s and deferred by the auto-drive-light minor-only rule.\n\n' "$MARK" "$PR"
    table ${IDS[@]+"${IDS[@]}"}; } > "$PRIV/follow-up-body.md" || failed body
  LABEL=()
  NAMES="$(gh_call "label list" label list --repo "$GH_REPO_NAME" --limit 500 --json name --jq '.[].name')"
  rc=$?; [ "$rc" -ne 79 ] || exit 79
  if [ "$rc" -eq 0 ] && printf '%s\n' "$NAMES" | grep -qx 'follow-up'; then
    LABEL=(--label follow-up)
  fi
  URL="$(gh_call "issue create" issue create --repo "$GH_REPO_NAME" \
    --title "Follow-up: minor review concerns from PR #${PR}" --body-file "$PRIV/follow-up-body.md" ${LABEL[@]+"${LABEL[@]}"})"
  rc=$?
  [ "$rc" -ne 79 ] || exit 79
  [ "$rc" -eq 0 ] || failed issue-create
  rm -f -- "$PRIV/follow-up-body.md"
  URL="${URL##*$'\n'}"
  re="^https://github\\.com/${GH_REPO_NAME//./\\.}/issues/([0-9]{1,10})\$"
  [[ "$URL" =~ $re ]] || failed issue-url
  ISSUE="${BASH_REMATCH[1]}"
  printf '%s\n' "$ISSUE" | write_state follow-up-issue || failed state-write
  CREATED=1
  echo "INFO: light round: filed follow-up issue #${ISSUE} with ${#IDS[@]} concern(s)" >&2
fi

POSTED=()
f="$STATE/follow-up-posted-ids"
# Only lines that are valid ids count; anything else is ignored, so a junk
# line cannot stop an id from being posted.
if [ -f "$f" ] && [ ! -L "$f" ]; then
  while IFS= read -r l; do POSTED+=("$l"); done < <(grep -E '^[A-Za-z0-9._-]{1,64}$' -- "$f")
fi
NEW=()
if [ "$CREATED" = 1 ]; then
  POSTED+=(${IDS[@]+"${IDS[@]}"})
else
  for id in ${IDS[@]+"${IDS[@]}"}; do
    seen=0
    for p in ${POSTED[@]+"${POSTED[@]}"}; do [ "$p" = "$id" ] && { seen=1; break; }; done
    [ "$seen" = 1 ] || NEW+=("$id")
  done
fi
if [ "${#NEW[@]}" -gt 0 ]; then
  repo_name || failed pr-repository
  { printf 'Further minor review concerns raised by crusty on PR #%s and deferred by the auto-drive-light minor-only rule.\n\n' "$PR"
    table "${NEW[@]}"; } > "$PRIV/follow-up-comment.md" || failed body
  gh_call "issue comment" issue comment "$ISSUE" --repo "$GH_REPO_NAME" --body-file "$PRIV/follow-up-comment.md" >/dev/null
  rc=$?
  [ "$rc" -ne 79 ] || exit 79
  [ "$rc" -eq 0 ] || failed issue-comment
  POSTED+=("${NEW[@]}")
  echo "INFO: light round: posted ${#NEW[@]} new concern(s) to follow-up issue #${ISSUE}" >&2
fi
printf '%s\n' ${POSTED[@]+"${POSTED[@]}"} | awk 'NF && !seen[$0]++' | write_state follow-up-posted-ids || failed state-write

f="$STATE/follow-up-linked"
LINKED=""
if [ -f "$f" ] && [ ! -L "$f" ]; then LINKED="$(cat -- "$f")"; fi
if [ "$LINKED" != "$ISSUE" ]; then
  repo_name || failed pr-repository
  printf 'Minor review concerns deferred to #%s.\n' "$ISSUE" > "$PRIV/follow-up-link.md" || failed body
  gh_call "pr comment" pr comment "$PR" --repo "$GH_REPO_NAME" --body-file "$PRIV/follow-up-link.md" >/dev/null
  rc=$?
  [ "$rc" -ne 79 ] || exit 79
  [ "$rc" -eq 0 ] || failed pr-comment
  printf '%s\n' "$ISSUE" | write_state follow-up-linked || failed state-write
fi

# --- side files: the round's record of the deferral ------------------------------
printf '%s' "$ROWS" | jq -r '.[] | [.id, .eff, .summary] | join("\t")' | write_state "$REC_NAME.concerns.tsv" \
  || failed state-write
jq -nc --arg n "$ISSUE" --argjson ids "$IDS_JSON" '{follow_up_issue: $n, deferred_ids: $ids}' \
  | write_state "$REC_NAME.light.json" || failed state-write
write_state "$REC_NAME.findings" < /dev/null || failed state-write
rm -f -- "$STATE/follow-up-failed"
echo "INFO: light round: ${COUNT} minor concern(s) deferred to #${ISSUE}" >&2
jq -nc --arg n "$ISSUE" --argjson ids "$IDS_JSON" --argjson c "$COUNT" \
  '{light_exit: "minor-deferred", follow_up_issue: $n, deferred_ids: $ids, concern_count: $c}'
