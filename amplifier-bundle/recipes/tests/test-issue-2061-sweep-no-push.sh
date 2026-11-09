#!/usr/bin/env bash
# test-issue-2061-sweep-no-push.sh — regression tests for #2061.
#
# The step-04 worktree sweep archived each stale unmerged branch by pushing it
# to origin. Every such push ran the repository's pre-push hook, which in
# amplihack-rs runs the full workspace test suite, so one sweep could cost
# hours. The sweep must now:
#   - record archives as local refs only (refs/amplihack-archive/<branch>) and
#     never contact a remote;
#   - skip the update when the archive ref already holds the SHA, so the reflog
#     does not grow on every run;
#   - hold a non-blocking lock per common git dir, so a second concurrent sweep
#     of the same repository skips instead of repeating the work;
#   - fall back to running unlocked, with a WARN, when the lock cannot be used;
#   - keep the data-loss guard: a failed archive never leads to a prune.
#
# Every clone carries a pre-push hook that records each call. The test first
# proves the hook fires, then asserts the log stays empty in every scenario.
# No sleeps, polling or wall-clock windows: the staleness threshold is 0 and
# the lock is held by this shell on a file descriptor.
#
# Usage: bash amplifier-bundle/recipes/tests/test-issue-2061-sweep-no-push.sh
# Exit codes: 0 = pass, 1 = fail, 2 = test harness error.

set -euo pipefail

# A hook that exports GIT_DIR would otherwise send every write below to the
# real repository (S10).
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_COMMON_DIR GIT_OBJECT_DIRECTORY \
    GIT_ALTERNATE_OBJECT_DIRECTORIES GIT_NAMESPACE GIT_PREFIX GIT_CEILING_DIRECTORIES

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
SWEEP="${REPO_ROOT}/amplifier-bundle/tools/workflow_worktree_sweep.sh"

if [[ ! -f "$SWEEP" ]]; then
    echo "HARNESS-ERROR: sweep helper not found: $SWEEP" >&2
    exit 2
fi

SCRATCH="$(mktemp -d "${TMPDIR:-/tmp}/issue-2061.XXXXXX")"
trap 'rm -rf "$SCRATCH"' EXIT
SCRATCH="$(cd "$SCRATCH" && pwd -P)"
case "$SCRATCH" in
    *"'"*|*'"'*|*'$'*|*'`'*|*'\'*)
        echo "HARNESS-ERROR: scratch path has shell metacharacters: $SCRATCH" >&2
        exit 2
        ;;
esac

# Isolate from the operator's environment (S12, S13). Never touch the real
# ~/.amplihack or ~/.claude.
export HOME="$SCRATCH/home"
mkdir -p "$HOME"
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 GIT_TERMINAL_PROMPT=0
export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@example.invalid
export GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@example.invalid
# The lane exports a ten-year value as a stopgap; this test sets its own.
export AMPLIHACK_WORKTREE_STALE_SECS=0
export AMPLIHACK_WORKTREE_BASE_REF=main
unset AMPLIHACK_WORK_ROOT_EXPLICIT

PASS_COUNT=0
FAIL_COUNT=0
SKIP_COUNT=0
pass() { PASS_COUNT=$((PASS_COUNT + 1)); echo "  PASS[$1]: $2"; }
fail() { FAIL_COUNT=$((FAIL_COUNT + 1)); echo "  FAIL[$1]: $2" >&2; }
skip() { SKIP_COUNT=$((SKIP_COUNT + 1)); echo "  SKIP[$1]: $2"; }
check() { local id="$1" msg="$2"; shift 2; if "$@"; then pass "$id" "$msg"; else fail "$id" "$msg"; fi; }
harness_error() { echo "HARNESS-ERROR: $*" >&2; exit 2; }

not() { ! "$@"; }
eq() { [[ "$1" == "$2" ]]; }
file_empty() { [[ ! -s "$1" ]]; }
file_has() { grep -qF -- "$2" "$1"; }
file_lacks() { ! grep -qF -- "$2" "$1"; }
ref_sha() { git -C "$1" rev-parse --verify --quiet "$2" 2>/dev/null || true; }
reflog_count() { git -C "$1" reflog show --format=%H "$2" 2>/dev/null | wc -l | tr -d ' '; }
registered() { git -C "$1" worktree list --porcelain | grep -qxF "worktree $2"; }
branch_exists() { git -C "$1" rev-parse --verify --quiet "refs/heads/$2" >/dev/null; }

# Globals set by make_fixture.
R="" ORIGIN="" LOG="" WT="" LOCK=""

# make_fixture <name> — bare origin plus a clone at $R on `main`, with a
# recording pre-push hook writing to $LOG, core.logAllRefUpdates=always, and a
# linked worktree $WT on branch feat/a carrying one clean commit that is not on
# main. The hook is proven to fire, then $LOG is truncated.
make_fixture() {
    local name="$1" dir="$SCRATCH/$1"
    mkdir -p "$dir"
    ORIGIN="$dir/origin.git"
    R="$dir/clone"
    LOG="$dir/pre-push.log"
    git init -q --bare -b main "$ORIGIN"
    git init -q -b main "$R"
    echo base >"$R/file.txt"
    git -C "$R" add file.txt
    git -C "$R" commit -q -m base
    git -C "$R" remote add origin "$ORIGIN"
    git -C "$R" push -q -u origin main

    # S11: every write and push below must stay inside the scratch directory.
    local gitdir url
    gitdir="$(git -C "$R" rev-parse --absolute-git-dir)"
    case "$gitdir" in "$SCRATCH"/*) ;; *) harness_error "$name: git dir $gitdir is outside $SCRATCH" ;; esac
    url="$(git -C "$R" config --get remote.origin.url)"
    [[ "$url" == "$ORIGIN" ]] || harness_error "$name: origin URL $url is not the scratch bare repository"
    [[ -z "$(git -C "$R" config --get core.hooksPath || true)" ]] \
        || harness_error "$name: core.hooksPath is set; the recording hook would not run"

    cat >"$gitdir/hooks/pre-push" <<HOOK
#!/bin/sh
printf '%s\n' "\$*" >>'$LOG'
cat >>'$LOG'
exit 0
HOOK
    chmod +x "$gitdir/hooks/pre-push"
    git -C "$R" config core.logAllRefUpdates always

    WT="$R/worktrees/wt-a"
    git -C "$R" worktree add -q -b feat/a "$WT" main
    echo "unique work for $name" >"$WT/feature-$name.txt"
    git -C "$WT" add "feature-$name.txt"
    git -C "$WT" commit -q -m "feat: unique work"

    # Prove the recording hook fires, so an empty log later means something.
    git -C "$R" push -q origin main:refs/heads/hook-self-check
    [[ -s "$LOG" ]] || harness_error "$name: the recording pre-push hook did not fire"
    git -C "$R" push -q origin --delete hook-self-check
    : >"$LOG"

    LOCK="$(cd "$gitdir" && pwd -P)/amplihack-worktree-sweep.lock"
}

# run_sweep <repo> <errfile> — run the sweep as a CLI, stderr to <errfile>,
# without passing this shell's lock descriptor (fd 9) to the child. Echoes rc.
run_sweep() {
    local rc=0
    bash "$SWEEP" sweep "$1" 2>"$2" 9>&- || rc=$?
    printf '%s\n' "$rc"
}

run_cleanup() {
    local rc=0
    bash "$SWEEP" cleanup_on_failure "$1" "$2" 2>"$3" 9>&- || rc=$?
    printf '%s\n' "$rc"
}

ARCHIVE="refs/amplihack-archive/feat/a"

echo "== Static: the script never contacts a remote (S1) =="
check S1-a "no git push, fetch or ls-remote in the script" \
    not grep -nE 'git .*(push|fetch|ls-remote)' "$SWEEP"
check S1-b "no remote URL lookup in the script (S2)" \
    not grep -n 'remote get-url' "$SWEEP"
check S1-c "the header states that the sweep never contacts a remote" \
    grep -q 'never contacts a remote' "$SWEEP"

echo "== Unit: sourcing defines the new helpers and opens no descriptor =="
src_rc=0
(
    pid=$BASHPID
    before="$(ls "/proc/$pid/fd" 2>/dev/null | sort)"
    # shellcheck source=/dev/null
    . "$SWEEP"
    after="$(ls "/proc/$pid/fd" 2>/dev/null | sort)"
    [[ "$before" == "$after" ]] || exit 3
    declare -F common_git_dir >/dev/null || exit 4
    declare -F sweep_locked >/dev/null || exit 5
) || src_rc=$?
check U1-a "sourcing leaves the descriptor table unchanged" not eq "$src_rc" 3
check U1-b "common_git_dir is defined" eq "$src_rc" 0

echo "== Unit: common_git_dir gives one absolute path for a checkout and its worktrees =="
make_fixture unit
want="$(cd "$R/.git" && pwd -P)"
# shellcheck source=/dev/null
cgd_main="$( . "$SWEEP" 2>/dev/null; common_git_dir "$R" 2>/dev/null || true)"
# shellcheck source=/dev/null
cgd_wt="$( . "$SWEEP" 2>/dev/null; common_git_dir "$WT" 2>/dev/null || true)"
cgd_bad_rc=0
# shellcheck source=/dev/null
( . "$SWEEP" 2>/dev/null; common_git_dir "$SCRATCH/home" >/dev/null 2>&1 ) || cgd_bad_rc=$?
check U2-a "main checkout resolves to the canonical common dir" eq "$cgd_main" "$want"
check U2-b "linked worktree resolves to the same path" eq "$cgd_wt" "$want"
check U2-c "a non-repository returns non-zero" not eq "$cgd_bad_rc" 0

echo "== Unit: archive_branch rejects malformed object names (S3) =="
wt_sha="$(ref_sha "$R" refs/heads/feat/a)"
for bad in "abc" "$(printf '%s' "$wt_sha" | tr 'a-f' 'A-F')" "${wt_sha}0" "-d" ""; do
    bad_rc=0
    # shellcheck source=/dev/null
    ( . "$SWEEP"; archive_branch "$R" feat/a "$bad" ) 2>"$SCRATCH/unit-bad.err" 9>&- || bad_rc=$?
    check U3-rc "archive_branch '$bad' returns 1" eq "$bad_rc" 1
    check U3-ref "archive_branch '$bad' writes no ref" eq "$(ref_sha "$R" "$ARCHIVE")" ""
    check U3-msg "archive_branch '$bad' logs the refusal" \
        file_has "$SCRATCH/unit-bad.err" "WARN: Refusing to archive 'feat/a': '$bad' is not a full object name."
done
check U3-push "no push from the malformed-SHA calls" file_empty "$LOG"

echo "== Unit: archive_branch creates, then reports already current =="
u_rc=0
# shellcheck source=/dev/null
( . "$SWEEP"; archive_branch "$R" feat/a "$wt_sha" ) 2>"$SCRATCH/unit-ok.err" 9>&- || u_rc=$?
check U4-a "first call returns 0" eq "$u_rc" 0
check U4-b "first call records the SHA locally" eq "$(ref_sha "$R" "$ARCHIVE")" "$wt_sha"
check U4-c "first call logs the archive" \
    file_has "$SCRATCH/unit-ok.err" "INFO: Archived 'feat/a' (${wt_sha}) to ${ARCHIVE}."
u_rc=0
# shellcheck source=/dev/null
( . "$SWEEP"; archive_branch "$R" feat/a "$wt_sha" ) 2>"$SCRATCH/unit-again.err" 9>&- || u_rc=$?
check U4-d "second call returns 0" eq "$u_rc" 0
check U4-e "second call logs already current" \
    file_has "$SCRATCH/unit-again.err" "INFO: archive ${ARCHIVE} already current at ${wt_sha}"
check U4-f "second call writes no reflog entry" eq "$(reflog_count "$R" "$ARCHIVE")" 1
check U4-g "the reflog entry carries the archive message" \
    grep -q 'amplihack worktree archive' <(git -C "$R" reflog show "$ARCHIVE" 2>/dev/null)
check U4-h "no push from archive_branch" file_empty "$LOG"

echo "== A: first sweep archives locally, keeps the worktree, pushes nothing =="
make_fixture a
sha1="$(ref_sha "$R" refs/heads/feat/a)"
remote_before="$(git -C "$R" ls-remote origin)"
rc="$(run_sweep "$R" "$SCRATCH/a1.err")"
check A1 "sweep exits 0" eq "$rc" 0
check A2 "archive ref holds the branch SHA" eq "$(ref_sha "$R" "$ARCHIVE")" "$sha1"
check A3 "one reflog entry for the archive ref" eq "$(reflog_count "$R" "$ARCHIVE")" 1
check A4 "the pre-push hook never ran" file_empty "$LOG"
check A5 "origin is unchanged" eq "$(git -C "$R" ls-remote origin)" "$remote_before"
check A6 "the unmerged worktree is kept" registered "$R" "$WT"
check A7 "the unmerged branch is kept" branch_exists "$R" feat/a
check A8 "the archive is logged" file_has "$SCRATCH/a1.err" "Archived 'feat/a' (${sha1})"

echo "== B: second sweep finds the archive current and writes nothing =="
rc="$(run_sweep "$R" "$SCRATCH/b1.err")"
check B1 "sweep exits 0" eq "$rc" 0
check B2 "already current is logged" \
    file_has "$SCRATCH/b1.err" "INFO: archive ${ARCHIVE} already current at ${sha1}"
check B3 "reflog still has one entry" eq "$(reflog_count "$R" "$ARCHIVE")" 1
check B4 "the pre-push hook never ran" file_empty "$LOG"
check B5 "origin is unchanged" eq "$(git -C "$R" ls-remote origin)" "$remote_before"

echo "== C: a new commit moves the archive ref =="
echo more >>"$WT/feature-a.txt"
git -C "$WT" commit -q -am "feat: more"
sha2="$(ref_sha "$R" refs/heads/feat/a)"
rc="$(run_sweep "$R" "$SCRATCH/c1.err")"
check C1 "sweep exits 0" eq "$rc" 0
check C2 "archive ref moved to the new SHA" eq "$(ref_sha "$R" "$ARCHIVE")" "$sha2"
check C3 "reflog has two entries" eq "$(reflog_count "$R" "$ARCHIVE")" 2
check C4 "the pre-push hook never ran" file_empty "$LOG"
check C5 "origin is unchanged" eq "$(git -C "$R" ls-remote origin)" "$remote_before"

echo "== D: a branch already on origin is not updated there =="
make_fixture d
git -C "$R" push -q origin feat/a
: >"$LOG"
echo later >"$WT/later.txt"
git -C "$WT" add later.txt
git -C "$WT" commit -q -m "feat: local only"
dsha="$(ref_sha "$R" refs/heads/feat/a)"
remote_before="$(git -C "$R" ls-remote origin)"
rc="$(run_sweep "$R" "$SCRATCH/d1.err")"
check D1 "sweep exits 0" eq "$rc" 0
check D2 "git ls-remote origin is unchanged" eq "$(git -C "$R" ls-remote origin)" "$remote_before"
check D3 "no archive ref on origin" eq "$(git -C "$R" ls-remote origin 'refs/amplihack-archive/*')" ""
check D4 "the pre-push hook never ran" file_empty "$LOG"
check D5 "the archive is local" eq "$(ref_sha "$R" "$ARCHIVE")" "$dsha"

echo "== E: a held lock makes a second sweep skip without acting =="
make_fixture e
# State the sweep would change if it ran: an unarchived branch, a prunable
# worktree registration and a missing info/exclude rule.
git -C "$R" worktree add -q -b gone "$R/worktrees/gone" main
rm -rf "$R/worktrees/gone"
exclude="$R/.git/info/exclude"
mkdir -p "$R/.git/info"
grep -vxF '/worktrees/' "$exclude" >"$SCRATCH/e-exclude" 2>/dev/null || true
cp "$SCRATCH/e-exclude" "$exclude"
refs_before="$(git -C "$R" for-each-ref --format='%(refname) %(objectname)')"
wts_before="$(git -C "$R" worktree list --porcelain)"
exclude_before="$(cat "$exclude")"

exec 9>>"$LOCK"
if ! flock -n 9; then
    harness_error "E: could not take the lock $LOCK from the test shell"
fi
rc="$(run_sweep "$R" "$SCRATCH/e1.err")"
check E1 "sweep exits 0 while the lock is held" eq "$rc" 0
check E2 "the skip names the absolute lock path" \
    file_has "$SCRATCH/e1.err" "INFO: sweep skipped: lock held: ${LOCK}"
check E3 "no ref was created or moved" \
    eq "$(git -C "$R" for-each-ref --format='%(refname) %(objectname)')" "$refs_before"
check E4 "worktree list unchanged (no prune ran)" eq "$(git -C "$R" worktree list --porcelain)" "$wts_before"
check E5 "info/exclude unchanged" eq "$(cat "$exclude")" "$exclude_before"
check E6 "nothing else was logged" eq "$(grep -cv 'sweep skipped' "$SCRATCH/e1.err" || true)" 0
rc="$(run_sweep "$WT" "$SCRATCH/e2.err")"
check E7 "a linked worktree path exits 0" eq "$rc" 0
check E8 "a linked worktree path resolves to the same lock" \
    file_has "$SCRATCH/e2.err" "INFO: sweep skipped: lock held: ${LOCK}"
check E9 "the pre-push hook never ran" file_empty "$LOG"
flock -u 9
exec 9>&-

rc="$(run_sweep "$R" "$SCRATCH/e3.err")"
check E10 "after release the sweep runs and exits 0" eq "$rc" 0
check E11 "after release there is no skip message" file_lacks "$SCRATCH/e3.err" "sweep skipped"
check E12 "after release the archive is created" \
    eq "$(ref_sha "$R" "$ARCHIVE")" "$(ref_sha "$R" refs/heads/feat/a)"
check E13 "after release the stale registration is pruned" not registered "$R" "$R/worktrees/gone"
check E14 "after release /worktrees/ is excluded again" grep -qxF '/worktrees/' "$exclude"
check E15 "the pre-push hook never ran" file_empty "$LOG"
rc="$(run_sweep "$R" "$SCRATCH/e4.err")"
check E16 "a later sweep is not blocked by a leftover lock file" file_lacks "$SCRATCH/e4.err" "sweep skipped"

echo "== F: a lock path that cannot be opened falls back to running unlocked =="
make_fixture f
mkdir -p "$LOCK"
rc="$(run_sweep "$R" "$SCRATCH/f1.err")"
check F1 "sweep exits 0" eq "$rc" 0
check F2 "the fallback is logged" \
    file_has "$SCRATCH/f1.err" "WARN: cannot open lock ${LOCK}; running sweep without a lock."
check F3 "the archive is still created" eq "$(ref_sha "$R" "$ARCHIVE")" "$(ref_sha "$R" refs/heads/feat/a)"
check F4 "the pre-push hook never ran" file_empty "$LOG"

echo "== Symlink: a symlinked lock path is refused and its target untouched (S8) =="
make_fixture s
target="$SCRATCH/s/lock-target"
printf 'keep\n' >"$target"
ln -s "$target" "$LOCK"
rc="$(run_sweep "$R" "$SCRATCH/s1.err")"
check S8-a "sweep exits 0" eq "$rc" 0
check S8-b "the symlink is refused" \
    file_has "$SCRATCH/s1.err" "WARN: lock path ${LOCK} is a symlink; running sweep without a lock."
check S8-c "the target's content is unchanged" eq "$(cat "$target")" "keep"
check S8-d "the archive is still created" eq "$(ref_sha "$R" "$ARCHIVE")" "$(ref_sha "$R" refs/heads/feat/a)"
check S8-e "the pre-push hook never ran" file_empty "$LOG"

echo "== G: with no flock on PATH the sweep warns and runs unlocked =="
make_fixture g
SHIM="$SCRATCH/g/shim"
mkdir -p "$SHIM"
shim_ok=1
for tool in bash git stat date grep mkdir tail awk dirname cat rm; do
    p="$(command -v "$tool" 2>/dev/null || true)"
    case "$p" in /*) ln -s "$p" "$SHIM/$tool" ;; *) shim_ok=0 ;; esac
done
for tool in sh sed tr wc head sort cut basename readlink realpath env ls touch mktemp printf test id uname; do
    p="$(command -v "$tool" 2>/dev/null || true)"
    case "$p" in /*) ln -s "$p" "$SHIM/$tool" ;; esac
done
if [[ "$shim_ok" -eq 1 ]] && PATH="$SHIM" "$SHIM/git" --version >/dev/null 2>&1 \
    && ! PATH="$SHIM" "$SHIM/bash" -c 'command -v flock' >/dev/null 2>&1; then
    g_rc=0
    PATH="$SHIM" "$SHIM/bash" "$SWEEP" sweep "$R" 2>"$SCRATCH/g1.err" 9>&- || g_rc=$?
    check G1 "sweep exits 0" eq "$g_rc" 0
    check G2 "the missing flock is logged" \
        file_has "$SCRATCH/g1.err" "WARN: flock not found; running sweep without a lock."
    check G3 "the archive is still created" eq "$(ref_sha "$R" "$ARCHIVE")" "$(ref_sha "$R" refs/heads/feat/a)"
    check G4 "the pre-push hook never ran" file_empty "$LOG"
elif [[ "${CI:-}" == "true" ]]; then
    fail "G-shim" "could not build a PATH shim without flock; scenario G must run under CI"
else
    skip G "could not build a PATH shim without flock; fallback covered by inspection"
fi

echo "== H: cleanup_on_failure archives an unmerged branch locally and pushes nothing =="
make_fixture h
hsha="$(ref_sha "$R" refs/heads/feat/a)"
remote_before="$(git -C "$R" ls-remote origin)"
rc="$(run_cleanup "$R" feat/a "$SCRATCH/h1.err")"
check H1 "cleanup_on_failure exits 0" eq "$rc" 0
check H2 "the archive ref holds the branch SHA" eq "$(ref_sha "$R" "$ARCHIVE")" "$hsha"
check H3 "the pre-push hook never ran" file_empty "$LOG"
check H4 "origin is unchanged" eq "$(git -C "$R" ls-remote origin)" "$remote_before"
check H5 "cleanup_on_failure takes no sweep lock" not test -e "$LOCK"

echo "== I: a failed archive keeps the worktree and blocks cleanup (S5, data-loss guard) =="
make_fixture i
# refs/amplihack-archive/feat as a ref makes refs/amplihack-archive/feat/a
# impossible (directory/file conflict).
git -C "$R" update-ref refs/amplihack-archive/feat "$(ref_sha "$R" refs/heads/main)"
isha="$(ref_sha "$R" refs/heads/feat/a)"
rc="$(run_sweep "$R" "$SCRATCH/i1.err")"
check I1 "sweep exits 0" eq "$rc" 0
check I2 "the failed archive is logged" \
    file_has "$SCRATCH/i1.err" "WARN: Could not archive 'feat/a' to ${ARCHIVE} locally."
check I3 "the worktree is kept" registered "$R" "$WT"
check I4 "the pre-push hook never ran" file_empty "$LOG"
rc="$(run_cleanup "$R" feat/a "$SCRATCH/i2.err")"
check I5 "cleanup_on_failure exits 0" eq "$rc" 0
check I6 "cleanup_on_failure refuses to prune" file_has "$SCRATCH/i2.err" "refusing to prune (data-loss guard)"
check I7 "the worktree survives cleanup" registered "$R" "$WT"
check I8 "the branch survives cleanup at its SHA" eq "$(ref_sha "$R" refs/heads/feat/a)" "$isha"
check I9 "the pre-push hook never ran" file_empty "$LOG"

echo "== J: an archive is never moved to a commit that does not contain it =="
make_fixture j
old_sha="$(ref_sha "$R" refs/heads/feat/a)"
rc="$(run_sweep "$R" "$SCRATCH/j1.err")"
check J1 "first sweep archives the branch" eq "$(ref_sha "$R" "$ARCHIVE")" "$old_sha"
# Rewrite the branch: the archived commit is no longer on it.
git -C "$WT" reset -q --hard main
echo "rewritten" >"$WT/rewritten.txt"
git -C "$WT" add rewritten.txt
git -C "$WT" commit -q -m "feat: rewritten"
rc="$(run_sweep "$R" "$SCRATCH/j2.err")"
check J2 "sweep exits 0" eq "$rc" 0
check J3 "the archive still holds the old commit" eq "$(ref_sha "$R" "$ARCHIVE")" "$old_sha"
check J4 "the refusal is logged" file_has "$SCRATCH/j2.err" "WARN: Keeping ${ARCHIVE} at ${old_sha}"
check J5 "the rewritten worktree is kept" registered "$R" "$WT"
rc="$(run_cleanup "$R" feat/a "$SCRATCH/j3.err")"
check J6 "cleanup_on_failure refuses to prune" file_has "$SCRATCH/j3.err" "refusing to prune (data-loss guard)"
check J7 "the rewritten branch survives cleanup" branch_exists "$R" feat/a
check J8 "the pre-push hook never ran" file_empty "$LOG"

echo "== K: a flock error other than contention falls back to running unlocked =="
make_fixture k
KSHIM="$SCRATCH/k/shim"
mkdir -p "$KSHIM"
printf '#!/bin/sh\necho "flock: simulated ENOLCK" >&2\nexit 69\n' >"$KSHIM/flock"
chmod +x "$KSHIM/flock"
k_rc=0
PATH="$KSHIM:$PATH" bash "$SWEEP" sweep "$R" 2>"$SCRATCH/k1.err" 9>&- || k_rc=$?
check K1 "sweep exits 0" eq "$k_rc" 0
check K2 "the lock error is logged as a fallback, not a skip" \
    file_has "$SCRATCH/k1.err" "WARN: cannot lock ${LOCK} (flock exit 69); running sweep without a lock."
check K3 "no skip message" file_lacks "$SCRATCH/k1.err" "sweep skipped"
check K4 "the archive is still created" eq "$(ref_sha "$R" "$ARCHIVE")" "$(ref_sha "$R" refs/heads/feat/a)"
check K5 "the pre-push hook never ran" file_empty "$LOG"

echo
echo "Result: ${PASS_COUNT} passed, ${FAIL_COUNT} failed, ${SKIP_COUNT} skipped"
if [[ "$FAIL_COUNT" -gt 0 ]]; then
    exit 1
fi
exit 0
