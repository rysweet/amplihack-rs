#!/usr/bin/env bash
# expected-checks-fixtures.sh — sourced (never run) by the #2186 tests.
#
# Two things live here so the fixture set is written once:
#
#   ec_write_route <path>   writes an executable fake for the `gh api` GETs
#                           that autodrive_expected_checks.sh makes. It serves
#                           files from ${GH_CI_FIX}, answers 404 (exit 1) for a
#                           missing file, and applies `--jq` when one is given.
#   ec_fixture_green <dir> <head_sha>
#                           writes the default GREEN fixture set, shaped like
#                           this repository's own workflows (ci.yml with
#                           `Build ${{ matrix.target }}`, a skipped `Release`,
#                           step `- name:` lines that are not jobs; docs.yml
#                           with a paths filter and no run; recipe-runner-pin;
#                           a pull_request_target workflow; a push-only one).
#   ec_jq <file> <filter> [jq args]   edits one fixture file in place.
#   ec_wf <dir> <file> <yaml>          adds or replaces one workflow file.
#
# Fixtures may contain the literal __HEAD__; the route replaces it with
# ${GH_HEAD} when it serves the file, so a fixture can follow the head the
# caller uses. A literal other SHA is left alone (the "other SHA" cases).
#
# No network, no timers: every answer is a file read.

ec_write_route() { # ec_write_route <path>
  cat > "$1" <<'ROUTE'
#!/usr/bin/env bash
# Fake `gh api` GET router for autodrive_expected_checks.sh (#2186 tests).
[ "${1:-}" = "api" ] || exit 1
shift
JQ=""; EP=""; PAG=0; METHOD_FLAG=0
while [ $# -gt 0 ]; do
  case "$1" in
    --jq|-q) JQ="$2"; shift 2 ;;
    -H|--header|--hostname|-t|--template|--cache) shift 2 ;;
    --paginate) PAG=1; shift ;;
    --slurp|-i|--include|--silent|--verbose) shift ;;
    -X|--method|-f|-F|--field|--raw-field|--input) METHOD_FLAG=1; shift 2 ;;
    -*) shift ;;
    *) [ -z "$EP" ] && EP="$1"; shift ;;
  esac
done
# A write is never served: the helper is read-only.
[ "$METHOD_FLAG" = 0 ] || { echo "gh: fake route refuses non-GET flags" >&2; exit 1; }
FIX="${GH_CI_FIX:?GH_CI_FIX not set}"
P="${EP#/}"; Q=""
case "$P" in *\?*) Q="${P#*\?}"; P="${P%%\?*}" ;; esac
# repos/{owner}/{repo}/X, repos/:owner/:repo/X or repos/o/r/X -> X
case "$P" in repos/*/*/*) P="${P#repos/*/*/}" ;; esac
qv() { printf '%s' "$Q" | tr '&' '\n' | sed -n "s/^$1=//p" | head -n1; }
# Forced failure for an endpoint (a 500, a broken page).
if [ -n "${GH_FAIL_RE:-}" ] && printf '%s' "$P" | grep -qE "${GH_FAIL_RE}"; then
  echo "gh: Server Error (HTTP 500)" >&2; exit 1
fi
nf() { echo '{"message":"Not Found","status":"404"}'; echo "gh: Not Found (HTTP 404)" >&2; exit 1; }
emit() { # emit <file> [jq filter applied before --jq]
  local f="${FIX}/$1" body
  [ -f "$f" ] || nf
  body="$(sed "s/__HEAD__/${GH_HEAD:-__HEAD__}/g" "$f")"
  if [ -n "${2:-}" ]; then body="$(printf '%s' "$body" | jq -c "$2" 2>/dev/null)" || { printf '%s' "$body"; exit 0; }; fi
  if [ -n "$JQ" ]; then printf '%s' "$body" | jq -r "$JQ"; else printf '%s\n' "$body"; fi
  exit 0
}
case "$P" in
  rules/branches/*) emit rules.json ;;
  branches/*/protection*) echo '{"message":"Must have admin rights to Repository.","status":"404"}'; echo "gh: Not Found (HTTP 404)" >&2; exit 1 ;;
  branches/*) emit branch.json ;;
  commits/*/check-runs)
    S="${P#commits/}"; S="${S%%/*}"
    emit check-runs.json "(.check_runs |= map(select(.head_sha == \"${S}\"))) | .total_count = (.check_runs | length)" ;;
  commits/*/status)
    S="${P#commits/}"; S="${S%%/*}"
    [ -f "${FIX}/status-${S}.json" ] && emit "status-${S}.json"
    [ "$S" = "${GH_HEAD:-}" ] && [ -f "${FIX}/status.json" ] && emit status.json
    printf '{"state":"pending","sha":"%s","total_count":0,"statuses":[]}\n' "$S"; exit 0 ;;
  actions/workflows) emit workflows.json ;;
  contents/.github/workflows) emit wf-dir.json ;;
  contents/.github/workflows/*) emit "wf-${P#contents/.github/workflows/}.json" ;;
  actions/runs/*/jobs) R="${P#actions/runs/}"; R="${R%%/*}"; emit "jobs-${R}.json" ;;
  actions/runs)
    HS="$(qv head_sha)"; EV="$(qv event)"
    F='.'
    [ -n "$HS" ] && F="${F} | (.workflow_runs |= map(select(.head_sha == \"${HS}\")))"
    [ -n "$EV" ] && F="${F} | (.workflow_runs |= map(select(.event == \"${EV}\")))"
    emit runs.json "${F} | .total_count = (.workflow_runs | length)" ;;
esac
nf
ROUTE
  chmod +x "$1"
}

ec_jq() { # ec_jq <file> <filter> [extra jq args...]
  local f="$1" flt="$2"; shift 2
  jq "$@" "$flt" "$f" > "$f.new" && mv "$f.new" "$f"
}

# ec_wf <dir> <file> <yaml text>: writes the contents-API answer for one
# workflow file (base64, wrapped at 60 columns as GitHub does) and adds it to
# the directory listing and, unless EC_WF_STATE=none, to actions/workflows.
ec_wf() {
  local d="$1" f="$2" y="$3" b64
  b64="$(printf '%s' "$y" | base64 -w 60)"
  jq -n --arg n "$f" --arg c "$b64" \
    '{name:$n, path:(".github/workflows/" + $n), type:"file", encoding:"base64", content:($c + "\n")}' \
    > "${d}/wf-${f}.json"
  ec_jq "${d}/wf-dir.json" '[.[] | select(.name != $n)] + [{name:$n, path:(".github/workflows/" + $n), type:"file"}]' --arg n "$f"
  if [ "${EC_WF_STATE:-active}" != none ]; then
    ec_jq "${d}/workflows.json" \
      '.workflows = ([.workflows[] | select(.path != (".github/workflows/" + $n))] + [{id:(1000 + (.workflows|length)), name:$n, path:(".github/workflows/" + $n), state:$s}]) | .total_count = (.workflows|length)' \
      --arg n "$f" --arg s "${EC_WF_STATE:-active}"
  fi
}

# ec_run <dir> <run_id> <path> <head_sha> [event]: adds a workflow run.
ec_run() {
  ec_jq "$1/runs.json" \
    '.workflow_runs += [{id:($id|tonumber), name:$p, path:$p, head_sha:$h, event:$e, status:"completed", conclusion:"success"}] | .total_count = (.workflow_runs|length)' \
    --arg id "$2" --arg p "$3" --arg h "$4" --arg e "${5:-pull_request}"
}

# ec_job <dir> <run_id> <name> <status> <conclusion|null> [head] [suite]: adds a
# job to the run AND the matching check-run at the head (same suite per run).
ec_job() {
  local d="$1" r="$2" n="$3" st="$4" co="$5" h="${6:-__HEAD__}" su="${7:-}"
  [ -n "$su" ] || su=$((r + 5000))
  [ -f "${d}/jobs-${r}.json" ] || echo '{"total_count":0,"jobs":[]}' > "${d}/jobs-${r}.json"
  ec_jq "${d}/jobs-${r}.json" \
    '.jobs += [{id:(.jobs|length) + ($r|tonumber)*100, run_id:($r|tonumber), name:$n, status:$st, conclusion:(if $co == "null" then null else $co end), head_sha:$h}] | .total_count = (.jobs|length)' \
    --arg r "$r" --arg n "$n" --arg st "$st" --arg co "$co" --arg h "$h"
  ec_check "$d" "$n" "$st" "$co" "$h" "$su"
}

# ec_check <dir> <name> <status> <conclusion|null> [head] [suite] [app]: adds a
# check-run with no job behind it (another app, or a spoof).
ec_check() {
  local d="$1" n="$2" st="$3" co="$4" h="${5:-__HEAD__}" su="${6:-9001}" app="${7:-github-actions}"
  ec_jq "${d}/check-runs.json" \
    '.check_runs += [{id:((.check_runs|length) + 1 + ($su|tonumber)*1000), name:$n, head_sha:$h, status:$st, conclusion:(if $co == "null" then null else $co end), check_suite:{id:($su|tonumber)}, app:{slug:$app}}] | .total_count = (.check_runs|length)' \
    --arg n "$n" --arg st "$st" --arg co "$co" --arg h "$h" --arg su "$su" --arg app "$app"
}

# ec_required <dir> <context>...: adds classic-protection required contexts.
ec_required() {
  local d="$1" c; shift
  for c in "$@"; do
    ec_jq "${d}/branch.json" \
      '.protected = true | .protection.enabled = true | .protection.required_status_checks.contexts += [$c] | .protection.required_status_checks.checks += [{context:$c, app_id:null}]' \
      --arg c "$c"
  done
}

EC_CI_YML='name: CI

on:
  push:
    branches: [main]
    tags: ["v*"]
  pull_request:
    branches: [main]
  merge_group:
  workflow_dispatch:

permissions:
  contents: read

env:
  CARGO_TERM_COLOR: always

concurrency:
  group: ${{ github.workflow }}-${{ github.event_name }}-${{ github.ref }}
  cancel-in-progress: true

jobs:
  check:
    name: Lint & Format
    runs-on: ubuntu-latest
    steps:
      - name: Checkout
        uses: actions/checkout@v4
      - name: Clippy
        run: cargo clippy

  test:
    name: Test
    runs-on: ubuntu-latest
    steps:
      - name: Run the tests
        run: cargo test

  install-smoke:
    name: Install Smoke Test
    runs-on: ubuntu-latest
    steps:
      - name: Smoke
        run: echo ok

  cross-compile:
    name: Build ${{ matrix.target }}
    runs-on: ubuntu-latest
    strategy:
      matrix:
        target:
          - x86_64-unknown-linux-gnu
          - aarch64-unknown-linux-gnu
    steps:
      - name: Build step, not a job
        run: cargo build

  release:
    name: Release
    if: startsWith(github.ref, '"'"'refs/tags/v'"'"')
    runs-on: ubuntu-latest
    steps:
      - run: echo release
'

EC_DOCS_YML='name: Docs

on:
  push:
    branches: [main]
    paths:
      - "docs/**"
  pull_request:
    paths:
      - "docs/**"
      - "mkdocs.yml"
  workflow_dispatch:

jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - name: Build docs
        run: mkdocs build
  deploy:
    if: github.event_name == '"'"'push'"'"'
    needs: build
    runs-on: ubuntu-latest
    steps:
      - run: echo deploy
'

EC_PIN_YML='name: Recipe runner pin

# Runs on the plain pull_request trigger.
on:
  pull_request:
    branches: [main]
  merge_group:
  push:
    branches: [main]

jobs:
  recipe-runner-pin:
    runs-on: ubuntu-latest
    name: Recipe runner pin ancestry
    steps:
      - name: Check the pin
        run: echo ok
'

EC_SCAN_YML='name: Invisible Character Scan

on:
  pull_request_target:
    types: [opened, synchronize, reopened]

jobs:
  scan:
    name: Scan for invisible characters
    runs-on: ubuntu-latest
    steps:
      - run: echo scan
'

EC_RELEASE_YML='name: Release

on:
  push:
    tags: ["v*"]

jobs:
  publish:
    name: Publish
    runs-on: ubuntu-latest
    steps:
      - run: echo publish
'

# ec_fixture_green <dir> [head]: the default GREEN set. Head defaults to the
# __HEAD__ placeholder, which the route fills from ${GH_HEAD}.
ec_fixture_green() {
  local d="$1" h="${2:-__HEAD__}"
  mkdir -p "$d"
  echo '[]' > "${d}/rules.json"
  echo '{"name":"main","protected":false}' > "${d}/branch.json"
  echo '{"total_count":0,"check_runs":[]}' > "${d}/check-runs.json"
  echo '{"total_count":0,"workflows":[]}' > "${d}/workflows.json"
  echo '[]' > "${d}/wf-dir.json"
  echo '{"total_count":0,"workflow_runs":[]}' > "${d}/runs.json"
  ec_wf "$d" ci.yml "$EC_CI_YML"
  ec_wf "$d" docs.yml "$EC_DOCS_YML"
  ec_wf "$d" recipe-runner-pin.yml "$EC_PIN_YML"
  ec_wf "$d" invisible-char-scan.yml "$EC_SCAN_YML"
  ec_wf "$d" release.yml "$EC_RELEASE_YML"
  ec_run "$d" 1001 .github/workflows/ci.yml "$h"
  ec_job "$d" 1001 "Lint & Format" completed success "$h"
  ec_job "$d" 1001 "Test" completed success "$h"
  ec_job "$d" 1001 "Install Smoke Test" completed success "$h"
  ec_job "$d" 1001 "Build x86_64-unknown-linux-gnu" completed success "$h"
  ec_job "$d" 1001 "Build aarch64-unknown-linux-gnu" completed success "$h"
  ec_job "$d" 1001 "Release" completed skipped "$h"
  ec_run "$d" 1002 .github/workflows/recipe-runner-pin.yml "$h"
  ec_job "$d" 1002 "Recipe runner pin ancestry" completed success "$h"
  ec_required "$d" "Lint & Format" "Test" "Install Smoke Test" \
    "Build x86_64-unknown-linux-gnu" "Build aarch64-unknown-linux-gnu"
}

# ec_drop_job <dir> <run_id> <name>: the job was never created (no job, no
# check-run).
ec_drop_job() {
  ec_jq "$1/jobs-$2.json" '.jobs |= map(select(.name != $n)) | .total_count = (.jobs|length)' --arg n "$3"
  ec_jq "$1/check-runs.json" '.check_runs |= map(select(.name != $n)) | .total_count = (.check_runs|length)' --arg n "$3"
}

# ec_set_job <dir> <run_id> <name> <status> <conclusion|null>: changes a job
# and its check-run.
ec_set_job() {
  local co='(if $co == "null" then null else $co end)'
  ec_jq "$1/jobs-$2.json" "(.jobs[] | select(.name == \$n)) |= (.status = \$st | .conclusion = ${co})" \
    --arg n "$3" --arg st "$4" --arg co "$5"
  ec_jq "$1/check-runs.json" "(.check_runs[] | select(.name == \$n)) |= (.status = \$st | .conclusion = ${co})" \
    --arg n "$3" --arg st "$4" --arg co "$5"
}
