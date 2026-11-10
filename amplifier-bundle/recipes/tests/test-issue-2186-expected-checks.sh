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
#   4. AC2-AC5: missing jobs, required checks, workflow runs, other SHA.
#   5. AC6: unreadable inputs, and the empty answers that are NOT unreadable.
#   6. Conclusions: failure, pending, neutral, skipped required, other checks,
#      zero checks, status order.
#   7. Workflow selection: on: forms, branch filters, disabled, paths.
#   8. Job matching rules 1-4 and the empty prefix.
#   9. Security: S1 spoof, S2 paths and run ids, name injection, jq version.
#  10. Callers: gate section 5 (static), merge-evidence step-03 (run), light
#      drive, platform facts comment, docs.
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
REPO_ROOT="$(cd "${HERE}/../../.." && pwd -P)"
TOOLS="${REPO_ROOT}/amplifier-bundle/tools"
RECIPES="${REPO_ROOT}/amplifier-bundle/recipes"
HELPER="${TOOLS}/autodrive_expected_checks.sh"
GATE="${TOOLS}/autodrive_merge_gate.sh"
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
check "EC-mode-755" "the helper is mode 755" \
  test "$(stat -c '%a' "${HELPER}" 2>/dev/null)" = "755"
check "EC-shebang" "the helper starts with #!/bin/bash -p" \
  test "$(head -n1 "${HELPER}" 2>/dev/null)" = "#!/bin/bash -p"
check "EC-unset-f" "the helper drops gh/git/jq functions with builtin unset -f" \
  grep -qE '^[[:space:]]*builtin unset -f gh git jq' "${HELPER}"
check "EC-budget" "the helper is at most 400 lines" \
  test "$(wc -l < "${HELPER}" 2>/dev/null || echo 9999)" -le 400
check "EC-gate-budget" "the gate stays at most 400 lines" \
  test "$(wc -l < "${GATE}")" -le 400
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
  bash -c 'grep -qxF "evidence: workflow .github/workflows/ci.yml: run 1001" "$1" && grep -qxF "evidence: workflow .github/workflows/recipe-runner-pin.yml: run 1002" "$1"' _ "${WORK}/out"
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
# D8': GitHub does not always create a job whose job-level if: is false (docs.yml
# deploy on PR #2152). Absent from a completed run it is noted, not blocked;
# absent from a run still going it is PENDING.
FX="$(new_fx)"; ec_run "$FX" 1003 .github/workflows/docs.yml __HEAD__
ec_job "$FX" 1003 build completed success
run_helper "$FX"
expect "EC-D8-if-job-absent-completed" "an if: job absent from a completed run passes with a note" GREEN 0 \
  "evidence: expected 'deploy': not created (job-level if:, run 1003 completed)"
ec_jq "$FX/runs.json" '(.workflow_runs[] | select(.id == 1003)).status = "in_progress"'
run_helper "$FX"
expect "EC-D8-if-job-absent-running" "an if: job absent from a run still going is PENDING" PENDING 1 \
  "blocker: expected check 'deploy' is not created yet; run 1003 is in_progress at ${HEAD}"

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

# D6: the latest run per path decides (an older run at the head lacking a job).
FX="$(new_fx)"; ec_run "$FX" 900 .github/workflows/ci.yml __HEAD__
echo '{"total_count":1,"jobs":[{"id":90000,"run_id":900,"name":"Lint & Format","status":"completed","conclusion":"success"}]}' > "$FX/jobs-900.json"
run_helper "$FX"
expect "EC-D6-latest-run" "the latest run (highest id) for a workflow is the one whose jobs count" GREEN 0
check "EC-D6-latest-jobs-read" "the latest run's jobs are read" grep -qF 'actions/runs/1001/jobs' "${CALLS}"

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
for c in cancelled timed_out action_required stale startup_failure neutral; do
  FX="$(new_fx)"; ec_set_job "$FX" 1002 "Recipe runner pin ancestry" completed "$c"
  run_helper "$FX"
  expect "EC-R-${c}" "an expected job concluding ${c} gives RED" RED 1 \
    "blocker: expected check 'Recipe runner pin ancestry' is completed/${c} at ${HEAD}"
done
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
}
FX="$(new_fx)"; add_cancelled_ci_run "$FX" 999
run_helper "$FX"
expect "EC-S3-superseded-cancelled" "a cancelled earlier ci.yml run at the head, superseded by a green one, gives GREEN" GREEN 0
FX="$(new_fx)"; add_cancelled_ci_run "$FX" 1003
run_helper "$FX"
expect "EC-S3-latest-cancelled" "when the latest ci.yml run at the head is the cancelled one, it blocks" RED 1 \
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
expect "EC-R7-other-neutral" "neutral blocks on a check that is not expected too" RED 1 \
  "blocker: check 'Some Bot' is completed/neutral at ${HEAD}"
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
expect "EC-neutral-required-no-job" "a required check-run with no defined job that reports neutral blocks" RED 1 \
  "blocker: expected check 'External Scan' is completed/neutral at ${HEAD}"
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
# with NO run; expected -> MISSING (never ran), not expected -> GREEN.
wf_expected() {
  local fx; fx="$(new_fx)"; ec_wf "$fx" extra.yml "$3"
  run_helper "$fx"
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
wf_expected types-ignored "pull_request with a types: filter" "name: A
on:
  pull_request:
    types: [labeled]
${J}" yes
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
wf_expected branch-glob "branches with a glob" "name: A
on:
  pull_request:
    branches: ['releases/**']
${J}" yes
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
wf_expected ignore-glob "branches-ignore with a glob" "name: A
on:
  pull_request:
    branches-ignore: ['ma*']
${J}" yes
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
match_case empty-prefix "a job named only by an expression counts as missing" '  e:
    name: ${{ matrix.label }}
    runs-on: ubuntu-latest
    steps:
      - run: echo
' "anything" MISSING "blocker: expected check '<matrix.label>' was never created at ${HEAD}"
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

# A tab inside a quoted job name must not shift the scanner's fields: the job
# has no job-level if:, so absent from a completed run it blocks.
FX="$(new_fx)"
ec_wf "$FX" t.yml "$(printf 'name: T\non: pull_request\njobs:\n  t:\n    name: "x\t1"\n    runs-on: ubuntu-latest\n    steps:\n      - run: echo\n')"
ec_run "$FX" 1007 .github/workflows/t.yml __HEAD__; ec_job "$FX" 1007 Other completed success
run_helper "$FX"
if verdict_is MISSING 1 && ! has_prefix "not created (job-level if:"; then
  pass "EC-S-tab-in-name" "a tab in a job name does not turn the job into an if: job"
else
  fail "EC-S-tab-in-name" "a tab in a job name does not turn the job into an if: job"; explain
fi

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
# Gate section 5 (static; the gate's runtime cases are in
# test-auto-drive-to-merge.sh section 5).
S5="$(awk '/^# --- 5\./{f=1} /^# --- 6\./{f=0} f' "${GATE}")"
check "EC-gate-runs-helper" "gate section 5 runs the helper from GATE_HOME" \
  bash -c 'printf "%s" "$1" | grep -qF "\${GATE_HOME}/autodrive_expected_checks.sh"' _ "$S5"
check "EC-gate-passes-base" "gate section 5 passes HEAD_SHA and the baseRefName it read" \
  bash -c 'printf "%s" "$1" | grep -qE "autodrive_expected_checks\.sh\"? +\"\\\$HEAD_SHA\" +\"\\\$[A-Z_]+\""' _ "$S5"
check "EC-gate-no-pr-checks" "gate section 5 no longer decides CI from gh pr checks" \
  bash -c '! printf "%s" "$1" | grep -vE "^[[:space:]]*#" | grep -qF "gh pr checks"' _ "$S5"
check "EC-gate-tail-n1" "gate section 5 reads ci_status from the last line only" \
  bash -c 'printf "%s" "$1" | grep -qE "tail -n ?1"' _ "$S5"
check "EC-gate-unreadable-fallback" "gate section 5 carries the fallback blocker text" \
  bash -c 'printf "%s" "$1" | grep -qF "expected checks unreadable: the reader gave no verdict or no blocker"' _ "$S5"
check "EC-gate-maps-lines" "gate section 5 maps evidence: lines to note and blocker: lines to block" \
  bash -c 'printf "%s" "$1" | grep -qF "evidence:" && printf "%s" "$1" | grep -qF "blocker:" && printf "%s" "$1" | grep -qE "(^|[^a-z])note " && printf "%s" "$1" | grep -qE "(^|[^a-z])block "' _ "$S5"
check "EC-gate-base-read" "the gate reads baseRefName in its own pr view" \
  grep -qE 'gh pr view "\$PR" --json [a-zA-Z,]*baseRefName' "${GATE}"
check "EC-gate-argv-kept" "the gate's merge argv keeps --match-head-commit" grep -qF -- '--match-head-commit' "${GATE}"

# Merge-evidence step-03 (run with the fake gh, no wait: the rollup is green).
extract_step_command() {
  awk -v step="$2" '
    index($0, "id: \"" step "\"") { instep=1 }
    instep && $0 ~ /^    command: \|/ { incmd=1; next }
    incmd { if ($0 ~ /^    [a-zA-Z_]+:/ || $0 ~ /^  - id:/) { exit } sub(/^      /, ""); print }
  ' "$1"
}
S3="$(extract_step_command "${RECIPES}/autodrive-merge-evidence.yaml" step-03-wait-for-ci)"
[ -n "$S3" ] || { echo "HARNESS-ERROR: could not extract merge-evidence step-03" >&2; exit 2; }
check "EC-S3-amplihack-home" "step-03 runs the helper from \$AMPLIHACK_HOME/amplifier-bundle/tools only" \
  bash -c 'printf "%s" "$1" | grep -qE "AMPLIHACK_HOME[}]?/amplifier-bundle/tools/autodrive_expected_checks\.sh" && ! printf "%s" "$1" | grep -qE "(REPO_PATH|TOP|toplevel)[^\n]*autodrive_expected_checks"' _ "$S3"
check "EC-S3-one-sleep" "step-03 adds no new wait (one sleep, the existing loop)" \
  test "$(printf '%s\n' "$S3" | grep -cE '(^|[^A-Za-z_])sleep ')" -le 1
check "EC-S3-tail-n1" "step-03 reads the helper's verdict from the last line" \
  bash -c 'printf "%s" "$1" | grep -qE "tail -n ?1"' _ "$S3"
check "EC-S3-quoted" "step-03 passes the head and base to the helper quoted" \
  bash -c 'printf "%s" "$1" | grep -E "autodrive_expected_checks\.sh" | grep -qE "\"\\\$[A-Za-z_]+\" +\"\\\$[A-Za-z_]+\""' _ "$S3"
check "EC-S3-printf-signal" "step-03's final printf takes ci_signal as one %s" \
  bash -c 'printf "%s" "$1" | grep -qF "\"ci_signal\":\"%s\"}"' _ "$S3"

AH="${WORK}/amplihack-home"; mkdir -p "${AH}/amplifier-bundle"
ln -s "${TOOLS}" "${AH}/amplifier-bundle/tools"
run_s3() { # run_s3 <fixture> [extra env...] — sets S3_OUT (last line), S3_STATUS, S3_SIGNAL
  local fx="$1"; shift
  S3_OUT="$(cd "${WORK}" && env GH_CI_FIX="$fx" GH_CI_ROUTE="${ROUTE}" GH_HEAD="$HEAD" GH_BASE=main \
    GH_CALLS="${WORK}/s3-calls" PR_NUMBER=42 REPO_PATH="${WORK}" AMPLIHACK_HOME="${AH}" \
    PATH="${STUB_BIN}:${PATH}" env "$@" bash -c "$S3" 2>"${WORK}/s3-err" </dev/null | tail -n1)"
  S3_STATUS="$(printf '%s' "$S3_OUT" | jq -r .ci_status 2>/dev/null)"
  S3_SIGNAL="$(printf '%s' "$S3_OUT" | jq -r .ci_signal 2>/dev/null)"
}
s3_explain() { printf 'out=%s\n%s\n' "$S3_OUT" "$(cat "${WORK}/s3-err")" | sed 's/^/      /' >&2; }
FX="$(new_fx)"
run_s3 "$FX"
if [ "$S3_STATUS" = GREEN ] && grep -qF "commits/${HEAD}/check-runs" "${WORK}/s3-calls"; then
  pass "EC-AC8-green" "step-03 keeps GREEN when the helper says GREEN (and the helper ran)"
else fail "EC-AC8-green" "step-03 did not keep GREEN after a GREEN helper"; s3_explain; fi
FX="$(new_fx)"; ec_drop_job "$FX" 1001 "Install Smoke Test"
run_s3 "$FX"
if [ "$S3_STATUS" = MISSING ] && printf '%s' "$S3_SIGNAL" | grep -qF 'MISSING:missing=' \
   && printf '%s' "$S3_SIGNAL" | grep -qF 'Install Smoke Test'; then
  pass "EC-AC8-missing" "step-03 lowers a green rollup to MISSING and names the missing check in ci_signal"
else fail "EC-AC8-missing" "step-03 did not lower GREEN to MISSING"; s3_explain; fi
check "EC-AC8-signal-300" "ci_signal is at most 300 characters" test "${#S3_SIGNAL}" -le 300
FX="$(new_fx)"
for i in $(seq 1 30); do ec_required "$FX" "Required check number ${i} with a long name"; done
run_s3 "$FX"
check "EC-AC8-signal-cut" "a long missing list is cut to 300 characters in ci_signal" \
  bash -c '[ "$1" = MISSING ] && [ "${#2}" -le 300 ]' _ "$S3_STATUS" "$S3_SIGNAL"
FX="$(new_fx)"
run_s3 "$FX" -u AMPLIHACK_HOME
if [ "$S3_STATUS" = UNREADABLE ] && [ "$S3_SIGNAL" = expected-checks-unreadable ]; then
  pass "EC-AC8b-no-home" "step-03 with AMPLIHACK_HOME unset gives UNREADABLE / expected-checks-unreadable"
else fail "EC-AC8b-no-home" "step-03 with AMPLIHACK_HOME unset did not give UNREADABLE"; s3_explain; fi
run_s3 "$FX" AMPLIHACK_HOME="${WORK}/no-such-home"
if [ "$S3_STATUS" = UNREADABLE ] && [ "$S3_SIGNAL" = expected-checks-unreadable ]; then
  pass "EC-AC8b-no-helper" "step-03 with the helper missing gives UNREADABLE"
else fail "EC-AC8b-no-helper" "step-03 with the helper missing did not give UNREADABLE"; s3_explain; fi
# A helper planted in the PR checkout is never run.
PLANT="${WORK}/plant"; mkdir -p "${PLANT}/amplifier-bundle/tools"
printf '#!/bin/bash\necho %s\n' "'{\"ci_status\":\"GREEN\",\"missing\":[],\"blockers\":0}'" > "${PLANT}/amplifier-bundle/tools/autodrive_expected_checks.sh"
chmod +x "${PLANT}/amplifier-bundle/tools/autodrive_expected_checks.sh"
FX="$(new_fx)"; ec_drop_job "$FX" 1001 "Install Smoke Test"
run_s3 "$FX" REPO_PATH="${PLANT}" AMPLIHACK_HOME="${WORK}/no-such-home"
check "EC-AC8b-no-planted" "a helper planted under REPO_PATH is never run" test "$S3_STATUS" = UNREADABLE
FX="$(new_fx)"
run_s3 "$FX" GH_BASE=""
check "EC-AC8b-no-base" "step-03 with an unreadable base gives UNREADABLE" test "$S3_STATUS" = UNREADABLE
FX="$(new_fx)"
run_s3 "$FX" GH_HEAD='$(touch '"${WORK}"'/pwned3)'
check "EC-AC8b-bad-head" "a hostile head value is not executed and does not pass" \
  bash -c '[ ! -e "$1" ] && [ "$2" != GREEN ]' _ "${WORK}/pwned3" "$S3_STATUS"
FX="$(new_fx)"
run_s3 "$FX" GH_PR_CHECKS='[{"name":"Test","state":"FAILURE","bucket":"fail"}]'
check "EC-AC8-red-unchanged" "a RED rollup stays RED (the helper only lowers GREEN)" test "$S3_STATUS" = RED

# The light drive's merge loop runs the same gate (D17).
check "EC-D17-light-uses-merge-loop" "auto-drive-light runs autodrive-merge-loop" \
  grep -qE 'recipe: "autodrive-merge-loop"' "${RECIPES}/auto-drive-light.yaml"
check "EC-D17-merge-loop-runs-gate" "autodrive-merge-loop runs the gate from AMPLIHACK_HOME" \
  grep -qF 'GATE="${AMPLIHACK_HOME}/amplifier-bundle/tools/autodrive_merge_gate.sh"' "${RECIPES}/autodrive-merge-loop.yaml"
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
