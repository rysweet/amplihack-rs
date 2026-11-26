#!/usr/bin/env bash
# Issue #1930: setup pushes nothing, so a new branch is published at its first
# real push. Runs the actual recipe bodies (setup, then step-15, or setup, then
# consensus step9 and step12) against disposable repositories whose pre-push
# hook records each run and accepts it.
# Optional argument selects one named case, useful for mutant checks.
set -euo pipefail
if [[ ${ISSUE_1930_ISOLATED:-0} != 1 ]]; then
    exec env -i PATH=/usr/bin:/bin TMPDIR="${TMPDIR:-/tmp}" \
        ISSUE_1930_ISOLATED=1 ISSUE_1930_ARTIFACT_DIR="${ISSUE_1930_ARTIFACT_DIR:-}" bash "$0" "$@"
fi
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
SOURCE="$(cd "$SCRIPT_DIR/../../.." && pwd -P)"
TEST_TMP="$(mktemp -d "${ISSUE_1930_ARTIFACT_DIR:-${TMPDIR:-/tmp}}/issue-1930-push.XXXXXX")"
TEST_TMP="$(cd "$TEST_TMP" && pwd -P)"
if [[ -z "${ISSUE_1930_ARTIFACT_DIR:-}" ]]; then
    trap 'rm -rf -- "$TEST_TMP"' EXIT
else
    echo "Fixture evidence: $TEST_TMP"
fi
export HOME="$TEST_TMP/home" XDG_CONFIG_HOME="$TEST_TMP/config"
export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null GIT_ALLOW_PROTOCOL=file
REAL_GIT="$(command -v git)"
export REAL_GIT TEST_TMP
mkdir -p "$HOME" "$XDG_CONFIG_HOME" "$TEST_TMP/bin" "$TEST_TMP/bundle/amplifier-bundle/tools"
# Setup runs with the same small bundle as test-issue-1930-setup-does-not-push.sh.
for name in workflow_worktree_sweep workflow_pr_branch workflow_caller_branch_alias workflow_alias_reuse; do
    cp "$SOURCE/amplifier-bundle/tools/$name.sh" "$TEST_TMP/bundle/amplifier-bundle/tools/"
done
export PATH="$TEST_TMP/bin:$PATH"
# Record every push after Git's global options, then run the real Git.
cat > "$TEST_TMP/bin/git" <<'GIT'
#!/usr/bin/env bash
set -euo pipefail
args=("$@")
while (($#)); do
    case "$1" in
        -C|-c|--git-dir|--work-tree|--namespace) shift 2 ;;
        --git-dir=*|--work-tree=*|--no-optional-locks|--no-pager|--literal-pathspecs) shift ;;
        *) break ;;
    esac
done
if [[ ${1:-} == push ]]; then
    printf '%q ' "${args[@]}" >> "$PUSH_LOG"; printf '\n' >> "$PUSH_LOG"
fi
exec "$REAL_GIT" "${args[@]}"
GIT
# The publish steps call `amplihack hygiene artifact-guard`; nothing else is used.
cat > "$TEST_TMP/bin/amplihack" <<'STUB'
#!/usr/bin/env bash
[[ "${1:-} ${2:-}" == 'hygiene artifact-guard' ]] || { echo "unexpected amplihack $*" >&2; exit 98; }
STUB
chmod +x "$TEST_TMP/bin/"{git,amplihack}

# extract_step <recipe> <step-id> <output>: the step's `command: |` body.
extract_step() {
    awk -v id="$2" '
        $0 ~ "^[[:space:]]*- id: \"" id "\"[[:space:]]*$" { in_step = 1; next }
        in_step && /^[[:space:]]*- id: / { exit }
        in_step && /^[[:space:]]*command: \|[[:space:]]*$/ { in_cmd = 1; next }
        in_cmd {
            if (substr($0, 1, 6) == "      ") { print substr($0, 7); next }
            if ($0 ~ /^[[:space:]]*$/) { print ""; next }
            exit
        }' "$SOURCE/amplifier-bundle/recipes/$1" > "$3"
    [[ -s "$3" ]] || { echo "HARNESS-ERROR: no $2 in $1" >&2; exit 2; }
}
awk '/^    command: \|/ {body=1;next} body && /^    [A-Za-z_-]+:/ {exit}
     body {sub(/^      /, ""); print}' \
    "$SOURCE/amplifier-bundle/recipes/workflow-worktree.yaml" > "$TEST_TMP/default.sh"
awk '/^      ```bash/ {body=1;next} body && /^      ```/ {exit}
     body {sub(/^      /, ""); print}' \
    "$SOURCE/amplifier-bundle/recipes/consensus-issue-worktree.yaml" > "$TEST_TMP/consensus.sh"
extract_step workflow-publish.yaml step-15-commit-push "$TEST_TMP/step15.sh"
extract_step consensus-publish.yaml step9-commit "$TEST_TMP/step9.sh"
extract_step consensus-pr-feedback.yaml step12-push-updates "$TEST_TMP/step12.sh"
for f in default consensus step15 step9 step12; do
    [[ -s "$TEST_TMP/$f.sh" ]] || { echo "HARNESS-ERROR: empty $f.sh" >&2; exit 2; }
    bash -n "$TEST_TMP/$f.sh"
done

die() { echo "ASSERTION FAILED: $*" >&2; return 1; }
equal() { [[ "$1" == "$2" ]] || die "$3: expected '$2', got '$1'"; }
lines() { if [[ -s "$1" ]]; then wc -l < "$1" | tr -d ' '; else echo 0; fi; }
remote_tip() { "$REAL_GIT" --git-dir="$REMOTE" rev-parse --verify --quiet "refs/heads/$1" || true; }
fixture() {
    CASE_ROOT="$TEST_TMP/$1"; mkdir -p "$CASE_ROOT"
    export PUSH_LOG="$CASE_ROOT/push.log" HOOK_LOG="$CASE_ROOT/hook.log"
    : > "$PUSH_LOG"; : > "$HOOK_LOG"
    REPO="$CASE_ROOT/repo"; REMOTE="$CASE_ROOT/remote.git"
    "$REAL_GIT" init -q --bare "$REMOTE"
    "$REAL_GIT" init -q -b main "$REPO"
    "$REAL_GIT" -C "$REPO" config user.name Test
    "$REAL_GIT" -C "$REPO" config user.email t@example.com
    printf 'base\n' > "$REPO/base.txt"
    "$REAL_GIT" -C "$REPO" add base.txt
    "$REAL_GIT" -C "$REPO" commit -qm base
    "$REAL_GIT" -C "$REPO" remote add origin "$REMOTE"
    "$REAL_GIT" -C "$REPO" push -qu origin main
    "$REAL_GIT" --git-dir="$REMOTE" symbolic-ref HEAD refs/heads/main
    "$REAL_GIT" -C "$REPO" remote set-head origin main
    # Installed after the fixture's own push, so only the steps' pushes count.
    cat > "$REPO/.git/hooks/pre-push" <<'HOOK'
#!/usr/bin/env bash
printf 'pre-push %s\n' "$1" >> "${HOOK_LOG:?}"
exit 0
HOOK
    chmod +x "$REPO/.git/hooks/pre-push"
    export AMPLIHACK_RUNTIME_ROOT="$CASE_ROOT/runtime" AMPLIHACK_STATE_DIR="$CASE_ROOT/state"
    export AMPLIHACK_WORKTREE_BASE_REF=origin/main REPO_PATH="$REPO" WORKTREE_DIR=''
    export EXISTING_BRANCH='' PR_NUMBER='' TASK_DESCRIPTION='first push publishes'
    export ISSUE_NUMBER=1930 BRANCH_PREFIX=fix
}
# setup <default|consensus>: run worktree setup for a new branch; sets WT and B.
setup() {
    (cd "$REPO" && AMPLIHACK_HOME="$TEST_TMP/bundle" bash -euo pipefail "$TEST_TMP/$1.sh") \
        > "$CASE_ROOT/setup.json" 2> "$CASE_ROOT/setup.err" \
        || { cat "$CASE_ROOT/setup.err" >&2; die "$1 setup did not finish"; }
    WT="$(jq -r .worktree_path "$CASE_ROOT/setup.json")"
    B="$(jq -r .branch_name "$CASE_ROOT/setup.json")"
    [[ "$WT" == "$REPO/worktrees/"* && -d "$WT" && -n "$B" ]] || die 'invalid setup output'
    equal "$(lines "$PUSH_LOG")" 0 'pushes at setup'
    equal "$(lines "$HOOK_LOG")" 0 'pre-push hook runs at setup'
    equal "$(remote_tip "$B")" '' "origin/$B after setup"
    equal "$("$REAL_GIT" -C "$WT" rev-parse --abbrev-ref '@{u}' 2>/dev/null || true)" '' 'upstream after setup'
}
# run_step <step> <label>: run a publish step body in WT as the runner would.
run_step() {
    local rc=0
    (cd "$WT" && env AMPLIHACK_HOME="$SOURCE" \
        RECIPE_VAR_worktree_setup="$(cat "$CASE_ROOT/setup.json")" \
        RECIPE_VAR_task_description='first push publishes' RECIPE_VAR_is_critical_code=false \
        RECIPE_VAR_version_bump='{"change_classification":"PATCH"}' REMOTE_HOST_TYPE=github \
        bash "$TEST_TMP/$1.sh") > "$CASE_ROOT/$2.out" 2> "$CASE_ROOT/$2.err" || rc=$?
    STEP_RC="$rc"
    RESULT="$(tail -n 1 "$CASE_ROOT/$2.out")"
}
field() { printf '%s' "$RESULT" | jq -r ".$1"; }
expect_published() {
    equal "$(remote_tip "$B")" "$("$REAL_GIT" -C "$WT" rev-parse HEAD)" "origin/$B"
    equal "$("$REAL_GIT" -C "$WT" rev-parse --abbrev-ref '@{u}')" "origin/$B" 'upstream'
    equal "$(lines "$HOOK_LOG")" "$1" 'pre-push hook runs'
}
work() { printf '%s\n' "$1" > "$WT/$1.txt"; }

default_first_push() {
    fixture default-first-push
    setup default
    work change
    run_step step15 step15
    equal "$STEP_RC" 0 'step-15 exit'
    equal "$(field pushed)" true 'step-15 pushed'
    equal "$(field reason)" pushed 'step-15 reason'
    expect_published 1
}
default_no_origin() {
    fixture default-no-origin
    setup default
    "$REAL_GIT" -C "$REPO" remote remove origin
    work change
    run_step step15 step15
    equal "$STEP_RC" 0 'step-15 exit'
    equal "$(field pushed)" false 'step-15 pushed'
    equal "$(field reason)" no-upstream 'step-15 reason'
    equal "$(lines "$PUSH_LOG")" 0 'pushes with no origin'
    equal "$(lines "$HOOK_LOG")" 0 'pre-push hook runs with no origin'
}
consensus_first_push() {
    fixture consensus-first-push
    setup consensus
    work change
    run_step step9 step9
    equal "$STEP_RC" 0 'step9 exit'
    equal "$(field committed)" true 'step9 committed'
    equal "$(field pushed)" true 'step9 pushed'
    equal "$(field push_reason)" success 'step9 push_reason'
    expect_published 1
    # step12 on the branch that now has its upstream takes the existing path.
    work feedback
    run_step step12 step12
    equal "$STEP_RC" 0 'step12 exit'
    equal "$(field updates_pushed)" true 'step12 updates_pushed'
    equal "$(field push_reason)" success 'step12 push_reason'
    expect_published 2
}
consensus_step12_first_push() {
    fixture consensus-step12-first-push
    setup consensus
    work feedback
    run_step step12 step12
    equal "$STEP_RC" 0 'step12 exit'
    equal "$(field updates_pushed)" true 'step12 updates_pushed'
    equal "$(field push_reason)" success 'step12 push_reason'
    expect_published 1
}

PASS=0 FAIL=0 FILTER="${1:-*}"
run_case() {
    local name="$1" rc; shift
    [[ "$FILTER" == '*' || "$name" == "$FILTER" ]] || return 0
    echo "RUN: $name"
    set +e
    (set -e; "$@") > "$TEST_TMP/$name.log" 2>&1
    rc=$?
    set -e
    if [[ "$rc" == 0 ]]; then
        PASS=$((PASS + 1)); echo "PASS: $name"
    else
        FAIL=$((FAIL + 1)); echo "FAIL: $name (exit $rc)" >&2
        cat "$TEST_TMP/$name.log" >&2
        for f in "$TEST_TMP/$name"/*.err; do [[ -f "$f" ]] && { echo "--- $f" >&2; cat "$f" >&2; }; done
    fi
}
run_case default-first-push default_first_push
run_case default-no-origin default_no_origin
run_case consensus-first-push consensus_first_push
run_case consensus-step12-first-push consensus_step12_first_push
echo "Issue #1930 first push: $PASS passed, $FAIL failed"
((PASS + FAIL > 0)) || exit 2
((FAIL == 0))
