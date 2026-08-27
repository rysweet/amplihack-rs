#!/usr/bin/env bash
# Issue #1361 — the workflow must not open a second pull request for an issue it
# is already working on, and a planning artifact must never carry `Closes #N`.
#
# The incident: two PRs for the same issue, ~30 and ~63 minutes apart, from two
# branches. The second merged; the first was never closed and drifted into a
# conflict that looks like real unmerged work but is byte-identical to what
# shipped. For #1277 it happened five times in 103 minutes. The only
# de-duplication the workflow had was `workflow_pr_scope.sh`, whose key is
# (headRefName, baseRefName) — exactly the key a second run defeats by deriving
# a new branch.
#
# These tests drive the real helpers and the real recipe steps against real git
# repositories and a fake `gh`. Four properties are under test:
#
#   1. an issue already claimed by an open or recently-merged PR stops the run,
#      and the refusal names the PR and how to continue it instead;
#   2. a mere mention is NOT a claim, and our own branch is not a competitor;
#   3. a check that cannot be made warns and continues — no `gh`, no `jq`, no
#      network, a rate limit, a 403 or a missing helper must never stop a run
#      (#1268), and no token value is ever printed;
#   4. `Closes #N` is earned by the diff: a change that only writes a planning
#      document gets `Refs`, and both emitters (the step-15 commit message and
#      the PR body in workflow_publish_pr.sh) go through that decision.
#
# Every token in this file is an obvious fake.

set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CLAIM="$ROOT/amplifier-bundle/tools/workflow_issue_claim_check.sh"
IREF="$ROOT/amplifier-bundle/tools/workflow_issue_reference.sh"
PREP="$ROOT/amplifier-bundle/recipes/workflow-prep.yaml"
PUBLISH_RECIPE="$ROOT/amplifier-bundle/recipes/workflow-publish.yaml"
PUBLISH_TOOL="$ROOT/amplifier-bundle/tools/workflow_publish_pr.sh"

fails=0
pass() { printf '  ok    %s\n' "$1"; }
fail() { printf '  FAIL  %s\n' "$1"; fails=$((fails + 1)); }
need() { [ -f "$1" ] || { echo "missing $1"; exit 1; }; }

need "$CLAIM"; need "$IREF"; need "$PREP"; need "$PUBLISH_RECIPE"; need "$PUBLISH_TOOL"

WORK=$(mktemp -d "${TMPDIR:-/tmp}/issue1361.XXXXXX")
trap 'rm -rf "$WORK"' EXIT

export GIT_CONFIG_NOSYSTEM=1
export HOME="$WORK/fakehome"; mkdir -p "$HOME"
export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@e
export GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@e
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE
unset GH_TOKEN GITHUB_TOKEN GH_HOST AMPLIHACK_SKIP_ISSUE_CLAIM_CHECK

NOW="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
OLD="$(date -u -d '-40 days' +%Y-%m-%dT%H:%M:%SZ 2>/dev/null || date -u -v-40d +%Y-%m-%dT%H:%M:%SZ)"

# --- a fake `gh` -----------------------------------------------------------
# FAKE_GH selects the scenario; the `--state` argument selects which half of it
# is served, so open and merged lookups can differ within one scenario.
BIN="$WORK/bin"; mkdir -p "$BIN"
cat > "$BIN/gh" <<FAKEEOF
#!/usr/bin/env bash
printf '%s\n' "\$*" >> "\${GH_CALL_LOG:-/dev/null}"
state=""
prev=""
for a in "\$@"; do [ "\$prev" = "--state" ] && state="\$a"; prev="\$a"; done
# A token-shaped string is present in every error path so redaction is exercised.
case "\${FAKE_GH:-none}" in
  none)        printf '[]\n' ;;
  closing_ref) [ "\$state" = "open" ] && printf '%s\n' '[{"number":1128,"url":"https://github.com/o/r/pull/1128","title":"Update crates with 3 changed files","state":"OPEN","mergedAt":null,"headRefName":"fix/other-branch","closingIssuesReferences":[{"number":1084}]}]' || printf '[]\n' ;;
  title_paren) [ "\$state" = "open" ] && printf '%s\n' '[{"number":900,"url":"https://github.com/o/r/pull/900","title":"Update workflow recipes (#1084)","state":"OPEN","mergedAt":null,"headRefName":"feat/unrelated-slug","closingIssuesReferences":[]}]' || printf '[]\n' ;;
  branch_name) [ "\$state" = "open" ] && printf '%s\n' '[{"number":901,"url":"https://github.com/o/r/pull/901","title":"docs: add Step 5d security exploration plan","state":"OPEN","mergedAt":null,"headRefName":"docs/issue-1084-round2-step5d-clean","closingIssuesReferences":[]}]' || printf '[]\n' ;;
  own_branch)  [ "\$state" = "open" ] && printf '%s\n' '[{"number":902,"url":"https://github.com/o/r/pull/902","title":"Update x (#1084)","state":"OPEN","mergedAt":null,"headRefName":"fix/1084-mine","closingIssuesReferences":[{"number":1084}]}]' || printf '[]\n' ;;
  mention)     [ "\$state" = "open" ] && printf '%s\n' '[{"number":903,"url":"https://github.com/o/r/pull/903","title":"Update something similar to #1084 elsewhere","state":"OPEN","mergedAt":null,"headRefName":"fix/timeouts","closingIssuesReferences":[{"number":77}]}]' || printf '[]\n' ;;
  merged_new)  [ "\$state" = "merged" ] && printf '%s\n' '[{"number":1132,"url":"https://github.com/o/r/pull/1132","title":"Update crates (#1084)","state":"MERGED","mergedAt":"$NOW","headRefName":"fix/1084-winner","closingIssuesReferences":[{"number":1084}]}]' || printf '[]\n' ;;
  merged_old)  [ "\$state" = "merged" ] && printf '%s\n' '[{"number":40,"url":"https://github.com/o/r/pull/40","title":"Update crates (#1084)","state":"MERGED","mergedAt":"$OLD","headRefName":"fix/1084-ancient","closingIssuesReferences":[{"number":1084}]}]' || printf '[]\n' ;;
  ratelimit)   echo 'gh: API rate limit exceeded for token ghp_FAKEFAKEFAKEFAKEFAKEFAKE0001. (HTTP 403)' >&2; exit 1 ;;
  denied)      echo 'gh: Resource not accessible by integration (HTTP 403) token ghp_FAKEFAKEFAKEFAKEFAKEFAKE0001' >&2; exit 1 ;;
  network)     echo 'error connecting to api.github.com: dial tcp: lookup api.github.com: no such host' >&2; exit 1 ;;
  server)      echo 'gh: HTTP 502 Bad Gateway' >&2; exit 1 ;;
  weird)       echo 'gh: something nobody has ever seen before' >&2; exit 1 ;;
  garbage)     printf 'not json at all\n' ;;
  slow)        sleep 5 ;;
esac
FAKEEOF
chmod +x "$BIN/gh"

mkrepo() { git init -q -b main "$1"; [ "$2" = "-" ] || git -C "$1" remote add origin "$2"; }
mkrepo "$WORK/gh"    "https://github.com/o/r.git"
mkrepo "$WORK/azdo"  "https://dev.azure.com/org/proj/_git/r"
mkrepo "$WORK/noremote" "-"
mkdir -p "$WORK/notagit"

# run <repo> [env...] -- [extra helper args] ; sets OUT / RC
run() {
  local repo="$1"; shift
  local envs=() extra=()
  while [ "$#" -gt 0 ]; do
    if [ "$1" = "--" ]; then shift; extra=("$@"); break; fi
    envs+=("$1"); shift
  done
  GH_CALL_LOG="$WORK/calls.log"; : > "$GH_CALL_LOG"
  OUT="$(env PATH="$BIN:$PATH" GH_CALL_LOG="$GH_CALL_LOG" "${envs[@]}" \
        bash "$CLAIM" --issue 1084 --repo-path "$repo" "${extra[@]}" 2>&1)"
  RC=$?
}

echo "== 1. an issue already claimed by a pull request stops the run =="
claimed() { # claimed <label> <expected-pr-number>
  local label="$1" num="$2" missing=""
  grep -q "#${num}" <<<"$OUT" || missing="$missing pr-number"
  grep -q '#1084' <<<"$OUT" || missing="$missing issue"
  grep -q 'existing_branch=' <<<"$OUT" || missing="$missing how-to-continue"
  if [ "$RC" -eq 1 ] && [ -z "$missing" ]; then
    pass "$label refuses (exit 1) naming the claiming PR and how to continue it"
  else
    fail "$label: rc=$RC missing:${missing:-none} out=${OUT:0:400}"
  fi
}
run "$WORK/gh" FAKE_GH=closing_ref; claimed "an open PR whose body closes the issue" 1128
run "$WORK/gh" FAKE_GH=title_paren; claimed "an open PR whose title carries (#N)" 900
run "$WORK/gh" FAKE_GH=branch_name; claimed "an open PR on an issue-N branch" 901
run "$WORK/gh" FAKE_GH=merged_new;  claimed "a PR merged inside the recent window" 1132

# This is the case the branch-scoped lookup could never see: no PR exists for
# OUR head, and a duplicate would have been opened.
run "$WORK/gh" FAKE_GH=closing_ref -- --head "fix/1084-a-brand-new-branch"
[ "$RC" -eq 1 ] \
  && pass "a competitor on a DIFFERENT branch is still a claim (the #1361 hole)" \
  || fail "a different-branch competitor was not detected: rc=$RC out=${OUT:0:300}"

echo "== 2. a mention is not a claim, and our own PR is not a competitor =="
run "$WORK/gh" FAKE_GH=mention
[ "$RC" -eq 0 ] && grep -q 'issue_claim_check: ok' <<<"$OUT" \
  && pass "a PR that merely mentions #N in its title does not stop a run" \
  || fail "a passing mention was treated as a claim: rc=$RC out=${OUT:0:300}"
run "$WORK/gh" FAKE_GH=own_branch -- --head "fix/1084-mine"
[ "$RC" -eq 0 ] \
  && pass "the run's own branch is excluded via --head" \
  || fail "our own PR was treated as a duplicate: rc=$RC out=${OUT:0:300}"
run "$WORK/gh" FAKE_GH=merged_old
[ "$RC" -eq 0 ] \
  && pass "a PR merged long ago does not block later work on a reopened issue" \
  || fail "an ancient merge blocked the run: rc=$RC out=${OUT:0:300}"
run "$WORK/gh" FAKE_GH=none
[ "$RC" -eq 0 ] && grep -q 'issue_claim_check: ok' <<<"$OUT" \
  && pass "an unclaimed issue passes" \
  || fail "an unclaimed issue was refused: rc=$RC out=${OUT:0:300}"

echo "== 3. a check that cannot be made warns and continues =="
continues() { # continues <label> <expected-substring>
  if [ "$RC" -eq 0 ] && grep -q 'WARNING' <<<"$OUT" && grep -q "$2" <<<"$OUT"; then
    pass "$1 warns and continues (exit 0)"
  else
    fail "$1: rc=$RC out=${OUT:0:300}"
  fi
}
run "$WORK/gh" FAKE_GH=ratelimit; continues "a rate limit" "rate-limited"
run "$WORK/gh" FAKE_GH=denied;    continues "a 403 authorisation denial" "not-authorised"
run "$WORK/gh" FAKE_GH=network;   continues "a network failure" "network-or-server-error"
run "$WORK/gh" FAKE_GH=server;    continues "a 502 from the API" "network-or-server-error"
run "$WORK/gh" FAKE_GH=weird;     continues "an unrecognised API error" "unrecognised-api-error"
run "$WORK/gh" FAKE_GH=garbage;   continues "an unparseable answer" "claim-filter-unreadable"
run "$WORK/gh" FAKE_GH=slow AMPLIHACK_CLAIM_CHECK_TIMEOUT=1; continues "an API call that times out" "timed-out"

# 403 is GitHub's answer to BOTH "slow down" and "you may not"; rate limiting is
# classified first, and neither can ever become a refusal.
run "$WORK/gh" FAKE_GH=ratelimit
grep -q 'not-authorised' <<<"$OUT" \
  && fail "a rate limit was misread as an authorisation denial" \
  || pass "rate limiting is classified before a 403 is read as a denial"
for m in ratelimit denied; do
  run "$WORK/gh" FAKE_GH="$m"
  grep -q 'ghp_FAKEFAKEFAKEFAKEFAKEFAKE0001' <<<"$OUT" \
    && fail "a token value reached the output ($m)" \
    || pass "no token value reaches the output ($m)"
done

run "$WORK/azdo" FAKE_GH=closing_ref
if [ "$RC" -eq 0 ] && grep -q 'non-github-remote' <<<"$OUT" && [ ! -s "$WORK/calls.log" ]; then
  pass "an Azure DevOps remote is skipped without calling gh at all"
else
  fail "azdo remote mishandled: rc=$RC out=${OUT:0:200}"
fi
run "$WORK/noremote" FAKE_GH=closing_ref
[ "$RC" -eq 0 ] && grep -q 'no-origin-remote' <<<"$OUT" \
  && pass "a repository with no origin is skipped" || fail "no-origin mishandled: rc=$RC"
run "$WORK/notagit" FAKE_GH=closing_ref
[ "$RC" -eq 0 ] && grep -q 'not-a-git-repository' <<<"$OUT" \
  && pass "a non-repository directory is skipped" || fail "non-repo mishandled: rc=$RC"
OUT="$(env PATH="$BIN:$PATH" FAKE_GH=closing_ref bash "$CLAIM" --issue "local-issue-7" --repo-path "$WORK/gh" 2>&1)"; RC=$?
[ "$RC" -eq 0 ] && grep -q 'issue-reference-not-numeric' <<<"$OUT" \
  && pass "a local-tracking reference has no provider to ask and is skipped" \
  || fail "local tracking reference mishandled: rc=$RC out=${OUT:0:200}"
run "$WORK/gh" FAKE_GH=closing_ref AMPLIHACK_SKIP_ISSUE_CLAIM_CHECK=1
[ "$RC" -eq 0 ] && grep -q 'skipped' <<<"$OUT" \
  && pass "the documented escape hatch bypasses the check" || fail "escape hatch failed: rc=$RC"

# `gh` and `jq` absent: a PATH holding only the coreutils the helper needs.
STUB="$WORK/stub"; mkdir -p "$STUB"
for c in git sed awk grep head tr mktemp rm timeout printf env bash cat date basename; do
  p="$(command -v "$c" 2>/dev/null)" && ln -sf "$p" "$STUB/$c"
done
OUT="$(env -i PATH="$STUB" HOME="$HOME" GIT_CONFIG_NOSYSTEM=1 bash "$CLAIM" --issue 1084 --repo-path "$WORK/gh" 2>&1)"; RC=$?
continues "gh not installed" "gh-not-installed"
ln -sf "$BIN/gh" "$STUB/gh"
OUT="$(env -i PATH="$STUB" HOME="$HOME" GIT_CONFIG_NOSYSTEM=1 FAKE_GH=closing_ref bash "$CLAIM" --issue 1084 --repo-path "$WORK/gh" 2>&1)"; RC=$?
continues "jq not installed" "jq-not-installed"

echo "== 4. the closing keyword is earned by the diff =="
kw() { printf '%s\n' "$1" | bash "$IREF" --keyword; }
[ "$(kw 'docs/issue-1277-step-5d-exploration-plan.md')" = "Refs" ] \
  && pass "the exact PR #1283 diff (one planning document) yields Refs" \
  || fail "a planning-only diff still yields $(kw 'docs/issue-1277-step-5d-exploration-plan.md')"
[ "$(printf 'docs/issue-1277-step-5d-security-exploration-plan.json\nCargo.toml\nCargo.lock\npackage.json\n' | bash "$IREF" --keyword)" = "Refs" ] \
  && pass "a planning document plus the publish phase's own version bump yields Refs" \
  || fail "a version bump made a planning-only diff look substantive"
[ "$(kw 'crates/amplihack-cli/src/main.rs')" = "Closes" ] \
  && pass "a real code change yields Closes" || fail "a code change did not yield Closes"
[ "$(kw 'docs/howto/configure-scoped-workflow-closure.md')" = "Closes" ] \
  && pass "an ordinary docs fix still yields Closes (docs are not planning)" \
  || fail "an ordinary docs change was demoted to Refs"
[ "$(kw '')" = "Refs" ] && pass "an empty diff yields Refs" || fail "an empty diff did not yield Refs"

echo "== 5. the recipes and the publish helper are wired to all of it =="
STEP="$WORK/step03c.sh"
if python3 - "$PREP" "$STEP" <<'PY'
import sys, yaml
recipe, out = sys.argv[1], sys.argv[2]
steps = yaml.safe_load(open(recipe))["steps"]
ids = [s["id"] for s in steps]
assert "step-03c-issue-claim-check" in ids, ids
assert ids.index("step-03c-issue-claim-check") > ids.index("step-03b-extract-issue-number")
step = next(s for s in steps if s["id"] == "step-03c-issue-claim-check")
open(out, "w").write(step["command"])
PY
then
  pass "workflow-prep declares step-03c-issue-claim-check once the issue number is known"
else
  fail "workflow-prep does not declare the claim check after step-03b"
fi

if [ -s "$STEP" ]; then
  for tok in AMPLIHACK_HOME .copilot .amplihack workflow_issue_claim_check.sh; do
    grep -q -- "$tok" "$STEP" || fail "step-03c must resolve the helper through the standard cascade (missing $tok)"
  done
  pass "step-03c resolves the helper through the AMPLIHACK_HOME / REPO_PATH / cwd / ~/.copilot / ~/.amplihack cascade"

  OUT="$(cd "$WORK" && env PATH="$BIN:$PATH" FAKE_GH=closing_ref AMPLIHACK_HOME="$ROOT" \
        REPO_PATH="$WORK/gh" ISSUE_NUMBER=1084 bash "$STEP" 2>&1)"; RC=$?
  [ "$RC" -eq 1 ] && grep -q 'already working on issue #1084' <<<"$OUT" \
    && pass "the recipe step stops a run whose issue is already claimed" \
    || fail "recipe step did not refuse: rc=$RC out=${OUT:0:300}"

  OUT="$(cd "$WORK" && env PATH="$BIN:$PATH" FAKE_GH=none AMPLIHACK_HOME="$ROOT" \
        REPO_PATH="$WORK/gh" ISSUE_NUMBER=1084 bash "$STEP" 2>&1)"; RC=$?
  [ "$RC" -eq 0 ] && grep -q 'issue_claim_check: ok' <<<"$OUT" \
    && pass "the recipe step passes an unclaimed issue" \
    || fail "recipe step refused an unclaimed issue: rc=$RC out=${OUT:0:300}"

  OUT="$(cd "$WORK" && env PATH="$BIN:$PATH" FAKE_GH=closing_ref AMPLIHACK_HOME="$ROOT" \
        REPO_PATH="$WORK/gh" ISSUE_NUMBER=1084 EXISTING_BRANCH="fix/1084-resume" bash "$STEP" 2>&1)"; RC=$?
  [ "$RC" -eq 0 ] && grep -q 'explicit-existing-branch-or-pr' <<<"$OUT" \
    && pass "a run that explicitly targets an existing branch is not its own duplicate" \
    || fail "resuming an existing branch was refused: rc=$RC out=${OUT:0:300}"

  EMPTY="$WORK/empty"; mkdir -p "$EMPTY"
  OUT="$(cd "$EMPTY" && env PATH="$BIN:$PATH" FAKE_GH=closing_ref HOME="$HOME" \
        AMPLIHACK_HOME="$EMPTY" REPO_PATH="$EMPTY" ISSUE_NUMBER=1084 bash "$STEP" 2>&1)"; RC=$?
  [ "$RC" -eq 0 ] && grep -q 'WARNING' <<<"$OUT" && grep -q 'helper-not-found' <<<"$OUT" \
    && pass "a missing helper warns and continues rather than gating the run" \
    || fail "missing helper gated the run: rc=$RC out=${OUT:0:300}"
fi

# The claim check must be consulted BEFORE the create call, not after it.
claim_pos=$(grep -n 'workflow_issue_claim_check.sh' "$PUBLISH_TOOL" | tail -1 | cut -d: -f1)
create_pos=$(grep -n 'PR_URL_RESULT="\$(gh_pr_create_with_retry)"' "$PUBLISH_TOOL" | head -1 | cut -d: -f1)
if [ -n "$claim_pos" ] && [ -n "$create_pos" ] && [ "$claim_pos" -lt "$create_pos" ]; then
  pass "workflow_publish_pr.sh consults the claim check before it creates a PR"
else
  fail "workflow_publish_pr.sh does not claim-check before creating a PR (claim=$claim_pos create=$create_pos)"
fi
grep -q 'workflow_issue_reference.sh' "$PUBLISH_TOOL" \
  && ! grep -q 'ISSUE_LINK="Closes #' "$PUBLISH_TOOL" \
  && pass "the PR body's closing keyword comes from the diff, not a hardcoded Closes" \
  || fail "workflow_publish_pr.sh still hardcodes 'Closes #N' in the PR body"

STEP15="$WORK/step15.sh"
python3 - "$PUBLISH_RECIPE" "$STEP15" <<'PY'
import sys, yaml
steps = yaml.safe_load(open(sys.argv[1]))["steps"]
step = next(s for s in steps if s["id"] == "step-15-commit-push")
open(sys.argv[2], "w").write(step["command"])
PY
grep -q 'workflow_issue_reference.sh' "$STEP15" && ! grep -q 'ISSUE_REF="Closes #' "$STEP15" \
  && pass "the step-15 commit message's closing keyword comes from the diff too" \
  || fail "step-15 still hardcodes 'Closes #N' in the commit message"

echo
if [ "$fails" -eq 0 ]; then
  echo "issue #1361 duplicate-PR claim check: all checks passed"
else
  echo "issue #1361 duplicate-PR claim check: $fails check(s) failed"
fi
exit $((fails > 0))
