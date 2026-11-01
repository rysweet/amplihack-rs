#!/usr/bin/env bash
# autodrive_light_eligibility.sh - measure whether a change is small enough for
# the auto-drive-light recipe (issue #1802).
#
# Usage:
#   autodrive_light_eligibility.sh --repo <dir> --base <rev> --head <rev>
#       [--max-lines N] [--max-new-modules N] [--max-public-api N]
#       [--max-schema N] [--max-recipe-contract N]
#
# A stateless measurement of <base>..<head> with git plumbing only. It writes
# nothing: not to the repository, not to a state directory. The caller records
# the decision.
#
# Rules, in the order used for `reason` (docs/reference/auto-drive-light.md):
#
#   lines            added + deleted lines from --numstat; Cargo.lock, *.lock
#                    and package-lock.json excluded; binary files count 0
#   new-module       added crates/*/src/**/*.rs, any added Cargo.toml, added
#                    amplifier-bundle/recipes/*.yaml, any added file under
#                    amplifier-bundle/tools/
#   public-api       added or removed lines in crates/*/src/lib.rs matching
#                    ^\s*pub[\s(], except pub(crate), pub(super), pub(in ...)
#   schema           changed *.schema.json files and files under a schemas/ dir
#   recipe-contract  modified amplifier-bundle/recipes/*.yaml whose top-level
#                    context: key set differs between base and head
#
# Each rule escalates when its measurement is above its limit; equal passes.
# Limits must match ^[0-9]{1,6}$ and are read as base 10.
#
# Output on a decision (exit 0), two lines on stdout:
#   AUTODRIVE_LIGHT_ELIGIBILITY=<light|escalate> reason=<token>
#   {"decision":...,"reason":...,"base":...,"head":...,"lines":N,...,"limits":{...}}
#
# A fault exits 2, prints nothing on stdout, and prints
#   AUTODRIVE_LIGHT_ELIGIBILITY=fault reason=<bad-arg|not-a-repo|bad-rev|not-ancestor|jq-missing>
# on stderr.
#
# Revisions are resolved with rev-parse --verify --end-of-options, the diff
# runs with --no-ext-diff --no-textconv, path lists are read with -z, and
# every pathspec follows --. Nothing is passed through eval.

set -uo pipefail
export GIT_PAGER=cat PAGER=cat LESS=FRX LC_ALL=C
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_EXTERNAL_DIFF

fault() {
  [ -z "${2:-}" ] || echo "ERROR: autodrive_light_eligibility: $2" >&2
  echo "AUTODRIVE_LIGHT_ELIGIBILITY=fault reason=$1" >&2
  exit 2
}

REPO=""; BASE=""; HEAD=""
HAVE_REPO=0; HAVE_BASE=0; HAVE_HEAD=0
MAX_LINES=300; MAX_NEW=0; MAX_API=0; MAX_SCHEMA=0; MAX_CONTRACT=0

limit() {
  [[ "$2" =~ ^[0-9]{1,6}$ ]] || fault bad-arg "$1 must match ^[0-9]{1,6}$"
  printf '%s' "$((10#$2))"
}

rev_arg() {
  case "$2" in
    '') fault bad-arg "$1 is empty" ;;
    -*) fault bad-arg "$1 must not begin with '-'" ;;
  esac
  [[ "$2" =~ [[:cntrl:]] ]] && fault bad-arg "$1 contains a control character"
  return 0
}

while [ $# -gt 0 ]; do
  opt="$1"
  case "$opt" in
    --repo|--base|--head|--max-lines|--max-new-modules|--max-public-api|--max-schema|--max-recipe-contract)
      [ $# -ge 2 ] || fault bad-arg "$opt needs a value"
      val="$2"; shift 2 ;;
    *) fault bad-arg "unknown option" ;;
  esac
  case "$opt" in
    --repo) REPO="$val"; HAVE_REPO=1 ;;
    --base) rev_arg --base "$val"; BASE="$val"; HAVE_BASE=1 ;;
    --head) rev_arg --head "$val"; HEAD="$val"; HAVE_HEAD=1 ;;
    --max-lines) MAX_LINES="$(limit "$opt" "$val")" || exit 2 ;;
    --max-new-modules) MAX_NEW="$(limit "$opt" "$val")" || exit 2 ;;
    --max-public-api) MAX_API="$(limit "$opt" "$val")" || exit 2 ;;
    --max-schema) MAX_SCHEMA="$(limit "$opt" "$val")" || exit 2 ;;
    --max-recipe-contract) MAX_CONTRACT="$(limit "$opt" "$val")" || exit 2 ;;
  esac
done
[ "$HAVE_REPO" = 1 ] && [ "$HAVE_BASE" = 1 ] && [ "$HAVE_HEAD" = 1 ] \
  || fault bad-arg "--repo, --base and --head are required"
[ -n "$REPO" ] || fault bad-arg "--repo is empty"

command -v jq >/dev/null 2>&1 || fault jq-missing "jq is not on PATH"

[ -d "$REPO" ] || fault not-a-repo "--repo is not a directory"
G=(git -C "$REPO" -c core.fsmonitor=false -c core.quotepath=false)
[ "$("${G[@]}" rev-parse --is-inside-work-tree 2>/dev/null)" = "true" ] \
  || fault not-a-repo "--repo is not a git work tree"

resolve() {
  "${G[@]}" rev-parse --verify --quiet --end-of-options "$1^{commit}" 2>/dev/null
}
B="$(resolve "$BASE")" && [ -n "$B" ] || fault bad-rev "--base does not resolve to a commit"
H="$(resolve "$HEAD")" && [ -n "$H" ] || fault bad-rev "--head does not resolve to a commit"
"${G[@]}" merge-base --is-ancestor "$B" "$H" 2>/dev/null
case $? in
  0) ;;
  1) fault not-ancestor "--base is not an ancestor of --head" ;;
  *) fault bad-rev "git merge-base failed" ;;
esac

DIFF=(diff --no-ext-diff --no-textconv --no-renames --no-color)
# A diff that fails must not read as an empty, light change.
"${G[@]}" "${DIFF[@]}" --numstat -z "$B" "$H" -- >/dev/null 2>&1 || fault bad-rev "git diff failed for base..head"

# --- lines -------------------------------------------------------------------
LINES=0
while IFS= read -r -d '' rec; do
  added="${rec%%$'\t'*}"; rest="${rec#*$'\t'}"
  deleted="${rest%%$'\t'*}"; path="${rest#*$'\t'}"
  case "${path##*/}" in
    Cargo.lock|*.lock|package-lock.json) continue ;;
  esac
  # Binary files print "-" for both counts.
  [[ "$added" =~ ^[0-9]+$ ]] || added=0
  [[ "$deleted" =~ ^[0-9]+$ ]] || deleted=0
  LINES=$((LINES + 10#$added + 10#$deleted))
done < <("${G[@]}" "${DIFF[@]}" --numstat -z "$B" "$H" -- 2>/dev/null)

# --- new modules -------------------------------------------------------------
NEW=0
while IFS= read -r -d '' path; do
  if [[ "$path" =~ ^crates/[^/]+/src/.+\.rs$ ]] \
     || [ "${path##*/}" = "Cargo.toml" ] \
     || [[ "$path" =~ ^amplifier-bundle/recipes/[^/]+\.yaml$ ]] \
     || [[ "$path" == amplifier-bundle/tools/* ]]; then
    NEW=$((NEW + 1))
  fi
done < <("${G[@]}" "${DIFF[@]}" --name-only -z --diff-filter=A "$B" "$H" -- 2>/dev/null)

# --- public API --------------------------------------------------------------
API="$("${G[@]}" "${DIFF[@]}" -U0 "$B" "$H" -- ':(glob)crates/*/src/lib.rs' 2>/dev/null | awk '
  /^diff --git / { hunk = 0; next }
  /^@@/ { hunk = 1; next }
  hunk && /^[+-]/ {
    s = substr($0, 2)
    if (s ~ /^[ \t]*pub[ \t(]/ && s !~ /^[ \t]*pub[ \t]*\([ \t]*(crate|super|in[ \t])/) n++
  }
  END { print n + 0 }')"
[[ "$API" =~ ^[0-9]+$ ]] || API=0

# --- schema ------------------------------------------------------------------
SCHEMA=0
while IFS= read -r -d '' path; do
  if [[ "$path" == *.schema.json ]] || [[ "$path" == schemas/* ]] || [[ "$path" == */schemas/* ]]; then
    SCHEMA=$((SCHEMA + 1))
  fi
done < <("${G[@]}" "${DIFF[@]}" --name-only -z "$B" "$H" -- 2>/dev/null)

# --- recipe contract ---------------------------------------------------------
# context_keys <rev> <path>: the sorted top-level keys of the recipe's
# `context:` mapping (block style), one per line.
context_keys() {
  "${G[@]}" cat-file blob "$1:$2" 2>/dev/null | awk '
    /^context:[ \t]*(#.*)?$/ { inctx = 1; ind = -1; next }
    inctx {
      if ($0 ~ /^[ \t]*(#.*)?$/) next
      if ($0 !~ /^[ \t]/) { inctx = 0; next }
      match($0, /^[ \t]+/); i = RLENGTH
      if (ind < 0) ind = i
      if (i != ind) next
      line = substr($0, i + 1)
      if (match(line, /^[^:#\t ][^:]*[ \t]*:/)) {
        k = substr(line, 1, RLENGTH - 1); sub(/[ \t]+$/, "", k); print k
      }
    }' | sort -u
}
CONTRACT=0
while IFS= read -r -d '' path; do
  if [ "$(context_keys "$B" "$path")" != "$(context_keys "$H" "$path")" ]; then
    CONTRACT=$((CONTRACT + 1))
  fi
done < <("${G[@]}" "${DIFF[@]}" --name-only -z --diff-filter=M "$B" "$H" -- ':(glob)amplifier-bundle/recipes/*.yaml' 2>/dev/null)

# --- decision ----------------------------------------------------------------
DECISION=light; REASON=within-limits
if [ "$LINES" -gt "$MAX_LINES" ]; then REASON=lines
elif [ "$NEW" -gt "$MAX_NEW" ]; then REASON=new-module
elif [ "$API" -gt "$MAX_API" ]; then REASON=public-api
elif [ "$SCHEMA" -gt "$MAX_SCHEMA" ]; then REASON=schema
elif [ "$CONTRACT" -gt "$MAX_CONTRACT" ]; then REASON=recipe-contract
fi
[ "$REASON" = within-limits ] || DECISION=escalate

JSON="$(jq -nc --arg d "$DECISION" --arg r "$REASON" --arg b "$B" --arg h "$H" \
  --argjson lines "$LINES" --argjson nm "$NEW" --argjson api "$API" \
  --argjson schema "$SCHEMA" --argjson rc "$CONTRACT" \
  --argjson ml "$MAX_LINES" --argjson mnm "$MAX_NEW" --argjson mapi "$MAX_API" \
  --argjson ms "$MAX_SCHEMA" --argjson mrc "$MAX_CONTRACT" \
  '{decision: $d, reason: $r, base: $b, head: $h, lines: $lines, new_modules: $nm,
    public_api: $api, schema: $schema, recipe_contract: $rc,
    limits: {lines: $ml, new_modules: $mnm, public_api: $mapi, schema: $ms, recipe_contract: $mrc}}')" \
  || fault jq-missing "jq could not encode the measurements"
printf 'AUTODRIVE_LIGHT_ELIGIBILITY=%s reason=%s\n%s\n' "$DECISION" "$REASON" "$JSON"
