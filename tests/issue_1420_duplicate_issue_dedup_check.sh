#!/usr/bin/env bash
# Issue #1420 — a relaunched run must find the tracking issue it already opened,
# instead of opening a second one and taking a second branch and a second PR
# with it.
#
# The incident: a `default-workflow` run through `smart-orchestrator` died on a
# Claude usage limit. The relaunch re-ran `workflow-prep` from step 1 and made
# issue #112 for the same task 27 minutes after issue #110, with a
# byte-identical title, then worktree, branch and PR #118 to match.
#
# `workflow-prep.yaml` HAD a de-duplication guard. It could not fire, and the
# reason is reproducible with no usage limit anywhere in sight: it asked
# GitHub's SEARCH INDEX for `${ISSUE_TITLE:0:100}` restricted to `--state open`.
# For a title that starts with a filesystem path, that prefix is cut mid-token,
# carries an unbalanced `(`, and holds slashes the index tokenises differently
# from the stored title. It matched nothing, `2>/dev/null || echo ''` swallowed
# the empty answer, and control fell through to `gh issue create` in silence.
#
# These tests drive the REAL helper and the REAL step-03 command extracted from
# the recipe, against real git repositories and a fake `gh` whose search
# endpoint models exactly that index behaviour: a query full of paths and
# brackets finds nothing, a query of ordinary words finds the issue. Five
# properties are under test:
#
#   1. the end-to-end step-03 path reuses the existing issue and never calls
#      `gh issue create` — this is the assertion that fails on the old guard;
#   2. the match does not depend on the search index at all, and a deterministic
#      task key written into the issue body survives a title that is entirely
#      punctuation and paths;
#   3. a check that cannot be made warns and creates the issue anyway — no `gh`,
#      no `jq`, no network, a rate limit, a 403, a timeout or a missing helper
#      must never stop a run (#1268), and no token value is ever printed;
#   4. an unrelated issue is not a match, and an issue closed long ago does not
#      swallow a task legitimately repeated months later;
#   5. the outcome is logged in BOTH directions — the silent fall-through is
#      what made the original duplicate hard to attribute — and this test is
#      wired into CI, because a guard CI never runs is not a guard (#1404).
#
# Every token in this file is an obvious fake.

set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEDUP="$ROOT/amplifier-bundle/tools/workflow_issue_dedup_check.sh"
PREP="$ROOT/amplifier-bundle/recipes/workflow-prep.yaml"
CI="$ROOT/.github/workflows/ci.yml"

fails=0
pass() { printf '  ok    %s\n' "$1"; }
fail() { printf '  FAIL  %s\n' "$1"; fails=$((fails + 1)); }
need() { [ -f "$1" ] || { printf '  FAIL  missing %s\n' "$1"; exit 1; }; }

need "$PREP"; need "$CI"
[ -f "$DEDUP" ] || { echo "  FAIL  missing $DEDUP — the de-duplication helper (issue #1420)"; exit 1; }

WORK=$(mktemp -d "${TMPDIR:-/tmp}/issue1420.XXXXXX")
trap 'rm -rf "$WORK"' EXIT

export GIT_CONFIG_NOSYSTEM=1
export HOME="$WORK/fakehome"; mkdir -p "$HOME"
export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@e
export GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@e
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE
unset GH_TOKEN GITHUB_TOKEN GH_HOST AMPLIHACK_SKIP_ISSUE_DEDUP_CHECK
unset AMPLIHACK_ISSUE_DEDUP_NO_SEARCH_FALLBACK

NOW="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
OLD="$(date -u -d '-40 days' +%Y-%m-%dT%H:%M:%SZ 2>/dev/null || date -u -v-40d +%Y-%m-%dT%H:%M:%SZ)"

# The verbatim title from the report: a task description that begins with prose
# and carries a filesystem path and a bracket, which is what defeated the old
# guard's 100-character prefix.
TASK='Build development infrastructure for the Jamestown repository at ~/src/mistt/jamestown (GitHub Actions CI, pre-commit hooks, and a test harness)'

# --- a fake `gh` -----------------------------------------------------------
# FAKE_ISSUES is the repository's issue list as JSON. `--search` is served by a
# model of GitHub's search index: it answers only for a query made of ordinary
# words, and returns nothing for one carrying paths or brackets — which is the
# behaviour reproduced against the live repository in issue #1420.
BIN="$WORK/bin"; mkdir -p "$BIN"
cat > "$BIN/gh" <<'FAKEEOF'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "${GH_CALL_LOG:-/dev/null}"
case "${FAKE_GH_ERR:-}" in
  ratelimit) echo 'gh: API rate limit exceeded for token ghp_FAKEFAKEFAKEFAKEFAKEFAKE0001. (HTTP 403)' >&2; exit 1 ;;
  denied)    echo 'gh: Resource not accessible by integration (HTTP 403) token ghp_FAKEFAKEFAKEFAKEFAKEFAKE0001' >&2; exit 1 ;;
  network)   echo 'error connecting to api.github.com: dial tcp: lookup api.github.com: no such host' >&2; exit 1 ;;
  server)    echo 'gh: HTTP 502 Bad Gateway' >&2; exit 1 ;;
  weird)     echo 'gh: something nobody has ever seen before' >&2; exit 1 ;;
  garbage)   printf 'not json at all\n'; exit 0 ;;
  slow)      sleep 5; exit 0 ;;
esac

search=""; jqf=""; body=""; title=""; prev=""
for a in "$@"; do
  case "$prev" in
    --search) search="$a" ;;
    --jq) jqf="$a" ;;
    --body) body="$a" ;;
    --title) title="$a" ;;
  esac
  prev="$a"
done

emit() { # emit <json>
  if [ -n "$jqf" ]; then printf '%s' "$1" | jq -r "$jqf"; else printf '%s\n' "$1"; fi
}

case "$1 ${2:-}" in
  "issue list")
    if [ -n "${FAKE_ISSUES_FILE:-}" ]; then json="$(cat "$FAKE_ISSUES_FILE")"; else json="${FAKE_ISSUES:-[]}"; fi
    if [ -n "$search" ]; then
      # The index answers only ordinary words. Anything carrying a path
      # separator, a bracket or a tilde tokenises differently from the stored
      # title and comes back empty — the #1420 reproduction.
      case "$search" in
        *[!A-Za-z0-9\ :]*) json='[]' ;;
      esac
    fi
    emit "$json"
    ;;
  "issue create")
    printf '%s\n' "$title" > "${GH_CREATED_TITLE:-/dev/null}"
    printf '%s\n' "$body"  > "${GH_CREATED_BODY:-/dev/null}"
    echo "https://github.com/o/r/issues/112"
    ;;
  "issue view") emit '{}' ;;
  "label create") exit 0 ;;
  *) exit 0 ;;
esac
FAKEEOF
chmod +x "$BIN/gh"

mkrepo() { git init -q -b main "$1"; [ "$2" = "-" ] || git -C "$1" remote add origin "$2"; }
mkrepo "$WORK/gh"      "https://github.com/o/r.git"
mkrepo "$WORK/azdo"    "https://dev.azure.com/org/proj/_git/r"
mkrepo "$WORK/noremote" "-"
mkdir -p "$WORK/notagit"

# The issue the first run created, exactly as the report describes it: open,
# byte-identical title, and (being pre-fix) no task key in its body.
issue_110() { printf '%s' "$(jq -nc --arg t "$TASK" --arg now "$NOW" '
  [{number:110,url:"https://github.com/o/r/issues/110",title:$t,state:"OPEN",
    body:"## Task Description\nsomething\n",closedAt:null,createdAt:$now}]')"; }

# run <label-env...> -- <helper args...>  ; sets OUT (stdout), ERR (stderr), RC
run() {
  local envs=() extra=()
  while [ "$#" -gt 0 ]; do
    if [ "$1" = "--" ]; then shift; extra=("$@"); break; fi
    envs+=("$1"); shift
  done
  : > "$WORK/calls.log"
  env PATH="$BIN:$PATH" GH_CALL_LOG="$WORK/calls.log" "${envs[@]}" \
      bash "$DEDUP" "${extra[@]}" > "$WORK/out" 2> "$WORK/err"
  RC=$?
  OUT="$(cat "$WORK/out")"; ERR="$(cat "$WORK/err")"
}

echo "== 1. the end-to-end step-03 path reuses the issue it already opened =="

STEP="$WORK/step03.sh"
if python3 - "$PREP" "$STEP" <<'PY'
import sys, yaml
steps = yaml.safe_load(open(sys.argv[1]))["steps"]
step = next(s for s in steps if s["id"] == "step-03-create-issue")
open(sys.argv[2], "w").write(step["command"])
PY
then
  pass "workflow-prep's step-03-create-issue extracted from the recipe"
else
  fail "could not extract step-03-create-issue from workflow-prep.yaml"
fi

step03() { # step03 <issues-json> ; sets OUT / RC / created-title & body files
  : > "$WORK/calls.log"; : > "$WORK/created_title"; : > "$WORK/created_body"
  OUT="$(env PATH="$BIN:$PATH" GH_CALL_LOG="$WORK/calls.log" \
        GH_CREATED_TITLE="$WORK/created_title" GH_CREATED_BODY="$WORK/created_body" \
        FAKE_ISSUES="$1" AMPLIHACK_HOME="$ROOT" \
        REPO_PATH="$WORK/gh" TASK_DESCRIPTION="$TASK" FINAL_REQUIREMENTS="do the thing" \
        REMOTE_HOST_TYPE=github ISSUE_NUMBER="" \
        bash "$STEP" 2>"$WORK/err")"
  RC=$?
  ERR="$(cat "$WORK/err")"
}

if [ -s "$STEP" ]; then
  step03 "$(issue_110)"
  created="$(grep -c 'issue create' "$WORK/calls.log"; true)"
  if [ "$RC" -eq 0 ] && [ "$OUT" = "https://github.com/o/r/issues/110" ] && [ "$created" -eq 0 ]; then
    pass "a relaunch finds issue #110 and never calls 'gh issue create' (the #1420 duplicate)"
  else
    fail "step-03 duplicated the issue: rc=$RC out='${OUT:0:120}' issue-create-calls=$created"
  fi

  # And it says so. The silent fall-through is half the bug.
  grep -qi 'reusing it instead of opening a duplicate' <<<"$ERR" \
    && pass "the reuse is logged, naming the issue it reused" \
    || fail "reuse was silent: err=${ERR:0:300}"

  # A genuinely new task still creates an issue — and stamps it with the key
  # that makes the NEXT relaunch a one-step lookup.
  step03 '[]'
  if [ "$RC" -eq 0 ] && grep -q 'issues/112' <<<"$OUT"; then
    pass "a task with no existing issue still gets one created"
  else
    fail "a first run failed to create an issue: rc=$RC out='${OUT:0:200}' err=${ERR:0:200}"
  fi
  KEY="$(bash "$DEDUP" --print-key --task-description "$TASK")"
  if [ -n "$KEY" ] && grep -q "amplihack-task-key: ${KEY}" "$WORK/created_body"; then
    pass "the created issue body carries the deterministic task key ($KEY)"
  else
    fail "the created issue body has no task key: key='${KEY}' body=$(head -c 200 "$WORK/created_body")"
  fi
  grep -qi 'no open issue' <<<"$ERR" \
    && pass "the 'nothing matched' outcome is logged too, not swallowed" \
    || fail "a fall-through to creation was silent: err=${ERR:0:300}"

  # The key round-trips: an issue whose TITLE was rewritten is still found.
  RENAMED="$(jq -nc --arg k "$KEY" --arg now "$NOW" '
    [{number:110,url:"https://github.com/o/r/issues/110",title:"a completely different title",
      state:"OPEN",body:("x\n<!-- amplihack-task-key: " + $k + " -->\n"),
      closedAt:null,createdAt:$now}]')"
  step03 "$RENAMED"
  created="$(grep -c 'issue create' "$WORK/calls.log"; true)"
  [ "$OUT" = "https://github.com/o/r/issues/110" ] && [ "$created" -eq 0 ] \
    && pass "a renamed issue is still matched by its task key, not by prose" \
    || fail "the task key did not round-trip: out='${OUT:0:120}' creates=$created"

  # A missing helper is not a verdict: the run creates its issue and continues.
  # The bundle offered here holds every OTHER helper step-03 resolves, so only
  # the de-duplication helper is absent and only that branch is exercised.
  EMPTY="$WORK/empty"; mkdir -p "$EMPTY"
  PARTIAL="$WORK/partial/amplifier-bundle/tools"; mkdir -p "$PARTIAL"
  cp "$ROOT/amplifier-bundle/tools/workflow_issue_tracking.sh" "$PARTIAL/"
  : > "$WORK/calls.log"
  OUT="$(cd "$EMPTY" && env PATH="$BIN:$PATH" GH_CALL_LOG="$WORK/calls.log" \
        FAKE_ISSUES="$(issue_110)" AMPLIHACK_HOME="$WORK/partial" HOME="$EMPTY" \
        REPO_PATH="$WORK/gh" TASK_DESCRIPTION="$TASK" FINAL_REQUIREMENTS="r" \
        REMOTE_HOST_TYPE=github ISSUE_NUMBER="" bash "$STEP" 2>"$WORK/err")"; RC=$?
  if [ "$RC" -eq 0 ] && grep -q 'WARNING: issue de-duplication helper not found' "$WORK/err"; then
    pass "a missing de-duplication helper warns and continues rather than gating"
  else
    fail "missing helper mishandled: rc=$RC err=$(head -c 300 "$WORK/err")"
  fi
fi

echo "== 2. the match does not depend on the search index =="

# The reproduction from the report, made explicit: the OLD query shape finds
# nothing while the helper finds the issue.
OLDQ="$(env PATH="$BIN:$PATH" FAKE_ISSUES="$(issue_110)" \
        gh issue list --state open --search "${TASK:0:100}" --json url --jq '.[0].url // ""')"
[ -z "$OLDQ" ] \
  && pass "the old 100-character prefix query still returns nothing (the bug is real)" \
  || fail "the fixture no longer reproduces the failing query: '$OLDQ'"

run FAKE_ISSUES="$(issue_110)" AMPLIHACK_ISSUE_DEDUP_NO_SEARCH_FALLBACK=1 \
  -- --task-description "$TASK" --repo-path "$WORK/gh"
if [ "$RC" -eq 0 ] && [ "$OUT" = "https://github.com/o/r/issues/110" ]; then
  pass "the helper finds it with the search index disabled entirely"
else
  fail "the helper depends on the search index: rc=$RC out='${OUT:0:120}' err=${ERR:0:200}"
fi
grep -q -- '--search' "$WORK/calls.log" \
  && fail "the primary lookup used the search index" \
  || pass "the primary lookup is a direct listing, not a search"

# All states, not open only: an issue closed between the two runs is still ours.
CLOSED_NOW="$(jq -nc --arg t "$TASK" --arg now "$NOW" '
  [{number:110,url:"https://github.com/o/r/issues/110",title:$t,state:"CLOSED",
    body:"x",closedAt:$now,createdAt:$now}]')"
run FAKE_ISSUES="$CLOSED_NOW" -- --task-description "$TASK" --repo-path "$WORK/gh"
[ "$RC" -eq 0 ] && [ "$OUT" = "https://github.com/o/r/issues/110" ] \
  && pass "an issue closed between the two runs is found (the old guard searched open only)" \
  || fail "a recently closed issue was missed: rc=$RC out='${OUT:0:120}'"

# A repository of real size. Linux caps a SINGLE argv/environ entry at 128 KiB,
# so the issue list may never be handed to jq as an argument — it has to arrive
# on stdin. Passing it in argv makes the check die with "Argument list too long"
# and go permanently `unknown` on exactly the busy repositories where a
# duplicate issue costs the most. 600 issues with padded bodies is over 1 MB.
BIG="$WORK/big.json"
jq -n --arg t "$TASK" --arg now "$NOW" \
  '[range(600) | {number: (200 + .), url: ("https://github.com/o/r/issues/" + (200 + . | tostring)),
                  title: ("unrelated work item " + (. | tostring)), state: "OPEN",
                  body: ("padding " * 200), closedAt: null, createdAt: $now}]
   + [{number:110,url:"https://github.com/o/r/issues/110",title:$t,state:"OPEN",
       body:"## Task Description\nsomething\n",closedAt:null,createdAt:$now}]' > "$BIG"
BIG_BYTES="$(wc -c < "$BIG" | tr -d ' ')"
T0="$(date +%s)"
run FAKE_ISSUES_FILE="$BIG" AMPLIHACK_ISSUE_DEDUP_NO_SEARCH_FALLBACK=1 \
  -- --task-description "$TASK" --repo-path "$WORK/gh"
ELAPSED=$(( $(date +%s) - T0 ))
if [ "$RC" -eq 0 ] && [ "$OUT" = "https://github.com/o/r/issues/110" ]; then
  pass "a ${BIG_BYTES}-byte issue list is matched, not rejected as an over-long argument"
else
  fail "a large issue list broke the check: rc=$RC out='${OUT:0:120}' err=${ERR:0:250}"
fi
# And it is matched PROMPTLY. `${JSON//[[:space:]]/}` — the idiomatic bash
# "is this blank" test — is quadratic, and on a 300 KB listing it does not
# finish inside a minute. A de-duplication check that stalls every run costs
# more than the duplicate it prevents. The bound is deliberately loose; the
# failure it guards against is measured in minutes, not seconds.
if [ "$ELAPSED" -le 15 ]; then
  pass "the same list is processed in ${ELAPSED}s, not quadratic time"
else
  fail "processing ${BIG_BYTES} bytes took ${ELAPSED}s — a blank-test or filter went quadratic"
fi
# The wall clock is a backstop, not the guard: it depends on the runner. The
# guard is that the listing is never fed through bash pattern substitution at
# all, which is the specific construct that went quadratic.
DCODE="$WORK/dedup.code"; grep -vE '^[[:space:]]*#' "$DEDUP" > "$DCODE"
if grep -nE '\$\{(LIST|SEARCH)_JSON//' "$DCODE" > "$WORK/quad" ; then
  fail "the issue listing goes through a bash pattern substitution: $(cat "$WORK/quad")"
else
  pass "neither issue listing is passed through a bash pattern substitution"
fi
if grep -q -- '--argjson' "$DCODE"; then
  fail "the issue listing is handed to jq in argv, which dies past 128 KiB"
else
  pass "the issue listing reaches jq on stdin, never in argv"
fi

echo "== 3. a check that cannot be made warns and creates the issue anyway =="
continues() { # continues <label> <expected-substring>
  if [ "$RC" -eq 4 ] && [ -z "$OUT" ] && grep -q 'WARNING' <<<"$ERR" && grep -q "$2" <<<"$ERR"; then
    pass "$1 warns and continues (exit 4, no URL)"
  else
    fail "$1: rc=$RC out='${OUT:0:80}' err=${ERR:0:250}"
  fi
}
base=(FAKE_ISSUES="$(issue_110)")
run "${base[@]}" FAKE_GH_ERR=ratelimit -- --task-description "$TASK" --repo-path "$WORK/gh"
continues "a rate limit" "rate-limited"
run "${base[@]}" FAKE_GH_ERR=denied -- --task-description "$TASK" --repo-path "$WORK/gh"
continues "a 403 authorisation denial" "not-authorised"
run "${base[@]}" FAKE_GH_ERR=network -- --task-description "$TASK" --repo-path "$WORK/gh"
continues "a network failure" "network-or-server-error"
run "${base[@]}" FAKE_GH_ERR=server -- --task-description "$TASK" --repo-path "$WORK/gh"
continues "a 502 from the API" "network-or-server-error"
run "${base[@]}" FAKE_GH_ERR=weird -- --task-description "$TASK" --repo-path "$WORK/gh"
continues "an unrecognised API error" "unrecognised-api-error"
run "${base[@]}" FAKE_GH_ERR=garbage -- --task-description "$TASK" --repo-path "$WORK/gh"
continues "an unparseable answer" "issue-list-unreadable"
run "${base[@]}" FAKE_GH_ERR=slow AMPLIHACK_ISSUE_DEDUP_TIMEOUT=1 \
  -- --task-description "$TASK" --repo-path "$WORK/gh"
continues "an API call that times out" "timed-out"

# 403 is GitHub's answer to BOTH "slow down" and "you may not". Rate limiting is
# classified first, and neither can ever become a match.
run "${base[@]}" FAKE_GH_ERR=ratelimit -- --task-description "$TASK" --repo-path "$WORK/gh"
grep -q 'not-authorised' <<<"$ERR" \
  && fail "a rate limit was misread as an authorisation denial" \
  || pass "rate limiting is classified before a 403 is read as a denial"
for m in ratelimit denied; do
  run "${base[@]}" FAKE_GH_ERR="$m" -- --task-description "$TASK" --repo-path "$WORK/gh"
  grep -q 'ghp_FAKEFAKEFAKEFAKEFAKEFAKE0001' <<<"$OUT$ERR" \
    && fail "a token value reached the output ($m)" \
    || pass "no token value reaches the output ($m)"
done

skipped() { # skipped <label> <expected-substring>
  if [ "$RC" -eq 5 ] && [ -z "$OUT" ] && grep -q "$2" <<<"$ERR"; then
    pass "$1 is skipped (exit 5, no URL)"
  else
    fail "$1: rc=$RC out='${OUT:0:80}' err=${ERR:0:250}"
  fi
}
run "${base[@]}" -- --task-description "$TASK" --repo-path "$WORK/azdo"
skipped "an Azure DevOps remote" "non-github-remote"
[ -s "$WORK/calls.log" ] && fail "gh was called for a non-GitHub remote" \
  || pass "a non-GitHub remote is skipped without calling gh at all"
run "${base[@]}" -- --task-description "$TASK" --repo-path "$WORK/noremote"
skipped "a repository with no origin" "no-origin-remote"
run "${base[@]}" -- --task-description "$TASK" --repo-path "$WORK/notagit"
skipped "a non-repository directory" "not-a-git-repository"
run "${base[@]}" -- --task-description "   " --repo-path "$WORK/gh"
skipped "an empty task description" "empty-task-description"
run "${base[@]}" AMPLIHACK_SKIP_ISSUE_DEDUP_CHECK=1 -- --task-description "$TASK" --repo-path "$WORK/gh"
skipped "the documented escape hatch" "AMPLIHACK_SKIP_ISSUE_DEDUP_CHECK"

# `gh` and `jq` absent: a PATH holding only the coreutils the helper needs.
STUB="$WORK/stub"; mkdir -p "$STUB"
for c in git sed awk grep head cut tr mktemp rm timeout printf env bash cat date sha256sum shasum; do
  p="$(command -v "$c" 2>/dev/null)" && ln -sf "$p" "$STUB/$c"
done
env -i PATH="$STUB" HOME="$HOME" GIT_CONFIG_NOSYSTEM=1 bash "$DEDUP" \
  --task-description "$TASK" --repo-path "$WORK/gh" > "$WORK/out" 2> "$WORK/err"
RC=$?; OUT="$(cat "$WORK/out")"; ERR="$(cat "$WORK/err")"
continues "gh not installed" "gh-not-installed"
ln -sf "$BIN/gh" "$STUB/gh"
env -i PATH="$STUB" HOME="$HOME" GIT_CONFIG_NOSYSTEM=1 bash "$DEDUP" \
  --task-description "$TASK" --repo-path "$WORK/gh" > "$WORK/out" 2> "$WORK/err"
RC=$?; OUT="$(cat "$WORK/out")"; ERR="$(cat "$WORK/err")"
continues "jq not installed" "jq-not-installed"

echo "== 4. an unrelated issue is not a match =="
UNRELATED="$(jq -nc --arg now "$NOW" '
  [{number:9,url:"https://github.com/o/r/issues/9",title:"Fix the flaky release pipeline",
    state:"OPEN",body:"x",closedAt:null,createdAt:$now}]')"
run FAKE_ISSUES="$UNRELATED" -- --task-description "$TASK" --repo-path "$WORK/gh"
[ "$RC" -eq 3 ] && [ -z "$OUT" ] \
  && pass "an unrelated open issue is not folded into this task (exit 3)" \
  || fail "a false positive: rc=$RC out='${OUT:0:120}'"

CLOSED_OLD="$(jq -nc --arg t "$TASK" --arg old "$OLD" '
  [{number:110,url:"https://github.com/o/r/issues/110",title:$t,state:"CLOSED",
    body:"x",closedAt:$old,createdAt:$old}]')"
run FAKE_ISSUES="$CLOSED_OLD" -- --task-description "$TASK" --repo-path "$WORK/gh"
[ "$RC" -eq 3 ] && [ -z "$OUT" ] \
  && pass "an issue closed 40 days ago does not swallow a task repeated later" \
  || fail "an ancient closed issue was reused: rc=$RC out='${OUT:0:120}'"

echo "== 5. the recipe is wired to it, and CI runs this test =="
if [ -s "$STEP" ]; then
  for tok in AMPLIHACK_HOME .copilot .amplihack workflow_issue_dedup_check.sh; do
    grep -q -- "$tok" "$STEP" || fail "step-03 must resolve the helper through the standard cascade (missing $tok)"
  done
  pass "step-03 resolves the helper through the AMPLIHACK_HOME / REPO_PATH / cwd / ~/.copilot / ~/.amplihack cascade"
  grep -q 'amplihack-task-key' "$STEP" \
    && pass "step-03 stamps the task key into the issue body it creates" \
    || fail "step-03 creates issues with no task key — the next relaunch has nothing to match"
  # Comments describe the old query on purpose; only executable lines count.
  CODE="$WORK/step03.code"; grep -vE '^[[:space:]]*#' "$STEP" > "$CODE"
  grep -q 'state open --search' "$CODE" \
    && fail "step-03 still asks the search index for a raw title prefix (the #1420 query)" \
    || pass "the raw '--state open --search <title prefix>' query is gone"
  grep -q 'SEARCH_Q=' "$CODE" \
    && fail "step-03 still builds a lookup from a fixed-length title prefix" \
    || pass "the 100-character mid-token title prefix is no longer a lookup key"
fi
grep -q 'issue_1420_duplicate_issue_dedup_check.sh' "$CI" \
  && pass "CI runs this test (a guard CI never runs is not a guard — issue #1404)" \
  || fail ".github/workflows/ci.yml does not run tests/issue_1420_duplicate_issue_dedup_check.sh"

echo
if [ "$fails" -eq 0 ]; then
  echo "issue #1420 duplicate-issue de-duplication: all checks passed"
else
  echo "issue #1420 duplicate-issue de-duplication: $fails check(s) failed"
fi
exit $((fails > 0))
