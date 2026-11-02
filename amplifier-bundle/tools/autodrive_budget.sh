#!/usr/bin/env bash
# autodrive_budget.sh: the per-run usage budget of auto-drive (#1743).
# docs/reference/auto-drive-to-merge.md#budget-keys-and-defaults.
#
#   mark <state>
#       prints the number of rows in <state>/autodrive-usage.tsv.
#   add <state> <loop> <label> <phase> <seconds> [--since <mark>]
#       appends one row: loop, round_label, phase (before|round|evaluator),
#       sessions, seconds, tokens. With --since, the rows appended after
#       <mark> (a nested loop's) are subtracted, so each second and each
#       session is counted once. Then rewrites autodrive-usage.json.
#   totals <state>
#       rewrites autodrive-usage.json from the rows.
#   check <state> <loop> <pre|post|before> <label> <pr> [--force] [key=value...]
#       compares the recorded use with the limits. Exits 10, prints the keys
#       reached and writes autodrive-budget-summary.md when a limit is
#       reached (or with --force: a nested loop reached one). Exits 0
#       otherwise. A malformed limit, an unknown autodrive_budget_* key or an
#       untrusted usage file exits 2 (fails closed).
#   post <state> <pr> <file>
#       posts <file>, sanitised, as a PR comment with `gh pr comment`. A
#       failure is a WARNING; it never changes the loop result.
#
# A limit is reached when it is above 0 and the use is equal to or above it;
# 0 or empty is unlimited. All round-count comparisons of auto-drive live here.
# Values are validated as ^[0-9]{1,12}$ and read in base 10 before any
# arithmetic; rows are summed in awk; no state is ever run as code.
# tokens is `unknown`: no agent CLI prints a token count the driver can read,
# and an unknown count is never recorded as 0.

set -uo pipefail
umask 077
export LC_ALL=C

usage() { echo "ERROR: autodrive_budget.sh: ${1:-usage}. Usage: mark <state> | add <state> <loop> <label> <phase> <seconds> [--since <mark>] | totals <state> | check <state> <loop> <pre|post|before> <label> <pr> [--force] [key=value...] | post <state> <pr> <file>" >&2; exit 2; }

CMD="${1:-}"
[ $# -gt 0 ] && shift
case "$CMD" in
  mark|totals) [ $# -eq 1 ] || usage "${CMD} takes 1 argument" ;;
  add) [ $# -eq 5 ] || [ $# -eq 7 ] || usage "add takes 5 or 7 arguments" ;;
  check) [ $# -ge 5 ] || usage "check takes at least 5 arguments" ;;
  post) [ $# -eq 3 ] || usage "post takes 3 arguments" ;;
  *) usage "unknown command '${CMD}'" ;;
esac
STATE="$1"
[ -d "$STATE" ] && [ ! -L "$STATE" ] || usage "state dir '${STATE}' is not a directory"
USAGE="${STATE}/autodrive-usage.tsv"
SUMMARY="${STATE}/autodrive-budget-summary.md"

budget_int() { [[ "$1" =~ ^[0-9]{1,12}$ ]] && printf '%s' "$((10#$1))"; }
safe() { printf '%s' "$1" | tr -c 'A-Za-z0-9._-' '_'; }

# trusted <file>: absent, or a regular file of ours with no group/other access.
trusted() {
  [ -e "$1" ] || [ -L "$1" ] || return 0
  [ -f "$1" ] && [ ! -L "$1" ] && [ -O "$1" ] || return 1
  local out # a find error is untrusted, never "no match" (fails closed)
  out="$(find "$1" -maxdepth 0 \( -perm -0040 -o -perm -0020 -o -perm -0004 -o -perm -0002 \) -print)" || return 1
  [ -z "$out" ]
}
require_trusted() {
  trusted "$USAGE" && return 0
  echo "ERROR: ${USAGE} is a symlink, not a regular file of this user, or readable by others; the budget cannot be read, so no agent starts. Inspect and remove it." >&2
  exit 2
}

# write_atomic <file> <content>: mktemp beside it, then mv -f over it.
write_atomic() {
  local t
  t="$(mktemp "${STATE}/.budget.XXXXXX")" && printf '%s' "$2" > "$t" && mv -f -- "$t" "$1" && return 0
  rm -f -- "${t:-}"; echo "WARNING: could not write $1." >&2; return 1
}

# rows [skip]: the valid rows after the first <skip>; malformed ones are
# skipped with a WARNING. Output: loop phase sessions seconds tokens.
rows() {
  [ -f "$USAGE" ] || return 0
  awk -F'\t' -v skip="${1:-0}" '
    NR <= skip { next }
    NF == 6 && $1 ~ /^[A-Za-z0-9._-]+$/ && $3 ~ /^(before|round|evaluator)$/ \
      && $4 ~ /^[0-9]+$/ && length($4) <= 12 && $5 ~ /^[0-9]+$/ && length($5) <= 12 \
      && ($6 == "unknown" || ($6 ~ /^[0-9]+$/ && length($6) <= 12)) {
      print $1, $3, $4 + 0, $5 + 0, $6; next }
    { bad++ }
    END { if (bad) printf "WARNING: skipped %d malformed row(s) in autodrive-usage.tsv\n", bad > "/dev/stderr" }' "$USAGE"
}

# totals_json: the totals by loop and overall, as one JSON line.
totals_json() {
  rows | awk '
    { if (!($1 in seen)) { seen[$1] = 1; order[++n] = $1 }
      if ($2 == "round") { r[$1]++; R++ }
      s[$1] += $3; S += $3; w[$1] += $4; W += $4
      if ($5 == "unknown") unk = 1; else T += $5 }
    END {
      printf "{\"loops\":{"
      for (i = 1; i <= n; i++) { l = order[i]
        printf "%s\"%s\":{\"rounds\":%d,\"sessions\":%d,\"seconds\":%d}", (i > 1 ? "," : ""), l, r[l], s[l], w[l] }
      printf "},\"total\":{\"rounds\":%d,\"sessions\":%d,\"seconds\":%d,\"tokens\":", R, S, W
      if (unk || NR == 0) printf "\"unknown\"}}\n"; else printf "%d}}\n", T }'
}

case "$CMD" in
  mark)
    require_trusted
    if [ -f "$USAGE" ]; then wc -l < "$USAGE" | tr -cd '0-9'; else printf '0'; fi
    echo; exit 0 ;;
  totals)
    require_trusted
    write_atomic "${STATE}/autodrive-usage.json" "$(totals_json)"; exit $? ;;
  add)
    require_trusted
    LOOP="$(safe "$2")"; LABEL="$(safe "$3")"; PHASE="$4"
    case "$PHASE" in before|round|evaluator) ;; *) usage "phase must be before, round or evaluator" ;; esac
    SECS="$(budget_int "$5")" || usage "seconds must be an integer, not '$5'"
    SESSIONS=1
    if [ $# -eq 7 ]; then
      [ "$6" = "--since" ] || usage "unknown option '$6'"
      MARK="$(budget_int "$7")" || usage "mark must be an integer, not '$7'"
      read -r N_SESS N_SECS <<<"$(rows "$MARK" | awk '{ s += $3; w += $4 } END { printf "%d %d\n", s, w }')"
      [ "${N_SESS:-0}" -gt 0 ] && SESSIONS=0
      SECS=$(( SECS > ${N_SECS:-0} ? SECS - ${N_SECS:-0} : 0 ))
    fi
    ROW="$(printf '%s\t%s\t%s\t%s\t%s\tunknown' "$LOOP" "$LABEL" "$PHASE" "$SESSIONS" "$SECS")"
    # One write on an O_APPEND descriptor, under flock (10 s at most) where it exists.
    if ! { if command -v flock >/dev/null 2>&1; then flock -w 10 9 || echo "WARNING: lock on ${USAGE} not taken in 10 s; appending without it." >&2; fi; printf '%s\n' "$ROW" >&9; } 9>>"$USAGE"; then
      echo "WARNING: could not append a usage row to ${USAGE}." >&2; exit 1
    fi
    write_atomic "${STATE}/autodrive-usage.json" "$(totals_json)"; exit $? ;;
  post)
    PR="$2"; BODY_IN="$3"
    [[ "$PR" =~ ^[0-9]{1,10}$ ]] || { echo "INFO: no valid pr_number; the stop is not posted as a PR comment." >&2; exit 0; }
    [ -f "$BODY_IN" ] || { echo "WARNING: comment body ${BODY_IN} is missing; nothing posted." >&2; exit 1; }
    command -v gh >/dev/null 2>&1 || { echo "WARNING: gh is not on PATH; the stop is not posted on PR #${PR}." >&2; exit 1; }
    # @mentions and #N cross-links neutralised, absolute paths cut to base
    # names, fences unclosable; the text sits inside one fenced block.
    TEXT="$(head -c 59000 -- "$BODY_IN" | tr -d '\r' \
      | sed -E 's/```/'"'''"'/g; s/~~~/'"'''"'/g; s/@/(at)/g; s/#([0-9])/# \1/g' \
      | sed -E "s#(^|[[:space:](\"'=:\`])/([^[:space:]/]+/)+#\\1#g")"
    BODY="$(printf 'auto-drive stopped a loop. The record of the run is below.\n\n```text\n%s\n```\n' "$TEXT")"
    F="$(mktemp "${STATE}/.comment.XXXXXX")" || { echo "WARNING: could not create the comment body file; nothing posted on PR #${PR}." >&2; exit 1; }
    printf '%s' "$BODY" | head -c 60000 > "$F"
    RUN=(gh pr comment "$PR" --body-file "$F")
    command -v setsid >/dev/null 2>&1 && RUN=(setsid -w "${RUN[@]}")
    command -v timeout >/dev/null 2>&1 && RUN=(timeout 900 "${RUN[@]}") # a hung post only; no real post comes near it (#439)
    # gh stderr can name the host or account: it goes to a 0600 file in the
    # private state dir, never into the WARNING (exit code only).
    ERRLOG="${STATE}/comment-error.log"
    ERR="$(GH_PROMPT_DISABLED=1 GH_NO_UPDATE_NOTIFIER=1 "${RUN[@]}" </dev/null 2>&1 >/dev/null)"
    RC=$?
    rm -f -- "$F"
    if [ "$RC" -ne 0 ]; then
      if rm -f -- "$ERRLOG" && printf '%s\n' "$ERR" | head -c 4096 > "$ERRLOG"; then
        echo "WARNING: gh pr comment on PR #${PR} exited ${RC}; the loop result is unchanged. Its error output is in ${ERRLOG}." >&2
      else
        echo "WARNING: gh pr comment on PR #${PR} exited ${RC}; the loop result is unchanged. Its error output could not be saved." >&2
      fi
      exit 1
    fi
    echo "INFO: the stop is posted as a comment on PR #${PR}." >&2
    exit 0 ;;
esac

# --- check ------------------------------------------------------------------
LOOP="$(safe "$2")"; POINT="$3"; LABEL=""; PR="$5"
[ "$4" = "?" ] || LABEL="$(safe "$4")"
shift 5
case "$POINT" in
  pre) POINT_TEXT="pre-round check" ;; post) POINT_TEXT="post-round check" ;;
  before) POINT_TEXT="before-round step" ;; *) usage "check point must be pre, post or before" ;;
esac
FORCE=0; L_LOOP=0; L_TOTAL=0; L_WALL=0
for a in "$@"; do
  case "$a" in
    --force) FORCE=1; continue ;;
    autodrive_budget_loop_rounds=*|autodrive_budget_total_rounds=*|autodrive_budget_wall_seconds=*) ;;
    *) echo "ERROR: autodrive_budget.sh: unknown budget key '${a%%=*}'; only autodrive_budget_loop_rounds, autodrive_budget_total_rounds and autodrive_budget_wall_seconds are accepted." >&2; exit 2 ;;
  esac
  k="${a%%=*}"; v="${a#*=}"
  if [ -n "$v" ]; then
    v="$(budget_int "$v")" || { echo "ERROR: ${k} must be empty or 0 to 12 digits (base 10), not '$(printf '%s' "${a#*=}" | tr -c 'A-Za-z0-9._-' '_')'." >&2; exit 2; }
  else v=0; fi
  case "$k" in
    autodrive_budget_loop_rounds) L_LOOP="$v" ;;
    autodrive_budget_total_rounds) L_TOTAL="$v" ;;
    autodrive_budget_wall_seconds) L_WALL="$v" ;;
  esac
done
require_trusted

# Use: rounds of this loop, of crusty and merge-ready, of all; seconds; sessions.
read -r U_LOOP U_CRUSTY U_MERGE U_TOTAL U_WALL U_SESS U_TOK <<<"$(rows | awk -v me="$LOOP" '
  $2 == "round" { t++; if ($1 == me) m++; if ($1 == "crusty") c++; if ($1 == "merge-ready") g++ }
  { w += $4; s += $3; if ($5 == "unknown") unk = 1; else tok += $5 }
  END { printf "%d %d %d %d %d %d %s\n", m, c, g, t, w, s, ((unk || NR == 0) ? "unknown" : tok) }')"
reached() { [ "$1" -gt 0 ] && [ "$2" -ge "$1" ]; }
HIT=()
reached "$L_LOOP" "$U_LOOP" && HIT+=(autodrive_budget_loop_rounds)
reached "$L_TOTAL" "$U_TOTAL" && HIT+=(autodrive_budget_total_rounds)
reached "$L_WALL" "$U_WALL" && HIT+=(autodrive_budget_wall_seconds)
if [ "${#HIT[@]}" -eq 0 ]; then
  [ "$FORCE" = 1 ] || exit 0
  # The nested crusty loop stopped on a limit of its own loop.
  reached "$L_LOOP" "$U_CRUSTY" && HIT+=(autodrive_budget_loop_rounds)
fi

lim() { if [ "$1" -gt 0 ]; then printf '%s' "$1"; else printf 'none'; fi; }
TABLE="| crusty rounds | ${U_CRUSTY} | $(lim "$L_LOOP") |
| merge-ready rounds | ${U_MERGE} | $(lim "$L_LOOP") |"
case "$LOOP" in crusty|merge-ready) ;; *) TABLE="${TABLE}
| ${LOOP} rounds | ${U_LOOP} | $(lim "$L_LOOP") |" ;; esac
OPEN="none"
FINDINGS="${STATE}/${LOOP}-${LABEL}.json.findings"
if [ -n "$LABEL" ] && [ -f "$FINDINGS" ] && [ ! -L "$FINDINGS" ] && [ -s "$FINDINGS" ]; then
  OPEN="$(head -c 8192 -- "$FINDINGS" | tr -d '\r' | tr '\001-\010\013-\037\177' '_' | sed "s/\`\`\`/'''/g")"
fi
NOT_DONE=""
for l in crusty merge-ready; do
  if [ "$l" = "$LOOP" ] || ! grep -q '"loop_result":"DONE"' "${STATE}/${l}-result.json" 2>/dev/null; then
    NOT_DONE="${NOT_DONE:+${NOT_DONE}, }${l}"
  fi
done
PR_ARG="<pr>"; [[ "$PR" =~ ^[0-9]{1,10}$ ]] && PR_ARG="$PR"
RESUME="amplihack recipe run auto-drive-to-merge -c pr_number=${PR_ARG}"
for k in ${HIT[@]+"${HIT[@]}"}; do
  case "$k" in
    autodrive_budget_loop_rounds) RESUME="${RESUME} -c ${k}=$((L_LOOP * 2))" ;;
    autodrive_budget_total_rounds) RESUME="${RESUME} -c ${k}=$((L_TOTAL * 2))" ;;
    autodrive_budget_wall_seconds) RESUME="${RESUME} -c ${k}=$((L_WALL * 2))" ;;
  esac
done
HIT_TEXT="$(IFS=,; printf '%s' "${HIT[*]:-}" | sed 's/,/, /g')"
[ -n "$HIT_TEXT" ] || HIT_TEXT="a limit of the nested crusty loop (see its escalate line)"

write_atomic "$SUMMARY" "# Auto-drive usage budget reached

Stopped: loop \`${LOOP}\` at round label \`${LABEL:-?}\` (${POINT_TEXT}).
Limits reached: ${HIT_TEXT}

| Measure | Used | Limit |
| --- | --- | --- |
${TABLE}
| total rounds | ${U_TOTAL} | $(lim "$L_TOTAL") |
| wall seconds | ${U_WALL} | $(lim "$L_WALL") |
| sessions | ${U_SESS} | none |
| tokens | ${U_TOK} | none |

## Open items of ${LABEL:-?}

\`\`\`text
${OPEN}
\`\`\`

## Remaining work

- Loops without a DONE result: ${NOT_DONE}
- The open items above

## Resume

${RESUME}
"
printf '%s\n' "${HIT_TEXT}"
exit 10
