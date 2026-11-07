#!/usr/bin/env bash
# Issue #1930: setup and orphan archival are local operations, with no pushes.
# Run actual recipe bodies against disposable repositories, never host worktrees.
# Optional argument selects one named case, useful for mutant checks.
set -euo pipefail
if [[ ${ISSUE_1930_ISOLATED:-0} != 1 ]]; then
    exec env -i PATH=/usr/bin:/bin TMPDIR="${TMPDIR:-/tmp}" \
        ISSUE_1930_ISOLATED=1 ISSUE_1930_ARTIFACT_DIR="${ISSUE_1930_ARTIFACT_DIR:-}" bash "$0" "$@"
fi
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
SOURCE="$(cd "$SCRIPT_DIR/../../.." && pwd -P)"
# Optional owned evidence directory keeps fixture commands, hooks and outputs.
TEST_TMP="$(mktemp -d "${ISSUE_1930_ARTIFACT_DIR:-${TMPDIR:-/tmp}}/issue-1930.XXXXXX")"
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
TOOLS="$TEST_TMP/bundle/amplifier-bundle/tools"
for name in workflow_worktree_sweep workflow_pr_branch workflow_caller_branch_alias workflow_alias_reuse; do
    cp "$SOURCE/amplifier-bundle/tools/$name.sh" "$TOOLS/"
done
export PATH="$TEST_TMP/bin:$PATH"
# Resolve the command after Git's global options; matching arbitrary argv words
# would confuse a ref/path containing "push" with a real publication attempt.
cat > "$TEST_TMP/bin/git" <<'GIT'
#!/usr/bin/env bash
set -euo pipefail
args=("$@"); repo="$PWD"
while (($#)); do
    case "$1" in
        -C) repo="$(cd "$repo" && cd "$2" && pwd -P)"; shift 2 ;;
        -c|--git-dir|--work-tree|--namespace) shift 2 ;;
        --git-dir=*|--work-tree=*|--no-optional-locks|--no-pager|--literal-pathspecs) shift ;;
        *) break ;;
    esac
done
case "$repo" in "$TEST_TMP"/*) ;; *) echo 'fixture escape' >&2; exit 97 ;; esac
printf '%q ' "${args[@]}" >> "$GIT_LOG"; printf '\n' >> "$GIT_LOG"
if [[ ${1:-} == push ]]; then
    printf '%q ' "${args[@]}" >> "$PUSH_LOG"; printf '\n' >> "$PUSH_LOG"
fi
if [[ ${FAIL_ARCHIVE:-0} == 1 && ${1:-} == update-ref && ${2:-} == refs/amplihack-archive/* ]]; then
    echo 'injected archive write failure' >&2; exit 73
fi
if [[ ${FAIL_STATUS:-0} == 1 && ${1:-} == status ]]; then exit 74; fi
exec "$REAL_GIT" "${args[@]}"
GIT
cat > "$TEST_TMP/bin/gh" <<'GH'
#!/usr/bin/env bash
set -euo pipefail
[[ "$*" == 'pr view 1930 --json headRefName -q .headRefName' ]] || exit 98
printf '%s\n' "$PR_TARGET"
GH
chmod +x "$TEST_TMP/bin/"{git,gh}
# Extract the executable command, or the consensus agent's first bash fence.
awk '/^    command: \|/ {body=1;next} body && /^    [A-Za-z_-]+:/ {exit}
     body {sub(/^      /, ""); print}' \
    "$SOURCE/amplifier-bundle/recipes/workflow-worktree.yaml" > "$TEST_TMP/default.sh"
awk '/^      ```bash/ {body=1;next} body && /^      ```/ {exit}
     body {sub(/^      /, ""); print}' \
    "$SOURCE/amplifier-bundle/recipes/consensus-issue-worktree.yaml" > "$TEST_TMP/consensus.sh"
[[ -s "$TEST_TMP/default.sh" && -s "$TEST_TMP/consensus.sh" ]]
bash -n "$TEST_TMP/default.sh" "$TEST_TMP/consensus.sh"

die() { echo "ASSERTION FAILED: $*" >&2; return 1; }
equal() { [[ "$1" == "$2" ]] || die "$3: expected '$2', got '$1'"; }
no_push() {
    if [[ -s "$PUSH_LOG" || -s "$HOOK_LOG" ]]; then
        cat "$PUSH_LOG" "$HOOK_LOG" >&2
        die 'setup/sweep attempted git push or invoked pre-push hook'
    fi
}
remote_refs() { "$REAL_GIT" --git-dir="$REMOTE" for-each-ref --format='%(refname) %(objectname)'; }
fixture() {
    CASE_ROOT="$TEST_TMP/$1"; mkdir -p "$CASE_ROOT"
    export GIT_LOG="$CASE_ROOT/git.log" PUSH_LOG="$CASE_ROOT/push.log" HOOK_LOG="$CASE_ROOT/hook.log"
    : > "$GIT_LOG"; : > "$PUSH_LOG"; : > "$HOOK_LOG"
    REPO="$CASE_ROOT/repo"; REMOTE="$CASE_ROOT/remote.git"
    "$REAL_GIT" init -q --bare "$REMOTE"
    "$REAL_GIT" init -q -b main "$REPO"
    # Same synthetic fixture identity as the registered #840 test helper.
    "$REAL_GIT" -C "$REPO" config user.name Test
    "$REAL_GIT" -C "$REPO" config user.email t@example.com
    printf 'base\n' > "$REPO/base.txt"
    "$REAL_GIT" -C "$REPO" add base.txt
    "$REAL_GIT" -C "$REPO" commit -qm base
    "$REAL_GIT" -C "$REPO" remote add origin "$REMOTE"
    "$REAL_GIT" -C "$REPO" push -qu origin main
    "$REAL_GIT" --git-dir="$REMOTE" symbolic-ref HEAD refs/heads/main
    "$REAL_GIT" -C "$REPO" remote set-head origin main
    BASE="$("$REAL_GIT" -C "$REPO" rev-parse HEAD)"
    cat > "$REPO/.git/hooks/pre-push" <<'HOOK'
#!/usr/bin/env bash
printf 'rejecting-pre-push\n' >> "${HOOK_LOG:?}"
echo 'ISSUE-1930 rejecting hook reached' >&2
exit 42
HOOK
    chmod +x "$REPO/.git/hooks/pre-push"
    export AMPLIHACK_HOME="$TEST_TMP/bundle" AMPLIHACK_RUNTIME_ROOT="$CASE_ROOT/runtime"
    export AMPLIHACK_STATE_DIR="$CASE_ROOT/state" AMPLIHACK_WORKTREE_BASE_REF=origin/main
    export REPO_PATH="$REPO" WORKTREE_DIR='' EXISTING_BRANCH='' PR_NUMBER=''
    export TASK_DESCRIPTION='no setup push' ISSUE_NUMBER=1930 BRANCH_PREFIX=fix
}
target_branch() {
    TARGET=feat/resume
    "$REAL_GIT" -C "$REPO" branch --no-track "$TARGET" main
}
remote_target() {
    "$REAL_GIT" --git-dir="$REMOTE" update-ref "refs/heads/$TARGET" "$BASE"
    "$REAL_GIT" -C "$REPO" fetch -q origin
}
unique_commit() {
    printf 'unique recovery work\n' > "$1/unique.txt"
    "$REAL_GIT" -C "$1" add unique.txt
    "$REAL_GIT" -C "$1" commit -qm unique
}
run_setup() {
    local recipe="$1"
    (cd "$REPO" && bash -euo pipefail "$TEST_TMP/$recipe.sh") \
        > "$CASE_ROOT/setup.json" 2> "$CASE_ROOT/setup.err" \
        || { cat "$CASE_ROOT/setup.err" >&2; die "$recipe setup did not finish"; }
    jq -e 'type == "object" and (.branch_name | type == "string") and
        (.worktree_path | type == "string")' "$CASE_ROOT/setup.json" >/dev/null
    WT="$(jq -r .worktree_path "$CASE_ROOT/setup.json")"
    [[ "$WT" == "$REPO/worktrees/"* && -d "$WT" ]] || die 'invalid task worktree'
    if [[ "$recipe" == default ]]; then
        jq -e '.created | type == "boolean"' "$CASE_ROOT/setup.json" >/dev/null
    else
        jq -e '.setup_complete == true and (.commands_executed | type == "array")' \
            "$CASE_ROOT/setup.json" >/dev/null
    fi
}
hook_control() {
    fixture hook-control
    local rc=0 before
    before="$(remote_refs)"
    "$REAL_GIT" -C "$REPO" push origin HEAD:refs/heads/probe \
        > "$CASE_ROOT/control.out" 2> "$CASE_ROOT/control.err" || rc=$?
    [[ "$rc" -ne 0 ]] || die 'rejecting hook allowed deliberate push'
    grep -qx 'rejecting-pre-push' "$HOOK_LOG"
    grep -q 'ISSUE-1930 rejecting hook reached' "$CASE_ROOT/control.err"
    echo "Deliberate real-Git push exit=$rc; rejecting hook invocations=$(wc -l < "$HOOK_LOG")"
    equal "$(remote_refs)" "$before" 'rejected push remote refs'
}
setup_case() {
    local recipe="$1" state="$2" expected upstream before
    fixture "$recipe-$state"
    TARGET=feat/resume
    if [[ "$state" != new ]]; then
        [[ "$state" == remote-only ]] || target_branch
        [[ "$state" == local-only ]] || remote_target
        if [[ "$state" == ahead || "$state" == pr ]]; then
            "$REAL_GIT" -C "$REPO" checkout -q "$TARGET"
            unique_commit "$REPO"
            "$REAL_GIT" -C "$REPO" checkout -q main
            "$REAL_GIT" -C "$REPO" branch --set-upstream-to="origin/$TARGET" "$TARGET" >/dev/null
        fi
        expected="$("$REAL_GIT" -C "$REPO" rev-parse --verify --quiet "${TARGET}^{commit}" 2>/dev/null || printf '%s' "$BASE")"
        export EXISTING_BRANCH="$TARGET"
        if [[ "$state" == pr ]]; then
            export EXISTING_BRANCH='' PR_NUMBER=1930 PR_TARGET="$TARGET"
        fi
    else
        expected="$BASE"
    fi
    if [[ "$state" == orphan ]]; then
        local orphan="$REPO/worktrees/feat/stale-orphan"
        "$REAL_GIT" -C "$REPO" worktree add -qb feat/stale-orphan "$orphan" main
        unique_commit "$orphan"
        export AMPLIHACK_WORKTREE_STALE_SECS=0
    fi
    before="$(remote_refs)"
    run_setup "$recipe"
    echo "Setup exited 0; pushes=$(wc -l < "$PUSH_LOG"); hooks=$(wc -l < "$HOOK_LOG")"
    no_push
    equal "$(remote_refs)" "$before" 'setup changed remote refs'
    [[ "$state" != new ]] || TARGET="$(jq -r .branch_name "$CASE_ROOT/setup.json")"
    equal "$(jq -r .branch_name "$CASE_ROOT/setup.json")" "$TARGET" 'logical branch'
    equal "$("$REAL_GIT" -C "$WT" rev-parse HEAD)" "$expected" 'preserved tip'
    upstream="$("$REAL_GIT" -C "$WT" rev-parse --abbrev-ref '@{u}' 2>/dev/null || true)"
    if [[ "$state" != local-only && "$state" != new ]]; then
        equal "$upstream" "origin/$TARGET" 'matching upstream'
    elif [[ "$state" == local-only ]]; then
        equal "$upstream" '' 'local-only upstream'
    else
        # Creating from a remote base may inherit its tracking; never invent an
        # origin/task ref that is absent. Publication must handle base tracking.
        [[ "$upstream" != "origin/$TARGET" ]] || die 'fabricated task upstream'
    fi
    equal "$("$REAL_GIT" -C "$REPO" rev-parse main)" "$BASE" 'base branch'
    if [[ "$state" == orphan ]]; then
        equal "$("$REAL_GIT" -C "$REPO" rev-parse refs/amplihack-archive/feat/stale-orphan)" \
            "$("$REAL_GIT" -C "$orphan" rev-parse HEAD)" 'setup sweep recovery ref'
    fi
}
alias_case() {
    fixture default-alias
    target_branch; remote_target
    "$REAL_GIT" -C "$REPO" checkout -q "$TARGET"
    export EXISTING_BRANCH="$TARGET"
    run_setup default
    local first="$WT" alias
    alias="$(jq -r .local_branch "$CASE_ROOT/setup.json")"
    equal "$alias" "$TARGET--wt-1" 'caller alias'
    equal "$("$REAL_GIT" -C "$WT" rev-parse --abbrev-ref '@{u}')" "origin/$TARGET" 'alias upstream'
    run_setup default
    equal "$WT" "$first" 'alias reuse'
    no_push
}
consensus_refusal() {
    fixture consensus-refusal
    target_branch; remote_target
    "$REAL_GIT" -C "$REPO" checkout -q "$TARGET"
    export EXISTING_BRANCH="$TARGET"
    local rc=0
    (cd "$REPO" && bash -euo pipefail "$TEST_TMP/consensus.sh") \
        > "$CASE_ROOT/refusal.out" 2> "$CASE_ROOT/refusal.err" || rc=$?
    [[ "$rc" -ne 0 ]] || die 'consensus accepted caller checkout'
    grep -q 'refusing to use the caller checkout' "$CASE_ROOT/refusal.err"
    no_push
}
sweep_case() {
    local mode="$1" presence="$2" state="$3" before tip rc=0
    fixture "$mode-$presence-$state"
    TARGET=feat/orphan
    WT="$REPO/worktrees/$TARGET"
    "$REAL_GIT" -C "$REPO" worktree add -qb "$TARGET" "$WT" main
    [[ "$presence" != present ]] || remote_target
    unique_commit "$WT"
    tip="$("$REAL_GIT" -C "$WT" rev-parse HEAD)"
    case "$state" in
        dirty) printf 'dirty\n' >> "$WT/unique.txt" ;;
        untracked) printf 'untracked\n' > "$WT/untracked.txt" ;;
        archive-failure) export FAIL_ARCHIVE=1 ;;
        unknown-status) export FAIL_STATUS=1 ;;
        unknown-base)
            "$REAL_GIT" -C "$REPO" checkout -q --detach main
            "$REAL_GIT" -C "$REPO" branch -D main >/dev/null
            "$REAL_GIT" -C "$REPO" update-ref -d refs/remotes/origin/main
            "$REAL_GIT" -C "$REPO" symbolic-ref --delete refs/remotes/origin/HEAD
            ;;
    esac
    before="$(remote_refs)"
    export AMPLIHACK_WORKTREE_STALE_SECS=0
    [[ "$state" != fresh ]] || export AMPLIHACK_WORKTREE_STALE_SECS=86400
    bash "$TOOLS/workflow_worktree_sweep.sh" "$mode" "$REPO" "$TARGET" \
        > "$CASE_ROOT/sweep.out" 2> "$CASE_ROOT/sweep.err" || rc=$?
    equal "$rc" 0 'best-effort sweep exit'
    no_push
    equal "$(remote_refs)" "$before" 'archive changed remote refs'
    if [[ "$state" == archive-failure ]]; then
        grep -q 'Could not archive' "$CASE_ROOT/sweep.err"
        ! "$REAL_GIT" -C "$REPO" show-ref --verify --quiet "refs/amplihack-archive/$TARGET"
    elif [[ "$state" == clean || "$mode" == cleanup_on_failure ]]; then
        equal "$("$REAL_GIT" -C "$REPO" rev-parse "refs/amplihack-archive/$TARGET")" "$tip" 'reachable recovery ref'
    else
        ! "$REAL_GIT" -C "$REPO" show-ref --verify --quiet "refs/amplihack-archive/$TARGET"
    fi
    if [[ "$mode" == cleanup_on_failure && "$state" == clean ]]; then
        [[ ! -e "$WT" ]] || die 'failure cleanup left worktree'
        ! "$REAL_GIT" -C "$REPO" show-ref --verify --quiet "refs/heads/$TARGET"
    else
        [[ -d "$WT" ]] || die 'recoverable worktree removed'
        equal "$("$REAL_GIT" -C "$REPO" rev-parse "refs/heads/$TARGET")" "$tip" 'preserved branch'
        [[ "$state" != dirty ]] || grep -q dirty "$WT/unique.txt"
        [[ "$state" != untracked ]] || grep -q untracked "$WT/untracked.txt"
    fi
}
containment_case() {
    fixture sweep-containment
    "$REAL_GIT" -C "$REPO" worktree add -qb feat/outside "$CASE_ROOT/outside" main
    "$REAL_GIT" -C "$REPO" worktree add -qb feat/clean "$REPO/worktrees/feat/clean" main
    "$REAL_GIT" -C "$REPO" worktree add -q --detach "$REPO/worktrees/detached" main
    mkdir -p "$REPO/worktrees/unregistered"
    printf 'keep\n' > "$REPO/worktrees/unregistered/sentinel"
    AMPLIHACK_WORKTREE_STALE_SECS=0 bash "$TOOLS/workflow_worktree_sweep.sh" sweep "$REPO"
    [[ -d "$CASE_ROOT/outside" && -d "$REPO/worktrees/detached" ]]
    [[ -f "$REPO/worktrees/unregistered/sentinel" && ! -e "$REPO/worktrees/feat/clean" ]]
    ! "$REAL_GIT" -C "$REPO" show-ref --verify --quiet refs/heads/feat/clean
    no_push
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
        [[ "$name" != hook-control ]] || cat "$TEST_TMP/$name.log"
    else
        FAIL=$((FAIL + 1)); echo "FAIL: $name (exit $rc)" >&2
        cat "$TEST_TMP/$name.log" >&2
    fi
}
run_case hook-control hook_control
# Ahead default goes first: the original issue's fail-first reproduction.
for recipe in default consensus; do
    for state in ahead existing local-only remote-only new pr; do
        run_case "$recipe-$state" setup_case "$recipe" "$state"
    done
done
run_case default-orphan setup_case default orphan
run_case default-alias alias_case
run_case consensus-refusal consensus_refusal
for mode in sweep cleanup_on_failure; do
    for presence in present absent; do
        run_case "$mode-$presence-clean" sweep_case "$mode" "$presence" clean
    done
    for state in archive-failure dirty untracked unknown-status; do
        run_case "$mode-absent-$state" sweep_case "$mode" absent "$state"
    done
done
run_case sweep-absent-unknown-base sweep_case sweep absent unknown-base
run_case sweep-absent-fresh sweep_case sweep absent fresh
run_case sweep-containment containment_case
echo "Issue #1930: $PASS passed, $FAIL failed"
((PASS + FAIL > 0)) || exit 2
((FAIL == 0))
