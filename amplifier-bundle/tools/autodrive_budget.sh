#!/usr/bin/env bash
# autodrive_budget.sh: the per-run usage budget of auto-drive (#1743).
# docs/reference/auto-drive-to-merge.md#budget-keys-and-defaults.
#
#   mark <state>
#       prints the number of rows in <state>/autodrive-usage.tsv.
#   add <state> <loop> <label> <phase> <seconds> [--since <mark>] [--waited <ledger>]
#       appends one row: loop, round_label, phase (before|round|evaluator),
#       sessions, seconds, tokens. With --since, the rows appended after
#       <mark> (a nested loop's, wait rows included) are subtracted, so each
#       second and each session is counted once. With --waited, the seconds
#       the child spent waiting for a usage limit to lift (one integer per
#       line of <ledger>, written by the wait through
#       AMPLIHACK_USAGE_WAIT_LEDGER) are taken out of the row and appended as
#       a second row of phase `wait` with 0 sessions. The wall budget leaves
#       wait rows out. Then rewrites autodrive-usage.json.
#   totals <state>
#       rewrites autodrive-usage.json from the rows.
#   check <state> <loop> <pre|post|before> <label> <pr> [--force] [key=value...]
#       compares the recorded use with the limits. Exits 10, prints the keys
#       reached and writes autodrive-budget-summary.md when a limit is
#       reached (or with --force: a nested loop reached one). Exits 0
#       otherwise. A malformed limit, an unknown autodrive_budget_* key or an
#       untrusted usage file exits 2 (fails closed).
#   post <state> <pr> <file>
#       posts <file>, with secrets masked (mask_secrets below) and
#       sanitised, as a PR comment with `gh pr comment`. A failure is a
#       WARNING; it never changes the loop result.
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

# mask_secrets: stdin to stdout, with every credential a stop comment could
# quote replaced by [REDACTED]. The comment goes to a public PR, and GitHub
# push protection does not scan comments. Masked, in this order:
#   - PEM private key blocks, from the BEGIN line to the END line (or to the
#     end of the text when the END line was cut off), as one marker line;
#   - user:password@ in URLs;
#   - GitHub tokens gh[pousr]_ and github_pat_, Anthropic and OpenAI sk-,
#     AWS access key ids AKIA, ASIA and A3T, Slack xox?-, GitLab glpat-,
#     npm_, Google AIza, JSON web tokens eyJ*.*.*;
#   - the value of an Authorization or Proxy-Authorization key, after : or
#     =, quoted or not, with or without a scheme: after a known scheme word
#     (Bearer, Basic, Token, Negotiate, NTLM) the next word is masked and the
#     scheme kept; any other unquoted value is masked to the end of the line
#     (so "Authorization: s3cr3t" and "the authorization: rules" both become
#     "...: [REDACTED]"), and a quoted value is masked inside its quotes;
#   - elsewhere, the word of 12 or more characters after Bearer, Basic or
#     token, but only when that word contains a digit or one of _ - / + =,
#     so prose such as "the basic implementation" is left alone;
#   - the value after = or : of any key whose name ends in token, secret,
#     password, passwd, api_key, access_key, account_key, private_key,
#     credential(s), signature, sig or sharedaccesskey (any case; _, - or
#     nothing between the words), quoted or not.
# Not recognised: a secret with neither a known shape nor a telling key name,
# and a letters-only word after Bearer, Basic or token outside an
# Authorization header. The 8 KiB findings cut and the 59,000-byte body cut
# still apply.
# Portability: the rules use only POSIX ERE. The marker bytes \001 (value
# already handled) and \002 (mask here), removed from the input first, are
# built with printf into M1 and M2 and put into the expressions as raw
# bytes. They are never written as \xHH, which only GNU sed reads as a byte:
# BSD/macOS sed would read [\x01\x02] as the characters \, x, 0, 1 and 2.
ci() { awk '{ for (i = 1; i <= length($0); i++) { c = substr($0, i, 1); if (c ~ /[a-z]/) printf "[%s%s]", c, toupper(c); else printf "%s", c } }' <<<"$1"; }
SECRET_KEYS="$(ci 'token|secret|passw(or)?d|api[_-]?key|access[_-]?key|account[_-]?key|private[_-]?key|credentials?|signature|sig|sharedaccesskey')"
AUTH_WORDS="$(ci 'bearer|basic|token')"
AUTH_HEADER="$(ci 'authorization')"
AUTH_SCHEMES="$(ci 'bearer|basic|token|negotiate|ntlm')"
M1="$(printf '\001')"
M2="$(printf '\002')"
mask_secrets() {
  awk '
    /-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----/ { print "[REDACTED PRIVATE KEY]"; k = 1 }
    k && /-----END [A-Z0-9 ]*PRIVATE KEY-----/ { k = 0; next }
    !k' \
  | tr -d '\001\002' \
  | sed -E \
    -e 's#([A-Za-z][A-Za-z0-9+.-]*://)[^/@[:space:]]+@#\1[REDACTED]@#g' \
    -e 's#github_pat_[A-Za-z0-9_]{16,}#[REDACTED]#g' \
    -e 's#gh[pousr]_[A-Za-z0-9]{16,}#[REDACTED]#g' \
    -e 's#sk-[A-Za-z0-9_-]{16,}#[REDACTED]#g' \
    -e 's#(A3T[A-Z0-9]|AKIA|ASIA)[A-Z0-9]{16}#[REDACTED]#g' \
    -e 's#xox[abeoprs]-[A-Za-z0-9-]{10,}#[REDACTED]#g' \
    -e 's#glpat-[A-Za-z0-9_-]{16,}#[REDACTED]#g' \
    -e 's#npm_[A-Za-z0-9]{30,}#[REDACTED]#g' \
    -e 's#AIza[A-Za-z0-9_-]{30,}#[REDACTED]#g' \
    -e 's#eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}#[REDACTED]#g' \
    -e "s#((^|[^A-Za-z0-9])${AUTH_HEADER}[\"']?[[:space:]]*[:=][[:space:]]*)\"[^\"]*\"#\\1\"${M1}${M2}\"#g" \
    -e "s#((^|[^A-Za-z0-9])${AUTH_HEADER}[\"']?[[:space:]]*[:=][[:space:]]*)'[^']*'#\\1'${M1}${M2}'#g" \
    -e "s#((^|[^A-Za-z0-9])${AUTH_HEADER}[\"']?[[:space:]]*[:=][[:space:]]*)(${AUTH_SCHEMES})([[:space:]]+)[^][[:space:]\"'\`,;&<>|()${M1}${M2}]+#\\1${M1}\\3\\4${M2}#g" \
    -e "s#((^|[^A-Za-z0-9])${AUTH_HEADER}[\"']?[[:space:]]*[:=][[:space:]]*)([^${M1}[:space:]\"']|[\"'][^${M1}]).*#\\1${M2}#" \
    -e "s#${M1}##g" \
    -e "s#${M2}#[REDACTED]#g" \
    -e "s#(^|[^A-Za-z0-9])(${AUTH_WORDS})([[:space:]]+)([A-Za-z0-9._~+/=-]{12,})#\\1\\2\\3${M1}\\4${M2}#g" \
    -e "s#${M1}[^${M2}]*[0-9_/+=-][^${M2}]*${M2}#[REDACTED]#g" \
    -e "s#[${M1}${M2}]##g" \
    -e "s#((^|[^A-Za-z0-9])[A-Za-z0-9_-]*(${SECRET_KEYS})[\"']?[[:space:]]*[:=][[:space:]]*)\"[^\"]*\"#\\1\"[REDACTED]\"#g" \
    -e "s#((^|[^A-Za-z0-9])[A-Za-z0-9_-]*(${SECRET_KEYS})[\"']?[[:space:]]*[:=][[:space:]]*)'[^']*'#\\1'[REDACTED]'#g" \
    -e "s#((^|[^A-Za-z0-9])[A-Za-z0-9_-]*(${SECRET_KEYS})[\"']?[[:space:]]*[:=][[:space:]]*)[^][[:space:]\"'\`,;&<>|()]+#\\1[REDACTED]#g"
}

usage() { echo "ERROR: autodrive_budget.sh: ${1:-usage}. Usage: mark <state> | add <state> <loop> <label> <phase> <seconds> [--since <mark>] [--waited <ledger>] | totals <state> | check <state> <loop> <pre|post|before> <label> <pr> [--force] [key=value...] | post <state> <pr> <file>" >&2; exit 2; }

CMD="${1:-}"
[ $# -gt 0 ] && shift
case "$CMD" in
  mark|totals) [ $# -eq 1 ] || usage "${CMD} takes 1 argument" ;;
  add) [ $# -eq 5 ] || [ $# -eq 7 ] || [ $# -eq 9 ] || usage "add takes 5, 7 or 9 arguments" ;;
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
    NF == 6 && $1 ~ /^[A-Za-z0-9._-]+$/ && $3 ~ /^(before|round|evaluator|wait)$/ \
      && $4 ~ /^[0-9]+$/ && length($4) <= 12 && $5 ~ /^[0-9]+$/ && length($5) <= 12 \
      && ($6 == "unknown" || ($6 ~ /^[0-9]+$/ && length($6) <= 12)) {
      print $1, $3, $4 + 0, $5 + 0, $6; next }
    { bad++ }
    END { if (bad) printf "WARNING: skipped %d malformed row(s) in autodrive-usage.tsv\n", bad > "/dev/stderr" }' "$USAGE"
}

# totals_json: the totals by loop and overall, as one JSON line. seconds
# leaves usage-limit waits out; wait_seconds counts them.
totals_json() {
  rows | awk '
    { if (!($1 in seen)) { seen[$1] = 1; order[++n] = $1 }
      if ($2 == "round") { r[$1]++; R++ }
      if ($2 == "wait") { x[$1] += $4; X += $4 } else { w[$1] += $4; W += $4 }
      s[$1] += $3; S += $3
      if ($5 == "unknown") unk = 1; else T += $5 }
    END {
      printf "{\"loops\":{"
      for (i = 1; i <= n; i++) { l = order[i]
        printf "%s\"%s\":{\"rounds\":%d,\"sessions\":%d,\"seconds\":%d,\"wait_seconds\":%d}", (i > 1 ? "," : ""), l, r[l], s[l], w[l], x[l] }
      printf "},\"total\":{\"rounds\":%d,\"sessions\":%d,\"seconds\":%d,\"wait_seconds\":%d,\"tokens\":", R, S, W, X
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
    SESSIONS=1; MARK=""; LEDGER=""
    shift 5
    while [ $# -gt 0 ]; do
      case "$1" in
        --since) MARK="$(budget_int "$2")" || usage "mark must be an integer, not '$2'" ;;
        --waited) LEDGER="$2" ;;
        *) usage "unknown option '$1'" ;;
      esac
      shift 2
    done
    if [ -n "$MARK" ]; then
      read -r N_SESS N_SECS <<<"$(rows "$MARK" | awk '{ s += $3; w += $4 } END { printf "%d %d\n", s, w }')"
      [ "${N_SESS:-0}" -gt 0 ] && SESSIONS=0
      SECS=$(( SECS > ${N_SECS:-0} ? SECS - ${N_SECS:-0} : 0 ))
    fi
    WAITED=0 # an unreadable ledger counts no wait: the budget is never undercounted
    if [ -n "$LEDGER" ] && { [ -e "$LEDGER" ] || [ -L "$LEDGER" ]; }; then
      if [ -f "$LEDGER" ] && [ ! -L "$LEDGER" ] && [ -O "$LEDGER" ]; then
        WAITED="$(awk '$0 ~ /^[0-9]+$/ && length($0) <= 12 { w += $0; next } { bad++ }
          END { if (bad) printf "WARNING: skipped %d malformed line(s) in the usage-limit wait ledger\n", bad > "/dev/stderr"; printf "%d\n", w }' "$LEDGER")" || WAITED=0
      else
        echo "WARNING: the usage-limit wait ledger is not a regular file of this user; no wait is subtracted." >&2
      fi
    fi
    WAITED="$(budget_int "${WAITED:-0}")" || WAITED=0
    [ "$WAITED" -le "$SECS" ] || WAITED="$SECS" # never more than the child's own time
    SECS=$((SECS - WAITED))
    ROW="$(printf '%s\t%s\t%s\t%s\t%s\tunknown' "$LOOP" "$LABEL" "$PHASE" "$SESSIONS" "$SECS")"
    [ "$WAITED" -eq 0 ] || ROW="${ROW}
$(printf '%s\t%s\twait\t0\t%s\tunknown' "$LOOP" "$LABEL" "$WAITED")"
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
    # Secrets masked first (mask_secrets, PR #1862 review), then @mentions
    # and #N cross-links neutralised, absolute paths cut to base names,
    # fences unclosable; the text sits inside one fenced block. A masking
    # failure posts nothing (fails closed).
    if ! TEXT="$(head -c 59000 -- "$BODY_IN" | tr -d '\r' | mask_secrets \
      | sed -E 's/```/'"'''"'/g; s/~~~/'"'''"'/g; s/@/(at)/g; s/#([0-9])/# \1/g' \
      | sed -E "s#(^|[[:space:](\"'=:\`])/([^[:space:]/]+/)+#\\1#g")"; then
      echo "WARNING: the comment body could not be masked; nothing posted on PR #${PR}." >&2; exit 1
    fi
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
# Wall seconds leave usage-limit waits (phase wait) out; they are reported apart.
read -r U_LOOP U_CRUSTY U_MERGE U_TOTAL U_WALL U_WAIT U_SESS U_TOK <<<"$(rows | awk -v me="$LOOP" '
  $2 == "round" { t++; if ($1 == me) m++; if ($1 == "crusty") c++; if ($1 == "merge-ready") g++ }
  { if ($2 == "wait") x += $4; else w += $4; s += $3; if ($5 == "unknown") unk = 1; else tok += $5 }
  END { printf "%d %d %d %d %d %d %d %s\n", m, c, g, t, w, x, s, ((unk || NR == 0) ? "unknown" : tok) }')"
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
# The loops refuse an empty task_description (autodrive_use_task_file), so the
# command names it. A placeholder, never the task: this summary is posted to the PR.
RESUME="amplihack recipe run auto-drive-to-merge -c pr_number=${PR_ARG} -c task_description=\"<the original task>\""
for k in ${HIT[@]+"${HIT[@]}"}; do
  case "$k" in
    autodrive_budget_loop_rounds) RESUME="${RESUME} -c ${k}=$((L_LOOP * 2))" ;;
    autodrive_budget_total_rounds) RESUME="${RESUME} -c ${k}=$((L_TOTAL * 2))" ;;
    # #2022: a wall-clock limit is never raised, only removed: resume with 0.
    autodrive_budget_wall_seconds) RESUME="${RESUME} -c ${k}=0" ;;
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
| usage-limit wait seconds | ${U_WAIT} | not counted |
| sessions | ${U_SESS} | none |
| tokens | ${U_TOK} | none |

Wall seconds are the time children ran, less the time they waited for a
usage limit to lift. Those waits are the usage-limit wait seconds row, and
no limit counts them.

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
