#!/bin/bash -p
# autodrive_expected_checks.sh — "CI green" for one head SHA (issue #2186).
#
#   /bin/bash -p autodrive_expected_checks.sh <HEAD_SHA> <BASE_REF>
#
# CI green means every EXPECTED check exists at HEAD_SHA and passed: the
# required checks of BASE_REF (rulesets plus classic protection as the branch
# object reports it), and every job of each active workflow at HEAD_SHA that
# runs on pull_request for BASE_REF. A missing, pending or failing check, any
# other pending or failing check at the head, zero checks, and anything that
# cannot be read all block. `neutral` passes, as GitHub passes it.
#
# Output (stdout): `evidence:` lines, `blocker:` lines, then ONE final JSON
# line {"ci_status":"GREEN|RED|PENDING|MISSING|UNREADABLE","missing":[..],
# "blockers":N}; callers read only that line. Exit 0 only when GREEN, 1
# otherwise, 2 for bad arguments (before any gh call). Read-only `gh api` GETs;
# it never sleeps, loops to wait or repeats a request. Untrusted text is data:
# cleaned before it is printed, never evaluated. Needs jq 1.6 (@base64d).
# See docs/reference/auto-drive-to-merge.md ("Expected checks at the head").

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
case "$BASE" in ''|-*|/*|*/|*..*|*//*|*.lock|*[!A-Za-z0-9._/-]*) usage "BASE_REF is not a plain branch name" ;; esac

# Stop with an UNREADABLE verdict (a literal, so it works without jq).
fatal() { echo "blocker: expected checks unreadable: $1"; printf '%s\n' '{"ci_status":"UNREADABLE","missing":[],"blockers":1}'; exit 1; }
command -v jq >/dev/null 2>&1 || fatal "jq is not installed"
[ "$(printf '%s' '"YQ=="' | jq -r '@base64d' 2>/dev/null)" = "a" ] || fatal "jq 1.6 or later is required (@base64d)"
T="$(mktemp -d "${TMPDIR:-/tmp}/autodrive-ec.XXXXXX" 2>/dev/null)" || fatal "no private temporary directory"
trap 'rm -rf -- "$T"' EXIT
: > "${T}/unr"
unr() { printf '%s\n' "$1" >> "${T}/unr"; }

# get <paginate 0|1> <endpoint>: sets GOT to a file holding the answer's pages as one JSON array
# (named by a counter, never by response data). Returns 0, 4 for a 404, 1 for any other failure.
N=0; GOT=""
get() {
  local rc; N=$((N + 1)); GOT="${T}/r${N}"
  if [ "$1" = 1 ]; then gh api --paginate "$2"; else gh api "$2"; fi > "${GOT}.raw" 2> "${GOT}.err"; rc=$?
  if [ "$rc" -ne 0 ]; then grep -qF '(HTTP 404)' "${GOT}.err" && return 4; return 1; fi
  # Only gh's 404 above may return 4, so a jq failure is always 1.
  [ -s "${GOT}.raw" ] && jq -s . "${GOT}.raw" > "$GOT" 2>/dev/null || return 1
}

# read_into <out> <paginate> <endpoint> <jq filter over the pages>: writes the
# filter's result, or records the endpoint as unreadable and writes [].
read_into() {
  local out="$1" ep="$3" rc; get "$2" "repos/{owner}/{repo}/${ep}"; rc=$?
  [ "$rc" -eq 0 ] && jq -c --arg sha "$SHA" "$4" "$GOT" > "$out" 2>/dev/null && return 0
  case "$rc" in 4) unr "${ep%%\?*} (not found)" ;; 0) unr "${ep%%\?*} (unexpected answer)" ;; *) unr "${ep%%\?*} (gh failed)" ;; esac
  echo '[]' > "$out"; return 1
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

# --- What exists at the head: check-runs (filter=all, so S1 sees every suite) and statuses.
read_into "${T}/cr.json" 1 "commits/${SHA}/check-runs?per_page=100&filter=all" \
  'if all(.[]; type == "object" and (.check_runs | type) == "array") then
     [.[].check_runs[] | {id, name, status, conclusion, suite: (.check_suite.id // null)}]
   else error("shape") end'
read_into "${T}/st.json" 1 "commits/${SHA}/status?per_page=100" \
  'if all(.[]; type == "object" and (.statuses | type) == "array") then [.[].statuses[] | {context, state}] else error("shape") end'

# --- Workflows: active ones (actions/workflows), files at the head, and the
# pull_request runs at the head. A 404 on the workflows directory is the one
# 404 that means "none"; every other unreadable answer blocks.
read_into "${T}/active.json" 1 "actions/workflows?per_page=100" \
  'if all(.[]; type == "object" and (.workflows | type) == "array") then
     [.[].workflows[] | select(.state == "active") | .path | strings] else error("shape") end'
get 0 "repos/{owner}/{repo}/contents/.github/workflows?ref=${SHA}"; rc=$?
if [ "$rc" -eq 4 ]; then echo '[]' > "${T}/dir.json"
elif [ "$rc" -ne 0 ] || ! jq -c '.[0] | if type == "array" then [.[] | .path | strings | select(test("\\.ya?ml$"))]
     else error("shape") end' "$GOT" > "${T}/dir.json" 2>/dev/null; then
  unr "contents/.github/workflows (unreadable)"; echo '[]' > "${T}/dir.json"
fi
# S2: a workflow path must be a plain file name directly in .github/workflows.
WF_RE='^\\.github/workflows/[A-Za-z0-9._-]+\\.ya?ml$'
[ "$(jq -r "[.[] | select((test(\"${WF_RE}\") and (contains(\"..\") | not)) | not)] | length" "${T}/dir.json")" = 0 ] \
  || unr "a workflow path is not a plain file in .github/workflows"
jq -r --slurpfile w "${T}/active.json" ".[] | select(test(\"${WF_RE}\") and (contains(\"..\") | not))
  | select(. as \$p | any(\$w[0][]; . == \$p))" "${T}/dir.json" > "${T}/paths"
read_into "${T}/runs.json" 1 "actions/runs?head_sha=${SHA}&event=pull_request&per_page=100" \
  'if all(.[]; type == "object" and (.workflow_runs | type) == "array") then [.[].workflow_runs[]]
     | if all(.[]; ((.id | tostring) | test("^[0-9]+$")) and (.path | type) == "string")
       then [.[] | select(.head_sha == $sha) | {id: (.id | tonumber), path, status, conclusion, suite: (.check_suite_id // null)}]
       else error("run id or path") end
   else error("shape") end'

# --- The YAML scanner (D1): the top-level `on:` (scalar, flow or block list, mapping; also "on",
# 'on', true) with pull_request's child keys, and each top-level `jobs:` key with its own `name:`.
# Tab-separated records: O trigger, B/I branches(-ignore) entry, P paths, Y types, T types entry,
# J key name, E reason, X reason under jobs: (blocks only an expected workflow). No system(),
# command getline or redirection: the YAML is data.
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
function flush() { if (jk != "") { gsub(/\t/, " ", jk); gsub(/\t/, " ", jn); print "J\t" jk "\t" jn }; jk = "" }
function bsend() {
  if (be) print "X\tjob " jk ": name: keeps a line break"; else if (bs != "f" && !bn) print "X\tjob " jk ": name: is an empty block scalar"
  bs = ""
}
BEGIN { SQ = sprintf("%c", 39); KEYRE = "^[^ #-][^:]*:([ \t]|$)" }
{ sub(/\r$/, "") }
# Lines deeper than a job-level name: continue it: a block scalar (bs | or >, indent bc) or a
# plain or quoted value (bs f, raw fr), joined by one space. A line break the value keeps (blank,
# deeper or second | line) is refused before the first ${{ only: no text after it is matched.
bs != "" {
  if ($0 ~ /^[ \t]*$/) { bg = 1; next }
  i = ind($0)
  if (i <= bi) bsend()
  else {
    if (bc == 0) bc = i
    if ((bg || (bs != "f" && (i != bc || (bs == "|" && bn)))) && !index(jn, "${{")) be = 1
    else if (bs == "f") { fr = fr " " trim(substr($0, i + 1)); jn = val(fr) }
    else jn = (bn ? jn " " : "") trim(substr($0, i + 1))
    bn++; next
  }
}
/^[ \t]*(#|$)/ { next }
{
  i = ind($0); line = substr($0, i + 1)
  # A block list under on: may sit at the key indent (YAML 1.2 example 8.22).
  if (i == 0 && sec == "on" && ol && oi == 0 && line ~ /^-([ \t]|$)/) { print "O\t" val(substr(line, 2)); next }
  if (i == 0) {
    flush(); sec = ""
    if (line !~ KEYRE) next
    k = key(line); r = rest(line)
    if (k == "on" || k == "true") {
      sawon = 1; sec = "on"; oi = 0; ol = (r == "")
      if (substr(r, 1, 1) == "{") print "E\tflow mapping under on:"; else items(r, "O")
    } else if (k == "jobs") {
      sawjobs = 1; sec = "jobs"; ji = 0
      if (r != "") print "X\tinline jobs: value"
    }
    next
  }
  if (sec == "on") {
    if (oi == 0) oi = i
    if (i < oi) { print "E\tbad indentation under on:"; next }
    if (i == oi) {
      pr = 0; lk = ""
      if (line ~ /^-([ \t]|$)/) { print "O\t" val(substr(line, 2)); next }
      if (line !~ KEYRE) { print "E\tunreadable on: entry"; next }
      k = key(line); r = rest(line); print "O\t" k
      if (k == "pull_request") { pr = 1; pi = 0; if (r != "" && r != "null" && r != "~" && r != "{}") print "E\tinline pull_request: value" }
      next
    }
    if (!pr) next
    if (pi == 0) pi = i
    if (i == pi && line !~ /^-([ \t]|$)/) {
      lk = ""
      if (line !~ KEYRE) next
      k = key(line); r = rest(line)
      if (k == "branches") { lk = "B"; items(r, "B") }
      else if (k == "branches-ignore") { lk = "I"; items(r, "I") }
      else if (k == "types") { lk = "T"; print "Y\t1"; items(r, "T") }
      else if (k == "paths" || k == "paths-ignore") print "P\t1"
      next
    }
    if (lk != "" && line ~ /^-([ \t]|$)/) print lk "\t" val(substr(line, 2))
    else if (lk != "") print "E\tunreadable list under pull_request:"
    next
  }
  if (sec == "jobs") {
    if (ji == 0) ji = i
    if (i == ji) {
      flush()
      if (line !~ KEYRE) { print "X\tunreadable job key"; next }
      jk = key(line); jn = ""; bi = 0; njobs++
      next
    }
    if (i < ji || jk == "") { print "X\tbad indentation under jobs:"; next }
    if (bi == 0) bi = i
    if (i == bi && line ~ /^name[ \t]*:/) {
      jn = rest(line); fr = line; sub(/^name[ \t]*:[ \t]*/, "", fr); bs = "f"; bc = bn = bg = be = 0
      r = fr; sub(/[ \t]+#.*$/, "", r); sub(/[ \t]+$/, "", r)
      if (r ~ /^[|>][-+1-9]*$/) { bs = substr(r, 1, 1); jn = ""; if (match(r, /[1-9]/)) bc = bi + substr(r, RSTART, 1) }
    }
  }
}
END { if (bs != "") bsend(); flush(); if (!sawon) print "E\tno top-level on: key"; if (!sawjobs || njobs == 0) print "X\tno jobs under jobs:" }
'
# Expected (D2, D3)? pull_request (not _target) in `on`, and the base admitted by GitHub's filter
# rules (`*` stops at `/`, `**` does not, the last matching `branches` pattern decides). `cond` names
# why it is expected only if it ran: paths (D4), types: that can skip a head, or a pattern jq cannot
# compile. An X reason makes only an expected workflow unreadable: the others run no job here.
DECIDE='def re: "^" + (gsub("(?<c>[.(){}|^$\\\\])"; "\\\(.c)") | gsub("\\*\\*"; "\u0001")
    | gsub("\\*"; "[^/]*") | gsub("\u0001"; ".*")) + "$";
  def admits($ps): reduce $ps[] as $p (false; if . == null then null else
    (try ($base | test($p | ltrimstr("!") | re)) catch null) as $m
    | if $m == null then null elif $m then ($p | startswith("!") | not) else . end end);
  split("\n") | map(select(length > 0) | split("\t")) as $r
  | [$r[] | select(.[0] == "E") | .[1]] as $e | [$r[] | select(.[0] == "X") | .[1]] as $x
  | if ($e | length) > 0 then {error: $e[0]} else
    [$r[] | select(.[0] == "B") | .[1]] as $b | [$r[] | select(.[0] == "I") | .[1]] as $i
    | [$r[] | select(.[0] == "T") | .[1]] as $t
    | (if ($b | length) == 0 then true else admits($b) end) as $in
    | (if any($i[]; startswith("!")) then null else admits($i) end) as $out
    | {expected: (any($r[]; .[0] == "O" and .[1] == "pull_request") and $in != false and $out != true),
       cond: ([(if $in == null or $out == null then "branch pattern" else empty end),
         (if any($r[]; .[0] == "Y") and any("opened", "synchronize", "reopened"; . as $x | any($t[]; . == $x) | not)
          then "types filter" else empty end),
         (if any($r[]; .[0] == "P") then "paths filter" else empty end)] | join(", ")),
       jobs: [$r[] | select(.[0] == "J") | {key: .[1], name: (.[2] // "")}]}
    | if .expected and ($x | length) > 0 then {error: $x[0]} else . end end'

: > "${T}/wfs.jsonl"; W=0
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
  case "$DEC" in "error: "*) unr "${P} could not be scanned (${DEC#error: })"; continue ;; yes) ;; *) continue ;; esac
  # D6: the highest-id run of this path that holds evidence decides. Ids are creation order, not
  # the order a concurrency group runs or cancels in, so a run cancelled with zero jobs holds none
  # and the next one down decides. If every run is such, the highest id decides (and blocks).
  IDS="$(jq -r --arg p "$P" '[.[] | select(.path == $p) | .id] | sort | reverse | map(tostring) | join(" ")' "${T}/runs.json")"
  RID=""; SKIP=""; ALL=1; echo null > "${F}.j"
  for R in $IDS; do
    [ -z "$RID" ] || SKIP="${SKIP}${SKIP:+,}${RID}"
    RID="$R"
    read_into "${F}.j" 1 "actions/runs/${R}/jobs?per_page=100" \
      'if all(.[]; type == "object" and (.jobs | type) == "array") then
         [.[].jobs[] | {name: (.name | tostring), status, conclusion}] else error("shape") end' \
      || { echo null > "${F}.j"; ALL=0; break; }
    jq -e --argjson r "$R" --slurpfile runs "${T}/runs.json" '. == [] and any($runs[0][]; .id == $r
      and .status == "completed" and .conclusion == "cancelled")' "${F}.j" > /dev/null || { ALL=0; break; }
  done
  [ "$ALL" = 0 ] || [ -z "$RID" ] || { RID="${IDS%% *}"; SKIP=""; }
  jq -cn --arg p "$P" --argjson rid "${RID:-null}" --arg skip "$SKIP" --slurpfile runs "${T}/runs.json" \
    --slurpfile d "${F}.d" --slurpfile j "${F}.j" '{path: $p, run: ([$runs[0][] | select(.id == $rid)][0]),
      skipped: ($skip | split(",")), cond: $d[0].cond, defined: $d[0].jobs, jobs: $j[0]}' >> "${T}/wfs.jsonl"
done < "${T}/paths"

jq -R -s -c 'split("\n") | map(select(length > 0))' "${T}/unr" > "${T}/unr.json"
jq -s -c '.' "${T}/wfs.jsonl" > "${T}/wfs.json"

# --- Evaluate. Categories: U unreadable, M missing, R red, P pending. S1:
# check-runs kept per (name, suite), latest id; a required name passes only if
# every kept run passes; a status never overrides a failing check-run. S3: the
# suites of a path's runs other than the one D6 chose are dropped. D8: a job
# absent from a completed run is MISSING, `if:` or not; from a running one,
# PENDING.
EVAL='
def cl: tostring | gsub("\\$\\{\\{\\s*"; "<") | gsub("\\s*\\}\\}"; ">")
  | gsub("[[:cntrl:]\"\u0027\\\\$`{}]"; "") | .[0:120];
def q: "\u0027" + cl + "\u0027";
def at: " at " + $sha;
def st: (.status | cl) + "/" + ((.conclusion // "none") | cl);
def ok($skip): .status == "completed" and (.conclusion == "success" or .conclusion == "neutral"
  or ($skip and .conclusion == "skipped"));
def disp: if .name == "" then .key else .name end;
# pre: the literal text before the first ${{ (trailing space kept), null when
# there is none. score($j): [3] rule 1, [2] rules 3-4, [1, prefix length] rule
# 2, [] none. A reported job meets only the defined jobs that match it best.
def pre: disp | if contains("${{") then split("${{")[0] | sub("^\\s+"; "") else null end;
def score($j): . as $n | ($j | disp) as $d | ($j | pre) as $p |
  if $p != null then (if $p != "" and (($n | startswith($p)) or $n == ($p | sub("\\s+$"; "")))
                      then [1, ($p | length)] else [] end)
  elif $n == $d then [3]
  elif ($n | startswith($j.key + " (")) or ($n | startswith($d + " (")) or ($n | startswith($d + " / ")) then [2]
  else [] end;
def uniq: reduce .[] as $x ([]; if any(.[]; . == $x) then . else . + [$x] end);
$req[0] as $req | $wfs[0] as $wfs | $st[0] as $st
| ([ $wfs[] | select(.run != null) | {key: .path, value: .run.id} ] | from_entries) as $pick
| [ $runs[0] | group_by(.path)[] | ($pick[.[0].path] // max_by(.id).id) as $keep
    | .[] | select(.id != $keep) | .suite | select(. != null) ] as $old
| ($cr[0] | map(select(.suite as $s | any($old[]; . == $s) | not))
   | group_by([.name, .suite]) | map(max_by(.id))) as $crd
| [ $unr[0][] | {k: "B", c: "U", t: ("expected checks unreadable: " + cl)} ] as $A
| [ {k: "E", t: ("required: " + (if ($req | length) == 0 then "(none)" else ($req | map(cl) | join(", ")) end))} ] as $B
| [ $wfs[] | . as $w |
    if $w.run == null then
      if $w.cond != "" then {k: "E", t: ("workflow " + ($w.path | cl) + ": no run" + at + " (" + $w.cond + "; not expected)")}
      else {k: "B", c: "M", t: ("expected workflow " + ($w.path | q) + " never ran" + at), miss: ($w.path | cl)} end
    else
      ($w.skipped[] | {k: "E", t: ("workflow " + ($w.path | cl) + ": run " + . + " was cancelled before it created any job; run "
         + ($w.run.id | tostring) + " decides")}),
      {k: "E", t: ("workflow " + ($w.path | cl) + ": run " + ($w.run.id | tostring) + " is " + ($w.run | st))},
      (if $w.jobs == null then empty
       elif $w.jobs == [] and $w.run.status == "completed" and $w.run.conclusion == "cancelled" then
         {k: "B", c: "M", t: ("expected workflow " + ($w.path | q) + " run " + ($w.run.id | tostring)
            + " was cancelled before it created any job" + at + "; re-run it"), miss: ($w.path | cl)}
       else
        # An expression-only name ($x) is met by jobs no other defined job matches. A reported job
        # two defined jobs match equally ($tie) belongs to neither: both are unreadable, not both met.
        ([$w.defined[] | select(pre == "")] | length) as $nx
        | ($w.jobs | map(.name as $n | [$w.defined[] | . as $k | $n | score($k)] as $s | ($s | max) as $b
            | .best = $b | .tied = ([$s[] | select(. == $b)] | length))) as $rj
        | $w.defined[] as $j | ($j | disp) as $d | (($j | pre) == "") as $x
        | [$rj[] | if $x then select(.best == []) else select(.best != [] and (.name | score($j)) == .best) end] as $m
        | [$m[] | select(.best != [] and .tied > 1) | .name] as $tie | ("expected checks unreadable: job " + ($d | q) + " of " + ($w.path | cl)) as $u |
        if $x and $nx > 1 then
          {k: "B", c: "U", t: ($u + " cannot be matched: " + ($nx | tostring) + " of its jobs are named only by an expression; give them literal names"), name: $d}
        elif ($tie | length) > 0 then {k: "B", c: "U", t: ($u + " cannot be matched: another of its jobs matches "
             + ($tie[0] | q) + " as well; give them distinct literal names"), name: $d}
        elif ($m | length) == 0 and $w.run.status != "completed" then
          {k: "B", c: "P", t: ("expected check " + ($d | q) + " is not created yet; run " + ($w.run.id | tostring)
             + " is " + ($w.run.status | cl) + at), name: $d}
        elif ($m | length) == 0 and $x then {k: "B", c: "M", t: ("expected job " + ($j.key | q) + " of " + ($w.path | cl)
             + " was never created" + at + ": its name " + ($d | q) + " is only an expression and every job of run "
             + ($w.run.id | tostring) + " matches another job; if it ran, give it a literal name"), miss: ($j.key | cl), name: $d}
        elif ($m | length) == 0 then
          {k: "B", c: "M", t: ("expected check " + ($d | q) + " was never created" + at), miss: ($d | cl), name: $d}
        else $m[] | . as $x | (if any($req[]; . == $x.name) then " (required)" else "" end) as $tag |
          if ok(true) then {k: "E", t: ("expected " + (.name | q) + $tag + ": " + (.conclusion | cl)), job: .name}
          else {k: "B", c: (if .status != "completed" then "P" else "R" end),
                t: ("expected check " + (.name | q) + " is " + st + at), name: .name, job: .name} end
        end end)
    end ] as $C
| [ if any($C[]; .c == "M" and (.t | contains(" never "))) then {k: "E", t: ("jobs are read from the workflow files at"
      + " the head, but pull_request runs build the merge commit with " + ($base | cl) + ": if " + ($base | cl)
      + " changed a missing job or workflow since this branch forked, merge " + ($base | cl) + " in")} else empty end ] as $H
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
          else {k: "E", t: ("expected " + ($r | q) + " (required): " + ([$rows[].conclusion, $ss[].state] | map(cl) | uniq | join(", ")))} end
      end ] as $D
| ($blocked + [ $D[] | .name // empty ]) as $blocked
| [ (if ($crd | length) + ($st | length) == 0 then {k: "B", c: "M", t: ("no checks" + at)} else empty end),
    ($crd[] | . as $x | select(any($blocked[]; . == $x.name) | not) | select(ok(true) | not)
      | {k: "B", c: (if .status != "completed" then "P" else "R" end), t: ("check " + (.name | q) + " is " + st + at)}),
    ($st[] | . as $x | select(any($blocked[]; . == $x.context) | not) | select(.state != "success")
      | {k: "B", c: (if .state == "pending" then "P" else "R" end), t: ("check " + (.context | q) + " is " + (.state | cl) + at)})
  ] as $E
| ($A + $B + $C + $H + $D + $E) as $all
| [ $all[] | .c // empty ] as $cats
| [ $all[] | select(.k == "B") | "blocker: " + .t ] | uniq | length as $n
| { lines: ([ $all[] | (if .k == "B" then "blocker: " else "evidence: " end) + .t ] | uniq),
    missing: ([ $all[] | .miss // empty ] | uniq),
    blockers: $n,
    status: (if any($cats[]; . == "U") then "UNREADABLE" elif any($cats[]; . == "M") then "MISSING"
             elif any($cats[]; . == "R") then "RED" elif any($cats[]; . == "P") then "PENDING"
             elif $n > 0 then "RED" else "GREEN" end) }
'
# One jq run prints the lines and the verdict, buffered: a failed evaluation prints only UNREADABLE.
if ! jq -n -r --arg sha "$SHA" --arg base "$BASE" --slurpfile req "${T}/req.json" --slurpfile cr "${T}/cr.json" \
     --slurpfile st "${T}/st.json" --slurpfile wfs "${T}/wfs.json" --slurpfile unr "${T}/unr.json" \
     --slurpfile runs "${T}/runs.json" \
     "${EVAL} | .lines[], ({ci_status: .status, missing, blockers} | tojson)" > "${T}/result" 2>/dev/null; then
  fatal "the evaluation failed"
fi
cat "${T}/result"
case "$(tail -n 1 "${T}/result")" in '{"ci_status":"GREEN",'*) exit 0 ;; esac
exit 1
