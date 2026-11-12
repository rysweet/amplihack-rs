#!/usr/bin/env bash
# test-issue-2186-expected-checks.sh — contract tests for issue #2186:
# "CI green" means every expected check exists at the head SHA and passed.
#
# Subject: amplifier-bundle/tools/autodrive_expected_checks.sh, run as
#   /bin/bash -p autodrive_expected_checks.sh <HEAD_SHA> <BASE_REF>
# plus its two callers: merge gate section 5 and merge-evidence step-03.
#
# Every `gh` call goes to a fake on PATH that serves fixture files
# (expected-checks-fixtures.sh). No network, no sleep, no wall-clock window.
#
# Sections
#   1. Static contract of the helper (exists, mode, header, budget, bash 3.2,
#      no sleep/wait loop, read-only).
#   2. Arguments (exit 2, no gh call before validation).
#   3. AC1/AC7: the default green set.
#   4. AC2-AC5: missing jobs, required checks, workflow runs, other SHA;
#      D6 run choice with concurrency-cancelled runs (tokio loom.yml).
#   5. AC6: unreadable inputs, and the empty answers that are NOT unreadable.
#   6. Conclusions: failure, pending, neutral, skipped required, other checks,
#      zero checks, status order.
#   7. Workflow selection: on: forms, branch filters, disabled, paths.
#   8. Job matching rules 1-4, the best match, expression-only names;
#      8b. block-scalar and multi-line job names (real cpython/tokio shapes).
#   9. Security: S1 spoof, S2 paths and run ids, name injection, jq version.
#  10. Callers: merge-evidence step-03's agent prompt (#2329 retired the
#      shell gate), light drive, platform facts comment, docs.
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
REPO_ROOT="$(cd "${HERE}/../../.." && pwd -P)"
TOOLS="${REPO_ROOT}/amplifier-bundle/tools"
RECIPES="${REPO_ROOT}/amplifier-bundle/recipes"
HELPER="${TOOLS}/autodrive_expected_checks.sh"
# shellcheck source=expected-checks-fixtures.sh
. "${HERE}/expected-checks-fixtures.sh"

command -v jq >/dev/null 2>&1 || { echo "HARNESS-ERROR: jq is required" >&2; exit 2; }

PASS_COUNT=0
FAIL_COUNT=0
pass() { PASS_COUNT=$((PASS_COUNT + 1)); echo "  PASS[$1]: $2"; }
fail() { FAIL_COUNT=$((FAIL_COUNT + 1)); echo "  FAIL[$1]: $2" >&2; }
check() { # check <id> <description> <command...>
  local id="$1" d="$2"; shift 2
  if "$@"; then pass "$id" "$d"; else fail "$id" "$d"; fi
}

WORK="$(mktemp -d)" || { echo "HARNESS-ERROR: mktemp -d failed" >&2; exit 2; }
trap 'rm -rf "${WORK}"' EXIT
STUB_BIN="${WORK}/bin"; mkdir -p "${STUB_BIN}"
ROUTE="${WORK}/ci-api-route"; ec_write_route "${ROUTE}"

# The fake gh: `api` goes to the fixture route; `pr view` prints the PR fields
# (head from GH_HEAD, base from GH_BASE), `pr checks` prints GH_PR_CHECKS.
# Every call is logged to GH_CALLS. Anything else fails.
cat > "${STUB_BIN}/gh" <<'GH'
#!/usr/bin/env bash
echo "$*" >> "${GH_CALLS:-/dev/null}"
JQ=""; prev=""; for a in "$@"; do [ "$prev" = "--jq" ] && JQ="$a"; prev="$a"; done
case "${1:-} ${2:-}" in
  "pr view")
    j="$(printf '{"number":42,"state":"OPEN","headRefOid":"%s","baseRefName":"%s"}' "${GH_HEAD:-}" "${GH_BASE-main}")"
    [ "${GH_PR_VIEW_FAIL:-0}" = 1 ] && exit 1
    if [ -n "$JQ" ]; then printf '%s' "$j" | jq -r "$JQ"; else printf '%s\n' "$j"; fi
    exit 0 ;;
  "pr checks")
    d='[{"name":"Test","state":"SUCCESS","bucket":"pass"}]'
    printf '%s\n' "${GH_PR_CHECKS:-$d}"; exit 0 ;;
esac
[ "${1:-}" = "api" ] && exec "${GH_CI_ROUTE:?}" "$@"
exit 1
GH
chmod +x "${STUB_BIN}/gh"

HEAD="3f9c2a7e1b4d5c6f708192a3b4c5d6e7f8091a2b"
OTHER="0123456789abcdef0123456789abcdef01234567"
# new_fx: a fresh copy of the default green fixture set; prints its path.
GREEN_FX="${WORK}/fx-green"; ec_fixture_green "${GREEN_FX}"
# Runs inside $(...), so it takes a fresh directory from mktemp, not a counter.
new_fx() { local d; d="$(mktemp -d "${WORK}/fx.XXXXXX")" && cp -R "${GREEN_FX}/." "$d/" && printf '%s' "$d"; }

# run_helper <fixture_dir> [args...]: runs the helper the way both callers do.
# Sets RC, OUT (stdout), ERR, LAST (last stdout line), STATUS, MISSING (JSON).
run_helper() {
  local fx="$1"; shift
  # RH_NOARGS=1 runs it with no arguments at all (the usage case).
  local -a a=("$@"); [ ${#a[@]} -gt 0 ] || [ "${RH_NOARGS:-0}" = 1 ] || a=("$HEAD" main)
  CALLS="$(mktemp "${WORK}/calls.XXXXXX")"
  (cd "${WORK}" && env -u BASH_ENV -u ENV GH_CI_FIX="$fx" GH_CI_ROUTE="${ROUTE}" GH_HEAD="$HEAD" \
     GH_CALLS="${CALLS}" GH_FAIL_RE="${GH_FAIL_RE:-}" PATH="${RUN_PATH:-${STUB_BIN}:${PATH}}" \
     /bin/bash -p "${HELPER}" ${a[@]+"${a[@]}"} >"${WORK}/out" 2>"${WORK}/err" </dev/null)
  RC=$?
  OUT="$(cat "${WORK}/out")"; ERR="$(cat "${WORK}/err")"
  LAST="$(tail -n1 "${WORK}/out")"
  STATUS="$(printf '%s' "$LAST" | jq -r '.ci_status // empty' 2>/dev/null)"
  MISSING="$(printf '%s' "$LAST" | jq -c '.missing // empty' 2>/dev/null)"
}
ran() { [ -n "$STATUS" ]; }   # the helper printed a verdict line
has_line() { printf '%s\n' "$OUT" | grep -qxF -- "$1"; }
has_prefix() { printf '%s\n' "$OUT" | grep -qF -- "$1"; }
blockers() { printf '%s\n' "$OUT" | grep -c '^blocker: ' || true; }
verdict_is() { # verdict_is <status> <rc>
  [ "$STATUS" = "$1" ] && [ "$RC" = "$2" ]
}
explain() { printf 'rc=%s status=%s missing=%s\n--- out\n%s\n--- err\n%s\n' "$RC" "$STATUS" "$MISSING" "$OUT" "$ERR" | sed 's/^/      /' >&2; }
expect() { # expect <id> <description> <status> <rc> [exact blocker line...]
  local id="$1" d="$2" s="$3" r="$4" l ok=1; shift 4
  verdict_is "$s" "$r" || ok=0
  for l in "$@"; do has_line "$l" || ok=0; done
  if [ "$ok" = 1 ]; then pass "$id" "$d"; else fail "$id" "$d"; explain; fi
}

# ran_check: a negative-sense check that counts only when the helper ran.
ran_check() { local id="$1" d="$2"; shift 2
  if ran && "$@"; then pass "$id" "$d"; else fail "$id" "$d (or the helper gave no verdict)"; fi; }

echo "=== #2186 expected checks at the head ==="

# ---------------------------------------------------------------------------
# 1. Static contract of the helper.
# ---------------------------------------------------------------------------
if [ ! -f "${HELPER}" ]; then
  fail "EC-exists" "amplifier-bundle/tools/autodrive_expected_checks.sh does not exist"
  echo "  (the helper is missing: every runtime case below will fail too)" >&2
else
  pass "EC-exists" "the helper exists"
fi
# The mode git records, not the checkout's: a umask of 0002 checks a 100755
# file out as 775. Outside a git checkout, the owner execute bit stands in.
committed_mode() {
  local m
  m="$(git -C "${REPO_ROOT}" ls-files -s -- amplifier-bundle/tools/autodrive_expected_checks.sh 2>/dev/null | cut -d' ' -f1)"
  if [ -n "$m" ]; then [ "$m" = 100755 ]; else test -x "${HELPER}"; fi
}
check "EC-mode-755" "the helper is committed as mode 100755" committed_mode
check "EC-shebang" "the helper starts with #!/bin/bash -p" \
  test "$(head -n1 "${HELPER}" 2>/dev/null)" = "#!/bin/bash -p"
check "EC-unset-f" "the helper drops gh/git/jq functions with builtin unset -f" \
  grep -qE '^[[:space:]]*builtin unset -f gh git jq' "${HELPER}"
check "EC-budget" "the helper is at most 400 lines" \
  test "$(wc -l < "${HELPER}" 2>/dev/null || echo 9999)" -le 400
# Every recipe this change grows keeps the shared brick budget (crusty round 5,
# item 1: autodrive-merge-round.yaml reached 403 lines and turned CI red).
for y in autodrive-merge-round autodrive-merge-evidence; do
  check "EC-budget-${y}" "${y}.yaml stays at most 400 lines" test "$(wc -l < "${RECIPES}/${y}.yaml")" -le 400
done
check "EC-bash-n" "the helper parses (bash -n)" bash -n "${HELPER}"
if command -v shellcheck >/dev/null 2>&1; then
  check "EC-shellcheck" "the helper is shellcheck clean" shellcheck -x "${HELPER}"
fi
# Bracketed so this pattern does not trip the issue_1423 bash-4 scan itself.
no_bash4() { ! grep -nE 'declare -[A]|local -[A]|typeset -[A]|mapfile|readarray|\$\{[A-Za-z_][A-Za-z0-9_]*(,,|\^\^)|local -[n]|declare -[n]|&>[>]|\|[&]' "${HELPER}" | grep -vE '^[0-9]+:[[:space:]]*#' | grep -q .; }
[ -f "${HELPER}" ] && check "EC-bash32" "the helper uses no bash-4 features (runs on macOS /bin/bash 3.2)" no_bash4 \
  || fail "EC-bash32" "the helper uses no bash-4 features (helper missing)"
no_wait() { ! grep -nE '(^|[^A-Za-z_])(sleep|until|wait)([^A-Za-z_]|$)|retry|retries' "${HELPER}" | grep -vE '^[0-9]+:[[:space:]]*#' | grep -q .; }
[ -f "${HELPER}" ] && check "EC-no-timer" "the helper has no sleep, until, wait or retry" no_wait \
  || fail "EC-no-timer" "the helper has no sleep, until, wait or retry (helper missing)"
FORBIDDEN_RE='(--no-verify|--admin|--bypass|gh[^|;&]*api[^|;&]*/merge|pr[[:space:]]+merge)'
no_forbidden() { ! grep -nE "$FORBIDDEN_RE" "$1" | grep -vE '^[0-9]+:[[:space:]]*#' | grep -q .; }
[ -f "${HELPER}" ] && check "EC-forbidden" "the helper never matches the merge/bypass pattern" no_forbidden "${HELPER}" \
  || fail "EC-forbidden" "the helper never matches the merge/bypass pattern (helper missing)"
check "EC-route-forbidden" "no fixture route matches gh ... api .../merge" no_forbidden "${HERE}/expected-checks-fixtures.sh"
no_writes() { ! grep -nE 'gh[^#]*api[^#]*([[:space:]]-X|--method|[[:space:]]-f[[:space:]]|[[:space:]]-F[[:space:]]|--input|--field|--raw-field)' "${HELPER}" | grep -q .; }
[ -f "${HELPER}" ] && check "EC-read-only-static" "the helper's gh api calls carry no write flags" no_writes \
  || fail "EC-read-only-static" "the helper's gh api calls carry no write flags (helper missing)"
check "EC-no-admin-endpoint-static" "the helper never names protection/required_status_checks" \
  bash -c 'test -f "$1" && ! grep -vE "^[[:space:]]*#" "$1" | grep -qF "protection/required_status_checks"' _ "${HELPER}"
check "EC-no-eval" "the helper never evals or sources decoded data" \
  bash -c 'test -f "$1" && ! grep -vE "^[[:space:]]*#" "$1" | grep -qE "(^|[^A-Za-z_])(eval|source)[[:space:]]|system\(|\| *getline"' _ "${HELPER}"
check "EC-jq-cn-verdict" "the verdict line is built with jq -cn" grep -qF 'jq -cn' "${HELPER}"

# ---------------------------------------------------------------------------
# 2. Arguments: exit 2, and no gh call before they are checked.
# ---------------------------------------------------------------------------
FX="$(new_fx)"
bad_arg() { # bad_arg <id> <args...>
  local id="$1"; shift
  run_helper "$FX" "$@"
  if [ "$RC" = 2 ] && [ ! -s "${CALLS}" ] && [ -n "$ERR" ]; then
    pass "EC-args-${id}" "bad arguments ($*) exit 2 with a usage line and no gh call"
  else
    fail "EC-args-${id}" "bad arguments ($*) did not exit 2 before any gh call"; explain
  fi
}
RH_NOARGS=1 bad_arg none
bad_arg one-arg "$HEAD"
bad_arg short-sha 3f9c2a7 main
bad_arg upper-sha "$(printf '%s' "$HEAD" | tr 'a-f' 'A-F')" main
bad_arg long-sha "${HEAD}0" main
bad_arg nonhex-sha "g${HEAD#?}" main
bad_arg dash-base "$HEAD" -main
bad_arg slash-base "$HEAD" /main
bad_arg dotdot-base "$HEAD" 'a..b'
bad_arg double-slash "$HEAD" 'a//b'
bad_arg trailing-slash "$HEAD" 'main/'
bad_arg lock-base "$HEAD" 'main.lock'
bad_arg space-base "$HEAD" 'ma in'
bad_arg dollar-base "$HEAD" 'ma$(id)'
bad_arg empty-base "$HEAD" ''
bad_arg three-args "$HEAD" main extra
# A good nested base passes validation (and reads branches/release/1.x).
FXR="$(new_fx)"
run_helper "$FXR" "$HEAD" release/1.x
if [ "$RC" != 2 ] && grep -qF 'rules/branches/release/1.x' "${CALLS}"; then
  pass "EC-args-nested-base" "a nested base ref like release/1.x is accepted"
else
  fail "EC-args-nested-base" "a nested base ref like release/1.x was refused"; explain
fi

# ---------------------------------------------------------------------------
# 3. AC1 / AC7: the default green set.
# ---------------------------------------------------------------------------
FX="$(new_fx)"
run_helper "$FX"
expect "EC-AC1-green" "every expected check present and green gives GREEN, exit 0" GREEN 0
check "EC-AC1-verdict-shape" "the final line is exactly {\"ci_status\":\"GREEN\",\"missing\":[],\"blockers\":0}" \
  test "$LAST" = '{"ci_status":"GREEN","missing":[],"blockers":0}'
ran_check "EC-AC1-no-blockers" "a green run prints no blocker: line" test "$(blockers)" = 0
check "EC-AC1-evidence-required" "the required set is recorded as an evidence: line" \
  has_prefix "evidence: required: Lint & Format, Test, Install Smoke Test, Build x86_64-unknown-linux-gnu, Build aarch64-unknown-linux-gnu"
check "EC-AC1-evidence-runs" "each expected workflow's run is recorded" \
  bash -c 'grep -qxF "evidence: workflow .github/workflows/ci.yml: run 1001 is completed/success" "$1" && grep -qxF "evidence: workflow .github/workflows/recipe-runner-pin.yml: run 1002 is completed/success" "$1"' _ "${WORK}/out"
check "EC-AC1-evidence-per-check" "each expected check's result is recorded" \
  bash -c 'grep -qF "evidence: expected '"'"'Test'"'"' (required): success" "$1" && grep -qF "evidence: expected '"'"'Recipe runner pin ancestry'"'"': success" "$1"' _ "${WORK}/out"
check "EC-AC1-only-last-json" "only the last stdout line is a JSON object" \
  test "$(grep -c '^{' "${WORK}/out")" = 1
check "EC-AC7-skipped-passes" "a by-design skipped job (Release, job-level if:) passes and is recorded as skipped" \
  has_prefix "evidence: expected 'Release': skipped"
ran_check "EC-steps-not-jobs" "step '- name:' lines are not read as jobs" \
  bash -c '! grep -qE "Checkout|Clippy|Build step, not a job|Run the tests" "$1"' _ "${WORK}/out"
ran_check "EC-prt-not-expected" "a pull_request_target workflow is not expected" \
  bash -c '! grep -qE "^blocker: .*(invisible-char-scan|Scan for invisible)" "$1"' _ "${WORK}/out"
ran_check "EC-push-only-not-expected" "a push-only workflow is not expected" \
  bash -c '! grep -qE "^blocker: .*(release\.yml|Publish)" "$1"' _ "${WORK}/out"
check "EC-D4-paths-note" "a paths-filtered workflow with no run gets an evidence note, not a block" \
  has_prefix "evidence: workflow .github/workflows/docs.yml: no run at ${HEAD}"
check "EC-stderr-no-verdict" "nothing JSON-shaped leaks to stdout from stderr" test -n "$LAST"
# The calls it made: all GETs through `gh api`, at the head, paginated reads.
check "EC-calls-api-only" "every gh call is a gh api call" \
  bash -c '[ -s "$1" ] && ! grep -vqE "^api " "$1"' _ "${CALLS}"
ran_check "EC-calls-no-writes" "no gh call carries -X, --method, -f, -F or --input" \
  bash -c '! grep -qE "(^| )(-X|--method|-f|-F|--field|--raw-field|--input)( |$)" "$1"' _ "${CALLS}"
ran_check "EC-AC6c-no-admin-endpoint" "branches/<base>/protection/required_status_checks is never called" \
  bash -c '! grep -qF "protection" "$1"' _ "${CALLS}"
check "EC-reads-at-head" "check-runs, status, runs and workflow contents are read at the head SHA" \
  bash -c 'grep -qF "commits/$2/check-runs" "$1" && grep -qF "commits/$2/status" "$1" && grep -qF "head_sha=$2" "$1" && grep -qE "contents/\.github/workflows.*ref=$2" "$1"' _ "${CALLS}" "$HEAD"
check "EC-runs-event-pr" "workflow runs are read for event=pull_request" grep -qF 'event=pull_request' "${CALLS}"
check "EC-paginate" "check-runs, runs and jobs are read with --paginate" \
  bash -c 'for e in check-runs actions/runs\? /jobs rules/branches; do grep -E "$e" "$1" | grep -q -- --paginate || exit 1; done' _ "${CALLS}"
check "EC-check-runs-filter-all" "check-runs are requested with filter=all, not GitHub's default latest" \
  grep -qF "commits/${HEAD}/check-runs?per_page=100&filter=all" "${CALLS}"
ran_check "EC-no-other-sha" "nothing is read at any SHA other than the head" \
  bash -c '! grep -oE "[0-9a-f]{40}" "$1" | grep -vxF "$2" | grep -q .' _ "${CALLS}" "$HEAD"

# The repository's real workflows: the default green set built from the files
# in .github/workflows must be readable (not UNREADABLE) by the scanner.
FXREAL="$(new_fx)"
for wf in "${REPO_ROOT}"/.github/workflows/*.yml; do
  ec_wf "$FXREAL" "$(basename "$wf")" "$(cat "$wf")"
done
# GitHub creates a job with a job-level if: and reports it skipped; the real
# ci.yml has one more such job than the default fixture (sccache-prune).
ec_job "$FXREAL" 1001 "Prune sccache caches" completed skipped
run_helper "$FXREAL"
if [ "$STATUS" != UNREADABLE ] && [ -n "$STATUS" ] && ! has_prefix "blocker: expected check 'Build'" \
   && ! has_prefix "Prune sccache caches' was never"; then
  pass "EC-real-workflows-readable" "this repository's own workflow files scan without UNREADABLE"
else
  fail "EC-real-workflows-readable" "this repository's own workflow files could not be scanned"; explain
fi

# ---------------------------------------------------------------------------
# 4. AC2-AC5: missing checks.
# ---------------------------------------------------------------------------
# AC2: a defined job that is not required, never created.
FX="$(new_fx)"; ec_drop_job "$FX" 1002 "Recipe runner pin ancestry"
run_helper "$FX"
expect "EC-AC2-missing-job" "a defined job never created is named and blocks with MISSING" MISSING 1 \
  "blocker: expected check 'Recipe runner pin ancestry' was never created at ${HEAD}"
check "EC-AC2-missing-list" "the missing job is listed in missing" test "$MISSING" = '["Recipe runner pin ancestry"]'

# AC2 (doc example): required AND defined, never created: named once.
FX="$(new_fx)"; ec_drop_job "$FX" 1001 "Install Smoke Test"
run_helper "$FX"
expect "EC-AC2-required-and-defined" "a check that is required and defined, never created, blocks" MISSING 1 \
  "blocker: expected check 'Install Smoke Test' was never created at ${HEAD}"
check "EC-AC2-named-once" "it is named once in missing and gets one blocker line" \
  bash -c '[ "$1" = "[\"Install Smoke Test\"]" ] && [ "$2" = 1 ]' _ "$MISSING" "$(blockers)"
check "EC-AC2-blockers-count" "the verdict's blockers count equals the blocker lines" \
  test "$(printf '%s' "$LAST" | jq -r .blockers)" = "$(blockers)"

# AC2: one matrix leg dropped.
FX="$(new_fx)"; ec_drop_job "$FX" 1001 "Build aarch64-unknown-linux-gnu"
run_helper "$FX"
expect "EC-AC2-matrix-leg" "a required matrix leg that was dropped blocks" MISSING 1 \
  "blocker: expected check 'Build aarch64-unknown-linux-gnu' was never created at ${HEAD}"

# AC2: every Build leg dropped -> the defined job itself is missing.
FX="$(new_fx)"; ec_jq "$FX/branch.json" '.protected=false | del(.protection)'
ec_drop_job "$FX" 1001 "Build x86_64-unknown-linux-gnu"; ec_drop_job "$FX" 1001 "Build aarch64-unknown-linux-gnu"
run_helper "$FX"
check "EC-AC2-defined-matrix-gone" "a matrix job with no legs at all is MISSING" verdict_is MISSING 1
check "EC-AC2-defined-matrix-named" "the blocker names the job by its expected display name" \
  has_prefix "blocker: expected check 'Build"
[ "$STATUS" = MISSING ] || explain

# AC3: a required check (classic protection) with no job behind it, absent.
FX="$(new_fx)"; ec_required "$FX" "External Scan"
run_helper "$FX"
expect "EC-AC3-required-missing" "a required check that never ran is named and blocks" MISSING 1 \
  "blocker: expected check 'External Scan' was never created at ${HEAD}"

# AC3: a required check from a ruleset, absent.
FX="$(new_fx)"
echo '[{"type":"required_status_checks","ruleset_source_type":"Repository","ruleset_id":7,"parameters":{"strict_required_status_checks_policy":false,"required_status_checks":[{"context":"Ruleset Check","integration_id":15368}]}},{"type":"pull_request","parameters":{}}]' > "$FX/rules.json"
run_helper "$FX"
expect "EC-AC3-ruleset-missing" "a ruleset required check that never ran blocks" MISSING 1 \
  "blocker: expected check 'Ruleset Check' was never created at ${HEAD}"
# ... and present as a commit status: success passes.
printf '{"state":"success","sha":"%s","total_count":1,"statuses":[{"context":"Ruleset Check","state":"success"}]}\n' "$HEAD" > "$FX/status.json"
run_helper "$FX"
expect "EC-AC3-ruleset-status-green" "a ruleset required check reported as a successful commit status passes" GREEN 0

# AC4: an expected workflow with no run at the head.
FX="$(new_fx)"; ec_jq "$FX/runs.json" '.workflow_runs |= map(select(.id != 1002))'
ec_drop_job "$FX" 1002 "Recipe runner pin ancestry"
run_helper "$FX"
expect "EC-AC4-no-run" "an expected workflow that never ran is named and blocks" MISSING 1 \
  "blocker: expected workflow '.github/workflows/recipe-runner-pin.yml' never ran at ${HEAD}"

# AC4b: a paths-filtered workflow WITH a run: all its jobs are expected.
FX="$(new_fx)"; ec_run "$FX" 1003 .github/workflows/docs.yml __HEAD__
ec_job "$FX" 1003 build completed success; ec_job "$FX" 1003 deploy completed skipped
run_helper "$FX"
expect "EC-AC4b-paths-run-green" "a paths-filtered workflow that ran with all jobs passes" GREEN 0
FX="$(new_fx)"; ec_run "$FX" 1003 .github/workflows/docs.yml __HEAD__
ec_job "$FX" 1003 deploy completed skipped
run_helper "$FX"
expect "EC-AC4b-paths-run-missing" "a paths-filtered workflow that ran but lacks a job blocks" MISSING 1 \
  "blocker: expected check 'build' was never created at ${HEAD}"
# D8: GitHub reports a job that its job-level if: skipped as `skipped`, so a
# job absent from a completed run is MISSING, if: or not, whatever the run
# concluded. Absent from a run still going, it is PENDING.
FX="$(new_fx)"; ec_run "$FX" 1003 .github/workflows/docs.yml __HEAD__
ec_job "$FX" 1003 build completed success
run_helper "$FX"
expect "EC-D8-if-job-absent-completed" "an if: job absent from a run that concluded success is MISSING" MISSING 1 \
  "blocker: expected check 'deploy' was never created at ${HEAD}"
ec_jq "$FX/runs.json" '(.workflow_runs[] | select(.id == 1003)) |= (.status = "in_progress" | .conclusion = null)'
run_helper "$FX"
expect "EC-D8-if-job-absent-running" "an if: job absent from a run still going is PENDING" PENDING 1 \
  "blocker: expected check 'deploy' is not created yet; run 1003 is in_progress at ${HEAD}"
FX="$(new_fx)"; ec_run "$FX" 1003 .github/workflows/docs.yml __HEAD__
ec_jq "$FX/runs.json" '(.workflow_runs[] | select(.id == 1003)) |= (.status = "queued" | .conclusion = null)'
echo '{"total_count":0,"jobs":[]}' > "$FX/jobs-1003.json"
run_helper "$FX"
expect "EC-D8-job-absent-queued" "a job without if: absent from a queued run is PENDING" PENDING 1 \
  "blocker: expected check 'build' is not created yet; run 1003 is queued at ${HEAD}" \
  "evidence: workflow .github/workflows/docs.yml: run 1003 is queued/none"
# #2186's own failure mode: the only job carries a job-level if:, and the run
# completed without creating it. No run conclusion excuses the missing job. A
# run cancelled with no jobs and no other run of its path gets one blocker that
# names the run and says to re-run it (crusty round 4, item 3).
E2E_YML='name: E2E
on:
  pull_request:
    branches: [main]
jobs:
  e2e:
    name: E2E
    if: github.event.pull_request.draft == false
    runs-on: ubuntu-latest
    steps:
      - run: echo e2e
'
for c in startup_failure failure cancelled success; do
  FX="$(new_fx)"; ec_wf "$FX" e2e.yml "$E2E_YML"; ec_run "$FX" 1008 .github/workflows/e2e.yml __HEAD__
  ec_jq "$FX/runs.json" '(.workflow_runs[] | select(.id == 1008)).conclusion = $c' --arg c "$c"
  echo '{"total_count":0,"jobs":[]}' > "$FX/jobs-1008.json"
  run_helper "$FX"
  want="blocker: expected check 'E2E' was never created at ${HEAD}"
  [ "$c" = cancelled ] && want="blocker: expected workflow '.github/workflows/e2e.yml' run 1008 was cancelled before it created any job at ${HEAD}; re-run it"
  expect "EC-D8-no-jobs-${c}" "an if: job absent from a run that concluded ${c} with no jobs is MISSING" MISSING 1 \
    "evidence: workflow .github/workflows/e2e.yml: run 1008 is completed/${c}" "$want"
  [ "$c" != cancelled ] || check "EC-D8-cancelled-missing-path" "the cancelled run's workflow is what missing names" \
    test "$MISSING" = '[".github/workflows/e2e.yml"]'
done

# AC5: the check exists only on another SHA (an earlier push).
FX="$(new_fx)"
ec_jq "$FX/runs.json" '(.workflow_runs[] | select(.id == 1002)).head_sha = $o' --arg o "$OTHER"
ec_jq "$FX/check-runs.json" '(.check_runs[] | select(.name == "Recipe runner pin ancestry")).head_sha = $o' --arg o "$OTHER"
run_helper "$FX"
expect "EC-AC5-other-sha-run" "a workflow that ran only on another SHA does not satisfy the head" MISSING 1 \
  "blocker: expected workflow '.github/workflows/recipe-runner-pin.yml' never ran at ${HEAD}"
FX="$(new_fx)"; ec_required "$FX" "External Scan"; ec_check "$FX" "External Scan" completed success "$OTHER" 9100 other-app
printf '{"state":"success","sha":"%s","total_count":1,"statuses":[{"context":"External Scan","state":"success"}]}\n' "$OTHER" > "$FX/status-${OTHER}.json"
run_helper "$FX"
expect "EC-AC5-other-sha-required" "a required check green only on another SHA does not satisfy the head" MISSING 1 \
  "blocker: expected check 'External Scan' was never created at ${HEAD}"
ran_check "EC-AC5-required-no-merge-hint" "a required check (read from the base) gets no merge-commit hint" \
  bash -c '! grep -qF "merge commit" "$1"' _ "${WORK}/out"

# D6: the latest run per path decides (an older run at the head lacking a job).
FX="$(new_fx)"; ec_run "$FX" 900 .github/workflows/ci.yml __HEAD__
echo '{"total_count":1,"jobs":[{"id":90000,"run_id":900,"name":"Lint & Format","status":"completed","conclusion":"success"}]}' > "$FX/jobs-900.json"
run_helper "$FX"
expect "EC-D6-latest-run" "the latest run (highest id) for a workflow is the one whose jobs count" GREEN 0
check "EC-D6-latest-jobs-read" "the latest run's jobs are read" grep -qF 'actions/runs/1001/jobs' "${CALLS}"

# D6 and concurrency (crusty round 4, item 3): run ids do not follow the order
# a concurrency group handles runs in. tokio PR #8771's loom.yml has four runs
# made at 12:38:07-09Z; the highest id was cancelled before it created a job.
# A run cancelled with zero jobs holds no evidence: the next run decides.
FX="$(new_fx)"; ec_tokio_loom "$FX"
run_helper "$FX" "$HEAD" master
expect "EC-D6-tokio-cancelled-dup" "tokio's loom.yml run shape is GREEN, not six jobs 'never created'" GREEN 0 \
  "evidence: workflow .github/workflows/loom.yml: run 42618656717 was cancelled before it created any job; run 42618656471 decides" \
  "evidence: workflow .github/workflows/loom.yml: run 42618656471 is completed/skipped" \
  "evidence: expected 'loom tokio::sync': skipped"
ran_check "EC-D6-tokio-no-never-created" "no loom job is called never created" \
  bash -c '! grep -qF "was never created" "$1"' _ "${WORK}/out"
check "EC-D6-tokio-reads" "it reads the cancelled run's jobs, then the next run's, and no further" \
  bash -c 'grep -qF actions/runs/42618656717/jobs "$1" && grep -qF actions/runs/42618656471/jobs "$1" && ! grep -qF actions/runs/42618656066/jobs "$1"' _ "${CALLS}"
# The run that decides is checked like any other: a failing job in it blocks.
ec_set_job "$FX" 42618656471 "loom tokio::sync" completed failure
run_helper "$FX" "$HEAD" master
expect "EC-D6-tokio-next-run-counts" "a failing job in the run that decides blocks" RED 1 \
  "blocker: expected check 'loom tokio::sync' is completed/failure at ${HEAD}"
# Every run cancelled with zero jobs: the latest decides, and the blocker names it.
FX="$(new_fx)"; ec_tokio_loom "$FX"
for r in 42615877258 42618653969 42618656066 42618656471; do
  ec_jq "$FX/runs.json" '(.workflow_runs[] | select(.id == ($r | tonumber))).conclusion = "cancelled"' --arg r "$r"
  echo '{"total_count":0,"jobs":[]}' > "$FX/jobs-${r}.json"
  ec_jq "$FX/check-runs.json" '.check_runs |= map(select(.check_suite.id != (($r | tonumber) + 5000)))' --arg r "$r"
done
run_helper "$FX" "$HEAD" master
expect "EC-D6-all-cancelled" "when every run was cancelled before it created a job, the latest is named with 're-run it'" MISSING 1 \
  "blocker: expected workflow '.github/workflows/loom.yml' run 42618656717 was cancelled before it created any job at ${HEAD}; re-run it"
ran_check "EC-D6-all-cancelled-no-skip-note" "no run is said to decide in its place" \
  bash -c '! grep -qF " decides" "$1"' _ "${WORK}/out"
ran_check "EC-D6-all-cancelled-no-merge-hint" "a run cancelled before any job is not blamed on a stale branch" \
  bash -c '! grep -qF "merge commit" "$1"' _ "${WORK}/out"
# A cancelled latest run that created jobs is evidence, not skipped: it blocks.
FX="$(new_fx)"; ec_run "$FX" 1013 .github/workflows/recipe-runner-pin.yml __HEAD__
ec_jq "$FX/runs.json" '(.workflow_runs[] | select(.id == 1013)).conclusion = "cancelled"'
ec_job "$FX" 1013 "Recipe runner pin ancestry" completed cancelled
run_helper "$FX"
expect "EC-D6-cancelled-with-jobs" "a cancelled latest run that created jobs still decides and blocks" RED 1 \
  "blocker: expected check 'Recipe runner pin ancestry' is completed/cancelled at ${HEAD}"
# A cancelled zero-job latest run above a run still going: the running one decides.
FX="$(new_fx)"; ec_run "$FX" 1013 .github/workflows/recipe-runner-pin.yml __HEAD__
ec_jq "$FX/runs.json" '(.workflow_runs[] | select(.id == 1013)).conclusion = "cancelled"'
echo '{"total_count":0,"jobs":[]}' > "$FX/jobs-1013.json"
ec_jq "$FX/runs.json" '(.workflow_runs[] | select(.id == 1002)) |= (.status = "in_progress" | .conclusion = null)'
ec_set_job "$FX" 1002 "Recipe runner pin ancestry" in_progress null
run_helper "$FX"
expect "EC-D6-cancelled-over-running" "a zero-job cancelled run above a running one gives PENDING from the running one" PENDING 1 \
  "evidence: workflow .github/workflows/recipe-runner-pin.yml: run 1013 was cancelled before it created any job; run 1002 decides"
# S3 follows D6's choice: the suites of the path's other runs are dropped and
# the chosen run's suite is kept, so a required job of the chosen run is found
# (dropping every suite but the highest id's would call it never created).
FX="$(new_fx)"; ec_tokio_loom "$FX"; ec_required "$FX" "loom tokio::sync"
ec_jq "$FX/check-runs.json" '(.check_runs[] | select(.check_suite.id == 42618658969 and .name == "loom tokio::sync")) |= (.conclusion = "failure")'
run_helper "$FX" "$HEAD" master
expect "EC-D6-S3-follows-choice" "a required job of the chosen run passes, and an older run's failing check-run is superseded" GREEN 0 \
  "evidence: expected 'loom tokio::sync' (required): skipped"

# ---------------------------------------------------------------------------
# 5. AC6: unreadable means blocked; the empty answers that are not unreadable.
# ---------------------------------------------------------------------------
unreadable() { # unreadable <id> <description> <fixture>
  run_helper "$3"
  if verdict_is UNREADABLE 1 && has_prefix "blocker: expected checks unreadable: "; then
    pass "EC-AC6-$1" "$2 gives UNREADABLE with an 'expected checks unreadable:' blocker"
  else
    fail "EC-AC6-$1" "$2 did not give UNREADABLE"; explain
  fi
}
FX="$(new_fx)"; GH_FAIL_RE='^rules/branches/' unreadable rules-500 "a failing rules read" "$FX"
FX="$(new_fx)"; rm "$FX/rules.json"; unreadable rules-404 "a 404 on rules/branches" "$FX"
FX="$(new_fx)"; rm "$FX/branch.json"; unreadable branch-404 "a 404 on branches/<base>" "$FX"
FX="$(new_fx)"; rm "$FX/jobs-1001.json"; unreadable jobs-404 "a 404 on a run's jobs" "$FX"
FX="$(new_fx)"; GH_FAIL_RE='/jobs$' unreadable jobs-500 "a failing jobs read" "$FX"
FX="$(new_fx)"; GH_FAIL_RE='check-runs$' unreadable check-runs-500 "a failing check-runs read" "$FX"
FX="$(new_fx)"; GH_FAIL_RE='/status$' unreadable status-500 "a failing commit-status read" "$FX"
FX="$(new_fx)"; GH_FAIL_RE='^actions/runs$' unreadable runs-500 "a failing workflow-runs read" "$FX"
FX="$(new_fx)"; rm "$FX/workflows.json"; unreadable workflows-404 "a 404 on actions/workflows" "$FX"
FX="$(new_fx)"; rm "$FX/wf-ci.yml.json"; unreadable wf-file-404 "a 404 on a listed workflow file" "$FX"
FX="$(new_fx)"; echo '{"total_count":3,"check_runs":[' > "$FX/check-runs.json"; unreadable bad-json "an invalid JSON answer" "$FX"
FX="$(new_fx)"; echo 'not json' > "$FX/rules.json"; unreadable bad-json-rules "a non-JSON rules answer" "$FX"
FX="$(new_fx)"; ec_jq "$FX/runs.json" '(.workflow_runs[] | select(.id == 1001)) |= del(.path)'
unreadable run-no-path "a workflow run with no path field (GHES)" "$FX"
FX="$(new_fx)"; ec_wf "$FX" broken.yml 'name: Broken
this file has no on key and no jobs
'
unreadable no-on-no-jobs "a workflow with no top-level on: or jobs:" "$FX"
FX="$(new_fx)"; ec_wf "$FX" nojobs.yml 'name: No jobs
on:
  pull_request:
'
unreadable no-jobs "a pull_request workflow with no jobs: key" "$FX"
# Empty answers that are NOT unreadable.
FX="$(new_fx)"; ec_jq "$FX/branch.json" '{name:"main", protected:false}'
run_helper "$FX"
expect "EC-AC6-unprotected-empty" "an unprotected branch (protected: false) means no classic required checks, not unreadable" GREEN 0
FX="$(new_fx)"
run_helper "$FX"
expect "EC-AC6-rules-empty" "rules/branches returning [] means no ruleset required checks, not unreadable" GREEN 0
FX="$(new_fx)"; rm "$FX/wf-dir.json"
run_helper "$FX"
expect "EC-AC6-no-workflows-dir" "a 404 on contents/.github/workflows means no defined jobs; required checks still apply" GREEN 0
FX="$(new_fx)"; rm "$FX/wf-dir.json"; ec_required "$FX" "External Scan"
run_helper "$FX"
expect "EC-AC6-no-workflows-dir-required" "with no workflows dir, a missing required check still blocks" MISSING 1 \
  "blocker: expected check 'External Scan' was never created at ${HEAD}"

# ---------------------------------------------------------------------------
# 6. Conclusions and status order.
# ---------------------------------------------------------------------------
FX="$(new_fx)"; ec_set_job "$FX" 1001 Test completed failure
run_helper "$FX"
expect "EC-R-failure" "an expected check that failed gives RED" RED 1 \
  "blocker: expected check 'Test' is completed/failure at ${HEAD}"
for c in cancelled timed_out action_required stale startup_failure; do
  FX="$(new_fx)"; ec_set_job "$FX" 1002 "Recipe runner pin ancestry" completed "$c"
  run_helper "$FX"
  expect "EC-R-${c}" "an expected job concluding ${c} gives RED" RED 1 \
    "blocker: expected check 'Recipe runner pin ancestry' is completed/${c} at ${HEAD}"
done
# neutral passes, as GitHub passes it for a required check.
FX="$(new_fx)"; ec_set_job "$FX" 1002 "Recipe runner pin ancestry" completed neutral
run_helper "$FX"
expect "EC-neutral-expected-job" "an expected job concluding neutral passes and is recorded as neutral" GREEN 0 \
  "evidence: expected 'Recipe runner pin ancestry': neutral"
FX="$(new_fx)"; ec_set_job "$FX" 1001 Test completed neutral
run_helper "$FX"
expect "EC-neutral-required-job" "a required defined job concluding neutral passes" GREEN 0 \
  "evidence: expected 'Test' (required): neutral"
# S3: ci.yml cancels a running build when a new one starts for the same ref,
# so a reopened PR or a re-pushed SHA leaves a cancelled earlier run at the
# same head. The newer run of the same workflow decides; the older suite is
# dropped.
add_cancelled_ci_run() { # add_cancelled_ci_run <dir> <run_id>
  local n
  ec_run "$1" "$2" .github/workflows/ci.yml __HEAD__
  ec_jq "$1/runs.json" '(.workflow_runs[] | select(.id == ($r|tonumber))) |= (.conclusion = "cancelled")' --arg r "$2"
  for n in "Lint & Format" Test "Install Smoke Test" "Build x86_64-unknown-linux-gnu" "Build aarch64-unknown-linux-gnu"; do
    ec_job "$1" "$2" "$n" completed cancelled
  done
  ec_job "$1" "$2" Release completed skipped   # its job-level if: is false
}
FX="$(new_fx)"; add_cancelled_ci_run "$FX" 999
run_helper "$FX"
expect "EC-S3-superseded-cancelled" "a cancelled earlier ci.yml run at the head, superseded by a green one, gives GREEN" GREEN 0
FX="$(new_fx)"; add_cancelled_ci_run "$FX" 1003
run_helper "$FX"
# Run 1003 has the highest id and was cancelled AFTER it created its jobs, so
# it holds evidence and decides (only a cancelled run with zero jobs is passed
# over, EC-D6-tokio-cancelled-dup): its cancelled jobs block as RED.
expect "EC-S3-latest-cancelled" "a cancelled highest-id ci.yml run that created jobs decides, and its cancelled jobs block" RED 1 \
  "blocker: expected check 'Test' is completed/cancelled at ${HEAD}"
FX="$(new_fx)"; add_cancelled_ci_run "$FX" 999; ec_set_job "$FX" 1001 Test completed failure
ec_jq "$FX/check-runs.json" '(.check_runs[] | select(.check_suite.id == 5999 and .name == "Test")) |= (.conclusion = "cancelled")'
run_helper "$FX"
expect "EC-S3-latest-failing" "dropping the superseded suite does not hide a failure in the latest run" RED 1 \
  "blocker: expected check 'Test' is completed/failure at ${HEAD}"
FX="$(new_fx)"; ec_set_job "$FX" 1001 Test in_progress null
run_helper "$FX"
check "EC-P-pending" "an expected check still running gives PENDING, exit 1" verdict_is PENDING 1
check "EC-P-pending-line" "the pending blocker names the check and its state" \
  has_prefix "blocker: expected check 'Test' is in_progress/"
[ "$STATUS" = PENDING ] || explain
FX="$(new_fx)"; ec_set_job "$FX" 1001 Test queued null
run_helper "$FX"
check "EC-P-queued" "a queued expected check gives PENDING" verdict_is PENDING 1
# Other checks at the head (not expected).
FX="$(new_fx)"; ec_check "$FX" "Some Bot" completed failure __HEAD__ 9200 some-bot
run_helper "$FX"
expect "EC-R7-other-failing" "a failing check that is not expected still gives RED" RED 1 \
  "blocker: check 'Some Bot' is completed/failure at ${HEAD}"
FX="$(new_fx)"; ec_check "$FX" "Some Bot" in_progress null __HEAD__ 9200 some-bot
run_helper "$FX"
check "EC-R7-other-pending" "a pending check that is not expected gives PENDING" verdict_is PENDING 1
check "EC-R7-other-pending-line" "its blocker uses the 'check' form" has_prefix "blocker: check 'Some Bot' is in_progress/"
FX="$(new_fx)"; ec_check "$FX" "Some Bot" completed neutral __HEAD__ 9200 some-bot
run_helper "$FX"
expect "EC-R7-other-neutral" "neutral passes on a check that is not expected (gh pr checks counts it as skipping)" GREEN 0
FX="$(new_fx)"; ec_check "$FX" "Some Bot" completed skipped __HEAD__ 9200 some-bot
run_helper "$FX"
expect "EC-R7-other-skipped" "skipped on a check that is not expected passes" GREEN 0
FX="$(new_fx)"; printf '{"state":"failure","sha":"%s","total_count":1,"statuses":[{"context":"ci/legacy","state":"failure"}]}\n' "$HEAD" > "$FX/status.json"
run_helper "$FX"
expect "EC-R7-status-failure" "a failing commit status at the head gives RED" RED 1 \
  "blocker: check 'ci/legacy' is failure at ${HEAD}"
# A required check with no defined job behind it must be success.
FX="$(new_fx)"; ec_required "$FX" "External Scan"; ec_check "$FX" "External Scan" completed skipped __HEAD__ 9300 other-app
run_helper "$FX"
expect "EC-skipped-required-no-job" "a required check-run with no defined job that reports skipped blocks" RED 1 \
  "blocker: expected check 'External Scan' is completed/skipped at ${HEAD}"
FX="$(new_fx)"; ec_required "$FX" "External Scan"; ec_check "$FX" "External Scan" completed neutral __HEAD__ 9300 other-app
run_helper "$FX"
expect "EC-neutral-required-no-job" "a required check-run with no defined job that reports neutral passes, as on GitHub" GREEN 0 \
  "evidence: expected 'External Scan' (required): neutral"
FX="$(new_fx)"; ec_required "$FX" "External Scan"
printf '{"state":"pending","sha":"%s","total_count":1,"statuses":[{"context":"External Scan","state":"pending"}]}\n' "$HEAD" > "$FX/status.json"
run_helper "$FX"
check "EC-pending-required-status" "a required commit status still pending gives PENDING" verdict_is PENDING 1
FX="$(new_fx)"; ec_required "$FX" "External Scan"; ec_check "$FX" "External Scan" completed success __HEAD__ 9300 other-app
run_helper "$FX"
expect "EC-required-other-app-success" "a required check-run with no job behind it that succeeded passes" GREEN 0
# Zero checks at the head.
FX="${WORK}/fx-zero"; mkdir -p "$FX"
echo '[]' > "$FX/rules.json"; echo '{"name":"main","protected":false}' > "$FX/branch.json"
echo '{"total_count":0,"check_runs":[]}' > "$FX/check-runs.json"; echo '{"total_count":0,"workflows":[]}' > "$FX/workflows.json"
echo '{"total_count":0,"workflow_runs":[]}' > "$FX/runs.json"
run_helper "$FX"
check "EC-R7-zero-checks" "zero check-runs and zero statuses at the head blocks" \
  bash -c '[ "$1" = 1 ] && [ -n "$2" ] && [ "$2" != GREEN ]' _ "$RC" "$STATUS"
check "EC-R7-zero-checks-line" "its blocker is exactly 'no checks at <sha>'" has_line "blocker: no checks at ${HEAD}"
# Status order: UNREADABLE > MISSING > RED > PENDING, all blockers printed.
FX="$(new_fx)"; ec_drop_job "$FX" 1002 "Recipe runner pin ancestry"; ec_set_job "$FX" 1001 Test completed failure
ec_set_job "$FX" 1001 "Lint & Format" in_progress null
run_helper "$FX"
expect "EC-order-missing-first" "MISSING outranks RED and PENDING, and every blocker line is printed" MISSING 1 \
  "blocker: expected check 'Recipe runner pin ancestry' was never created at ${HEAD}" \
  "blocker: expected check 'Test' is completed/failure at ${HEAD}"
check "EC-order-pending-printed" "the pending blocker is printed too" has_prefix "blocker: expected check 'Lint & Format' is in_progress/"
FX="$(new_fx)"; ec_set_job "$FX" 1001 Test completed failure; ec_set_job "$FX" 1001 "Lint & Format" in_progress null
run_helper "$FX"
check "EC-order-red-over-pending" "RED outranks PENDING" verdict_is RED 1
FX="$(new_fx)"; ec_drop_job "$FX" 1002 "Recipe runner pin ancestry"; rm "$FX/jobs-1001.json"
run_helper "$FX"
check "EC-order-unreadable-first" "UNREADABLE outranks MISSING" verdict_is UNREADABLE 1

# ---------------------------------------------------------------------------
# 7. Which workflows are expected.
# ---------------------------------------------------------------------------
# wf_expected <id> <description> <yaml> <expected: yes|no>: adds the workflow
# with NO run; expected -> MISSING (never ran), not expected -> GREEN. The base
# is main, or WF_BASE.
wf_expected() {
  local fx; fx="$(new_fx)"; ec_wf "$fx" extra.yml "$3"
  run_helper "$fx" "$HEAD" "${WF_BASE:-main}"
  if [ "$4" = yes ]; then
    expect "EC-wf-$1" "$2 is expected" MISSING 1 "blocker: expected workflow '.github/workflows/extra.yml' never ran at ${HEAD}"
  else
    expect "EC-wf-$1" "$2 is not expected" GREEN 0
  fi
}
J='
jobs:
  x:
    runs-on: ubuntu-latest
    steps:
      - run: echo
'
wf_expected scalar "on: pull_request (scalar)" "name: A
on: pull_request
${J}" yes
wf_expected flow-list "on: [push, pull_request] (flow list)" "name: A
on: [push, pull_request]
${J}" yes
wf_expected dq-on '"on": mapping' "name: A
\"on\":
  pull_request:
${J}" yes
wf_expected sq-on "'on': mapping" "name: A
'on':
  pull_request:
${J}" yes
wf_expected true-on "true: (YAML 1.1 on)" "name: A
true:
  pull_request:
${J}" yes
wf_expected block-list "on: block list" "name: A
on:
  - push
  - pull_request
${J}" yes
wf_expected block-list-unindented "on: block list at the key's own indent" "name: A
on:
- push
- pull_request
${J}" yes
# no_run_note <id> <reason>: the last run noted extra.yml as expected only if it ran.
no_run_note() {
  check "EC-wf-$1-note" "the workflow with no run gets a '$2' evidence note" \
    has_line "evidence: workflow .github/workflows/extra.yml: no run at ${HEAD} ($2; not expected)"
}
# types: a head reaches a run only through opened, synchronize or reopened.
wf_expected types-labeled "pull_request with types: [labeled] and no run" "name: A
on:
  pull_request:
    types: [labeled]
${J}" no
no_run_note types-labeled "types filter"
wf_expected types-default "types: naming opened, synchronize and reopened" "name: A
on:
  pull_request:
    types: [opened, labeled, synchronize, reopened]
${J}" yes
wf_expected types-partial "a types: block list without reopened, and no run" "name: A
on:
  pull_request:
    types:
      - opened
      - synchronize
${J}" no
FX="$(new_fx)"; ec_wf "$FX" extra.yml "name: A
on:
  pull_request:
    types: [labeled]
${J}"
ec_run "$FX" 1009 .github/workflows/extra.yml __HEAD__; echo '{"total_count":0,"jobs":[]}' > "$FX/jobs-1009.json"
run_helper "$FX"
expect "EC-wf-types-ran" "a types-filtered workflow that ran at the head has every job expected" MISSING 1 \
  "blocker: expected check 'x' was never created at ${HEAD}"
wf_expected prt-only "on: pull_request_target" "name: A
on: pull_request_target
${J}" no
wf_expected push-only "on: push" "name: A
on:
  push:
    branches: [main]
${J}" no
wf_expected nested-key "pull_request named only under another trigger" "name: A
on:
  workflow_dispatch:
    inputs:
      pull_request:
        description: not a trigger
${J}" no
wf_expected branch-match "branches: [main] with base main" "name: A
on:
  pull_request:
    branches: [main]
${J}" yes
wf_expected branch-other "branches: [release] with base main" "name: A
on:
  pull_request:
    branches: [release]
${J}" no
wf_expected branch-block-list "branches block list naming main" "name: A
on:
  pull_request:
    branches:
      - develop
      - main
${J}" yes
# Branch patterns are evaluated against the one base name (GitHub's rules).
B_RELEASES="name: A
on:
  pull_request:
    branches: ['releases/**']
${J}"
wf_expected branch-glob-other "branches: ['releases/**'] with base main" "$B_RELEASES" no
WF_BASE=releases/1.x wf_expected branch-glob-base "branches: ['releases/**'] with base releases/1.x" "$B_RELEASES" yes
WF_BASE=release/1.x wf_expected branch-star-slash "branches: ['*'] with base release/1.x (* stops at /)" "name: A
on:
  pull_request:
    branches: ['*']
${J}" no
wf_expected branch-star "branches: ['ma*'] with base main" "name: A
on:
  pull_request:
    branches: ['ma*']
${J}" yes
wf_expected branch-negated "branches: ['**', '!main'] with base main" "name: A
on:
  pull_request:
    branches: ['**', '!main']
${J}" no
wf_expected branch-negated-order "branches: ['!main', '**']: the later positive pattern decides" "name: A
on:
  pull_request:
    branches: ['!main', '**']
${J}" yes
wf_expected branch-optional "branches: ['mai?n'] (? is zero or one of the character before)" "name: A
on:
  pull_request:
    branches: ['mai?n']
${J}" yes
wf_expected branch-class "branches: ['[lm]ain']" "name: A
on:
  pull_request:
    branches:
      - '[lm]ain'
${J}" yes
wf_expected branch-bad-pattern "branches: ['+main'] (jq cannot compile it), no run" "name: A
on:
  pull_request:
    branches: ['+main']
${J}" no
no_run_note branch-bad-pattern "branch pattern"
wf_expected ignore-base "branches-ignore naming the base" "name: A
on:
  pull_request:
    branches-ignore: [main]
${J}" no
wf_expected ignore-other "branches-ignore naming another branch" "name: A
on:
  pull_request:
    branches-ignore: [develop]
${J}" yes
wf_expected ignore-glob "branches-ignore with a glob matching the base" "name: A
on:
  pull_request:
    branches-ignore: ['ma*']
${J}" no
wf_expected ignore-glob-other "branches-ignore with a glob not matching the base" "name: A
on:
  pull_request:
    branches-ignore: ['release/**']
${J}" yes
wf_expected ignore-negated "branches-ignore with a ! entry, and no run" "name: A
on:
  pull_request:
    branches-ignore: ['release/**', '!release/keep']
${J}" no
no_run_note ignore-negated "branch pattern"
wf_expected paths-no-run "pull_request with paths: and no run" "name: A
on:
  pull_request:
    paths: ['src/**']
${J}" no
wf_expected paths-ignore-no-run "pull_request with paths-ignore: and no run" "name: A
on:
  pull_request:
    paths-ignore: ['docs/**']
${J}" no
EC_WF_STATE=disabled_manually wf_expected disabled "a disabled workflow" "name: A
on: pull_request
${J}" no
FX="$(new_fx)"; EC_WF_STATE=none ec_wf "$FX" extra.yml "name: A
on: pull_request
${J}"
run_helper "$FX"
expect "EC-wf-not-in-actions" "a workflow file that actions/workflows does not report active is not expected (D2)" GREEN 0
# The `name:` of a workflow does not count as a job name.
FX="$(new_fx)"; ec_wf "$FX" extra.yml "name: Some Workflow
on: pull_request
jobs:
  only:
    runs-on: ubuntu-latest
    steps:
      - run: echo
"
ec_run "$FX" 1004 .github/workflows/extra.yml __HEAD__; ec_job "$FX" 1004 only completed success
run_helper "$FX"
expect "EC-wf-key-as-name" "a job with no name: is matched by its key" GREEN 0
ran_check "EC-wf-name-not-job" "the workflow's own name: is not read as a job" bash -c '! grep -qF "Some Workflow" "$1"' _ "${WORK}/out"

# ---------------------------------------------------------------------------
# 8. Matching rules.
# ---------------------------------------------------------------------------
match_case() { # match_case <id> <description> <yaml jobs block> <reported name> <status> [blocker]
  local fx; fx="$(new_fx)"
  ec_wf "$fx" m.yml "name: M
on: pull_request
jobs:
$3"
  ec_run "$fx" 1005 .github/workflows/m.yml __HEAD__; ec_job "$fx" 1005 "$4" completed success
  run_helper "$fx"
  if [ -n "${6:-}" ]; then expect "EC-match-$1" "$2" "$5" 1 "$6"; else expect "EC-match-$1" "$2" "$5" 0; fi
}
match_case rule1-exact "rule 1: exact name" '  a:
    name: Exact Name
    runs-on: ubuntu-latest
    steps:
      - run: echo
' "Exact Name" GREEN
match_case rule2-prefix "rule 2: literal text before the first \${{" '  b:
    name: Pack ${{ matrix.os }} build
    runs-on: ubuntu-latest
    steps:
      - run: echo
' "Pack ubuntu build" GREEN
match_case rule3-matrix-key "rule 3: <key> ( for a matrix job with no name:" '  lint:
    runs-on: ubuntu-latest
    strategy:
      matrix:
        os: [ubuntu]
    steps:
      - run: echo
' "lint (ubuntu)" GREEN
match_case rule3-matrix-name "rule 3: <name> ( for a matrix job with a literal name:" '  test:
    name: Test
    runs-on: ubuntu-latest
    strategy:
      matrix:
        os: [ubuntu]
    steps:
      - run: echo
' "Test (ubuntu)" GREEN
match_case rule3-not-bare-prefix "rule 3 needs '<key> (' not just the key as prefix" '  lint:
    runs-on: ubuntu-latest
    steps:
      - run: echo
' "linter" MISSING "blocker: expected check 'lint' was never created at ${HEAD}"
match_case rule4-reusable "rule 4: <name> / for a reusable workflow" '  deploy:
    name: Deploy
    uses: ./.github/workflows/reusable.yml
' "Deploy / build" GREEN
match_case rule4-key-reusable "rule 4 with the key when there is no name:" '  deploy:
    uses: ./.github/workflows/reusable.yml
' "deploy / build" GREEN
match_case no-prefix-overmatch "a plain name does not match another job that only shares a prefix" '  t:
    name: Test
    runs-on: ubuntu-latest
    steps:
      - run: echo
' "Testing other" MISSING "blocker: expected check 'Test' was never created at ${HEAD}"
# A job named only by an expression (crusty round 4, item 5) is met by the jobs
# of its run that no other defined job matches; here it is the run's only job.
match_case expression-only "a job named only by an expression is met by a job no other defined job matches" '  e:
    name: ${{ matrix.label }}
    runs-on: ubuntu-latest
    steps:
      - run: echo
' "anything" GREEN
# Rule 2 keeps the space before the first ${{ (crusty round 4, item 4), and a
# leg whose expression is empty is matched by the prefix GitHub trims.
match_case rule2-trailing-space "rule 2 keeps the space before \${{, so 'Testing' does not match" '  t:
    name: Test ${{ matrix.os }}
    runs-on: ubuntu-latest
    steps:
      - run: echo
' "Testing" MISSING "blocker: expected check 'Test <matrix.os>' was never created at ${HEAD}"
match_case rule2-trimmed-equal "rule 2 matches the trimmed prefix (an empty expression)" '  f:
    name: features exclude ${{ matrix.name }}
    runs-on: ubuntu-latest
    steps:
      - run: echo
' "features exclude" GREEN
match_case quoted-name "a quoted name: value" '  q:
    name: "Quoted Job"
    runs-on: ubuntu-latest
    steps:
      - run: echo
' "Quoted Job" GREEN
match_case name-after-steps "a job-level name: placed after a nested block" '  late:
    runs-on: ubuntu-latest
    env:
      name: not-the-job-name
    steps:
      - name: also not
        run: echo
    name: Late Name
' "Late Name" GREEN
match_case comment-line "a commented-out job is not expected" '  real:
    name: Real
    runs-on: ubuntu-latest
    steps:
      - run: echo
#  ghost:
#    name: Ghost
' "Real" GREEN

# A comment after a key is not its value: `pull_request: # note` and
# `jobs: # build jobs` must scan as empty values, not as inline ones.
comment_case() { # comment_case <id> <description> <yaml>
  local fx; fx="$(new_fx)"; ec_wf "$fx" c.yml "$3"
  ec_run "$fx" 1006 .github/workflows/c.yml __HEAD__; ec_job "$fx" 1006 Commented completed success
  run_helper "$fx"
  expect "EC-comment-$1" "$2 stays GREEN" GREEN 0
  ran_check "EC-comment-$1-no-inline" "$2 gives no inline-value reason" \
    bash -c '! grep -qF "inline" "$1"' _ "${WORK}/out"
}
comment_case after-keys "comments after on:, pull_request: and jobs:" "name: C
on: # triggers
  pull_request: # note
    branches: [main] # base
jobs: # build jobs
  c: # the job
    name: Commented # shown name
    runs-on: ubuntu-latest
    steps:
      - run: echo
"
comment_case after-flow-list "a comment after an on: flow list" "name: C
on: [pull_request] # x
jobs:
  c:
    name: Commented
    runs-on: ubuntu-latest
    steps:
      - run: echo
"

# A tab inside a quoted job name must not shift the scanner's fields: the
# job keeps its whole name (the tab read as a space) and, absent, blocks.
FX="$(new_fx)"
ec_wf "$FX" t.yml "$(printf 'name: T\non: pull_request\njobs:\n  t:\n    name: "x\t1"\n    runs-on: ubuntu-latest\n    steps:\n      - run: echo\n')"
ec_run "$FX" 1007 .github/workflows/t.yml __HEAD__; ec_job "$FX" 1007 Other completed success
run_helper "$FX"
expect "EC-S-tab-in-name" "a tab in a job name does not shift the scanner's fields" MISSING 1 \
  "blocker: expected check 'x 1' was never created at ${HEAD}"

# A reported job meets only the defined jobs that match it best (crusty round
# 4, item 4): rule 1 beats rules 3-4, which beat rule 2, and a longer prefix
# beats a shorter one. `two_jobs` defines `$1` and `$2` in m.yml.
two_jobs() { # two_jobs <name a> <name b>: prints a fixture dir with m.yml and run 1005
  local fx; fx="$(new_fx)"
  ec_wf "$fx" m.yml "name: M
on: pull_request
jobs:
  a:
    name: $1
    runs-on: ubuntu-latest
    steps:
      - run: echo
  b:
    name: $2
    runs-on: ubuntu-latest
    steps:
      - run: echo
"
  ec_run "$fx" 1005 .github/workflows/m.yml __HEAD__; printf '%s' "$fx"
}
FX="$(two_jobs 'Test ${{ matrix.os }}' 'Test docs')"; ec_job "$FX" 1005 "Test docs" completed success
run_helper "$FX"
expect "EC-match-best-rule1" "a job another defined job names exactly does not meet a prefix" MISSING 1 \
  "blocker: expected check 'Test <matrix.os>' was never created at ${HEAD}"
ec_job "$FX" 1005 "Test ubuntu-latest" completed success
run_helper "$FX"
expect "EC-match-best-own-leg" "with a leg of its own, the prefix job passes" GREEN 0
FX="$(two_jobs 'Build ${{ matrix.t }}' 'Build docs ${{ matrix.f }}')"; ec_job "$FX" 1005 "Build docs html" completed success
run_helper "$FX"
expect "EC-match-best-longer-prefix" "a longer matching prefix wins the reported job" MISSING 1 \
  "blocker: expected check 'Build <matrix.t>' was never created at ${HEAD}"
FX="$(two_jobs '${{ matrix.a }}' '${{ matrix.b }}')"; ec_job "$FX" 1005 "x1" completed success
run_helper "$FX"
expect "EC-match-two-expression-only" "two expression-only jobs in one workflow are UNREADABLE, by name" UNREADABLE 1 \
  "blocker: expected checks unreadable: job '<matrix.a>' of .github/workflows/m.yml cannot be matched: 2 of its jobs are named only by an expression; give them literal names"
ran_check "EC-match-two-expression-only-not-missing" "neither is called never created" \
  bash -c '! grep -qF "was never created" "$1"' _ "${WORK}/out"
# Each reported job goes to at most one defined job (crusty round 5, item 4).
# Two defined jobs with the same literal prefix match a leg equally: it belongs
# to neither, so both are UNREADABLE by name instead of both being met by one
# leg while the other job's legs were never created.
FX="$(two_jobs 'Test ${{ matrix.os }}' 'Test ${{ matrix.py }}')"; ec_job "$FX" 1005 "Test ubuntu-latest" completed success
run_helper "$FX"
expect "EC-match-tie-unreadable" "a leg two defined jobs match equally meets neither: both UNREADABLE, by name" UNREADABLE 1 \
  "blocker: expected checks unreadable: job 'Test <matrix.os>' of .github/workflows/m.yml cannot be matched: another of its jobs matches 'Test ubuntu-latest' as well; give them distinct literal names" \
  "blocker: expected checks unreadable: job 'Test <matrix.py>' of .github/workflows/m.yml cannot be matched: another of its jobs matches 'Test ubuntu-latest' as well; give them distinct literal names"
ran_check "EC-match-tie-not-passed" "the tied leg is not evidence for either job" \
  bash -c '! grep -qF "evidence: expected '"'"'Test ubuntu-latest'"'"'" "$1"' _ "${WORK}/out"
# Two jobs with the same exact name tie too: one reported job cannot stand for both.
FX="$(two_jobs 'Lint' 'Lint')"; ec_job "$FX" 1005 "Lint" completed success
run_helper "$FX"
expect "EC-match-tie-same-name" "two defined jobs with one name and one reported job are UNREADABLE" UNREADABLE 1 \
  "blocker: expected checks unreadable: job 'Lint' of .github/workflows/m.yml cannot be matched: another of its jobs matches 'Lint' as well; give them distinct literal names"
# No reported job at all: nothing is tied, so both are simply never created.
FX="$(two_jobs 'Test ${{ matrix.os }}' 'Test ${{ matrix.py }}')"; ec_job "$FX" 1005 "Other" completed success
run_helper "$FX"
expect "EC-match-tie-none-created" "with no leg of either, both same-prefix jobs are MISSING" MISSING 1 \
  "blocker: expected check 'Test <matrix.os>' was never created at ${HEAD}" \
  "blocker: expected check 'Test <matrix.py>' was never created at ${HEAD}"

# A matrix job skipped before its matrix expanded is reported under its raw
# name, `${{ }}` and all (crusty round 6, item 1; home-assistant/core run
# 42858618158). That is rule 1, an exact match, and beats the template match
# a sibling with the same literal text gets, so neither job is tied.
raw_jobs() { # raw_jobs <name a> <name b>: m.yml with two `if: false` jobs and run 1005
  local fx; fx="$(new_fx)"
  ec_wf "$fx" m.yml "name: M
on: pull_request
jobs:
  a:
    name: $1
    if: false
    runs-on: ubuntu-latest
    steps:
      - run: echo
  b:
    name: $2
    if: false
    runs-on: ubuntu-latest
    steps:
      - run: echo
  lint:
    name: Lint
    runs-on: ubuntu-latest
    steps:
      - run: echo
"
  ec_run "$fx" 1005 .github/workflows/m.yml __HEAD__; ec_job "$fx" 1005 Lint completed success; printf '%s' "$fx"
}
PG='Run ${{ matrix.postgresql-group }} tests Python ${{ matrix.python-version }}'
MARIA='Run ${{ matrix.mariadb-group }} tests Python ${{ matrix.python-version }}'
FX="$(raw_jobs "$PG" "$MARIA")"; ec_job "$FX" 1005 "$PG" completed skipped; ec_job "$FX" 1005 "$MARIA" completed skipped
run_helper "$FX"
expect "EC-match-raw-siblings" "two skipped jobs with one literal prefix, reported under their raw names, are GREEN" GREEN 0 \
  "evidence: expected 'Run <matrix.postgresql-group> tests Python <matrix.python-version>': skipped" \
  "evidence: expected 'Run <matrix.mariadb-group> tests Python <matrix.python-version>': skipped"
ec_drop_job "$FX" 1005 "$MARIA"
run_helper "$FX"
expect "EC-match-raw-sibling-gone" "the raw name of one sibling does not stand in for the other" MISSING 1 \
  "blocker: expected check 'Run <matrix.mariadb-group> tests Python <matrix.python-version>' was never created at ${HEAD}"
# An expression-only job meets its own raw name too: the exact match scores
# highest, and the expression-only branch takes it as well as the unclaimed jobs.
FX="$(raw_jobs '${{ matrix.target }}' 'Build')"; ec_job "$FX" 1005 '${{ matrix.target }}' completed skipped
ec_job "$FX" 1005 Build completed success
run_helper "$FX"
expect "EC-match-raw-expression-only" "a skipped expression-only job reported under its raw name is GREEN" GREEN 0 \
  "evidence: expected '<matrix.target>': skipped"
# Two expression-only jobs are told apart when each is reported under its raw
# name; only a job that neither raw name claims leaves them UNREADABLE.
FX="$(raw_jobs '${{ matrix.a }}' '${{ matrix.b }}')"; ec_job "$FX" 1005 '${{ matrix.a }}' completed skipped
ec_job "$FX" 1005 '${{ matrix.b }}' completed skipped
run_helper "$FX"
expect "EC-match-raw-two-expression-only" "two expression-only jobs both reported raw are GREEN" GREEN 0
ec_job "$FX" 1005 "x1" completed success
run_helper "$FX"
expect "EC-match-raw-two-expression-only-unclaimed" "a job neither raw name claims leaves them UNREADABLE" UNREADABLE 1 \
  "blocker: expected checks unreadable: job '<matrix.a>' of .github/workflows/m.yml cannot be matched: 2 of its jobs are named only by an expression; give them literal names"
# GitHub re-serialises a raw expression (pydantic/pydantic #14268 shows
# `${{ (matrix.interpreter || 'all') }}` for `${{ matrix.interpreter || 'all' }}`),
# so a raw name is also matched by rule 2: each `${{ }}` stands for any text.
FX="$(raw_jobs "Build on \${{ matrix.os }} (\${{ matrix.interpreter || 'all' }})" "Build PGO on \${{ matrix.os }}")"
ec_job "$FX" 1005 "Build on \${{ matrix.os }} (\${{ (matrix.interpreter || 'all') }})" completed skipped
ec_job "$FX" 1005 "Build PGO on \${{ matrix.os }}" completed skipped
run_helper "$FX"
expect "EC-match-raw-reserialized" "a raw name with a re-serialised expression is matched by the template" GREEN 0 \
  "evidence: expected 'Build on <matrix.os> (<(matrix.interpreter || all)>)': skipped"

# Rule 2 compares the whole name, not only the text before the first ${{
# (crusty round 6, item 2). deno's ci.generated.yml at #32579 names its test
# jobs `test ${{ matrix.test_crate }} ${{ matrix.shard_label }}debug macos-x86_64`
# and `...release macos-x86_64`: the text after the expressions tells them apart.
DEB='test ${{ matrix.test_crate }} ${{ matrix.shard_label }}debug macos-x86_64'
REL='test ${{ matrix.test_crate }} ${{ matrix.shard_label }}release macos-x86_64'
FX="$(two_jobs "'$DEB'" "'$REL'")"
for n in "test integration (1/2) debug macos-x86_64" "test libs debug macos-x86_64" \
         "test integration (1/2) release macos-x86_64" "test libs release macos-x86_64"; do
  ec_job "$FX" 1005 "$n" completed success
done
run_helper "$FX"
expect "EC-match-deno-whole-name" "deno's debug and release jobs, which differ after the expressions, are GREEN" GREEN 0 \
  "evidence: expected 'test libs debug macos-x86_64': success" "evidence: expected 'test libs release macos-x86_64': success"
ec_drop_job "$FX" 1005 "test integration (1/2) release macos-x86_64"; ec_drop_job "$FX" 1005 "test libs release macos-x86_64"
run_helper "$FX"
expect "EC-match-deno-release-gone" "with every release leg gone, the release job is MISSING, not met by a debug leg" MISSING 1 \
  "blocker: expected check 'test <matrix.test_crate> <matrix.shard_label>release macos-x86_64' was never created at ${HEAD}"
match_case rule2-suffix-compared "rule 2 compares the literal text after the expression" '  t:
    name: Run ${{ matrix.g }} tests
    runs-on: ubuntu-latest
    steps:
      - run: echo
' "Run unit testsuite" MISSING "blocker: expected check 'Run <matrix.g> tests' was never created at ${HEAD}"
match_case rule2-reusable-tail "rule 2 with a reusable workflow's ' / ' tail" '  t:
    name: Run ${{ matrix.g }} tests
    uses: ./.github/workflows/reusable.yml
' "Run unit tests / pytest" GREEN
match_case rule2-matrix-tail "rule 2 with GitHub's ' (' matrix tail" '  t:
    name: Run ${{ inputs.g }} tests
    runs-on: ubuntu-latest
    steps:
      - run: echo
' "Run unit tests (ubuntu-latest)" GREEN
# A name that starts with an expression still has literal text to match: it is
# not expression-only, and GitHub trims the space an empty expansion leaves.
match_case rule2-leading "a name starting with an expression is matched whole" '  t:
    name: ${{ matrix.os }} tests
    runs-on: ubuntu-latest
    steps:
      - run: echo
' "ubuntu tests" GREEN
match_case rule2-leading-trimmed "a leading expression that is empty leaves the trimmed text" '  t:
    name: ${{ matrix.os }} tests
    runs-on: ubuntu-latest
    steps:
      - run: echo
' "tests" GREEN
match_case rule2-leading-not-expression-only "a name starting with an expression is not met by any job" '  t:
    name: ${{ matrix.os }} tests
    runs-on: ubuntu-latest
    steps:
      - run: echo
' "anything" MISSING "blocker: expected check '<matrix.os> tests' was never created at ${HEAD}"
match_case rule2-metacharacters "regex metacharacters in a name are literal" '  t:
    name: "C++ [x] ${{ matrix.v }} (a|b).*"
    runs-on: ubuntu-latest
    steps:
      - run: echo
' "C++ [x] 17 (a|b).*" GREEN
match_case rule2-metacharacters-literal "a metacharacter does not act as a pattern" '  t:
    name: "C++ ${{ matrix.v }} (a|b)"
    runs-on: ubuntu-latest
    steps:
      - run: echo
' "C 17 a" MISSING "blocker: expected check 'C++ <matrix.v> (a|b)' was never created at ${HEAD}"
# Each literal between two expressions matches at its leftmost place, atomically,
# and a run of expressions with only spaces between is one: a name with many
# expressions cannot make the regex engine give up, which would turn the whole
# evaluation into a bare UNREADABLE. 25 of them against an 80-part leg.
MANY="x$(for i in $(seq 25); do printf -- '-${{ matrix.a }}'; done)-z"
LEG="x$(for i in $(seq 80); do printf -- '-a'; done)"
FX="$(new_fx)"; ec_wf "$FX" m.yml "name: M
on: pull_request
jobs:
  a:
    name: ${MANY}
    runs-on: ubuntu-latest
    steps:
      - run: echo
"
ec_run "$FX" 1005 .github/workflows/m.yml __HEAD__; ec_job "$FX" 1005 "$LEG" completed success
run_helper "$FX"
check "EC-match-many-expressions" "a name with 25 expressions against a near-miss leg is MISSING, not an evaluation failure" verdict_is MISSING 1
ran_check "EC-match-many-expressions-no-failure" "the evaluation did not fail" \
  bash -c '! grep -qF "the evaluation failed" "$1"' _ "${WORK}/out"
MANY="x$(for i in $(seq 25); do printf ' ${{ matrix.a }}'; done) z"; LEG="x$(for i in $(seq 80); do printf ' a'; done)"
ec_wf "$FX" m.yml "name: M
on: pull_request
jobs:
  a:
    name: ${MANY}
    runs-on: ubuntu-latest
    steps:
      - run: echo
"
run_helper "$FX"
check "EC-match-many-spaced-expressions" "25 space-separated expressions against a near-miss leg are MISSING too" verdict_is MISSING 1

# Two jobs with one name (home-assistant's ci.yaml: pytest-full and
# pytest-partial, run under opposite `if:`) cannot be told apart by any name
# rule: the skipped one's raw name and the other's leg both match both jobs.
# The helper does not read "any one of them" into this; it stays UNREADABLE.
SAME='Run tests Python ${{ matrix.python-version }} (${{ matrix.group }})'
FX="$(raw_jobs "$SAME" "$SAME")"; ec_job "$FX" 1005 "$SAME" completed skipped
ec_job "$FX" 1005 "Run tests Python 3.14.10 (hue)" completed success
run_helper "$FX"
expect "EC-match-same-name-template" "two jobs with one templated name stay UNREADABLE" UNREADABLE 1 \
  "blocker: expected checks unreadable: job 'Run tests Python <matrix.python-version> (<matrix.group>)' of .github/workflows/m.yml cannot be matched: another of its jobs matches 'Run tests Python <matrix.python-version> (<matrix.group>)' as well; give them distinct literal names"

# tokio's ci.yml at PR #8771 (crusty round 4, item 5): `wasm32-wasip1` is named
# only by `${{ matrix.target }}`, and its legs are the run's jobs that no other
# defined job matches. Before, it was "never created" though both legs ran.
FX="$(new_fx)"; ec_tokio_ci "$FX"
run_helper "$FX" "$HEAD" master
expect "EC-match-tokio-ci" "tokio's ci.yml (an expression-only job, a trimmed leg) is GREEN" GREEN 0 \
  "evidence: expected 'wasm32-wasip1': success" "evidence: expected 'wasm32-wasip1-threads': success" \
  "evidence: expected 'features exclude': success"
ec_drop_job "$FX" 1011 wasm32-wasip1; ec_drop_job "$FX" 1011 wasm32-wasip1-threads
run_helper "$FX" "$HEAD" master
# The blocker names the job by its key and says why (crusty round 5, item 5):
# no agent can look for a check called '<matrix.target>'.
expect "EC-match-tokio-expression-gone" "with its legs gone, the expression-only job is MISSING, named by key with the reason" MISSING 1 \
  "blocker: expected job 'wasm32-wasip1' of .github/workflows/tokio-ci.yml was never created at ${HEAD}: its name '<matrix.target>' is only an expression and every job of run 1011 matches another job; if it ran, give it a literal name"
check "EC-match-tokio-expression-gone-key" "missing names the job key, not the expression" \
  test "$MISSING" = '["wasm32-wasip1"]'
# Jobs are read at the head, but pull_request runs build the merge commit
# (crusty round 5, item 6): a job-level MISSING says how a stale branch shows.
expect "EC-merge-commit-hint" "a job never created carries the merge-commit hint, naming the base" MISSING 1 \
  "evidence: jobs are read from the workflow files at the head, but pull_request runs build the merge commit with master: if master changed a missing job or workflow since this branch forked, merge master in"

# ---------------------------------------------------------------------------
# 8b. Block-scalar and multi-line job names (crusty round 4, item 2).
# ---------------------------------------------------------------------------
# cpython's build.yml at PR #161842 names three jobs with `name: >-`. Before,
# each read as the literal '>-' and was "never created" for ever.
FX="$(new_fx)"; ec_cpython "$FX"
run_helper "$FX"
expect "EC-bs-cpython" "cpython's build.yml (three name: >- jobs) is GREEN" GREEN 0 \
  "evidence: expected 'Windows / Build and test (x64, tail-call)': success" \
  "evidence: expected 'macOS (free-threading) / build and test (macos-26)': success" \
  "evidence: expected 'Ubuntu  (bolt) / build and test (ubuntu-26.04)': success" \
  "evidence: expected 'Windows MSI<>': skipped"
ran_check "EC-bs-cpython-no-indicator" "no check is named by the block indicator" \
  bash -c '! grep -qF "'"'"'>-'"'"'" "$1"' _ "${WORK}/out"
# Every Windows leg gone: `Windows MSI${{ '' }}` shares the `Windows ` prefix
# but matches its own job better, so it does not stand in for them.
ec_jq "$FX/jobs-1010.json" '.jobs |= map(select(.name | test("^Windows (\\(free-threading\\) )?/") | not)) | .total_count = (.jobs|length)'
ec_jq "$FX/check-runs.json" '.check_runs |= map(select(.name | test("^Windows (\\(free-threading\\) )?/") | not)) | .total_count = (.check_runs|length)'
run_helper "$FX"
expect "EC-bs-cpython-windows-gone" "with every Windows leg gone, the folded Windows job is MISSING" MISSING 1 \
  "blocker: expected check 'Windows <fromJSON(matrix.free-threading) && (free-threading) || >' was never created at ${HEAD}"

# name_case <id> <description> <name: lines> <reported name> <status> [line]
name_case() {
  local fx; fx="$(new_fx)"
  ec_wf "$fx" n.yml "name: N
on: pull_request
jobs:
  n:
$3
    runs-on: ubuntu-latest
    steps:
      - name: not the job name
        run: |
          echo one
          echo two
"
  ec_run "$fx" 1012 .github/workflows/n.yml __HEAD__; ec_job "$fx" 1012 "$4" completed success
  run_helper "$fx"
  if [ -n "${6:-}" ]; then expect "EC-name-$1" "$2" "$5" 1 "$6"; else expect "EC-name-$1" "$2" "$5" 0; fi
}
BREAK="blocker: expected checks unreadable: .github/workflows/n.yml could not be scanned (job n: name: keeps a line break)"
name_case folded "a folded name (>-) is joined with spaces" '    name: >-
      Build
      all' "Build all" GREEN
name_case folded-comment "a folded name with a comment after its indicator" '    name: > # shown name
      Build all' "Build all" GREEN
name_case literal-one "a literal name (|) of one line" '    name: |
      Build all' "Build all" GREEN
name_case literal-trailing-blank "a blank line after a one-line literal name is not a line break" '    name: |
      Build all
' "Build all" GREEN
name_case indicator "an indentation indicator (>2-) sets the content indent" '    name: >2-
      Build
      all' "Build all" GREEN
name_case hash-content "a line starting with # inside a block scalar is content" '    name: >-
      Build
      #1' "Build #1" GREEN
name_case plain-multiline "a plain name continued on the next line" '    name: Build
      all' "Build all" GREEN
name_case quoted-multiline "a double-quoted name continued on the next line" '    name: "Build
      all"' "Build all" GREEN
name_case quoted-indicator "a quoted \">-\" is a literal name, not a block scalar" '    name: ">-"' ">-" GREEN
name_case literal-two "a literal name (|) over two lines keeps a line break: refused by name" '    name: |
      Build
      all' "Build" UNREADABLE "$BREAK"
name_case folded-blank "a blank line inside a folded name keeps a line break: refused by name" '    name: >-
      Build

      all' "Build all" UNREADABLE "$BREAK"
name_case folded-deeper "a more-indented line inside a folded name keeps its line break: refused by name" '    name: >-
      Build
        all' "Build all" UNREADABLE "$BREAK"
name_case plain-blank "a blank line before a plain name's continuation is a line break: refused by name" '    name: Build

      all' "Build" UNREADABLE "$BREAK"
name_case empty-block "an empty block-scalar name is refused by name" '    name: >-' "n" UNREADABLE \
  "blocker: expected checks unreadable: .github/workflows/n.yml could not be scanned (job n: name: is an empty block scalar)"
# A line break inside a ${{ }} is whitespace in the expression, not in the
# shown name, so it is not refused. cpython's reusable-san.yml splits its
# expression over lines inside a folded name: `${{ inputs.sanitizer }}${{`,
# then deeper lines, then `}}`.
name_case after-expression "a line break inside a \${{ }} is not refused" '    name: >-
      San ${{
        matrix.s
      }}' "San tsan" GREEN
# The whole name is matched (crusty round 6, item 2), so a line break kept
# between two expressions is in the shown name and is refused like any other.
name_case literal-after-expression "a literal (|) name breaking between two expressions is refused by name" '    name: |
      Build ${{ matrix.t }}
      ${{ matrix.u }}' "Build x86_64" UNREADABLE "$BREAK"
name_case folded-between-expressions "a folded name continued after an expression is joined, not refused" '    name: >-
      Build ${{ matrix.t }}
      for ${{ matrix.u }}' "Build x86_64 for linux" GREEN
name_case break-before-expression "a line break before the first \${{ is still refused" '    name: >-
      Build
        ${{ matrix.t }}' "Build x86_64" UNREADABLE "$BREAK"

# A malformed jobs: section matters only in a workflow that is expected: a
# workflow_call-only workflow (cpython's reusable-san.yml before the rule
# above) runs no job here, so it does not make the verdict UNREADABLE. An
# unreadable on: still blocks, because then nobody knows if it is expected.
reusable_case() { # reusable_case <id> <description> <on: line> <status> [line]
  local fx; fx="$(new_fx)"
  ec_wf "$fx" r.yml "name: R
$3
jobs:
  r:
    name: |
      Build
      all
    runs-on: ubuntu-latest
    steps:
      - run: echo
"
  run_helper "$fx"
  if [ -n "${5:-}" ]; then expect "EC-reusable-$1" "$2" "$4" 1 "$5"; else expect "EC-reusable-$1" "$2" "$4" 0; fi
}
reusable_case not-expected "a workflow_call-only workflow with a refused job name stays GREEN" 'on: workflow_call' GREEN
ran_check "EC-reusable-not-expected-no-scan-error" "and no scan error is printed for it" \
  bash -c '! grep -qF "r.yml could not be scanned" "$1"' _ "${WORK}/out"
reusable_case expected "the same job name in a pull_request workflow is refused by name" 'on: [pull_request, workflow_call]' UNREADABLE \
  "blocker: expected checks unreadable: .github/workflows/r.yml could not be scanned (job r: name: keeps a line break)"
reusable_case on-unreadable "an unreadable on: blocks even when the job section is the only other fault" 'on: {workflow_call: {}}' UNREADABLE \
  "blocker: expected checks unreadable: .github/workflows/r.yml could not be scanned (flow mapping under on:)"

# ---------------------------------------------------------------------------
# 9. Security.
# ---------------------------------------------------------------------------
# S1: a second suite reuses a required name with a success; the first fails.
FX="$(new_fx)"; ec_set_job "$FX" 1001 Test completed failure
ec_check "$FX" "Test" completed success __HEAD__ 7777 spoof-app
run_helper "$FX"
check "EC-S1-spoof-red" "a spoofed success in a second suite does not hide a failing check-run" verdict_is RED 1
[ "$STATUS" = RED ] || explain
# ... in the other order too (spoof failing id lower than the real success).
FX="$(new_fx)"; ec_check "$FX" "Test" completed failure __HEAD__ 1 spoof-app
run_helper "$FX"
check "EC-S1-any-suite-fails" "a failing run of a required name in any suite blocks" verdict_is RED 1
# A commit status never overrides a failing check-run of the same name.
FX="$(new_fx)"; ec_set_job "$FX" 1001 Test completed failure
printf '{"state":"success","sha":"%s","total_count":1,"statuses":[{"context":"Test","state":"success"}]}\n' "$HEAD" > "$FX/status.json"
run_helper "$FX"
check "EC-S1-status-no-override" "a success commit status does not override a failing check-run" verdict_is RED 1
# Within one suite the latest run (a rerun) wins.
FX="$(new_fx)"; ec_set_job "$FX" 1001 Test completed failure
ec_jq "$FX/check-runs.json" '.check_runs += [(.check_runs[] | select(.name == "Test") | .id = 99999999 | .conclusion = "success")]'
ec_jq "$FX/jobs-1001.json" '(.jobs[] | select(.name == "Test")).conclusion = "success"'
run_helper "$FX"
expect "EC-S1-rerun-same-suite" "a rerun in the same suite supersedes the failed attempt" GREEN 0
# S2: workflow paths.
FX="$(new_fx)"; ec_jq "$FX/wf-dir.json" '. + [{name:"x.yml", path:".github/workflows/../../evil.yml", type:"file"}]'
unreadable S2-dotdot-path "a workflow path with .." "$FX"
FX="$(new_fx)"; ec_jq "$FX/wf-dir.json" '. + [{name:"x y.yml", path:".github/workflows/x y.yml", type:"file"}]'
unreadable S2-space-path "a workflow path with a space" "$FX"
FX="$(new_fx)"; ec_jq "$FX/wf-dir.json" '. + [{name:"sub", path:".github/workflows/sub/a.yml", type:"file"}]'
unreadable S2-nested-path "a nested workflow path" "$FX"
FX="$(new_fx)"; ec_jq "$FX/wf-dir.json" '. + [{name:"README.md", path:".github/workflows/README.md", type:"file"}]'
run_helper "$FX"
expect "EC-S2-non-yaml-ignored" "a non-YAML file in the workflows dir is not a workflow" GREEN 0
FX="$(new_fx)"; ec_jq "$FX/runs.json" '(.workflow_runs[] | select(.id == 1002)).id = "1002;touch /tmp/pwn"'
unreadable S2-run-id "a run id that is not all digits" "$FX"
# Name injection: a failing check whose name tries to forge a verdict.
FX="$(new_fx)"
ec_check "$FX" 'x"},{"ci_status":"GREEN","missing":[],"blockers":0}' completed failure __HEAD__ 9400 evil
ec_check "$FX" $'Evil\n{"ci_status":"GREEN","missing":[],"blockers":0}' completed failure __HEAD__ 9401 evil
ec_check "$FX" 'Dollar $(touch '"${WORK}"'/pwned) `id` ${HOME}' completed failure __HEAD__ 9402 evil
run_helper "$FX"
check "EC-inject-verdict" "a check name cannot forge the verdict: it stays RED" verdict_is RED 1
check "EC-inject-one-json" "exactly one stdout line is a JSON object, and it is the last" \
  bash -c '[ "$(grep -c "^{" "$1")" = 1 ] && tail -n1 "$1" | grep -q "^{"' _ "${WORK}/out"
ran_check "EC-inject-clean" "printed names carry none of ' \" \\ \$ \` { } outside the verdict line" \
  bash -c '! sed "\$d" "$1" | grep -E "^(blocker|evidence): " | sed -E "s/^(blocker|evidence): (expected )?check '"'"'//; s/'"'"' (is|was) .*//" | grep -q "[\"\\\\\$\`{}]"' _ "${WORK}/out"
ran_check "EC-inject-no-exec" "a name is never executed" test ! -e "${WORK}/pwned"
ran_check "EC-inject-length" "printed names are cut to 120 characters" \
  bash -c '! grep -E "^blocker: " "$1" | awk "length > 400" | grep -q .' _ "${WORK}/out"
FX="$(new_fx)"; ec_check "$FX" "$(printf 'L%.0s' $(seq 1 300))" completed failure __HEAD__ 9500 long
run_helper "$FX"
check "EC-inject-cut-120" "a 300-character name is printed as at most 120 characters" \
  bash -c 'n="$(grep "^blocker: check " "$1" | sed -E "s/^blocker: check .//; s/. is .*//")"; [ -n "$n" ] && [ "${#n}" -le 120 ]' _ "${WORK}/out"
# Hostile workflow YAML is data: a job name with command substitution.
FX="$(new_fx)"; ec_wf "$FX" h.yml 'name: H
on: pull_request
jobs:
  h:
    name: $(touch '"${WORK}"'/pwned2) `id`
    runs-on: ubuntu-latest
    steps:
      - run: echo
'
run_helper "$FX"
ran_check "EC-inject-yaml-no-exec" "a workflow job name is never executed" test ! -e "${WORK}/pwned2"
check "EC-inject-yaml-blocks" "the hostile workflow without a run still blocks" verdict_is MISSING 1
# jq missing or older than 1.6.
REAL_JQ="$(command -v jq)"
NOJQ="${WORK}/nojq-bin"; mkdir -p "$NOJQ"
for b in bash sh env cat head tail tr sed awk grep mktemp rm mkdir printf wc sort uniq cut \
         base64 basename dirname chmod ls cp mv tee od expr; do
  t="$(command -v "$b" 2>/dev/null)" && ln -s "$t" "$NOJQ/$b"
done
cp "${STUB_BIN}/gh" "$NOJQ/gh"
# The route itself needs jq, so give the route a private copy via its PATH.
FX="$(new_fx)"
RUN_PATH="${NOJQ}" run_helper "$FX"
check "EC-no-jq" "with no jq on PATH the helper gives UNREADABLE (or exit 1 with no GREEN)" \
  bash -c '[ "$1" = 1 ] && [ "$2" != GREEN ]' _ "$RC" "$STATUS"
OLDJQ="${WORK}/oldjq-bin"; mkdir -p "$OLDJQ"
cat > "$OLDJQ/jq" <<JQ
#!/usr/bin/env bash
case " \$* " in *" --version "*) echo jq-1.5; exit 0 ;; esac
case "\$*" in *base64d*) echo "jq: error: base64d/0 is not defined at <top-level>" >&2; exit 3 ;; esac
exec "${REAL_JQ}" "\$@"
JQ
chmod +x "$OLDJQ/jq"
FX="$(new_fx)"
RUN_PATH="${OLDJQ}:${STUB_BIN}:${PATH}" run_helper "$FX"
check "EC-old-jq" "jq older than 1.6 gives UNREADABLE" verdict_is UNREADABLE 1
[ "$STATUS" = UNREADABLE ] || explain
# Exported functions are ignored (bash -p) and dropped (builtin unset -f).
FX="$(new_fx)"; ec_drop_job "$FX" 1002 "Recipe runner pin ancestry"
CALLS="${WORK}/calls-fn"; : > "$CALLS"
(cd "${WORK}" && gh() { printf '{"ci_status":"GREEN","missing":[],"blockers":0}\n'; } && export -f gh \
  && GH_CI_FIX="$FX" GH_CI_ROUTE="${ROUTE}" GH_HEAD="$HEAD" GH_CALLS="$CALLS" PATH="${STUB_BIN}:${PATH}" \
     /bin/bash -p "${HELPER}" "$HEAD" main > "${WORK}/out" 2>/dev/null </dev/null)
check "EC-exported-fn" "an exported gh function cannot answer for the helper" \
  test "$(tail -n1 "${WORK}/out" | jq -r .ci_status 2>/dev/null)" = MISSING

# ---------------------------------------------------------------------------
# 10. Callers.
# ---------------------------------------------------------------------------
# #2329 retired the shell merge gate, and merge-evidence step-03 is an agent
# step: its prompt must run the helper from AMPLIHACK_HOME and keep MISSING,
# PENDING and RED from passing.
S3_PROMPT="$(awk '/id: "step-03-wait-for-ci"/{f=1} f&&/^output:/{f=0} f' "${RECIPES}/autodrive-merge-evidence.yaml")"
check "EC-S3-amplihack-home" "step-03 runs the helper from \$AMPLIHACK_HOME/amplifier-bundle/tools only" \
  bash -c 'printf "%s" "$1" | grep -qF "/bin/bash -p \"\$AMPLIHACK_HOME/amplifier-bundle/tools/autodrive_expected_checks.sh\"" && printf "%s" "$1" | grep -qF "never a copy from the PR worktree"' _ "$S3_PROMPT"
check "EC-S3-missing-blocks" "step-03 lowers GREEN to RED, PENDING or MISSING and never passes a missing check" \
  bash -c 'printf "%s" "$1" | grep -qF "RED, PENDING or MISSING replace it" && printf "%s" "$1" | grep -qF "is MISSING, not a pass"' _ "$S3_PROMPT"

# The light drive's merge loop runs the same gate (D17).
check "EC-D17-light-uses-merge-loop" "auto-drive-light runs autodrive-merge-loop" \
  grep -qE 'recipe: "autodrive-merge-loop"' "${RECIPES}/auto-drive-light.yaml"
# Platform facts: CHECKS is advisory (D18).
check "EC-D18-advisory" "autodrive_platform_facts.sh says CHECKS is advisory and the helper decides" \
  bash -c 'grep -iE "^[[:space:]]*#.*advisory" "$1" | grep -qF "autodrive_expected_checks"' _ "${TOOLS}/autodrive_platform_facts.sh"

# Docs and the two SKILL.md copies.
DOC="${REPO_ROOT}/docs/reference/auto-drive-to-merge.md"
check "EC-docs-ci-green" "the reference doc defines CI green" \
  grep -qF 'CI green means every expected check exists at the head SHA and passed' "$DOC"
check "EC-docs-limits" "the reference doc has the limits section" grep -qF '### Limits of the expected set' "$DOC"
check "EC-skill-copies-identical" "both auto-drive-to-merge SKILL.md copies are identical" \
  cmp -s "${REPO_ROOT}/amplifier-bundle/skills/auto-drive-to-merge/SKILL.md" "${REPO_ROOT}/docs/claude/skills/auto-drive-to-merge/SKILL.md"
check "EC-skill-mentions" "SKILL.md states the expected-checks rule" \
  grep -qF 'autodrive_expected_checks' "${REPO_ROOT}/amplifier-bundle/skills/auto-drive-to-merge/SKILL.md"

echo
echo "=== #2186 expected checks: ${PASS_COUNT} passed, ${FAIL_COUNT} failed ==="
[ "${FAIL_COUNT}" -eq 0 ]
