#!/usr/bin/env bash
# Regression tests for issue #1268 — a brittle scoped-PR gate failed runs that
# had actually succeeded, orphaning live pull requests.
#
# The incident: `workflow_pr_scope.sh` could not match a PR against its expected
# scope, so `workflow_final_status.sh` exited non-zero at the very last step —
# AFTER the run's PR had been created, reviewed, quality-audited and MERGED. The
# run was declared a failure and its two follow-up PRs were abandoned; one rotted
# into a conflicted state.
#
# Contract under test:
#   1. The scope match is a SIGNAL. When it misses, the run's actual artifacts
#      decide: a merged PR, work already in the base branch, or a live open PR
#      all mean SUCCESS and the run exits 0.
#   2. It is NOT a rubber stamp. When the evidence IS readable and shows the work
#      never landed and no PR carries it, the verdict is FAILED and the run exits
#      non-zero — and no asserted agent verdict can lift that.
#   3. Judgement may always DOWNGRADE: an agent verdict of FAILED fails the run
#      even when the mechanical evidence looked positive.
#   4. An asserted SUCCESS with no positive artifact behind it is refused.
#   5. Unreadable evidence is UNCERTAIN: never reported as success, but never
#      thrown away either — the run exits 0 with the outstanding PRs enumerated.
#   6. Outstanding open PRs are enumerated in the final status whatever the
#      verdict.
#
# These are behavioural tests against real fixtures: real git repositories with
# real commits, and a fake `gh` on PATH that answers exactly as GitHub does. No
# source-greps.
#
# Run: bash tests/issue_1268_terminal_state_adjudication.sh

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
FINAL_STATUS="$REPO_ROOT/amplifier-bundle/tools/workflow_final_status.sh"
EVIDENCE="$REPO_ROOT/amplifier-bundle/tools/workflow_terminal_evidence.sh"

pass=0
fail=0
TMPROOT=""
cleanup() { [ -n "$TMPROOT" ] && rm -rf "$TMPROOT"; }
trap cleanup EXIT

record_pass() { echo "PASS: $1"; pass=$((pass + 1)); }
record_fail() { echo "FAIL: $1"; fail=$((fail + 1)); }

for tool in git jq; do
    command -v "$tool" >/dev/null 2>&1 || { echo "SKIP: $tool is required by this test"; exit 0; }
done
[ -f "$FINAL_STATUS" ] || { echo "FAIL: missing $FINAL_STATUS"; exit 1; }
[ -f "$EVIDENCE" ] || { echo "FAIL: missing $EVIDENCE"; exit 1; }

TMPROOT="$(mktemp -d)"
BINDIR="$TMPROOT/bin"
mkdir -p "$BINDIR"

# ---------------------------------------------------------------------------
# Fake `gh`. Driven entirely by files in $FAKE_GH_DIR so each scenario declares
# its own GitHub state:
#   pr_list_head.json   answer for `pr list --head <branch> --state all`
#   pr_list_open.json   answer for `pr list --state open`
#   pr_view.json        answer for `pr view <target>`
# A missing file means "that query returns an empty result". The sentinel file
# `fail_auth` makes every call fail with a permanent auth error, which the shared
# retry helper classifies as non-retryable — an unreadable GitHub.
# ---------------------------------------------------------------------------
cat >"$BINDIR/gh" <<'FAKE'
#!/usr/bin/env bash
set -uo pipefail
dir="${FAKE_GH_DIR:?FAKE_GH_DIR must be set}"
printf '%s\n' "$*" >>"$dir/calls.log"
if [ -f "$dir/fail_auth" ]; then
    echo "HTTP 401: Bad credentials" >&2
    exit 1
fi
if [ "${1:-}" = "pr" ] && [ "${2:-}" = "list" ]; then
    if printf '%s\n' "$@" | grep -qx -- "--head"; then
        cat "$dir/pr_list_head.json" 2>/dev/null || echo '[]'
    else
        cat "$dir/pr_list_open.json" 2>/dev/null || echo '[]'
    fi
    exit 0
fi
if [ "${1:-}" = "pr" ] && [ "${2:-}" = "view" ]; then
    if [ -f "$dir/pr_view.json" ]; then
        cat "$dir/pr_view.json"
        exit 0
    fi
    echo "no pull requests found" >&2
    exit 1
fi
if [ "${1:-}" = "api" ]; then
    # rate-limit probes and REST fallbacks: report an exhausted-but-readable
    # budget so nothing sleeps.
    echo '{}'
    exit 0
fi
echo "fake gh: unhandled args: $*" >&2
exit 1
FAKE
chmod +x "$BINDIR/gh"

# ---------------------------------------------------------------------------
# Fixture: a real git repo with an origin remote pointing at github.com, a base
# branch, and a feature branch carrying a real commit.
#   $1 name
#   $2 "landed" -> the feature commit is also merged into the base branch
# ---------------------------------------------------------------------------
make_repo() {
    local name="$1" landed="${2:-}"
    local remote="$TMPROOT/$name.git" work="$TMPROOT/$name"
    git init --quiet --bare "$remote"
    git init --quiet -b main "$work"
    git -C "$work" config user.email "test@example.com"
    git -C "$work" config user.name "Test"
    echo base > "$work/README.md"
    git -C "$work" add README.md
    git -C "$work" commit --quiet -m "base"
    git -C "$work" remote add origin "https://github.com/octo/example.git"
    git -C "$work" checkout --quiet -b fix/1268-example
    echo feature > "$work/feature.txt"
    git -C "$work" add feature.txt
    git -C "$work" commit --quiet -m "feat: the deliverable"
    # Synthesise refs/remotes/origin/* without talking to a real remote.
    git -C "$work" update-ref refs/remotes/origin/main "$(git -C "$work" rev-parse main)"
    if [ "$landed" = "landed" ]; then
        git -C "$work" update-ref refs/remotes/origin/main "$(git -C "$work" rev-parse HEAD)"
    fi
    git -C "$work" symbolic-ref refs/remotes/origin/HEAD refs/remotes/origin/main
    printf '%s\n' "$work"
}

pr_json() {
    # $1 number  $2 state  $3 headRefName  $4 title
    jq -nc --arg n "$1" --arg s "$2" --arg h "$3" --arg t "$4" \
        '{number: ($n|tonumber), state: $s, title: $t,
          url: ("https://github.com/octo/example/pull/" + $n),
          headRefName: $h, baseRefName: "main",
          headRefOid: "0000000000000000000000000000000000000000",
          createdAt: "2026-08-21T00:00:00Z", isCrossRepository: false}'
}

# Run workflow_final_status.sh against a fixture repo. Combined output lands in
# $RUN_OUT (a file, so the exit status survives) and the status in $RUN_RC.
#
# emulate_runner ARG...: sets EMU_ARGS to ARG... plus what the pinned runner
# writes alongside them (#1561). For every RECIPE_VAR_<key>__<field>=v the
# runner also writes the base RECIPE_VAR_<key> as the JSON object holding
# those fields, so the fixtures set both, unless an argument sets
# RECIPE_VAR_<key> itself. A STALE:NAME=v argument is passed through with no
# base key: it stands for a value inherited from a parent run.
emulate_runner() {
    EMU_ARGS=()
    local a k b field v json bases=""
    for a in "$@"; do
        case "$a" in STALE:*) EMU_ARGS+=("${a#STALE:}"); continue ;; esac
        EMU_ARGS+=("$a")
        k="${a%%=*}"
        case "$k" in
            RECIPE_VAR_*__*)
                b="${k%%__*}"
                case " $bases " in *" $b "*) ;; *) bases="$bases $b" ;; esac
                ;;
        esac
    done
    for b in $bases; do
        for a in "$@"; do case "$a" in "$b="*) continue 2 ;; esac; done
        json='{}'
        for a in "$@"; do
            case "$a" in
                STALE:*) ;;
                "$b"__*=*)
                    field="${a%%=*}"; field="${field#"$b"__}"; v="${a#*=}"
                    json="$(jq -c --arg f "$field" --arg v "$v" '.[$f] = $v' <<<"$json")"
                    ;;
            esac
        done
        EMU_ARGS+=("$b=$json")
    done
}

# Fixture defaults. An UNSET:NAME argument drops the default NAME for one run.
RUN_DEFAULTS=(
    "RECIPE_VAR_pr_url=https://github.com/octo/example/pull/1128"
    "RECIPE_VAR_pr_publish_result__state=FOLLOWUP_CREATED"
    "RECIPE_VAR_issue_number=1268"
)
RUN_OUT=""
RUN_RC=0
run_final_status() {
    local repo="$1" fakedir="$2" d a
    shift 2
    local -a args=() drop=()
    for a in "$@"; do case "$a" in UNSET:*) drop+=("${a#UNSET:}") ;; esac; done
    for d in "${RUN_DEFAULTS[@]}"; do
        for a in "${drop[@]+"${drop[@]}"}"; do [ "${d%%=*}" = "$a" ] && continue 2; done
        args+=("$d")
    done
    for a in "$@"; do case "$a" in UNSET:*) ;; *) args+=("$a") ;; esac; done
    emulate_runner "${args[@]}"
    RUN_OUT="$TMPROOT/last-run.out"
    : >"$fakedir/calls.log"
    (
        cd "$repo" || exit 99
        # Nothing the caller's own run exported may reach the tool.
        while IFS= read -r _v; do unset "$_v"; done < <(compgen -e | grep -E '^(RECIPE_VAR_|PR_PUBLISH_RESULT_|TERMINAL_STATE_|TERMINAL_ADJUDICATION_|WORKTREE_SETUP_)' || true)
        unset GH_TOKEN GITHUB_TOKEN PR_URL PR_NUMBER REPO_PATH
        export PATH="$BINDIR:$PATH"
        export FAKE_GH_DIR="$fakedir"
        export REMOTE_HOST_TYPE="github"
        export TASK_DESCRIPTION="issue 1268 fixture"
        export GH_RETRY_MAX_TRANSIENT=1
        export GH_RETRY_MAX_RL_WINDOWS=0
        export HOME="$TMPROOT/home"
        env "${EMU_ARGS[@]}" bash "$FINAL_STATUS"
    ) >"$RUN_OUT" 2>&1
    RUN_RC=$?
    return 0
}

mkdir -p "$TMPROOT/home"

# ===========================================================================
# 1. THE INCIDENT. The run's PR merged; two follow-ups are open. The scoped
#    match cannot resolve it (the merged PR's headRefOid no longer matches the
#    local HEAD — exactly the real failure). This must be SUCCESS, exit 0.
# ===========================================================================
repo="$(make_repo incident)"
fake="$TMPROOT/fake-incident"; mkdir -p "$fake"
jq -nc --argjson a "$(pr_json 1128 MERGED fix/1268-example 'fix: the deliverable')" '[$a]' > "$fake/pr_list_head.json"
jq -nc --argjson b "$(pr_json 1132 OPEN followup/rebase 'chore: follow-up A')" \
       --argjson c "$(pr_json 1133 OPEN followup/tests 'chore: follow-up B')" '[$b,$c]' > "$fake/pr_list_open.json"
jq -c '.[0]' "$fake/pr_list_head.json" > "$fake/pr_view.json"

run_final_status "$repo" "$fake" WORKFLOW_STARTED_AT=2026-08-20T00:00:00Z
out="$(cat "$RUN_OUT")"
rc=$RUN_RC
if [ "$rc" -eq 0 ] && printf '%s' "$out" | grep -q 'terminal_verdict=SUCCESS'; then
    record_pass "merged PR + open follow-ups is SUCCESS, not no_scoped_pr (exit 0)"
else
    record_fail "merged PR + open follow-ups must be SUCCESS (rc=$rc)"
    printf '%s\n' "$out" | sed 's/^/    | /'
fi
if printf '%s' "$out" | grep -q 'outstanding_pr=#1132' && printf '%s' "$out" | grep -q 'outstanding_pr=#1133'; then
    record_pass "outstanding open PRs #1132 and #1133 are enumerated in the final status"
else
    record_fail "outstanding open PRs must be enumerated whatever the verdict"
    printf '%s\n' "$out" | sed 's/^/    | /'
fi
if printf '%s' "$out" | grep -q 'Workflow final status failed'; then
    record_fail "a succeeded run must not report 'Workflow final status failed'"
else
    record_pass "no spurious 'terminal success was not proven' on a succeeded run"
fi

# ===========================================================================
# 2. SQUASH-MERGE. No PR is visible from the head branch at all, but the work's
#    content is already in the base branch. That is success by git fact.
# ===========================================================================
repo="$(make_repo squashed landed)"
fake="$TMPROOT/fake-squashed"; mkdir -p "$fake"
echo '[]' > "$fake/pr_list_head.json"
echo '[]' > "$fake/pr_list_open.json"
run_final_status "$repo" "$fake"
out="$(cat "$RUN_OUT")"
rc=$RUN_RC
if [ "$rc" -eq 0 ] && printf '%s' "$out" | grep -q 'terminal_verdict=SUCCESS'; then
    record_pass "work already contained in the base branch is SUCCESS"
else
    record_fail "work already in base must be SUCCESS (rc=$rc)"
    printf '%s\n' "$out" | sed 's/^/    | /'
fi

# ===========================================================================
# 3. NOT A RUBBER STAMP. Evidence is readable and shows: no PR anywhere, and the
#    work is not in the base branch. This is a genuine failure and must fail —
#    even when an agent verdict of SUCCESS is asserted.
# ===========================================================================
repo="$(make_repo genuinefail)"
fake="$TMPROOT/fake-genuinefail"; mkdir -p "$fake"
echo '[]' > "$fake/pr_list_head.json"
echo '[]' > "$fake/pr_list_open.json"
run_final_status "$repo" "$fake"
out="$(cat "$RUN_OUT")"
rc=$RUN_RC
if [ "$rc" -ne 0 ] && printf '%s' "$out" | grep -q 'Workflow final status failed'; then
    record_pass "a genuinely failed run (no PR, work not in base) still fails"
else
    record_fail "a genuinely failed run must still fail (rc=$rc)"
    printf '%s\n' "$out" | sed 's/^/    | /'
fi

run_final_status "$repo" "$fake" RECIPE_VAR_terminal_adjudication__agent_verdict=SUCCESS
out="$(cat "$RUN_OUT")"
rc=$RUN_RC
if [ "$rc" -ne 0 ]; then
    record_pass "an asserted agent SUCCESS cannot lift a hard negative"
else
    record_fail "RUBBER STAMP: an asserted agent SUCCESS lifted a proven failure"
    printf '%s\n' "$out" | sed 's/^/    | /'
fi

# Same via the adjudicator directly, so the guard is pinned at its own boundary.
ev="$(jq -nc '{mechanical_verdict:"FAILED", hard_negative:"true", positive_artifact:"false",
               mechanical_reason:"no PR and not in base", evidence_readable:"true",
               work_in_base:"false", merged_pr_count:0, open_pr_count:0,
               closed_unmerged_pr_count:0, outstanding_prs:[], outstanding_pr_count:0}')"
for token in SUCCESS success merged Complete LANDED; do
    verdict="$(printf '%s' "$ev" | bash "$EVIDENCE" adjudicate - --agent-verdict "$token" --report-only 2>/dev/null | jq -r '.terminal_verdict')"
    if [ "$verdict" != "FAILED" ]; then
        record_fail "hard negative was lifted by agent verdict '$token' (got $verdict)"
        break
    fi
done
[ "$verdict" = "FAILED" ] && record_pass "hard negative survives every flavour of asserted success"

# ===========================================================================
# 4. JUDGEMENT MAY DOWNGRADE. Mechanical evidence looks positive (a merged PR),
#    but the agent judged the run failed. Downgrades are always honoured.
# ===========================================================================
repo="$(make_repo downgrade)"
fake="$TMPROOT/fake-downgrade"; mkdir -p "$fake"
jq -nc --argjson a "$(pr_json 1128 MERGED fix/1268-example 'fix: the deliverable')" '[$a]' > "$fake/pr_list_head.json"
echo '[]' > "$fake/pr_list_open.json"
run_final_status "$repo" "$fake" RECIPE_VAR_terminal_adjudication__agent_verdict=FAILED
out="$(cat "$RUN_OUT")"
rc=$RUN_RC
if [ "$rc" -ne 0 ]; then
    record_pass "an agent verdict of FAILED downgrades a mechanically positive read"
else
    record_fail "agent downgrade was ignored (rc=$rc)"
    printf '%s\n' "$out" | sed 's/^/    | /'
fi

# ===========================================================================
# 5. ASSERTED SUCCESS WITH NOTHING BEHIND IT IS REFUSED. Readable evidence, no
#    merged PR, no open PR, work not in base is already a hard negative; the
#    subtler case is UNCERTAIN mechanical evidence with no positive artifact.
# ===========================================================================
ev="$(jq -nc '{mechanical_verdict:"UNCERTAIN", hard_negative:"false", positive_artifact:"false",
               mechanical_reason:"no PR matched and base state unknown", evidence_readable:"true",
               work_in_base:"unknown", merged_pr_count:0, open_pr_count:0,
               closed_unmerged_pr_count:0, outstanding_prs:[], outstanding_pr_count:0}')"
adj="$(printf '%s' "$ev" | bash "$EVIDENCE" adjudicate - --agent-verdict SUCCESS --report-only 2>/dev/null)"
if [ "$(printf '%s' "$adj" | jq -r '.terminal_verdict')" = "UNCERTAIN" ] \
   && [ "$(printf '%s' "$adj" | jq -r '.verdict_source')" = "agent_success_refused_no_artifact" ] \
   && [ "$(printf '%s' "$adj" | jq -r '.terminal_success_proven')" = "false" ]; then
    record_pass "asserted SUCCESS with no positive artifact is refused, not stamped"
else
    record_fail "asserted SUCCESS with no artifact was accepted: $adj"
fi

# Verdict tokens that CONTAIN an approving word must not be read as approval.
for token in NOT_MERGED UNSUCCESSFUL "not landed"; do
    v="$(printf '%s' "$ev" | bash "$EVIDENCE" adjudicate - --agent-verdict "$token" --report-only 2>/dev/null | jq -r '.terminal_verdict')"
    if [ "$v" = "SUCCESS" ]; then
        record_fail "substring match: verdict token '$token' was read as approval"
        break
    fi
done
[ "$v" != "SUCCESS" ] && record_pass "verdict tokens are matched exactly ('NOT_MERGED' is not 'MERGED')"

# ===========================================================================
# 6. UNREADABLE GITHUB IS UNCERTAIN. Not success, not a discarded run.
# ===========================================================================
repo="$(make_repo unreadable)"
fake="$TMPROOT/fake-unreadable"; mkdir -p "$fake"; touch "$fake/fail_auth"
run_final_status "$repo" "$fake"
out="$(cat "$RUN_OUT")"
rc=$RUN_RC
if [ "$rc" -eq 0 ] \
   && printf '%s' "$out" | grep -q 'terminal_verdict=UNCERTAIN' \
   && ! printf '%s' "$out" | grep -q 'All 23 workflow steps completed successfully'; then
    record_pass "unreadable evidence is UNCERTAIN: exit 0, and success is NOT claimed"
else
    record_fail "unreadable evidence must be UNCERTAIN without claiming success (rc=$rc)"
    printf '%s\n' "$out" | sed 's/^/    | /'
fi
if printf '%s' "$out" | grep -q 'still live and must be driven to a terminal state'; then
    record_pass "UNCERTAIN hands the outstanding artifacts back instead of orphaning them"
else
    record_fail "UNCERTAIN must surface the outstanding artifacts"
    printf '%s\n' "$out" | sed 's/^/    | /'
fi

# Unreadable evidence must stay UNCERTAIN even when SUCCESS is asserted.
run_final_status "$repo" "$fake" RECIPE_VAR_terminal_adjudication__agent_verdict=SUCCESS
out="$(cat "$RUN_OUT")"
if printf '%s' "$out" | grep -q 'All 23 workflow steps completed successfully'; then
    record_fail "RUBBER STAMP: unreadable evidence was stamped SUCCESS on assertion"
else
    record_pass "unreadable evidence is not stamped SUCCESS by assertion"
fi

# ===========================================================================
# 7. A CLOSED-UNMERGED PR with work still outstanding is a readable negative.
# ===========================================================================
repo="$(make_repo closedunmerged)"
fake="$TMPROOT/fake-closed"; mkdir -p "$fake"
jq -nc --argjson a "$(pr_json 1128 CLOSED fix/1268-example 'fix: abandoned')" '[$a]' > "$fake/pr_list_head.json"
echo '[]' > "$fake/pr_list_open.json"
run_final_status "$repo" "$fake"
out="$(cat "$RUN_OUT")"
rc=$RUN_RC
if [ "$rc" -ne 0 ]; then
    record_pass "closed-unmerged PR with unlanded work still fails the run"
else
    record_fail "closed-unmerged PR with unlanded work must fail (rc=$rc)"
    printf '%s\n' "$out" | sed 's/^/    | /'
fi

# ===========================================================================
# 8. The collector itself never dies mid-probe: it always emits parseable JSON.
# ===========================================================================
repo="$(make_repo collector)"
fake="$TMPROOT/fake-collector"; mkdir -p "$fake"; touch "$fake/fail_auth"
raw="$(cd "$repo" && PATH="$BINDIR:$PATH" FAKE_GH_DIR="$fake" GH_RETRY_MAX_RL_WINDOWS=0 \
    bash "$EVIDENCE" collect 2>/dev/null)"
if printf '%s' "$raw" | jq -e '.mechanical_verdict == "UNCERTAIN" and .evidence_readable == "false"' >/dev/null 2>&1; then
    record_pass "collector emits structured UNCERTAIN evidence when GitHub is unreadable"
else
    record_fail "collector must always emit parseable evidence JSON: $raw"
fi


# ===========================================================================
# 9. The recipe-facing wrappers. `collect-for-step` is what the adjudication
#    sub-recipe calls, and `verdict-token` is the ONLY path by which the
#    adjudicator agent's prose reaches control flow.
# ===========================================================================
repo="$(make_repo wrappers)"
fake="$TMPROOT/fake-wrappers"; mkdir -p "$fake"
jq -nc --argjson a "$(pr_json 1128 MERGED fix/1268-example 'fix: the deliverable')" '[$a]' > "$fake/pr_list_head.json"
jq -nc --argjson b "$(pr_json 1132 OPEN followup/rebase 'chore: follow-up A')" '[$b]' > "$fake/pr_list_open.json"
raw="$(cd "$repo" && PATH="$BINDIR:$PATH" FAKE_GH_DIR="$fake" GH_RETRY_MAX_RL_WINDOWS=0 \
    RECIPE_VAR_pr_publish_result='{"pr_url":"https://github.com/octo/example/pull/1128"}' \
    RECIPE_VAR_pr_publish_result__pr_url="https://github.com/octo/example/pull/1128" \
    bash "$EVIDENCE" collect-for-step 2>/dev/null)"
if printf '%s' "$raw" | jq -e '.mechanical_verdict == "SUCCESS" and .merged_pr_count == 1' >/dev/null 2>&1; then
    record_pass "collect-for-step reads the published PR from recipe env and sees the merge"
else
    record_fail "collect-for-step must resolve recipe env and see the merged PR: $raw"
fi

# The agent's prose is prose. Only the token line crosses over, and prose that
# merely mentions success must not be mistaken for a verdict.
narrative_case() {
    RECIPE_VAR_terminal_adjudication_narrative="$1" bash "$EVIDENCE" verdict-token 2>/dev/null | jq -r '.agent_verdict'
}
v="$(narrative_case 'PR #1128 merged.
TERMINAL_VERDICT: SUCCESS')"
[ "$v" = "SUCCESS" ] && record_pass "verdict-token extracts an explicit SUCCESS verdict line" \
    || record_fail "verdict-token failed to extract SUCCESS (got '$v')"

v="$(narrative_case 'The run looks successful and everything merged nicely.')"
[ -z "$v" ] && record_pass "prose that merely sounds successful yields NO verdict token" \
    || record_fail "prose without a verdict line produced a verdict '$v'"

v="$(narrative_case 'TERMINAL_VERDICT: SUCCESS
TERMINAL_VERDICT: FAILED')"
[ "$v" = "FAILED" ] && record_pass "the last explicit verdict line wins" \
    || record_fail "last verdict line did not win (got '$v')"

v="$(narrative_case 'TERMINAL_VERDICT: DEFINITELY_MERGED')"
[ -z "$v" ] && record_pass "an unrecognised verdict token is dropped, not coerced to approval" \
    || record_fail "unrecognised token '$v' leaked through verdict-token"

v="$(RECIPE_VAR_terminal_adjudication_narrative="" bash "$EVIDENCE" verdict-token 2>/dev/null | jq -r '.agent_verdict')"
[ -z "$v" ] && record_pass "a skipped adjudicator step offers no judgement (empty verdict)" \
    || record_fail "empty narrative produced a verdict '$v'"

# ===========================================================================
# 10. #1561: nested step outputs are read from the base key. The runner writes
#     RECIPE_VAR_<key>__<field> only for the fields the current object has, so
#     a nested name (or its bare alias) can hold a parent run's value. 9999 is
#     the stale PR: it must never reach gh or the output.
# ===========================================================================
STALE_URL="https://github.com/octo/example/pull/9999"
stale_pr_unused() {
    ! grep -qF 9999 "$1/calls.log" && ! grep -qF 9999 "$RUN_OUT"
}

repo="$(make_repo stale-nested)"
fake="$TMPROOT/fake-stale-nested"; mkdir -p "$fake"
jq -nc --argjson a "$(pr_json 1128 MERGED fix/1268-example 'fix: the deliverable')" '[$a]' > "$fake/pr_list_head.json"
echo '[]' > "$fake/pr_list_open.json"
jq -c '.[0]' "$fake/pr_list_head.json" > "$fake/pr_view.json"
run_final_status "$repo" "$fake" UNSET:RECIPE_VAR_pr_url \
    RECIPE_VAR_pr_publish_result='{"pr_url":"https://github.com/octo/example/pull/1128","pr_number":"1128","state":"FOLLOWUP_CREATED"}' \
    "STALE:RECIPE_VAR_pr_publish_result__pr_url=$STALE_URL" \
    "STALE:RECIPE_VAR_pr_publish_result__pr_number=9999" \
    "STALE:PR_PUBLISH_RESULT_PR_URL=$STALE_URL" \
    "STALE:PR_PUBLISH_RESULT_PR_NUMBER=9999"  # deliberately stale (#1561)
out="$(cat "$RUN_OUT")"
if [ "$RUN_RC" -eq 0 ] && printf '%s' "$out" | grep -q 'terminal_verdict=SUCCESS' && stale_pr_unused "$fake"; then
    record_pass "1561-stale-nested-pr: own pr_publish_result JSON (#1128) wins; stale #9999 never reaches gh"
else
    record_fail "1561-stale-nested-pr: own pr_publish_result JSON (#1128) must win over stale #9999 (rc=$RUN_RC)"
    printf '%s\n' "$out" | sed 's/^/    | /'; sed 's/^/    gh| /' "$fake/calls.log"
fi

repo="$(make_repo empty-base-stale)"
fake="$TMPROOT/fake-empty-base-stale"; mkdir -p "$fake"
echo '[]' > "$fake/pr_list_head.json"
echo '[]' > "$fake/pr_list_open.json"
run_final_status "$repo" "$fake" UNSET:RECIPE_VAR_pr_url RECIPE_VAR_pr_publish_result= \
    "STALE:RECIPE_VAR_pr_publish_result__pr_url=$STALE_URL" \
    "STALE:RECIPE_VAR_pr_publish_result__pr_number=9999"  # deliberately stale (#1561)
out="$(cat "$RUN_OUT")"
if [ "$RUN_RC" -eq 1 ] && printf '%s' "$out" | grep -qx 'terminal_state=FAILED_MISSING_TERMINAL_EVIDENCE' && stale_pr_unused "$fake"; then
    record_pass "1561-empty-base-stale-nested-pr: base \"\" + stale nested PR -> no PR, FAILED_MISSING_TERMINAL_EVIDENCE"
else
    record_fail "1561-empty-base-stale-nested-pr: base \"\" + stale nested PR must read as no PR (rc=$RUN_RC)"
    printf '%s\n' "$out" | sed 's/^/    | /'; sed 's/^/    gh| /' "$fake/calls.log"
fi

repo="$(make_repo stale-verdict)"
fake="$TMPROOT/fake-stale-verdict"; mkdir -p "$fake"
jq -nc --argjson a "$(pr_json 1128 MERGED fix/1268-example 'fix: the deliverable')" '[$a]' > "$fake/pr_list_head.json"
echo '[]' > "$fake/pr_list_open.json"
run_final_status "$repo" "$fake" RECIPE_VAR_terminal_adjudication='{"agent_verdict":"FAILED"}' \
    "STALE:RECIPE_VAR_terminal_adjudication__agent_verdict=SUCCESS" \
    "STALE:TERMINAL_ADJUDICATION_AGENT_VERDICT=SUCCESS"  # deliberately stale (#1561)
if [ "$RUN_RC" -ne 0 ]; then
    record_pass "1561-stale-nested-agent-verdict: own agent_verdict FAILED beats a stale nested SUCCESS"
else
    record_fail "1561-stale-nested-agent-verdict: a stale nested SUCCESS overrode the run's own FAILED verdict"
    sed 's/^/    | /' "$RUN_OUT"
fi

echo ""
echo "issue #1268 terminal-state adjudication: $pass passed, $fail failed"
[ "$fail" -eq 0 ] || exit 1
