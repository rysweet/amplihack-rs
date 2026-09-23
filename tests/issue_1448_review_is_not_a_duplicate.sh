#!/usr/bin/env bash
# Issue #1448 — working ON an issue's pull request is not racing it.
#
# `step-03c-issue-claim-check` (#1361) refuses to run whenever the issue already
# has a pull request. That is right for a second implementation attempt, which
# is the failure it was built to stop. It was also wrong for every legitimate
# follow-up run against the same issue: reviewing the PR, acting on review
# findings, fixing its CI, or resuming a run a usage limit killed after the PR
# was opened (#1390). Two review runs died at this step having done no work and
# the reporter abandoned `default-workflow` entirely.
#
# The evidence that separates the two cases was already in hand and unread: the
# branch checked out in the working directory. A second implementation attempt
# derives a NEW branch — that is the entire mechanism of #1361 — so it is never
# standing on the claiming PR's head. A review, a review-fix and a resume all
# are.
#
# Six properties are under test, and the first two are the whole point — they
# must hold in BOTH directions or the fix has either not worked or has undone
# #1361:
#
#   1. on the claiming pull request's head branch, the run PROCEEDS;
#   2. on any other branch, the claim still REFUSES the run — including a
#      same-named head in a FORK, where equal branch names are not one branch;
#   3. the allowance is per-claim, not a blanket bypass: a second competitor on
#      a different branch still refuses even while we stand on our own PR;
#   4. `-c allow_existing_pr=true` states the intent when the working directory
#      cannot be on that branch — and `=false` does not silently enable it;
#   5. the refusal now says how to proceed legitimately;
#   6. none of this touches the three-way split: a check that cannot be made
#      still warns and continues, and prints no token value (#1268).
#
# Every token in this file is an obvious fake.

# `+e` is explicit, not inherited-by-accident: almost every assertion below runs
# a helper that is EXPECTED to exit 1, so errexit would end the run at the first
# refusal it proved. CI invokes this as `bash tests/...` inside an `-eo pipefail`
# wrapper, which already gives it a fresh shell; saying `+e` here means the file
# also behaves when it is run under `bash -e` directly.
set +e -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CLAIM="$ROOT/amplifier-bundle/tools/workflow_issue_claim_check.sh"
PREP="$ROOT/amplifier-bundle/recipes/workflow-prep.yaml"

fails=0
pass() { printf '  ok    %s\n' "$1"; }
fail() { printf '  FAIL  %s\n' "$1"; fails=$((fails + 1)); }
need() { [ -f "$1" ] || { echo "missing $1"; exit 1; }; }

need "$CLAIM"; need "$PREP"

WORK=$(mktemp -d "${TMPDIR:-/tmp}/issue1448.XXXXXX")
trap 'rm -rf "$WORK"' EXIT

export GIT_CONFIG_NOSYSTEM=1
export HOME="$WORK/fakehome"; mkdir -p "$HOME"
export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@e
export GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@e
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE
unset GH_TOKEN GITHUB_TOKEN GH_HOST
unset AMPLIHACK_SKIP_ISSUE_CLAIM_CHECK ALLOW_EXISTING_PR AMPLIHACK_ALLOW_EXISTING_PR

# The reported incident: issue #142, claimed by PR #173 whose head is
# `fix/142-band-edge-previous-slice`, reviewed from a worktree already on that
# branch.
ISSUE=142
PR_HEAD="fix/142-band-edge-previous-slice"

# --- a fake `gh` -----------------------------------------------------------
BIN="$WORK/bin"; mkdir -p "$BIN"
cat > "$BIN/gh" <<FAKEEOF
#!/usr/bin/env bash
printf '%s\n' "\$*" >> "\${GH_CALL_LOG:-/dev/null}"
state=""; prev=""
for a in "\$@"; do [ "\$prev" = "--state" ] && state="\$a"; prev="\$a"; done
mine='{"number":173,"url":"https://github.com/o/r/pull/173","title":"fix(#142): band edge previous slice","state":"OPEN","mergedAt":null,"headRefName":"$PR_HEAD","isCrossRepository":false,"closingIssuesReferences":[{"number":142}]}'
fork='{"number":173,"url":"https://github.com/o/r/pull/173","title":"fix(#142): band edge previous slice","state":"OPEN","mergedAt":null,"headRefName":"$PR_HEAD","isCrossRepository":true,"closingIssuesReferences":[{"number":142}]}'
other='{"number":200,"url":"https://github.com/o/r/pull/200","title":"fix(#142): second attempt","state":"OPEN","mergedAt":null,"headRefName":"fix/142-second-attempt","isCrossRepository":false,"closingIssuesReferences":[{"number":142}]}'
case "\${FAKE_GH:-none}" in
  none)       printf '[]\n' ;;
  claim)      [ "\$state" = "open" ] && printf '[%s]\n' "\$mine"  || printf '[]\n' ;;
  fork_claim) [ "\$state" = "open" ] && printf '[%s]\n' "\$fork"  || printf '[]\n' ;;
  other_only) [ "\$state" = "open" ] && printf '[%s]\n' "\$other" || printf '[]\n' ;;
  two_claims) [ "\$state" = "open" ] && printf '[%s,%s]\n' "\$mine" "\$other" || printf '[]\n' ;;
  ratelimit)  echo 'gh: API rate limit exceeded for token ghp_FAKEFAKEFAKEFAKEFAKEFAKE1448. (HTTP 403)' >&2; exit 1 ;;
esac
FAKEEOF
chmod +x "$BIN/gh"

# --- repositories, distinguished only by the branch checked out ------------
mkrepo() { # mkrepo <dir> <branch>
  git init -q -b main "$1"
  git -C "$1" remote add origin "https://github.com/o/r.git"
  : > "$1/seed"; git -C "$1" add seed; git -C "$1" -c commit.gpgsign=false commit -qm seed
  [ "$2" = "main" ] || git -C "$1" checkout -q -b "$2"
}
mkrepo "$WORK/on-pr-branch" "$PR_HEAD"      # a review / resume: on PR #173's head
mkrepo "$WORK/on-main"      "main"          # a second implementation attempt
mkrepo "$WORK/on-other"     "fix/142-take-two"
mkrepo "$WORK/detached"     "main"
git -C "$WORK/detached" checkout -q --detach HEAD

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
        bash "$CLAIM" --issue "$ISSUE" --repo-path "$repo" "${extra[@]}" 2>&1)"
  RC=$?
}

echo "== 1. on the claiming pull request's own branch, the run proceeds =="
run "$WORK/on-pr-branch" FAKE_GH=claim
if [ "$RC" -eq 0 ] && grep -q 'issue_claim_check: ok-own-pr' <<<"$OUT" \
   && grep -q '#173' <<<"$OUT" && grep -q "$PR_HEAD" <<<"$OUT"; then
  pass "a review/resume standing on PR #173's head branch proceeds (exit 0), naming the PR"
else
  fail "the claiming PR's own branch was refused: rc=$RC out=${OUT:0:500}"
fi

echo "== 2. a second implementation attempt is still refused (#1361 must hold) =="
refused() { # refused <label> <expected-pr-number>
  local label="$1" num="$2" missing=""
  grep -q "#${num}" <<<"$OUT" || missing="$missing pr-number"
  grep -q "already working on issue #${ISSUE}" <<<"$OUT" || missing="$missing headline"
  if [ "$RC" -eq 1 ] && [ -z "$missing" ]; then
    pass "$label refuses (exit 1) naming PR #${num}"
  else
    fail "$label: rc=$RC missing:${missing:-none} out=${OUT:0:500}"
  fi
}
run "$WORK/on-main"  FAKE_GH=claim; refused "a fresh run on main while PR #173 claims the issue" 173
run "$WORK/on-other" FAKE_GH=claim; refused "a second attempt on a different fix/142-* branch" 173
run "$WORK/on-main"  FAKE_GH=other_only; refused "a competitor on a branch we are not on" 200

# A fork's PR may carry `headRefName: main` — or, here, a head named exactly
# like ours. Equal names across repositories are not one branch.
run "$WORK/on-pr-branch" FAKE_GH=fork_claim; refused "a same-named head in a FORK is not our branch" 173

# Detached HEAD has no branch to compare, so the pre-#1448 answer stands and the
# escape hatch is what the operator needs. The refusal has to say so.
run "$WORK/detached" FAKE_GH=claim
if [ "$RC" -eq 1 ] && grep -q 'allow_existing_pr=true' <<<"$OUT" \
   && grep -q 'detached HEAD' <<<"$OUT"; then
  pass "a detached HEAD cannot make the comparison, refuses, and names the escape hatch"
else
  fail "detached HEAD mishandled: rc=$RC out=${OUT:0:500}"
fi

echo "== 3. the allowance is per-claim, not a blanket bypass =="
run "$WORK/on-pr-branch" FAKE_GH=two_claims
if [ "$RC" -eq 1 ] && grep -q '#200' <<<"$OUT"; then
  pass "standing on PR #173 does not excuse a second competitor (#200) on another branch"
else
  fail "a second competitor was let through while on our own PR: rc=$RC out=${OUT:0:500}"
fi

echo "== 4. the explicit intent flag =="
run "$WORK/on-main" FAKE_GH=claim ALLOW_EXISTING_PR=true
if [ "$RC" -eq 0 ] && grep -q 'allow-existing-pr-requested' <<<"$OUT"; then
  pass "ALLOW_EXISTING_PR=true (from -c allow_existing_pr=true) proceeds off the branch"
else
  fail "the explicit flag did not proceed: rc=$RC out=${OUT:0:400}"
fi
run "$WORK/on-main" FAKE_GH=claim AMPLIHACK_ALLOW_EXISTING_PR=1
[ "$RC" -eq 0 ] && pass "AMPLIHACK_ALLOW_EXISTING_PR=1 proceeds too" \
  || fail "AMPLIHACK_ALLOW_EXISTING_PR ignored: rc=$RC out=${OUT:0:300}"
run "$WORK/on-main" FAKE_GH=claim -- --allow-existing-pr
[ "$RC" -eq 0 ] && pass "--allow-existing-pr proceeds too" \
  || fail "--allow-existing-pr ignored: rc=$RC out=${OUT:0:300}"
# `false` must not read as "set" — a bare -n test would have made the flag
# impossible to turn off once a caller had learned to pass it.
for v in false 0 no off ""; do
  run "$WORK/on-main" FAKE_GH=claim ALLOW_EXISTING_PR="$v"
  [ "$RC" -eq 1 ] || fail "ALLOW_EXISTING_PR='$v' wrongly bypassed the check: rc=$RC"
done
[ "$RC" -eq 1 ] && pass "ALLOW_EXISTING_PR=false/0/no/off/empty does NOT bypass the check"

echo "== 5. the refusal says how to proceed legitimately =="
run "$WORK/on-main" FAKE_GH=claim
missing=""
grep -q "git checkout ${PR_HEAD}" <<<"$OUT" || missing="$missing checkout-hint"
grep -q 'allow_existing_pr=true' <<<"$OUT" || missing="$missing flag-hint"
grep -q 'existing_branch=' <<<"$OUT" || missing="$missing existing-branch-hint"
grep -q 'branch here: main' <<<"$OUT" || missing="$missing branch-here"
[ -z "$missing" ] \
  && pass "the refusal names the branch we are on, the branch to check out, and the flag" \
  || fail "the refusal omits:$missing out=${OUT:0:600}"

echo "== 6. the three-way split is untouched =="
run "$WORK/on-pr-branch" FAKE_GH=ratelimit
if [ "$RC" -eq 0 ] && grep -q 'WARNING' <<<"$OUT" && grep -q 'rate-limited' <<<"$OUT"; then
  pass "a check that cannot be made still warns and continues, on the PR branch too"
else
  fail "an un-runnable check did not warn-and-continue: rc=$RC out=${OUT:0:400}"
fi
grep -q 'ghp_FAKEFAKEFAKEFAKEFAKEFAKE1448' <<<"$OUT" \
  && fail "a token value reached the output" \
  || pass "no token value reaches the output"
run "$WORK/on-pr-branch" FAKE_GH=none
[ "$RC" -eq 0 ] && grep -q 'issue_claim_check: ok issue=' <<<"$OUT" \
  && pass "an unclaimed issue still passes plainly, with no own-PR claim to report" \
  || fail "an unclaimed issue was mishandled: rc=$RC out=${OUT:0:300}"

echo "== 7. the recipe step carries all of it end to end =="
STEP="$WORK/step03c.sh"
if python3 - "$PREP" "$STEP" <<'PY'
import sys, yaml
steps = yaml.safe_load(open(sys.argv[1]))["steps"]
step = next(s for s in steps if s["id"] == "step-03c-issue-claim-check")
open(sys.argv[2], "w").write(step["command"])
PY
then
  pass "workflow-prep still declares step-03c-issue-claim-check"
else
  fail "workflow-prep no longer declares step-03c-issue-claim-check"
fi

step() { # step <repo> [env...]
  local repo="$1"; shift
  OUT="$(cd "$WORK" && env PATH="$BIN:$PATH" AMPLIHACK_HOME="$ROOT" \
        REPO_PATH="$repo" ISSUE_NUMBER="$ISSUE" "$@" bash "$STEP" 2>&1)"
  RC=$?
}
if [ -s "$STEP" ]; then
  step "$WORK/on-pr-branch" FAKE_GH=claim
  [ "$RC" -eq 0 ] && grep -q 'ok-own-pr' <<<"$OUT" \
    && pass "the recipe step lets a review of the issue's own PR run" \
    || fail "the recipe step blocked a review of the issue's own PR: rc=$RC out=${OUT:0:400}"

  step "$WORK/on-main" FAKE_GH=claim
  [ "$RC" -eq 1 ] && grep -q "already working on issue #${ISSUE}" <<<"$OUT" \
    && pass "the recipe step still refuses a second implementation attempt" \
    || fail "the recipe step let a duplicate through: rc=$RC out=${OUT:0:400}"

  # The flag reaches the helper only by OS environment inheritance through the
  # step, which is exactly how `-c allow_existing_pr=true` is delivered.
  step "$WORK/on-main" FAKE_GH=claim ALLOW_EXISTING_PR=true
  [ "$RC" -eq 0 ] && grep -q 'allow-existing-pr-requested' <<<"$OUT" \
    && pass "-c allow_existing_pr=true reaches the helper through the recipe step" \
    || fail "the flag did not survive the recipe step: rc=$RC out=${OUT:0:400}"
fi

echo
if [ "$fails" -eq 0 ]; then
  echo "issue #1448 review-is-not-a-duplicate: all checks passed"
else
  echo "issue #1448 review-is-not-a-duplicate: $fails check(s) failed"
fi
exit $((fails > 0))
