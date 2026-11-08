#!/usr/bin/env bash
# autodrive_light_round.sh - the light-mode check of an auto-drive-light crusty
# round (issues #1802 and #1982).
#
# Usage:
#   autodrive_light_round.sh check --record <abs path> --pr <N>
#       --reviewed-head <sha> --repo <dir>        (reads no stdin)
#
# Called by step-03 of autodrive-crusty-round.yaml (autodrive_light_step03),
# only for a crusty CONCERNS verdict and only when the state directory,
# `dirname <record>`, holds a `light-mode` marker that is a regular file.
# Without that marker the round never calls it.
#
# The tool:
#   1. validates the marker against light-eligibility.json and escalation.json;
#   2. measures the change again with autodrive_light_eligibility.sh when the
#      reviewed head differs from the marker head, from the merge-base of the
#      reviewed head and the PR's current base tip (gh pr view baseRefOid), so
#      a merge of the base branch into the PR does not count upstream changes.
#      When that base cannot be read, fetched or checked, it measures from the
#      pinned base, which is stricter. It escalates in the loop (marker
#      removed, escalation.json written) when the change is no longer light.
#
# It never changes the round's verdict, findings or count: since #1982 minor
# findings are classified, filed and gated only by autodrive_followups.sh. It
# never reads or writes <record>.findings and writes no file beside the record.
#
# Stdout is always exactly one JSON line:
#   {"light_exit":"off"}                       no marker
#   {"light_exit":"off","reason":"<token>"}    marker not accepted:
#       marker-invalid marker-stale state-unsafe
#   {"light_exit":"light"}                     marker valid; head unchanged or
#                                              measured again within limits
#   {"light_exit":"escalated","reason":"in-loop-<rule>"}
#       head measured again and no longer light; the reason always matches
#       ^in-loop-[a-z][a-z-]{0,54}$, otherwise it is in-loop-fault-unknown
#
# Exit 0 on any decision, 2 on bad arguments (nothing on stdout), and 79 when
# gh exits 79: that code is final and is passed through, never retried.
#
# State files are written under umask 077 through mktemp and mv -f. The only
# GitHub call is the read-only gh pr view of the PR's base. Logs carry fixed
# tokens, SHAs and digits only. Contract: docs/reference/auto-drive-light.md

set -uo pipefail
shopt -u patsub_replacement 2>/dev/null || true
export GIT_PAGER=cat GH_PAGER=cat PAGER=cat LESS=FRX
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE
umask 077

TOOLS="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"

usage() {
  echo "ERROR: autodrive_light_round: $1" >&2
  echo "usage: autodrive_light_round.sh check --record <abs path> --pr <N> --reviewed-head <sha> --repo <dir>" >&2
  exit 2
}

say() { printf '%s\n' "$1"; exit 0; }
# off <token>: the marker was not accepted; nothing was measured or written.
off() {
  echo "INFO: light round: light mode off for this round (reason=$1)" >&2
  say "{\"light_exit\":\"off\",\"reason\":\"$1\"}"
}

# --- arguments -----------------------------------------------------------------
[ "${1:-}" = check ] || usage "the only command is 'check'"
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

command -v jq >/dev/null 2>&1 || { echo "ERROR: light round: jq is not on PATH; the marker cannot be checked" >&2; off state-unsafe; }
if [ ! -d "$STATE" ] || ! private "$STATE"; then off state-unsafe; fi
private "$MARKER_FILE" || off state-unsafe
MHEAD="$(cat -- "$MARKER_FILE" 2>/dev/null)" || off marker-invalid
[[ "$MHEAD" =~ ^([0-9a-f]{40}|[0-9a-f]{64})$ ]] || off marker-invalid
{ [ -e "$ESC_FILE" ] || [ -L "$ESC_FILE" ]; } && off marker-invalid
{ [ -f "$ELIG_FILE" ] && [ ! -L "$ELIG_FILE" ]; } || off marker-invalid
private "$ELIG_FILE" || off state-unsafe
ELIG="$(cat -- "$ELIG_FILE")" || off marker-invalid
printf '%s' "$ELIG" | jq -e --arg h "$MHEAD" \
  'type == "object" and .decision == "light" and .head == $h and ((.base // "") | test("\\A([0-9a-f]{40}|[0-9a-f]{64})\\z"))' \
  >/dev/null 2>&1 || off marker-invalid
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

# --- gh helper -------------------------------------------------------------------
# Loaded once, for the re-measurement, which reads the PR's base tip. When it
# is missing the re-measurement uses the pinned base. PRIV holds the retry
# temporaries and is removed on every exit, 79 included.
PRIV="$(mktemp -d "${TMPDIR:-/tmp}/autodrive-light.XXXXXX")" || off state-unsafe
trap 'rm -rf -- "$PRIV"' EXIT
HELPER_MISSING=""
if [ -f "$TOOLS/workflow_gh_retry.sh" ]; then
  # shellcheck source=/dev/null
  . "$TOOLS/workflow_gh_retry.sh"
else
  HELPER_MISSING="workflow_gh_retry.sh"
fi

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
    ferr="$(GIT_TERMINAL_PROMPT=0 git -C "$REPO" fetch -q --no-tags origin "refs/heads/$name" </dev/null 2>&1 >/dev/null)" || true
    ferr="$(printf '%s' "$ferr" | head -n 1 | tr -d '[:cntrl:]' | cut -c 1-200)"
    git -C "$REPO" cat-file -e "${oid}^{commit}" 2>/dev/null \
      || { echo "INFO: light round: the PR's base tip could not be fetched (${ferr:-no error output}); measuring from the pinned base" >&2; return 1; }
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
  git -C "$REPO" merge-base --is-ancestor "$MHEAD" "$REVIEWED" >/dev/null 2>&1 || off marker-stale
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
  ERR="$(mktemp "${TMPDIR:-/tmp}/autodrive-light-measure.XXXXXX")" || off state-unsafe
  OUT="$(bash "$TOOLS/autodrive_light_eligibility.sh" --repo "$REPO" --base "$MBASE" --head "$REVIEWED" ${LIMITS[@]+"${LIMITS[@]}"} 2>"$ERR")"
  RC=$?
  FAULT="$(sed -n 's/^AUTODRIVE_LIGHT_ELIGIBILITY=fault reason=\([a-z-]*\)$/\1/p' "$ERR" | head -n 1)"
  rm -f -- "$ERR"
  LINE1="${OUT%%$'\n'*}"; JSON="${OUT#*$'\n'}"
  if [ "$RC" -eq 0 ] && [ "$LINE1" = "AUTODRIVE_LIGHT_ELIGIBILITY=light reason=within-limits" ]; then
    if ! printf '%s\n' "$JSON" | write_state light-eligibility.json \
      || ! printf '%s\n' "$REVIEWED" | write_state light-mode; then
      off state-unsafe
    fi
    echo "INFO: light round: head moved; measured again and still light" >&2
  else
    if [ "$RC" -eq 0 ] && [[ "$LINE1" =~ ^AUTODRIVE_LIGHT_ELIGIBILITY=escalate\ reason=([a-z-]+)$ ]]; then
      WHY="in-loop-${BASH_REMATCH[1]}"
    else
      WHY="in-loop-fault-${FAULT:-unknown}"; JSON='null'
    fi
    # S2: only a bounded token is written or printed.
    [[ "$WHY" =~ ^in-loop-[a-z][a-z-]{0,54}$ ]] || WHY="in-loop-fault-unknown"
    printf '%s' "$JSON" | jq -e 'type == "object"' >/dev/null 2>&1 || JSON='null'
    rm -f -- "$MARKER_FILE"
    jq -nc --arg r "$WHY" --arg b "$MBASE" --arg h "$REVIEWED" --argjson m "$JSON" \
      '{reason: $r, base_sha: $b, head_sha: $h, measurements: $m}' | write_state escalation.json \
      || echo "ERROR: light round: escalation.json could not be written" >&2
    echo "WARNING: light round: head moved and the change is no longer light (${WHY}); light mode ends for this state directory" >&2
    say "{\"light_exit\":\"escalated\",\"reason\":\"${WHY}\"}"
  fi
fi
say '{"light_exit":"light"}'
