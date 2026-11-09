#!/usr/bin/env bash
# autodrive_followups.sh - minor crusty findings end the crusty loop, and each
# one becomes a follow-up issue before the drive can be READY (issue #1979).
#
# Usage:
#   autodrive_followups.sh classify --record <abs path> --state-dir <dir>   (exactly one concerns JSON array on stdin)
#   autodrive_followups.sh collect  --state-dir <dir>
#   autodrive_followups.sh file     --state-dir <dir> --pr <N> [--task-id <id>] [--task-text-file <file>]
#   autodrive_followups.sh check    --state-dir <dir> --pr <N>
#   autodrive_followups.sh gate     --state-dir <dir> --pr <N>
#   autodrive_followups.sh resolve-task --task-id <id>
#
# classify  Step-03 of autodrive-crusty-round.yaml, on a crusty CONCERNS. Writes
#           <record>.severity (`id<TAB>minor|major` per concern). Prints
#           MINOR_ONLY, writes <record>.minor (id, summary, evidence per line)
#           and empties <record>.findings when every concern is minor under
#           autodrive_finding_rules.sh and no id was ever major in another
#           round (a .severity saying so, a .findings with no .severity, or a
#           resolved-concerns.txt id no .severity names). Anything but exactly
#           one JSON array on stdin leaves CONCERNS. Exit 0 when decided or
#           when jq cannot read the concerns (ERROR logged; CONCERNS stays);
#           exit 1, printing nothing, when the state is unusable.
# collect   After the crusty loop ends DONE: when the last loop-written record
#           (crusty-records.tsv row, hash checked against the record and
#           crusty-latest.json) is MINOR_ONLY, adds its .minor findings to the
#           follow-up set followups.jsonl, first entry per id kept. A bad line
#           fails; it is never dropped.
# file      One `gh issue create --repo <owner/repo of the PR>` per finding id
#           with no row in followups.tsv; the row (id, issue, task, PR) is
#           appended straight after each create. The task id: --task-id, else
#           the first Closes/Fixes/Resolves #N in the PR body, else in
#           --task-text-file; an empty set needs none. When the task is an
#           issue (#N), one `gh issue comment` on it then lists every
#           follow-up, because merging the PR closes that issue while the
#           follow-ups stay open; followups.parent records the list posted, and
#           a changed list is posted again. On failure: followups.error, exit 1
#           (gh's 79 is final and passed through); a full run removes it.
# check     No network: is every finding filed, and the parent comment posted?
# gate      check as one evidence or blocker line (autodrive_followups_check.sh).
# resolve-task  The one task-id rule (#1982): prints the id `file` uses for --task-id
#           (#N, zeros dropped, or the id), else exit 2; the value is never logged.
#
# Exit 2 is a usage error. Agent text reaches gh only through --body-file,
# after cell() (sanitised, HTML-escaped, mentions and links broken); the title
# is sanitised text with the same breaks (break_refs), clipped to 80, one argument.
# State files are private (umask 077, written through mktemp and mv -f), and a
# symlinked or loose state file is refused. Logs carry ids, numbers and tokens.
# A row append failing after gh succeeded refiles that finding on a rerun
# (accepted). Contract: docs/reference/auto-drive-to-merge.md.

set -uo pipefail
shopt -u patsub_replacement 2>/dev/null || true
export GIT_PAGER=cat GH_PAGER=cat PAGER=cat LESS=FRX
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE
umask 077

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" 2>/dev/null && pwd -P)"
ID_RE='^[A-Za-z0-9._-]{1,64}$'

usage() {
  echo "ERROR: autodrive_followups: $1" >&2
  echo "usage: autodrive_followups.sh classify|collect|file|check|gate|resolve-task [--state-dir <dir>] [--record <abs path>] [--pr <N>] [--task-id <id>] [--task-text-file <file>]" >&2
  exit 2
}
# resolve_task <value>: the normalised task id (#N for an issue reference).
resolve_task() {
  if [[ "$1" =~ ^#?[0-9]+$ ]]; then
    [[ "$1" =~ ^#?0*([1-9][0-9]{0,9})$ ]] || return 1
    printf '#%s' "${BASH_REMATCH[1]}"; return 0
  fi
  [[ "$1" =~ ^[A-Za-z0-9._:/-]{1,64}$ ]] || return 1
  printf '%s' "$1"
}

# --- arguments -----------------------------------------------------------------
CMD="${1:-}"
case "$CMD" in classify|collect|file|check|gate|resolve-task) shift ;; *) usage "the command must be classify, collect, file, check, gate or resolve-task" ;; esac
RECORD=""; STATE=""; PR=""; TASK_ID=""; TASK_FILE=""
while [ $# -gt 0 ]; do
  case "$1" in
    --record|--state-dir|--pr|--task-id|--task-text-file) [ $# -ge 2 ] || usage "$1 needs a value" ;;
    *) usage "unknown option '$(printf '%s' "$1" | LC_ALL=C tr -cd 'A-Za-z0-9_-' | cut -c 1-40)'" ;;
  esac
  case "$1" in
    --record) RECORD="$2" ;;
    --state-dir) STATE="$2" ;;
    --pr) PR="$2" ;;
    --task-id) TASK_ID="$2" ;;
    --task-text-file) TASK_FILE="$2" ;;
  esac
  shift 2
done
if [ "$CMD" = resolve-task ]; then # no state, no network
  [ -z "$STATE$RECORD$PR$TASK_FILE" ] && resolve_task "$TASK_ID" && exit 0
  usage "resolve-task takes only --task-id, as #N or 1 to 64 characters from A-Za-z0-9._:/-"; fi
[ -n "$STATE" ] || usage "--state-dir is required"
[[ "$STATE" =~ [[:cntrl:]] ]] && usage "--state-dir contains a control character"
STATE="${STATE%/}"; [ -n "$STATE" ] || usage "--state-dir must not be /"
case "$CMD" in
  file|check|gate) [[ "$PR" =~ ^[1-9][0-9]{0,9}$ ]] || usage "--pr must be a positive number of 1 to 10 digits" ;;
  classify)
    case "$RECORD" in /*) ;; *) usage "--record must be an absolute path" ;; esac
    [[ "$RECORD" =~ [[:cntrl:]] ]] && usage "--record contains a control character"
    [[ "${RECORD##*/}" =~ ^[A-Za-z0-9._-]+\.json$ ]] || usage "--record must name a .json round record"
    [ "${RECORD%/*}" = "$STATE" ] || usage "--record must be in --state-dir" ;;
esac

# --- helpers -------------------------------------------------------------------
for h in autodrive_finding_rules.sh autodrive_state.sh; do
  # shellcheck source=/dev/null
  . "$HERE/$h" 2>/dev/null || { echo "ERROR: autodrive_followups: $h is missing beside it (${HERE:-<unknown>})" >&2; exit 1; }
done
command -v jq >/dev/null 2>&1 || JQ_MISSING=1
TSV="$STATE/followups.tsv"; SET_FILE="$STATE/followups.jsonl"; ERR_FILE="$STATE/followups.error"
PARENT_FILE="$STATE/followups.parent"

PRIV=""
cleanup() { [ -z "$PRIV" ] || rm -rf -- "$PRIV"; }
trap cleanup EXIT
priv_dir() { [ -n "$PRIV" ] || PRIV="$(mktemp -d "${TMPDIR:-/tmp}/autodrive-followups.XXXXXX" 2>/dev/null)"; [ -n "$PRIV" ]; }

state_ok() { [ -d "$STATE" ] && autodrive_private "$STATE"; }
# safe_file <path>: absent, or a private regular file that is not a symlink.
safe_file() {
  [ -L "$1" ] && return 1
  [ -e "$1" ] || return 0
  [ -f "$1" ] && autodrive_private "$1"
}
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

# norm_set <file>: the follow-up set as one JSON array, or exit 1 when any
# line is not an object with a safe id and string summary and evidence.
norm_set() {
  [ -e "$1" ] || { printf '[]'; return 0; }
  jq -sc "$AUTODRIVE_FINDING_JQ"'
    if all(.[]; type == "object" and (.id | type) == "string" and (.id | test("\\A[A-Za-z0-9._-]{1,64}\\z"))
                and ((.summary // "") | type) == "string" and ((.evidence // "") | type) == "string")
    then map({id, summary: (.summary // "" | clean), evidence: (.evidence // "" | clean)})
    else error("bad follow-up line") end' "$1" 2>/dev/null
}
DEDUPE='reduce .[] as $x ([]; if any(.[]; .id == $x.id) then . else . + [$x] end)'

# rows_ok: every non-blank followups.tsv row is id, issue, task, this PR.
rows_ok() {
  [ -e "$TSV" ] || return 0
  LC_ALL=C awk -F '\t' -v pr="$PR" '
    /^$/ { next }
    { ok = (NF == 4 && $1 ~ /^[A-Za-z0-9._-]+$/ && length($1) <= 64 && $2 ~ /^[1-9][0-9]*$/ && $4 == pr)
      ok = ok && ($3 ~ /^#[1-9][0-9]*$/ || ($3 ~ /^[A-Za-z0-9._:\/-]+$/ && $3 !~ /^[0-9]+$/ && length($3) <= 64))
      if (!ok) { bad = 1; exit } }
    END { exit bad }' "$TSV"
}
# parent_key: `<task issue>:<follow-up issues>,` for the followups.tsv rows, or
# nothing when there is no row or the first row's task is not an issue (#N).
parent_key() {
  [ -s "$TSV" ] || return 0
  LC_ALL=C awk -F '\t' 'NF { if (p == "") p = $3; l = l $2 "," }
    END { if (p ~ /^#[1-9][0-9]*$/) print substr(p, 2) ":" l }' "$TSV"
}
# parent_commented: no comment is due, or followups.parent holds the current key.
parent_commented() {
  local key
  key="$(parent_key)"
  [ -z "$key" ] || { [ -f "$PARENT_FILE" ] && [ "$(cat -- "$PARENT_FILE")" = "$key" ]; }
}
filed_ids() { [ -e "$TSV" ] && LC_ALL=C awk -F '\t' 'NF { print $1 }' "$TSV" | jq -Rsc 'split("\n") | map(select(length > 0))' || printf '[]'; }

# closing_ref: stdin text -> the number of its first Closes/Fixes/Resolves #N.
# clip_line <text>: its first line, sanitised (when workflow_issue_tracking.sh
# is loaded), with control characters removed, clipped to 200 characters.
clip_line() {
  local s="${1%%$'\n'*}"
  ! declare -F sanitize_cli_output >/dev/null || s="$(sanitize_cli_output "$s")"
  printf '%s' "$s" | LC_ALL=C tr -d '[:cntrl:]' | cut -c 1-200
}
closing_ref() {
  { grep -oiE '(^|[^A-Za-z0-9_])(closes|fixes|resolves)[[:space:]]+#[0-9]+' || true; } | head -n 1 | sed -E 's/.*#//'
}

# --- classify --------------------------------------------------------------------
cmd_classify() {
  local concerns sevs ret seen f sev rows rc
  [ -z "${JQ_MISSING:-}" ] || { echo "ERROR: classify: jq is not on PATH; the verdict stays CONCERNS" >&2; exit 1; }
  state_ok || { echo "ERROR: followup-state-unsafe: the state dir is not private to this user; the verdict stays CONCERNS" >&2; exit 1; }
  concerns="$(cat)"
  priv_dir || { echo "ERROR: classify: no private temporary directory; the verdict stays CONCERNS" >&2; exit 1; }
  . "$HERE/workflow_issue_tracking.sh" 2>/dev/null || true
  # This round's severities, kept for later rounds whatever this one decides.
  # Written only when stdin is exactly one array; otherwise there is no file,
  # and a later round reads this round's .findings ids as major.
  rm -f -- "$RECORD.severity"
  if sevs="$(printf '%s' "$concerns" | jq -rs --arg kw "$ID_KEYWORDS" --arg tkw "$TEXT_KEYWORDS" "$AUTODRIVE_FINDING_JQ"'
      if length == 1 and (.[0] | type == "array") then .[0][] | select(type == "object" and .id != null)
        | "\(.id | tostring | gsub("[[:cntrl:]]"; " "))\t\(if minor_finding($kw; $tkw) then "minor" else "major" end)"
      else error("not one array") end' 2>/dev/null)"; then
    printf '%s\n' "$sevs" | write_state "${RECORD##*/}.severity" \
      || { echo "ERROR: classify: ${RECORD##*/}.severity could not be written; the verdict stays CONCERNS" >&2; exit 1; }
  fi
  # Severity history: ids another crusty round saw as major. A round with a
  # .findings but no .severity (classify did not run there) counts as major,
  # and so does a resolved-concerns.txt id that no .severity file names.
  ret=""; seen=""
  for f in "$STATE"/crusty-round-*.json.findings; do
    [ "$f" != "$RECORD.findings" ] && [ -f "$f" ] && [ ! -L "$f" ] || continue
    sev="${f%.findings}.severity"
    if [ -f "$sev" ] && [ ! -L "$sev" ]; then
      ret="${ret}$(LC_ALL=C awk -F '\t' '$2 != "minor" { print $1 }' "$sev")"$'\n'
      seen="${seen}$(LC_ALL=C cut -f 1 "$sev")"$'\n'
    else
      ret="${ret}$(cat -- "$f")"$'\n'
    fi
  done
  f="$STATE/resolved-concerns.txt"
  if [ -f "$f" ] && [ ! -L "$f" ]; then
    ret="${ret}$(printf '%s' "$seen" | LC_ALL=C grep -Fxv -f /dev/stdin -- "$f" 2>/dev/null)"$'\n'
  fi
  # -s: stdin is decided as a whole, so a second JSON value (a blocking array
  # after a minor one) can never be skipped; only exactly one array counts.
  rows="$(printf '%s' "$concerns" | jq -cs --arg ret "$ret" --arg kw "$ID_KEYWORDS" --arg tkw "$TEXT_KEYWORDS" "$AUTODRIVE_FINDING_JQ"'
    ($ret | split("\n") | map(select(length > 0))) as $r
    | if length == 1 and (.[0] | type == "array") then .[0]
        | if length > 0 and all(.[]; minor_finding($kw; $tkw) and (.id as $id | any($r[]; . == $id) | not))
          then map({id, summary: (.summary | clean), evidence: (.evidence | clean)}) | '"$DEDUPE"'
          else empty end
      else empty end' 2>"$PRIV/jq.err")"; rc=$?
  if [ "$rc" -ne 0 ]; then
    echo "ERROR: classify: the concerns could not be read (jq rc=${rc}): $(clip_line "$(cat -- "$PRIV/jq.err" 2>/dev/null)"); the verdict stays CONCERNS" >&2
    exit 0
  fi
  if [ -z "$rows" ]; then
    echo "INFO: classify: not every concern is a minor finding never seen as major; the verdict stays CONCERNS" >&2
    exit 0
  fi
  if ! printf '%s' "$rows" | jq -c '.[]' | write_state "${RECORD##*/}.minor" \
     || ! write_state "${RECORD##*/}.findings" < /dev/null; then
    rm -f -- "$RECORD.minor"
    echo "ERROR: classify: the .minor or .findings file could not be written; the verdict stays CONCERNS" >&2
    exit 1
  fi
  echo "INFO: classify: $(printf '%s' "$rows" | jq length) minor finding(s); the round is MINOR_ONLY" >&2
  printf 'MINOR_ONLY\n'
}

# --- collect ---------------------------------------------------------------------
cmd_collect() {
  local row file want rec latest verdict minor new old merged
  [ -z "${JQ_MISSING:-}" ] || { echo "ERROR: followup-set-invalid: jq is not on PATH" >&2; exit 1; }
  state_ok || { echo "ERROR: followup-state-unsafe: the state dir is not private to this user" >&2; exit 1; }
  # No manifest row: nothing loop-written to collect from. The merge gate refuses
  # criterion 3 without one anyway; a MINOR_ONLY latest record still fails here.
  if ! row="$(autodrive_crusty_manifest_row "$STATE")"; then
    if grep -q '^{"crusty_verdict":"MINOR_ONLY",' "$STATE/crusty-latest.json" 2>/dev/null; then
      echo "ERROR: followup-set-invalid: crusty-records.tsv is missing, a symlink or malformed, so the MINOR_ONLY round cannot be trusted" >&2; exit 1
    fi
    echo "INFO: collect: no loop-written crusty record; no follow-up to add" >&2
    exit 0
  fi
  file="${row%% *}"; want="${row#* }"; rec="$STATE/$file"; latest="$STATE/crusty-latest.json"
  priv_dir || { echo "ERROR: followup-state-unsafe: no private temporary directory" >&2; exit 1; }
  if [ ! -f "$rec" ] || [ -L "$rec" ] || [ ! -f "$latest" ] || [ -L "$latest" ] \
     || ! cat -- "$rec" > "$PRIV/record" \
     || [ "$(autodrive_blob_hash "$STATE" < "$PRIV/record")" != "$want" ] \
     || [ "$(autodrive_blob_hash "$STATE" < "$latest")" != "$want" ]; then
    echo "ERROR: followup-set-invalid: ${file} does not match the hash crusty-records.tsv holds for it" >&2
    exit 1
  fi
  verdict="$(sed -n 's/^{"crusty_verdict":"\([A-Z_]*\)",.*/\1/p' "$PRIV/record" | head -n 1)"
  if [ "$verdict" != "MINOR_ONLY" ]; then
    echo "INFO: collect: the crusty loop ended ${verdict:-without a verdict}; no follow-up to add" >&2
    exit 0
  fi
  minor="$rec.minor"
  { [ -f "$minor" ] && safe_file "$minor"; } \
    || { echo "ERROR: followup-set-invalid: ${file}.minor is missing, a symlink or not private" >&2; exit 1; }
  new="$(norm_set "$minor")" && [ "$(printf '%s' "$new" | jq length)" != 0 ] \
    || { echo "ERROR: followup-set-invalid: ${file}.minor has a line that is not a valid finding, or none" >&2; exit 1; }
  safe_file "$SET_FILE" || { echo "ERROR: followup-state-unsafe: followups.jsonl is a symlink or not private" >&2; exit 1; }
  old="$(norm_set "$SET_FILE")" || { echo "ERROR: followup-set-invalid: followups.jsonl has a line that is not a valid finding" >&2; exit 1; }
  merged="$(jq -cn --argjson a "$old" --argjson b "$new" '$a + $b | '"$DEDUPE")" \
    && printf '%s' "$merged" | jq -c '.[]' | write_state followups.jsonl \
    || { echo "ERROR: followup-set-invalid: followups.jsonl could not be written" >&2; exit 1; }
  echo "INFO: collect: $(printf '%s' "$merged" | jq length) finding(s) in the follow-up set after ${file}" >&2
}

# --- file ------------------------------------------------------------------------
fail() { # fail <token> <detail> [exit code]: records the token in followups.error, except on 79
  [ "${3:-1}" = 79 ] || printf '%s\n' "$1" | write_state followups.error || echo "ERROR: followups.error could not be written" >&2
  echo "ERROR: $1: $2" >&2
  exit "${3:-1}"
}
gh_line() { clip_line "$(cat -- "$1" 2>/dev/null)"; } # the first line of a gh stderr file

# post_parent: one comment on the task issue listing every follow-up, posted
# again only when the list changed; followups.parent records the list posted.
post_parent() {
  local key num tf rc
  key="$(parent_key)"
  if [ -z "$key" ]; then
    [ ! -s "$TSV" ] || echo "INFO: followups: the task is not an issue number; no comment lists the follow-ups" >&2
    return 0
  fi
  parent_commented && return 0
  num="${key%%:*}"; tf="$PRIV/parent.md"
  { printf 'Auto-drive filed follow-up issues for the minor crusty findings on PR #%s.\n' "$PR"
    printf 'Merging that PR closes this issue. The task is not complete until these are done:\n\n'
    LC_ALL=C awk -F '\t' 'NF { printf "- #%s (finding `%s`)\n", $2, $1 }' "$TSV"
    printf '\n<!-- autodrive-followups-parent pr=%s -->\n' "$PR"
  } > "$tf" || fail followup-comment-failed "the comment body for #${num} could not be written"
  gh issue comment "$num" --repo "$repo" --body-file "$tf" < /dev/null > /dev/null 2>"$PRIV/gh.err"; rc=$?
  [ "$rc" -ne 79 ] || fail followup-comment-failed "gh issue comment on #${num} exited 79; that code is final and is not retried" 79
  [ "$rc" -eq 0 ] || fail followup-comment-failed "gh issue comment on #${num} exited ${rc}: $(gh_line "$PRIV/gh.err")"
  printf '%s\n' "$key" | write_state followups.parent \
    || fail followup-state-unsafe "the comment on #${num} was posted but followups.parent could not be written; a rerun posts it again"
  echo "INFO: followups: the follow-up list is posted on task #${num}" >&2
}

cmd_file() {
  local fset filed n task="" prj url body repo out rc num re title plain id summary evidence created=0 tf
  [ -z "${JQ_MISSING:-}" ] || { echo "ERROR: followup-create-failed: jq is not on PATH" >&2; exit 1; }
  state_ok || { echo "ERROR: followup-state-unsafe: the state dir is not private to this user" >&2; exit 1; }
  for n in followups.jsonl followups.tsv followups.error followups.parent; do
    safe_file "$STATE/$n" || fail followup-state-unsafe "$n is a symlink, not a regular file, or not private"
  done
  fset="$(norm_set "$SET_FILE")" || fail followup-set-invalid "followups.jsonl has a line that is not a valid finding"
  rows_ok || fail followup-row-malformed "followups.tsv has a row that is not id, issue, task, PR #${PR}"
  filed="$(filed_ids)"
  n="$(printf '%s' "$fset" | jq --argjson d "$filed" '[.[] | select(.id as $i | $d | index([$i]) | not)] | length')"
  if [ "$n" = 0 ] && parent_commented; then
    rm -f -- "$ERR_FILE"
    echo "INFO: followups: every minor finding has an issue ($(printf '%s' "$fset" | jq length) in the set)" >&2
    printf 'followups-ok %s\n' "$(printf '%s' "$fset" | jq length)"
    exit 0
  fi
  if [ "$n" != 0 ] && [ -n "$TASK_ID" ]; then
    task="$(resolve_task "$TASK_ID")" || fail followup-task-unresolved "the plan task id is neither #N nor 1 to 64 of A-Za-z0-9._:/-"
  fi
  . "$HERE/workflow_issue_tracking.sh" 2>/dev/null && declare -F sanitize_cli_output >/dev/null \
    || fail followup-create-failed "workflow_issue_tracking.sh is missing beside autodrive_followups.sh"
  priv_dir || fail followup-create-failed "no private temporary directory"
  prj="$(gh pr view "$PR" --json url,body < /dev/null 2>"$PRIV/gh.err")"; rc=$?
  [ "$rc" -ne 79 ] || fail followup-repo-unresolved "gh exited 79 reading PR #${PR}; that code is final and is not retried" 79
  [ "$rc" -eq 0 ] || fail followup-repo-unresolved "gh pr view ${PR} exited ${rc}: $(gh_line "$PRIV/gh.err")"
  url="$(printf '%s' "$prj" | jq -r '.url // "" | strings' 2>/dev/null)"
  re="^https://github\\.com/([A-Za-z0-9-]+/[A-Za-z0-9._-]+)/pull/${PR}\$"
  [[ "$url" =~ $re ]] || fail followup-repo-unresolved "the URL of PR #${PR} is not https://github.com/<owner>/<repo>/pull/${PR}"
  repo="${BASH_REMATCH[1]}"
  if [ "$n" != 0 ] && [ -z "$task" ]; then
    body="$(printf '%s' "$prj" | jq -r '.body // "" | strings' 2>/dev/null)"
    num="$(printf '%s' "$body" | closing_ref)"
    if [ -z "$num" ] && [ -n "$TASK_FILE" ] && [ -f "$TASK_FILE" ]; then num="$(closing_ref < "$TASK_FILE")"; fi
    [ -n "$num" ] && task="$(resolve_task "$num")" \
      || fail followup-task-unresolved "no plan task id, and neither the PR body nor the task names Closes/Fixes/Resolves #N"
  fi
  [ "$n" = 0 ] || echo "INFO: followups: filing ${n} follow-up issue(s) in ${repo} for task ${task}, PR #${PR}" >&2
  tf="$PRIV/body.md"
  while IFS=$'\x1f' read -r id summary evidence; do
    [[ "$id" =~ $ID_RE ]] || fail followup-set-invalid "a finding id is not valid"
    plain="$(break_refs "$(sanitize_cli_output "$summary" | LC_ALL=C tr -d '[:cntrl:]')")"
    title="$(jq -rn --arg t "Follow-up (${task}): ${plain}" '$t[0:80]')"
    { printf 'Follow-up from the auto-drive crusty review of PR #%s.\n\n' "$PR"
      printf 'Task: %s\nFinding: %s (severity: minor)\n\n' "$task" "$id"
      printf 'Summary: %s\nEvidence: %s\n\n' "$(cell "$summary")" "$(cell "$evidence")"
      printf '<!-- autodrive-followup task=%s finding=%s -->\n' "$task" "$id"
    } > "$tf" || fail followup-create-failed "the issue body for ${id} could not be written"
    out="$(gh issue create --repo "$repo" --title "$title" --body-file "$tf" < /dev/null 2>"$PRIV/gh.err")"; rc=$?
    [ "$rc" -ne 79 ] || fail followup-create-failed "gh issue create for ${id} exited 79; that code is final and is not retried" 79
    [ "$rc" -eq 0 ] || fail followup-create-failed "gh issue create for ${id} exited ${rc}: $(gh_line "$PRIV/gh.err")"
    out="${out%$'\n'}"; out="${out##*$'\n'}"
    re="^https://github\\.com/${repo//./\\.}/issues/([1-9][0-9]{0,9})\$"
    [[ "$out" =~ $re ]] || fail followup-create-failed "gh issue create for ${id} printed no issue URL of ${repo}"
    num="${BASH_REMATCH[1]}"
    { safe_file "$TSV" && printf '%s\t%s\t%s\t%s\n' "$id" "$num" "$task" "$PR" >> "$TSV"; } \
      || fail followup-state-unsafe "issue #${num} was created for ${id} but its followups.tsv row could not be written; a rerun files it again"
    created=$((created + 1))
    echo "INFO: followup ${id} -> #${num} (task ${task}, PR #${PR})" >&2
  done < <(printf '%s' "$fset" | jq -r --argjson d "$filed" \
             '.[] | select(.id as $i | $d | index([$i]) | not) | [.id, .summary, .evidence] | join("\u001f")')
  [ "$created" = "$n" ] || fail followup-create-failed "${created} of ${n} follow-up issue(s) were filed"
  post_parent
  rm -f -- "$ERR_FILE"
  printf 'followups-ok %s\n' "$(printf '%s' "$fset" | jq length)"
}

# --- check, gate --------------------------------------------------------------
case "$CMD" in
  check|gate)
    # shellcheck source=/dev/null
    . "$HERE/autodrive_followups_check.sh" 2>/dev/null \
      || { echo "ERROR: autodrive_followups: autodrive_followups_check.sh is missing beside it" >&2; exit 1; } ;;
esac
"cmd_${CMD}"
