#!/bin/bash -p
# autodrive_expected_checks.sh — "CI green" for one head SHA (issue #2186).
#
#   /bin/bash -p autodrive_expected_checks.sh <HEAD_SHA> <BASE_REF>
#
# CI green means every EXPECTED check exists at HEAD_SHA and passed. The
# expected set is:
#   - the required checks of BASE_REF: rulesets (rules/branches/<base>) plus
#     classic protection as the branch object reports it (branches/<base>);
#   - every job of each active workflow at HEAD_SHA whose `on:` includes
#     pull_request and whose branch filters admit BASE_REF.
# A missing check, a pending or failing check, and anything that cannot be
# read all block. Every other check-run and status at HEAD_SHA must not be
# pending or failing either, and zero checks at the head blocks.
#
# Output (stdout): `evidence: ...` lines, `blocker: ...` lines, then ONE final
# JSON line {"ci_status":"GREEN|RED|PENDING|MISSING|UNREADABLE","missing":[..],
# "blockers":N}. Callers read only that last line. Exit 0 only when GREEN,
# 1 otherwise, 2 for bad arguments (before any gh call).
#
# Read-only: GETs through `gh api` from the repository in the working
# directory. It never sleeps, loops to wait or repeats a request; a read that
# fails is UNREADABLE. Untrusted text (names, YAML) is data only: it is
# cleaned before it is printed and never evaluated. Needs jq 1.6 (@base64d).
# See docs/reference/auto-drive-to-merge.md ("CI green at the head").

# The jq and awk programs below are single-quoted on purpose.
# shellcheck disable=SC2016
set -uo pipefail
# -p ignores exported functions and BASH_ENV; drop these functions anyway.
builtin unset -f gh git jq awk tr
export GH_PAGER=cat PAGER=cat GH_PROMPT_DISABLED=1
umask 077

usage() { echo "ERROR: autodrive_expected_checks.sh <HEAD_SHA> <BASE_REF>: $1" >&2; exit 2; }
[ $# -eq 2 ] || usage "exactly two arguments are required"
SHA="$1"; BASE="$2"
case "$SHA" in ''|*[!0-9a-f]*) usage "HEAD_SHA must be 40 lowercase hex characters" ;; esac
[ "${#SHA}" -eq 40 ] || usage "HEAD_SHA must be 40 lowercase hex characters"
case "$BASE" in
  ''|-*|/*|*/|*..*|*//*|*.lock|*[!A-Za-z0-9._/-]*) usage "BASE_REF is not a plain branch name" ;;
esac

# Stop with an UNREADABLE verdict (a literal, so it works without jq).
fatal() {
  echo "blocker: expected checks unreadable: $1"
  printf '%s\n' '{"ci_status":"UNREADABLE","missing":[],"blockers":1}'
  exit 1
}
command -v jq >/dev/null 2>&1 || fatal "jq is not installed"
[ "$(printf '%s' '"YQ=="' | jq -r '@base64d' 2>/dev/null)" = "a" ] || fatal "jq 1.6 or later is required (@base64d)"
T="$(mktemp -d "${TMPDIR:-/tmp}/autodrive-ec.XXXXXX" 2>/dev/null)" || fatal "no private temporary directory"
trap 'rm -rf -- "$T"' EXIT
: > "${T}/unr"
unr() { printf '%s\n' "$1" >> "${T}/unr"; }

# get <paginate 0|1> <endpoint>: sets GOT to a file holding the answer's pages
# as one JSON array. Returns 0, 4 for a 404, 1 for any other failure.
# Temp files are named by a counter, never by data from a response.
N=0; GOT=""
get() {
  local rc
  N=$((N + 1)); GOT="${T}/r${N}"
  if [ "$1" = 1 ]; then
    gh api --paginate "$2" > "${GOT}.raw" 2> "${GOT}.err"
  else
    gh api "$2" > "${GOT}.raw" 2> "${GOT}.err"
  fi
  rc=$?
  if [ "$rc" -ne 0 ]; then
    grep -qF '(HTTP 404)' "${GOT}.err" && return 4
    return 1
  fi
  # Only gh's 404 above may return 4, so a jq failure is always 1.
  [ -s "${GOT}.raw" ] && jq -s . "${GOT}.raw" > "$GOT" 2>/dev/null || return 1
}

# read_into <out> <paginate> <endpoint> <jq filter over the pages>: writes the
# filter's result, or records the endpoint as unreadable and writes [].
read_into() {
  local out="$1" pg="$2" ep="$3" flt="$4" rc
  get "$pg" "repos/{owner}/{repo}/${ep}"; rc=$?
  if [ "$rc" -eq 0 ] && jq -c --arg sha "$SHA" "$flt" "$GOT" > "$out" 2>/dev/null; then
    return 0
  fi
  if [ "$rc" -eq 4 ]; then unr "${ep%%\?*} (not found)"
  elif [ "$rc" -eq 0 ]; then unr "${ep%%\?*} (unexpected answer)"
  else unr "${ep%%\?*} (gh failed)"; fi
  echo '[]' > "$out"
  return 1
}

# --- Required checks (D12'): rulesets, then classic protection as the branch
# object reports it. The admin-only protection endpoint is never called: a
# non-admin token gets a 404 there that would look like "nothing required".
read_into "${T}/rules.json" 1 "rules/branches/${BASE}?per_page=100" \
  'if all(.[]; type == "array") then [(add // [])[] | select(.type == "required_status_checks")
     | .parameters.required_status_checks[] | .context | strings] else error("shape") end'
read_into "${T}/branch.json" 0 "branches/${BASE}" \
  '.[0] | if type == "object" and (.protected | type) == "boolean" then
     (if .protected then [((.protection.required_status_checks.contexts // [])[]),
        ((.protection.required_status_checks.checks // [])[] | .context)] | map(strings) else [] end)
   else error("shape") end'
jq -cn --slurpfile a "${T}/rules.json" --slurpfile b "${T}/branch.json" \
  '($a[0] + $b[0]) | reduce .[] as $x ([]; if any(.[]; . == $x) then . else . + [$x] end)' > "${T}/req.json"

# --- What exists at the head: check-runs (filter=all, so S1 sees every suite)
# and commit statuses.
read_into "${T}/cr.json" 1 "commits/${SHA}/check-runs?per_page=100&filter=all" \
  'if all(.[]; type == "object" and (.check_runs | type) == "array") then
     [.[].check_runs[] | {id, name, status, conclusion, suite: (.check_suite.id // null)}]
   else error("shape") end'
read_into "${T}/st.json" 1 "commits/${SHA}/status?per_page=100" \
  'if all(.[]; type == "object" and (.statuses | type) == "array") then
     [.[].statuses[] | {context, state}] else error("shape") end'

# --- Workflows: active ones (actions/workflows), files at the head, and the
# pull_request runs at the head. A 404 on the workflows directory is the one
# 404 that means "none"; every other unreadable answer blocks.
read_into "${T}/active.json" 1 "actions/workflows?per_page=100" \
  'if all(.[]; type == "object" and (.workflows | type) == "array") then
     [.[].workflows[] | select(.state == "active") | .path | strings] else error("shape") end'
get 0 "repos/{owner}/{repo}/contents/.github/workflows?ref=${SHA}"; rc=$?
if [ "$rc" -eq 4 ]; then
  echo '[]' > "${T}/dir.json"
elif [ "$rc" -ne 0 ] || ! jq -c '.[0] | if type == "array" then [.[] | .path | strings | select(test("\\.ya?ml$"))]
     else error("shape") end' "$GOT" > "${T}/dir.json" 2>/dev/null; then
  unr "contents/.github/workflows (unreadable)"; echo '[]' > "${T}/dir.json"
fi
# S2: a workflow path must be a plain file name directly in .github/workflows.
WF_RE='^\\.github/workflows/[A-Za-z0-9._-]+\\.ya?ml$'
if [ "$(jq -r "[.[] | select((test(\"${WF_RE}\") and (contains(\"..\") | not)) | not)] | length" "${T}/dir.json")" != 0 ]; then
  unr "a workflow path is not a plain file in .github/workflows"
fi
jq -r --slurpfile w "${T}/active.json" ".[] | select(test(\"${WF_RE}\") and (contains(\"..\") | not))
  | select(. as \$p | any(\$w[0][]; . == \$p))" "${T}/dir.json" > "${T}/paths"
read_into "${T}/runs.json" 1 "actions/runs?head_sha=${SHA}&event=pull_request&per_page=100" \
  'if all(.[]; type == "object" and (.workflow_runs | type) == "array") then [.[].workflow_runs[]]
     | if all(.[]; ((.id | tostring) | test("^[0-9]+$")) and (.path | type) == "string")
       then [.[] | select(.head_sha == $sha) | {id: (.id | tonumber), path, status}]
       else error("run id or path") end
   else error("shape") end'

# --- The YAML scanner (D1). It reads only the top-level `on:` key (scalar,
# flow list, block list or mapping; also "on", 'on' and true) with the child
# keys of pull_request, and the top-level `jobs:` keys with each job's own
# `name:` and `if:` at the job-body indent. Output records (tab-separated):
#   O <trigger>  B <branches entry>  I <branches-ignore entry>  P 1 (paths)
#   J <key> <name> <1 when the job has a job-level if:>  E <reason>
# No system(), no command getline, no redirection: the YAML is data.
SCAN='
function ind(s) { match(s, /^ */); return RLENGTH }
function trim(s) { sub(/^[ \t]+/, "", s); sub(/[ \t]+$/, "", s); return s }
function val(s,   c, e) {
  s = trim(s); c = substr(s, 1, 1)
  if (c == "\"" || c == SQ) { e = index(substr(s, 2), c); return (e > 0) ? substr(s, 2, e - 1) : substr(s, 2) }
  if (c == "#") return ""
  sub(/[ \t]+#.*$/, "", s); return trim(s)
}
function key(s) { sub(/[ \t]*:([ \t].*)?$/, "", s); return val(s) }
function rest(s) { sub(/^[^:]*:/, "", s); return val(s) }
function items(s, k,   n, a, i, v) {
  if (substr(s, 1, 1) == "[") {
    if (s !~ /\]$/) { print "E\tflow list split over lines"; return }
    sub(/^\[/, "", s); sub(/\]$/, "", s); n = split(s, a, ",")
    for (i = 1; i <= n; i++) { v = val(a[i]); if (v != "") print k "\t" v }
  } else if (s != "") print k "\t" s
}
function flush() { if (jk != "") { gsub(/\t/, " ", jk); gsub(/\t/, " ", jn); print "J\t" jk "\t" jn "\t" jif }; jk = "" }
BEGIN { SQ = sprintf("%c", 39); KEYRE = "^[^ #-][^:]*:([ \t]|$)" }
{ sub(/\r$/, "") }
/^[ \t]*(#|$)/ { next }
{
  i = ind($0); line = substr($0, i + 1)
  if (i == 0) {
    flush(); sec = ""
    if (line !~ KEYRE) next
    k = key(line); r = rest(line)
    if (k == "on" || k == "true") {
      sawon = 1; sec = "on"; oi = 0
      if (substr(r, 1, 1) == "{") print "E\tflow mapping under on:"; else items(r, "O")
    } else if (k == "jobs") {
      sawjobs = 1; sec = "jobs"; ji = 0
      if (r != "") print "E\tinline jobs: value"
    }
    next
  }
  if (sec == "on") {
    if (oi == 0) oi = i
    if (i < oi) { print "E\tbad indentation under on:"; next }
    if (i == oi) {
      pr = 0; lk = ""
      if (substr(line, 1, 2) == "- ") { print "O\t" val(substr(line, 3)); next }
      if (line !~ KEYRE) { print "E\tunreadable on: entry"; next }
      k = key(line); r = rest(line); print "O\t" k
      if (k == "pull_request") { pr = 1; pi = 0; if (r != "" && r != "null" && r != "~" && r != "{}") print "E\tinline pull_request: value" }
      next
    }
    if (!pr) next
    if (pi == 0) pi = i
    if (i == pi && substr(line, 1, 2) != "- ") {
      lk = ""
      if (line !~ KEYRE) next
      k = key(line); r = rest(line)
      if (k == "branches") { lk = "B"; items(r, "B") }
      else if (k == "branches-ignore") { lk = "I"; items(r, "I") }
      else if (k == "paths" || k == "paths-ignore") print "P\t1"
      next
    }
    if (lk != "" && substr(line, 1, 2) == "- ") print lk "\t" val(substr(line, 3))
    next
  }
  if (sec == "jobs") {
    if (ji == 0) ji = i
    if (i == ji) {
      flush()
      if (line !~ KEYRE) { print "E\tunreadable job key"; next }
      jk = key(line); jn = ""; jif = 0; bi = 0; njobs++
      next
    }
    if (i < ji || jk == "") { print "E\tbad indentation under jobs:"; next }
    if (bi == 0) bi = i
    if (i == bi && line ~ /^name[ \t]*:/) jn = rest(line)
    if (i == bi && line ~ /^if[ \t]*:/) jif = 1
  }
}
END { flush(); if (!sawon) print "E\tno top-level on: key"; if (!sawjobs || njobs == 0) print "E\tno jobs under jobs:" }
'
# Is the workflow expected (D2, D3)? pull_request (not pull_request_target) in
# `on`; `branches` names the base or holds a glob; `branches-ignore` does not
# name the base. `types:` is ignored (D5). `paths` matters only with no run (D4).
DECIDE='split("\n") | map(select(length > 0) | split("\t")) as $r
  | [$r[] | select(.[0] == "E") | .[1]] as $e
  | if ($e | length) > 0 then {error: $e[0]} else
    [$r[] | select(.[0] == "B") | .[1]] as $b | [$r[] | select(.[0] == "I") | .[1]] as $i
    | {expected: (any($r[]; .[0] == "O" and .[1] == "pull_request")
         and (($b | length) == 0 or any($b[]; test("[*?\\[!]") or . == $base))
         and (any($i[]; . == $base) | not)),
       paths: any($r[]; .[0] == "P"),
       jobs: [$r[] | select(.[0] == "J") | {key: .[1], name: (.[2] // ""), cond: (.[3] == "1")}]}
    end'

: > "${T}/wfs.jsonl"
W=0
while IFS= read -r P; do
  W=$((W + 1)); F="${T}/wf${W}"
  if ! get 0 "repos/{owner}/{repo}/contents/${P}?ref=${SHA}" \
     || ! jq -r '.[0] | if .encoding == "base64" and (.content | type) == "string"
          then .content | gsub("\\s"; "") | @base64d else error("encoding") end' "$GOT" > "${F}.yml" 2>/dev/null; then
    unr "contents/${P} (unreadable)"; continue
  fi
  if ! LC_ALL=C awk "$SCAN" "${F}.yml" > "${F}.tsv" 2>/dev/null \
     || ! jq -R -s -c --arg base "$BASE" "$DECIDE" "${F}.tsv" > "${F}.d" 2>/dev/null; then
    unr "${P} could not be scanned"; continue
  fi
  DEC="$(jq -r 'if .error then "error: " + .error elif .expected then "yes" else "no" end' "${F}.d")"
  case "$DEC" in
    "error: "*) unr "${P} could not be scanned (${DEC#error: })"; continue ;;
    yes) ;;
    *) continue ;;
  esac
  # D6: the latest run (highest id) for this path at the head decides.
  RID=""; RST=""
  IFS=$'\t' read -r RID RST < <(jq -r --arg p "$P" \
    '[.[] | select(.path == $p)] | max_by(.id) | [.id // "", .status // ""] | @tsv' "${T}/runs.json")
  echo null > "${F}.j"
  if [ -n "$RID" ]; then
    read_into "${F}.j" 1 "actions/runs/${RID}/jobs?per_page=100" \
      'if all(.[]; type == "object" and (.jobs | type) == "array") then
         [.[].jobs[] | {name: (.name | tostring), status, conclusion}] else error("shape") end' \
      || echo null > "${F}.j"
  fi
  jq -cn --arg path "$P" --arg run "$RID" --arg rst "$RST" --slurpfile d "${F}.d" --slurpfile j "${F}.j" \
    '{path: $path, run: (if $run == "" then null else ($run | tonumber) end), rstatus: $rst,
      paths: $d[0].paths, defined: $d[0].jobs, jobs: $j[0]}' >> "${T}/wfs.jsonl"
done < "${T}/paths"

jq -R -s -c 'split("\n") | map(select(length > 0))' "${T}/unr" > "${T}/unr.json"
jq -s -c '.' "${T}/wfs.jsonl" > "${T}/wfs.json"

# --- Evaluate. Categories: U unreadable, M missing, R red, P pending.
# S1: check-runs are kept per (name, check suite), latest id; a required name
# passes only if every kept run of that name passes, and a commit status never
# overrides a failing check-run. Names are cleaned before they are printed.
# D8': a defined job missing from its run blocks as MISSING, except a job with
# a job-level `if:`: GitHub does not always create one whose condition is false
# (docs.yml `deploy`, observed on PR #2152), so once the run has completed it
# gets a note, and while the run is still going it is PENDING.
EVAL='
def cl: tostring | gsub("\\$\\{\\{\\s*"; "<") | gsub("\\s*\\}\\}"; ">")
  | gsub("[[:cntrl:]\"\u0027\\\\$`{}]"; "") | .[0:120];
def q: "\u0027" + cl + "\u0027";
def at: " at " + $sha;
def st: (.status | cl) + "/" + ((.conclusion // "none") | cl);
def ok($skip): .status == "completed" and (.conclusion == "success" or ($skip and .conclusion == "skipped"));
def disp: if .name == "" then .key else .name end;
def matches($j): . as $n | ($j | disp) as $d |
  if ($d | contains("${{")) then
    ($d | split("${{")[0] | sub("^\\s+"; "") | sub("\\s+$"; "")) as $p | ($p != "" and ($n | startswith($p)))
  else ($n == $d or ($n | startswith($j.key + " (")) or ($n | startswith($d + " / "))) end;
def uniq: reduce .[] as $x ([]; if any(.[]; . == $x) then . else . + [$x] end);
$req[0] as $req | $wfs[0] as $wfs | $st[0] as $st
| ($cr[0] | group_by([.name, .suite]) | map(max_by(.id))) as $crd
| [ $unr[0][] | {k: "B", c: "U", t: ("expected checks unreadable: " + cl)} ] as $A
| [ {k: "E", t: ("required: " + (if ($req | length) == 0 then "(none)" else ($req | map(cl) | join(", ")) end))} ] as $B
| [ $wfs[] | . as $w |
    if $w.run == null then
      if $w.paths then {k: "E", t: ("workflow " + ($w.path | cl) + ": no run" + at + " (paths filter; not expected)")}
      else {k: "B", c: "M", t: ("expected workflow " + ($w.path | q) + " never ran" + at), miss: ($w.path | cl)} end
    else
      {k: "E", t: ("workflow " + ($w.path | cl) + ": run " + ($w.run | tostring))},
      (if $w.jobs == null then empty else
        $w.defined[] as $j | ($j | disp) as $d | [$w.jobs[] | select(.name | matches($j))] as $m |
        if ($m | length) == 0 and $j.cond and $w.rstatus == "completed" then
          {k: "E", t: ("expected " + ($d | q) + ": not created (job-level if:, run " + ($w.run | tostring) + " completed)")}
        elif ($m | length) == 0 and $j.cond then
          {k: "B", c: "P", t: ("expected check " + ($d | q) + " is not created yet; run " + ($w.run | tostring)
             + " is " + ($w.rstatus | cl) + at), name: $d}
        elif ($m | length) == 0 then
          {k: "B", c: "M", t: ("expected check " + ($d | q) + " was never created" + at), miss: ($d | cl), name: $d}
        else $m[] | . as $x | (if any($req[]; . == $x.name) then " (required)" else "" end) as $tag |
          if ok(true) then {k: "E", t: ("expected " + (.name | q) + $tag + ": " + (.conclusion | cl)), job: .name}
          else {k: "B", c: (if .status != "completed" then "P" else "R" end),
                t: ("expected check " + (.name | q) + " is " + st + at), name: .name, job: .name} end
        end end)
    end ] as $C
| [ $C[] | .job // empty ] as $jobs | [ $C[] | .name // empty ] as $blocked
| [ $req[] as $r | select(any($blocked[]; . == $r) | not)
    | [$crd[] | select(.name == $r)] as $rows | [$st[] | select(.context == $r)] as $ss
    | any($jobs[]; . == $r) as $isjob
    | if ($rows | length) + ($ss | length) == 0 then
        {k: "B", c: "M", t: ("expected check " + ($r | q) + " was never created" + at), miss: ($r | cl), name: $r}
      else
        ([$rows[] | select(ok($isjob) | not) | {k: "B", c: (if .status != "completed" then "P" else "R" end),
            t: ("expected check " + ($r | q) + " is " + st + at), name: $r}]
         + [$ss[] | select(.state != "success") | {k: "B", c: (if .state == "pending" then "P" else "R" end),
            t: ("expected check " + ($r | q) + " is " + (.state | cl) + at), name: $r}]) as $bad
        | if ($bad | length) > 0 then $bad[] elif $isjob then empty
          else {k: "E", t: ("expected " + ($r | q) + " (required): success")} end
      end ] as $D
| ($blocked + [ $D[] | .name // empty ]) as $blocked
| [ (if ($crd | length) + ($st | length) == 0 then {k: "B", c: "M", t: ("no checks" + at)} else empty end),
    ($crd[] | . as $x | select(any($blocked[]; . == $x.name) | not) | select(ok(true) | not)
      | {k: "B", c: (if .status != "completed" then "P" else "R" end), t: ("check " + (.name | q) + " is " + st + at)}),
    ($st[] | . as $x | select(any($blocked[]; . == $x.context) | not) | select(.state != "success")
      | {k: "B", c: (if .state == "pending" then "P" else "R" end), t: ("check " + (.context | q) + " is " + (.state | cl) + at)})
  ] as $E
| ($A + $B + $C + $D + $E) as $all
| [ $all[] | .c // empty ] as $cats
| [ $all[] | select(.k == "B") | "blocker: " + .t ] | uniq | length as $n
| { lines: ([ $all[] | (if .k == "B" then "blocker: " else "evidence: " end) + .t ] | uniq),
    missing: ([ $all[] | .miss // empty ] | uniq),
    blockers: $n,
    status: (if any($cats[]; . == "U") then "UNREADABLE" elif any($cats[]; . == "M") then "MISSING"
             elif any($cats[]; . == "R") then "RED" elif any($cats[]; . == "P") then "PENDING"
             elif $n > 0 then "RED" else "GREEN" end) }
'
# One jq run evaluates and prints the lines and the verdict; the output is
# buffered so a failed evaluation prints nothing but the UNREADABLE verdict.
if ! jq -n -r --arg sha "$SHA" --slurpfile req "${T}/req.json" --slurpfile cr "${T}/cr.json" \
     --slurpfile st "${T}/st.json" --slurpfile wfs "${T}/wfs.json" --slurpfile unr "${T}/unr.json" \
     "${EVAL} | .lines[], ({ci_status: .status, missing, blockers} | tojson)" > "${T}/result" 2>/dev/null; then
  fatal "the evaluation failed"
fi
cat "${T}/result"
case "$(tail -n 1 "${T}/result")" in '{"ci_status":"GREEN",'*) exit 0 ;; esac
exit 1
