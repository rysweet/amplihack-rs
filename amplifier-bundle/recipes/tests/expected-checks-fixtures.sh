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
    # The helper must ask for every run (filter=all), not GitHub's latest only.
    [ "$(qv filter)" = all ] || { echo "gh: fake route wants filter=all on check-runs" >&2; exit 1; }
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

# ec_run <dir> <run_id> <path> <head_sha> [event]: adds a workflow run. Its
# check suite is run_id + 5000, the suite ec_job gives the run's check-runs.
ec_run() {
  ec_jq "$1/runs.json" \
    '.workflow_runs += [{id:($id|tonumber), name:$p, path:$p, head_sha:$h, event:$e, status:"completed", conclusion:"success", check_suite_id:(($id|tonumber) + 5000)}] | .total_count = (.workflow_runs|length)' \
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

# ---------------------------------------------------------------------------
# Real workflow shapes (crusty round 4). Each YAML below is copied byte for
# byte from the named repository at the named commit; where a file is cut,
# whole jobs were removed and nothing inside a kept job was changed.

# python/cpython .github/workflows/build.yml at 8c99e7c713b2 (PR #161842),
# cut to lines 1-51, 199-317, 589-615 and 654-724: three jobs named by a
# folded block scalar (`name: >-`), `Windows MSI${{ '' }}`, which shares the
# `Windows ` prefix, literal names that start like the folded ones, and deep
# `>-` block scalars inside a step that are not job names.
EC_CPYTHON_YML="$(cat <<'YML'
name: Tests

on:
  workflow_dispatch:
  push:
    branches:
    - 'main'
    - '3.*'
  pull_request:
    branches:
    - 'main'
    - '3.*'

permissions:
  contents: read

concurrency:
  # https://docs.github.com/en/actions/writing-workflows/workflow-syntax-for-github-actions#concurrency
  # 'group' must be a key uniquely representing a PR or push event.
  # github.workflow is the workflow name
  # github.actor is the user invoking the workflow
  # github.head_ref is the source branch of the PR or otherwise blank
  # github.run_id is a unique number for the current run
  group: ${{ github.workflow }}-${{ github.actor }}-${{ github.head_ref || github.run_id }}
  cancel-in-progress: true

env:
  FORCE_COLOR: 1

jobs:
  build-context:
    name: Change detection
    # To use boolean outputs from this job, parse them as JSON.
    # Here's some examples:
    #
    #   if: fromJSON(needs.build-context.outputs.run-docs)
    #
    #   ${{
    #        fromJSON(needs.build-context.outputs.run-tests)
    #        && 'truthy-branch'
    #        || 'falsy-branch'
    #   }}
    #
    uses: ./.github/workflows/reusable-context.yml

  check-docs:
    name: Docs
    needs: build-context
    if: fromJSON(needs.build-context.outputs.run-docs)
    uses: ./.github/workflows/reusable-docs.yml


  build-windows:
    name: >-
      Windows
      ${{ fromJSON(matrix.free-threading) && '(free-threading)' || '' }}
    needs: build-context
    if: fromJSON(needs.build-context.outputs.run-windows-tests)
    strategy:
      fail-fast: false
      matrix:
        arch:
          - x64
          - Win32
          - arm64
        free-threading:
          - false
          - true
        interpreter:
          - switch-case
        exclude:
          # Skip Win32 on free-threaded builds
          - { arch: Win32, free-threading: true }
        include:
          # msvc::musttail is currently only supported on x64,
          # and only supported on 3.15+.
          - { arch: x64, free-threading: false, interpreter: tail-call }
          - { arch: x64, free-threading: true,  interpreter: tail-call }
    uses: ./.github/workflows/reusable-windows.yml
    with:
      arch: ${{ matrix.arch }}
      free-threading: ${{ matrix.free-threading }}
      interpreter: ${{ matrix.interpreter }}

  build-windows-msi:
    # ${{ '' } is a hack to nest jobs under the same sidebar category.
    name: Windows MSI${{ '' }}  # zizmor: ignore[obfuscation]
    needs: build-context
    if: fromJSON(needs.build-context.outputs.run-windows-msi)
    strategy:
      fail-fast: false
      matrix:
        arch:
        - x86
        - x64
        - arm64
    uses: ./.github/workflows/reusable-windows-msi.yml
    with:
      arch: ${{ matrix.arch }}

  build-macos:
    name: >-
      macOS
      ${{ fromJSON(matrix.free-threading) && '(free-threading)' || '' }}
    needs: build-context
    if: needs.build-context.outputs.run-macos == 'true'
    strategy:
      fail-fast: false
      matrix:
        # macos-26 is Apple Silicon, macos-26-intel is Intel.
        # macos-26-intel only runs tests against the GIL-enabled CPython.
        os:
        - macos-26
        - macos-26-intel
        free-threading:
        - false
        - true
        exclude:
        - os: macos-26-intel
          free-threading: true
    uses: ./.github/workflows/reusable-macos.yml
    with:
      free-threading: ${{ matrix.free-threading }}
      os: ${{ matrix.os }}

  build-ubuntu:
    name: >-
      Ubuntu
      ${{ fromJSON(matrix.free-threading) && '(free-threading)' || '' }}
      ${{ fromJSON(matrix.bolt) && '(bolt)' || '' }}
    needs: build-context
    if: needs.build-context.outputs.run-ubuntu == 'true'
    strategy:
      fail-fast: false
      matrix:
        bolt:
        - false
        - true
        free-threading:
        - false
        - true
        os:
        - ubuntu-26.04
        - ubuntu-26.04-arm
        exclude:
        # Do not test BOLT with free-threading, to conserve resources
        - bolt: true
          free-threading: true
        # BOLT currently crashes during instrumentation on aarch64
        - os: ubuntu-26.04-arm
          bolt: true
        include:
        # Enable CPU-intensive tests on ARM (default build only)
        - os: ubuntu-26.04-arm
          bolt: false
          free-threading: false
          test-opts: '-u cpu'
    uses: ./.github/workflows/reusable-ubuntu.yml
    with:
      bolt-optimizations: ${{ matrix.bolt }}
      free-threading: ${{ matrix.free-threading }}
      os: ${{ matrix.os }}
      test-opts: ${{ matrix.test-opts || '' }}

  build-ubuntu-installed:
    name: 'Ubuntu (installed)'
    needs: build-context
    if: needs.build-context.outputs.run-ubuntu == 'true'
    uses: ./.github/workflows/reusable-install.yml


  build-san:
    # ${{ '' } is a hack to nest jobs under the same sidebar category.
    name: Sanitizers${{ '' }}  # zizmor: ignore[obfuscation]
    needs: build-context
    if: needs.build-context.outputs.run-ubuntu == 'true'
    strategy:
      fail-fast: false
      matrix:
        check-name:
        - Thread
        free-threading:
        - false
        - true
        sanitizer:
        - TSan
        include:
        - check-name: Undefined behavior
          sanitizer: UBSan
          free-threading: false
        - check-name: Memory
          sanitizer: MSan
          free-threading: false
    uses: ./.github/workflows/reusable-san.yml
    with:
      sanitizer: ${{ matrix.sanitizer }}
      free-threading: ${{ matrix.free-threading }}


  all-required-green:  # This job does nothing and is only used for the branch protection
    name: All required checks pass
    runs-on: ubuntu-latest
    timeout-minutes: 5
    needs:
    - build-context  # Transitive dependency, needed to access `run-tests` value
    - check-docs
    - check-autoconf-regen
    - check-generated-files
    - check-c-api-docs
    - build-windows
    - build-windows-msi
    - build-macos
    - build-ubuntu
    - build-ubuntu-installed
    - build-ubuntu-ssltests
    - build-ios
    - build-emscripten
    - build-wasi
    - test-hypothesis
    - build-asan
    - build-san
    - cross-build-linux
    if: always()

    steps:
    - name: Check whether the needed jobs succeeded or failed
      uses: re-actors/alls-green@b5b5b37504aa4183270bd3d855c52a67f212be35
      with:
        allowed-failures: >-
          build-android,
          build-emscripten,
          build-windows-msi,
          build-ubuntu-ssltests,
          test-hypothesis,
        allowed-skips: >-
          ${{ !fromJSON(needs.build-context.outputs.run-docs) && 'check-docs,' || '' }}
          ${{
            needs.build-context.outputs.run-tests != 'true'
            && '
            check-autoconf-regen,
            check-generated-files,
            '
            || ''
          }}
          ${{
            !fromJSON(needs.build-context.outputs.run-tests)
            && !fromJSON(needs.build-context.outputs.run-docs)
            && 'check-c-api-docs,'
            || ''
          }}
          ${{ !fromJSON(needs.build-context.outputs.run-windows-tests) && 'build-windows,' || '' }}
          ${{ !fromJSON(needs.build-context.outputs.run-macos) && 'build-macos,' || '' }}
          ${{
            !fromJSON(needs.build-context.outputs.run-ubuntu)
            && '
            build-ubuntu,
            build-ubuntu-installed,
            build-ubuntu-ssltests,
            test-hypothesis,
            build-asan,
            build-san,
            cross-build-linux,
            '
            || ''
          }}
          ${{ !fromJSON(needs.build-context.outputs.run-android) && 'build-android,' || '' }}
          ${{ !fromJSON(needs.build-context.outputs.run-ios) && 'build-ios,' || '' }}
          ${{ !fromJSON(needs.build-context.outputs.run-emscripten) && 'build-emscripten,' || '' }}
          ${{ !fromJSON(needs.build-context.outputs.run-wasi) && 'build-wasi,' || '' }}
        jobs: ${{ toJSON(needs) }}
YML
)"

# ec_cpython <dir> [run_id]: adds build.yml and its run, with the jobs GitHub
# reported for run 43126805629 at that head (the jobs of the kept job keys).
ec_cpython() {
  local d="$1" r="${2:-1010}" n
  ec_wf "$d" build.yml "$EC_CPYTHON_YML"
  ec_run "$d" "$r" .github/workflows/build.yml __HEAD__
  while IFS= read -r n; do
    ec_job "$d" "$r" "$n" completed success
  done <<'JOBS'
Change detection / Create context from changed files
Docs / Docs
Docs / Doctest
Docs / Check EPUB
Docs / Check for removed HTML IDs / Check for removed HTML IDs
Windows / Build and test (Win32, switch-case)
Windows / Build and test (x64, tail-call)
Windows / Build and test (x64, switch-case)
Windows / Build and test (arm64, switch-case)
Windows (free-threading) / Build and test (arm64, switch-case)
Windows (free-threading) / Build and test (x64, tail-call)
Windows (free-threading) / Build and test (x64, switch-case)
macOS / build and test (macos-26)
macOS (free-threading) / build and test (macos-26)
macOS / build and test (macos-26-intel)
Ubuntu / build and test (ubuntu-26.04)
Ubuntu / build and test (ubuntu-26.04-arm)
Ubuntu (free-threading) / build and test (ubuntu-26.04)
Ubuntu (free-threading) / build and test (ubuntu-26.04-arm)
Ubuntu  (bolt) / build and test (ubuntu-26.04)
Ubuntu (installed) / build, install and test
Sanitizers / TSan
Sanitizers / MSan
Sanitizers / UBSan
Sanitizers / TSan (free-threading)
All required checks pass
JOBS
  ec_job "$d" "$r" "Windows MSI\${{ '' }}" completed skipped
}

# tokio-rs/tokio .github/workflows/loom.yml at 93df7c6cd7c9 (PR #8771), whole.
# Six jobs whose `if:` needs a label, so each run reports them skipped.
EC_TOKIO_LOOM_YML="$(cat <<'YML'
on:
  push:
    branches: ["master", "tokio-*.x"]
  pull_request:
    types: [labeled, opened, synchronize, reopened]
    branches: ["master", "tokio-*.x"]

name: Loom

concurrency:
  group: ${{ github.workflow }}-${{ github.event.pull_request.number || github.sha }}
  cancel-in-progress: true

env:
  RUSTFLAGS: -Dwarnings --cfg loom --cfg tokio_unstable -C debug_assertions
  LOOM_MAX_PREEMPTIONS: 2
  LOOM_MAX_BRANCHES: 10000
  RUST_BACKTRACE: 1
  # Change to specific Rust release to pin
  rust_stable: stable

permissions:
  contents: read

jobs:
  loom-blocking:
    name: loom tokio::runtime::spawn_blocking
    # base_ref is null when it's not a pull request
    if: github.repository_owner == 'tokio-rs' && (contains(github.event.pull_request.labels.*.name, 'R-loom-blocking') || (github.base_ref == null))
    runs-on: ubuntu-latest
    strategy:
      matrix:
        # Run the blocking pool loom tests against both `spawn_blocking`
        # queue implementations.
        include:
          - sharded_blocking_queue: "0"
          - sharded_blocking_queue: "1"
    steps:
      - uses: actions/checkout@v7
      - name: Install Rust ${{ env.rust_stable }}
        uses: dtolnay/rust-toolchain@master
        with:
            toolchain: ${{ env.rust_stable }}
      - uses: Swatinem/rust-cache@v2
      - name: run tests
        run: cargo test --lib --release --features full -- --nocapture loom_blocking
        working-directory: tokio
        env:
          TOKIO_UNSTABLE_SHARDED_BLOCKING_QUEUE: ${{ matrix.sharded_blocking_queue }}

  loom-sync:
    name: loom tokio::sync
    # base_ref is null when it's not a pull request
    if: github.repository_owner == 'tokio-rs' && (contains(github.event.pull_request.labels.*.name, 'R-loom-sync') || (github.base_ref == null))
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v7
      - name: Install Rust ${{ env.rust_stable }}
        uses: dtolnay/rust-toolchain@master
        with:
            toolchain: ${{ env.rust_stable }}
      - uses: Swatinem/rust-cache@v2
      - name: run tests
        run: cargo test --lib --release --features full -- --nocapture sync::tests
        working-directory: tokio

  loom-time-driver:
    name: loom time driver
    # base_ref is null when it's not a pull request
    if: github.repository_owner == 'tokio-rs' && (contains(github.event.pull_request.labels.*.name, 'R-loom-time-driver') || (github.base_ref == null))
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v7
      - name: Install Rust ${{ env.rust_stable }}
        uses: dtolnay/rust-toolchain@master
        with:
            toolchain: ${{ env.rust_stable }}
      - uses: Swatinem/rust-cache@v2
      - name: run tests
        run: |
          cargo test --lib --release --features full -- --nocapture runtime::time
          cargo test --lib --release --features full -- --nocapture runtime::io
        working-directory: tokio

  loom-current-thread:
    name: loom current-thread scheduler
    # base_ref is null when it's not a pull request
    if: github.repository_owner == 'tokio-rs' && (contains(github.event.pull_request.labels.*.name, 'R-loom-current-thread') || (github.base_ref == null))
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v7
      - name: Install Rust ${{ env.rust_stable }}
        uses: dtolnay/rust-toolchain@master
        with:
            toolchain: ${{ env.rust_stable }}
      - uses: Swatinem/rust-cache@v2
      - name: run tests
        run: cargo test --lib --release --features full -- --nocapture loom_current_thread
        working-directory: tokio

  loom-multi-thread:
    name: loom multi-thread scheduler
    # base_ref is null when it's not a pull request
    if: github.repository_owner == 'tokio-rs' && (contains(github.event.pull_request.labels.*.name, 'R-loom-multi-thread') || (github.base_ref == null))
    runs-on: ubuntu-latest
    strategy:
      matrix:
        include:
          - scope: loom_multi_thread::group_a
          - scope: loom_multi_thread::group_b
          - scope: loom_multi_thread::group_c
          - scope: loom_multi_thread::group_d
    steps:
      - uses: actions/checkout@v7
      - name: Install Rust ${{ env.rust_stable }}
        uses: dtolnay/rust-toolchain@master
        with:
            toolchain: ${{ env.rust_stable }}
      - uses: Swatinem/rust-cache@v2
      - name: loom ${{ matrix.scope }}
        run: cargo test --lib --release --features full -- --nocapture $SCOPE
        working-directory: tokio
        env:
          SCOPE: ${{ matrix.scope }}

  loom-util:
    name: loom tokio-util
    # base_ref is null when it's not a pull request
    if: github.repository_owner == 'tokio-rs' && (contains(github.event.pull_request.labels.*.name, 'R-loom-util') || (github.base_ref == null))
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v7
      - name: Install Rust ${{ env.rust_stable }}
        uses: dtolnay/rust-toolchain@master
        with:
            toolchain: ${{ env.rust_stable }}
      - uses: Swatinem/rust-cache@v2
      - name: run tests
        run: cargo test --lib --release --features full -- --nocapture
        working-directory: tokio-util
YML
)"
EC_TOKIO_LOOM_JOBS='loom tokio::runtime::spawn_blocking
loom tokio::sync
loom time driver
loom current-thread scheduler
loom multi-thread scheduler
loom tokio-util'

# ec_tokio_loom <dir>: loom.yml with the five pull_request runs GitHub has at
# that head. Three runs made in the same second as 42618656717 completed
# skipped with six jobs; 42618656717, the highest id, was cancelled by the
# workflow's concurrency group before it created a job.
ec_tokio_loom() {
  local d="$1" r n
  ec_wf "$d" loom.yml "$EC_TOKIO_LOOM_YML"
  for r in 42615877258 42618653969 42618656066 42618656471; do
    ec_run "$d" "$r" .github/workflows/loom.yml __HEAD__
    ec_jq "$d/runs.json" '(.workflow_runs[] | select(.id == ($r | tonumber))).conclusion = "skipped"' --arg r "$r"
    while IFS= read -r n; do ec_job "$d" "$r" "$n" completed skipped; done <<< "$EC_TOKIO_LOOM_JOBS"
  done
  ec_run "$d" 42618656717 .github/workflows/loom.yml __HEAD__
  ec_jq "$d/runs.json" '(.workflow_runs[] | select(.id == 42618656717)).conclusion = "cancelled"'
  echo '{"total_count":0,"jobs":[]}' > "$d/jobs-42618656717.json"
}

# tokio-rs/tokio .github/workflows/ci.yml at 93df7c6cd7c9 (PR #8771), cut to
# lines 1-57, 578-682, 742-773, 1107-1203 and 1264-1296: `wasm32-wasip1` is
# named only by `${{ matrix.target }}`; `features exclude ${{ matrix.name }}`
# has a leg GitHub reports as `features exclude` (empty name, space trimmed).
EC_TOKIO_CI_YML="$(cat <<'YML'
on:
  push:
    branches: ["master", "tokio-*.x"]
  pull_request:
    branches: ["master", "tokio-*.x"]

name: CI

concurrency:
  group: ${{ github.workflow }}-${{ github.event.pull_request.number || github.sha }}
  cancel-in-progress: true

env:
  RUSTFLAGS: -Dwarnings
  RUST_BACKTRACE: 1
  RUSTUP_WINDOWS_PATH_ADD_BIN: 1
  # Change to specific Rust release to pin
  rust_stable: stable
  rust_nightly: nightly-2025-10-12
  # Pin a specific miri version
  rust_miri_nightly: nightly-2026-08-17
  rust_emscripten_nightly: nightly-2026-08-17
  emsdk_version: '6.0.10'
  rust_clippy: '1.88'
  # When updating this, also update:
  # - README.md
  # - tokio/README.md
  # - CONTRIBUTING.md
  # - tokio/Cargo.toml
  # - tokio-util/Cargo.toml
  # - tokio-test/Cargo.toml
  # - tokio-stream/Cargo.toml
  rust_min: '1.85'
  # This excludes unstable features like io_uring,
  # which require '--cfg tokio_unstable'.
  TOKIO_STABLE_FEATURES: "full,test-util"

defaults:
  run:
    shell: bash

permissions:
  contents: read

jobs:
  # Basic actions that must pass before we kick off more expensive tests.
  basics:
    name: basic checks
    runs-on: ubuntu-latest
    needs:
      - clippy
      - fmt
      - docs
      - minrust
    steps:
      - run: exit 0

  cross-test-with-parking_lot:
    needs: basics
    runs-on: ${{ matrix.os }}
    strategy:
      matrix:
        include:
          - target: i686-unknown-linux-gnu
            os: ubuntu-latest
            extra_features: "taskdump"
          - target: armv5te-unknown-linux-gnueabi
            os: ubuntu-latest
          - target: armv7-unknown-linux-gnueabihf
            os: ubuntu-24.04-arm
          - target: aarch64-unknown-linux-gnu
            os: ubuntu-24.04-arm
            extra_features: "io-uring,taskdump"
          - target: aarch64-pc-windows-msvc
            os: windows-11-arm
    steps:
      - uses: actions/checkout@v7
      - name: Install Rust stable
        uses: dtolnay/rust-toolchain@stable
        with:
          toolchain: ${{ env.rust_stable }}
          target: ${{ matrix.target }}

      - name: Install cargo-nextest
        uses: taiki-e/install-action@v2
        with:
          tool: cargo-nextest

      - uses: taiki-e/setup-cross-toolchain-action@v1
        with:
          target: ${{ matrix.target }}
          qemu: '7.2'

      - uses: Swatinem/rust-cache@v2
      - name: Tests run with all features (including parking_lot)
        run: |
          set -euxo pipefail
          # We use `--features "$TOKIO_STABLE_FEATURES"` instead of `--all-features` since
          # `--all-features` includes `io_uring` and `taskdump`,
          # which is not available on all targets.
          cargo nextest run -p tokio --features $TOKIO_STABLE_FEATURES,${{ matrix.extra_features }} --target ${{ matrix.target }}
          cargo test --doc -p tokio --features $TOKIO_STABLE_FEATURES,${{ matrix.extra_features }} --target ${{ matrix.target }}
        env:
          RUST_TEST_THREADS: 1
          RUSTFLAGS: --cfg tokio_unstable -Dwarnings --cfg tokio_no_tuning_tests
          RUSTDOCFLAGS: --cfg tokio_unstable -Dwarnings

  cross-test-without-parking_lot:
    needs: basics
    runs-on: ${{ matrix.os }}
    strategy:
      matrix:
        include:
          - target: i686-unknown-linux-gnu
            os: ubuntu-latest
            extra_features: "taskdump"
          - target: armv5te-unknown-linux-gnueabi
            os: ubuntu-latest
          - target: armv7-unknown-linux-gnueabihf
            os: ubuntu-24.04-arm
          - target: aarch64-unknown-linux-gnu
            os: ubuntu-24.04-arm
            extra_features: "io-uring,taskdump"
          - target: aarch64-pc-windows-msvc
            os: windows-11-arm
    steps:
      - uses: actions/checkout@v7
      - name: Install Rust stable
        uses: dtolnay/rust-toolchain@stable
        with:
          toolchain: ${{ env.rust_stable }}
          target: ${{ matrix.target }}

      - name: Install cargo-nextest
        uses: taiki-e/install-action@v2
        with:
          tool: cargo-nextest

      - uses: taiki-e/setup-cross-toolchain-action@v1
        with:
          target: ${{ matrix.target }}
          qemu: '7.2'

      - name: Remove `parking_lot` from `full` feature
        run: sed -i '0,/parking_lot/{/parking_lot/d;}' tokio/Cargo.toml

      - uses: Swatinem/rust-cache@v2
      # The `tokio_no_parking_lot` cfg is here to ensure the `sed` above does not silently break.
      - name: Tests run with all features (without parking_lot)
        run: |
          set -euxo pipefail
          # We use `--features "$TOKIO_STABLE_FEATURES"` instead of `--all-features` since
          # `--all-features` includes `io_uring` and `taskdump`,
          # which is not available on all targets.
          cargo nextest run -p tokio --features $TOKIO_STABLE_FEATURES,${{ matrix.extra_features }} --target ${{ matrix.target }}
          cargo test --doc -p tokio --features $TOKIO_STABLE_FEATURES,${{ matrix.extra_features }} --target ${{ matrix.target }}
        env:
          RUST_TEST_THREADS: 1
          RUSTFLAGS: --cfg tokio_unstable -Dwarnings --cfg tokio_no_parking_lot --cfg tokio_no_tuning_tests ${{ matrix.rustflags }}
          RUSTDOCFLAGS: --cfg tokio_unstable -Dwarnings

  # See https://github.com/tokio-rs/tokio/issues/5187
  features:
    name: features exclude ${{ matrix.name }}
    needs: basics
    runs-on: ubuntu-latest
    strategy:
      matrix:
        include:
          - name: ""
            rustflags: ""
            exclude_features: "io-uring,taskdump,schedule-latency"
          - name: "--unstable"
            rustflags: "--cfg tokio_unstable -Dwarnings"
            exclude_features: "io-uring,taskdump,schedule-latency"
          - name: "--unstable io-uring,taskdump,schedule-latency"
            rustflags: "--cfg tokio_unstable -Dwarnings"
            exclude_features: ""
    steps:
      - uses: actions/checkout@v7
      - name: Install Rust ${{ env.rust_nightly }}
        uses: dtolnay/rust-toolchain@stable
        with:
          toolchain: ${{ env.rust_nightly }}
          target: ${{ matrix.target }}
      - name: Install cargo-hack
        uses: taiki-e/install-action@cargo-hack

      - uses: Swatinem/rust-cache@v2
      - name: check --feature-powerset ${{ matrix.name }}
        run: cargo hack check --all --feature-powerset --exclude-features "${{ matrix.exclude_features }}" --depth 2 --keep-going
        env:
          RUSTFLAGS: ${{ matrix.rustflags }}

  wasm32-wasip1:
    name: ${{ matrix.target }}
    needs: basics
    runs-on: ubuntu-latest
    strategy:
      matrix:
        target:
          - wasm32-wasip1
          - wasm32-wasip1-threads
    steps:
      - uses: actions/checkout@v7
      - name: Install Rust ${{ env.rust_stable }}
        uses: dtolnay/rust-toolchain@stable
        with:
          toolchain: ${{ env.rust_stable }}
          targets: ${{ matrix.target }}

      # Install dependencies
      - name: Install cargo-hack, wasmtime
        uses: taiki-e/install-action@v2
        with:
          # Wasmtime v46.0.1 is the last version to support
          # `wasm32-wasip1-threads` (which was an experiment that was never
          # standardized):
          tool: cargo-hack,wasmtime@46.0.1

      - uses: Swatinem/rust-cache@v2
      - name: WASI test tokio full
        run: cargo test -p tokio --target ${{ matrix.target }} --features "sync,macros,io-util,rt,time"
        env:
          CARGO_TARGET_WASM32_WASIP1_RUNNER: "wasmtime run --"
          CARGO_TARGET_WASM32_WASIP1_THREADS_RUNNER: "wasmtime run -W bulk-memory=y -W threads=y -W shared-memory=y -S threads=y --"
          RUSTFLAGS: --cfg tokio_unstable -Dwarnings -C target-feature=+atomics,+bulk-memory -C link-args=--max-memory=67108864
          # in order to run doctests for unstable features, we must also pass
          # the unstable cfg to RustDoc
          RUSTDOCFLAGS: --cfg tokio_unstable

      - name: WASI test tokio-util full
        run: cargo test -p tokio-util --target ${{ matrix.target }} --features full
        env:
          CARGO_TARGET_WASM32_WASIP1_RUNNER: "wasmtime run --"
          CARGO_TARGET_WASM32_WASIP1_THREADS_RUNNER: "wasmtime run -W bulk-memory=y -W threads=y -W shared-memory=y -S threads=y --"
          RUSTFLAGS: --cfg tokio_unstable -Dwarnings -C target-feature=+atomics,+bulk-memory -C link-args=--max-memory=67108864
          RUSTDOCFLAGS: -C link-args=--max-memory=67108864

      - name: WASI test tokio-stream
        run: cargo test --manifest-path=tokio-stream/Cargo.toml --target ${{ matrix.target }} --features time,net,io-util,sync
        env:
          CARGO_TARGET_WASM32_WASIP1_RUNNER: "wasmtime run --"
          CARGO_TARGET_WASM32_WASIP1_THREADS_RUNNER: "wasmtime run -W bulk-memory=y -W threads=y -W shared-memory=y -S threads=y --"
          RUSTFLAGS: --cfg tokio_unstable -Dwarnings -C target-feature=+atomics,+bulk-memory -C link-args=--max-memory=67108864

      - name: test tests-integration --features wasi-rt
        # TODO: this should become: `cargo hack wasi test --each-feature`
        run: cargo test --target ${{ matrix.target }} --test rt_yield --features wasi-rt
        if: matrix.target == 'wasm32-wasip1'
        working-directory: tests-integration
        env:
          CARGO_TARGET_WASM32_WASIP1_RUNNER: "wasmtime run --"
          RUSTFLAGS: -Dwarnings -C target-feature=+atomics,+bulk-memory -C link-args=--max-memory=67108864

      - name: test tests-integration --features wasi-threads-rt
        run: cargo test --target ${{ matrix.target }} --features wasi-threads-rt
        if: matrix.target == 'wasm32-wasip1-threads'
        working-directory: tests-integration
        env:
          CARGO_TARGET_WASM32_WASIP1_THREADS_RUNNER: "wasmtime run -W bulk-memory=y -W threads=y -W shared-memory=y -S threads=y --"
          RUSTFLAGS: --cfg tokio_unstable -Dwarnings -C target-feature=+atomics,+bulk-memory -C link-args=--max-memory=67108864

  wasm32-wasip2:
    name: test tokio for wasm32-wasip2
    runs-on: ubuntu-latest
    timeout-minutes: 10
    steps:
      - uses: actions/checkout@v7
      - name: Install Rust ${{ env.rust_stable }}
        uses: dtolnay/rust-toolchain@stable
        with:
          toolchain: ${{ env.rust_stable }}
          targets: wasm32-wasip2

      - name: Install cargo-nextest, wasmtime
        uses: taiki-e/install-action@v2
        with:
          # Wasmtime v48.0.0 or later should work, but we stick with a known,
          # specific version to avoid surprises:
          tool: cargo-nextest,wasmtime-cli@48.0.2

      - uses: Swatinem/rust-cache@v2

      - name: test tokio --target wasm32-wasip2
        run: cargo nextest run --target wasm32-wasip2 --features net,macros,rt,io-util
        working-directory: tokio
        env:
          RUSTFLAGS: --cfg tokio_unstable
          CARGO_TARGET_WASM32_WASIP2_RUNNER: wasmtime run -Sinherit-network

  check-external-types:
    name: check-external-types (${{ matrix.os }})
    needs: basics
    runs-on: ${{ matrix.os }}
    strategy:
      matrix:
        include:
          - os: windows-latest
            # Windows neither supports io-uring nor taskdump.
            extra_features: "tracing"
          - os: ubuntu-latest
            # includes all unstable features.
            extra_features: "tracing,io-uring,taskdump"
    steps:
      - uses: actions/checkout@v7
      - name: Install Rust ${{ matrix.rust }}
        uses: dtolnay/rust-toolchain@stable
        with:
          # `check-external-types` requires a specific Rust nightly version. See
          # the README for details: https://github.com/awslabs/cargo-check-external-types
          toolchain: nightly-2025-08-06
      - uses: Swatinem/rust-cache@v2
      - name: Install cargo-check-external-types
        uses: taiki-e/cache-cargo-install-action@v3
        with:
          tool: cargo-check-external-types@0.3.0
      - name: check-external-types
        env:
          RUSTFLAGS: --cfg tokio_unstable -Dwarnings
          RUSTDOCFLAGS: --cfg tokio_unstable
        run: cargo check-external-types --features $TOKIO_STABLE_FEATURES,${{ matrix.extra_features }}
        working-directory: tokio
YML
)"

# ec_tokio_ci <dir> [run_id]: ci.yml and its run, with the jobs GitHub reported
# for run 42615877644 at that head (the jobs of the kept job keys).
ec_tokio_ci() {
  local d="$1" r="${2:-1011}" n
  ec_wf "$d" tokio-ci.yml "$EC_TOKIO_CI_YML"
  ec_run "$d" "$r" .github/workflows/tokio-ci.yml __HEAD__
  while IFS= read -r n; do
    ec_job "$d" "$r" "$n" completed success
  done <<'JOBS'
basic checks
cross-test-with-parking_lot (i686-unknown-linux-gnu, ubuntu-latest, taskdump)
cross-test-with-parking_lot (armv5te-unknown-linux-gnueabi, ubuntu-latest)
cross-test-with-parking_lot (armv7-unknown-linux-gnueabihf, ubuntu-24.04-arm)
cross-test-with-parking_lot (aarch64-unknown-linux-gnu, ubuntu-24.04-arm, io-uring,taskdump)
cross-test-with-parking_lot (aarch64-pc-windows-msvc, windows-11-arm)
cross-test-without-parking_lot (i686-unknown-linux-gnu, ubuntu-latest, taskdump)
cross-test-without-parking_lot (armv5te-unknown-linux-gnueabi, ubuntu-latest)
cross-test-without-parking_lot (armv7-unknown-linux-gnueabihf, ubuntu-24.04-arm)
cross-test-without-parking_lot (aarch64-unknown-linux-gnu, ubuntu-24.04-arm, io-uring,taskdump)
cross-test-without-parking_lot (aarch64-pc-windows-msvc, windows-11-arm)
features exclude
features exclude --unstable
features exclude --unstable io-uring,taskdump,schedule-latency
wasm32-wasip1
wasm32-wasip1-threads
test tokio for wasm32-wasip2
check-external-types (windows-latest)
check-external-types (ubuntu-latest)
JOBS
}
