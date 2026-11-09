#!/usr/bin/env bash
# test-issue-1957-explicit-worktree-source.sh — issue #1957, explicit worktree source.
#
# A caller can name the commit a task worktree starts from with two recipe
# inputs, `base_branch` and `base_sha`. The one owner of that decision is
# amplifier-bundle/tools/workflow_worktree_source.sh (contract in
# amplifier-bundle/tools/workflow_worktree_source.md):
#
#   resolve --recipe default|consensus --repo <abs> [--base-branch B] [--base-sha S]
#       stdout: base_ref, base_branch, base_sha, source_verified, source_rule,
#               origin_fingerprint (six key=value lines, in that order)
#       stderr: exactly one `RULE: <rule>` line; on refusal one ERROR, last.
#       exit:   0 ok, 1 refused (nothing created), 2 usage.
#   verify-worktree <WT> <SHA> created|reused <MAIN> <fingerprint> [--branch b]
#   verified-base   exit 0 prints <b> (source_verified is exactly `verified` and
#                   refs/remotes/origin/<b> resolves); exit 1 one ERROR line
#                   (verified but the name is invalid, the ref is missing, or
#                   the context cannot be read); exit 3 no verified base.
#   requested-inputs  exit 0 base_branch or base_sha was given; exit 3 neither,
#                   or worktree_setup.source_verified is exactly not-applicable;
#                   exit 1 the context cannot be read (a missing helper counts
#                   as "cannot read"). Prints nothing on stdout.
#
# Decisions under test: D1 any input plus a failed fetch refuses
# (refuse-fetch-failed; no offline rule); D2 `--strict` makes one fetch, no
# sleep, and returns git's status; D3 the PR tools refuse when inputs were
# given and no verified base exists; D4 a SHA alone needs the object after a
# successful fetch, and cannot publish a PR because it names no branch.
#
# Parts:
#   A  static checks on the recipes and tools
#   F  workflow_fetch_origin.sh (C1), including the D2 single strict attempt
#   T  resolve (C2), including the D1 fetch-failure table (case d) and hostile input
#   V  verify-worktree (C3)
#   B  verified-base and requested-inputs (C4)
#   P  the three real PR tools under their own set -euo pipefail (D3, D4)
#   R  the real step-04 (workflow-worktree.yaml) and step3
#      (consensus-issue-worktree.yaml) bodies, extracted with awk, cases a–h,
#      the D8 rerun rules and an old bundle with and without inputs.
#
# Isolation: scratch under $TMPDIR (or /tmp), scratch HOME, GIT_CONFIG_GLOBAL=
# /dev/null, GIT_CONFIG_NOSYSTEM=1, GIT_TERMINAL_PROMPT=0, GIT_ASKPASS=
# /bin/false, `env -i` for every step and tool run, the #1937 GIT_* unset
# block, and a guard that refuses any origin outside the scratch dir. Origins
# are local bare repositories; nothing touches the network. Assertions are on
# SHAs, exit codes and fixed message text; no URL outside scratch is printed.
#
# Usage: bash amplifier-bundle/recipes/tests/test-issue-1957-explicit-worktree-source.sh
# Exit codes: 0 = pass, 1 = fail, 2 = test harness error.

set -uo pipefail

# Fixture repository selection must not inherit an enclosing Git hook (#1937).
unset GIT_ALTERNATE_OBJECT_DIRECTORIES GIT_CEILING_DIRECTORIES GIT_COMMON_DIR \
    GIT_DIR GIT_DISCOVERY_ACROSS_FILESYSTEM GIT_INDEX_FILE GIT_INDEX_VERSION \
    GIT_NAMESPACE GIT_OBJECT_DIRECTORY GIT_PREFIX GIT_QUARANTINE_PATH GIT_WORK_TREE \
    GIT_CONFIG_PARAMETERS GIT_CONFIG_COUNT
while IFS= read -r git_variable; do
    case "${git_variable}" in GIT_CONFIG_KEY_*|GIT_CONFIG_VALUE_*) unset "${git_variable}" ;; esac
done < <(compgen -e)
unset git_variable BASH_ENV

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
TOOLS="${REPO_ROOT}/amplifier-bundle/tools"
SRC="${TOOLS}/workflow_worktree_source.sh"
FETCHER="${TOOLS}/workflow_fetch_origin.sh"
DEFAULT_YAML="${REPO_ROOT}/amplifier-bundle/recipes/workflow-worktree.yaml"
CONSENSUS_YAML="${REPO_ROOT}/amplifier-bundle/recipes/consensus-issue-worktree.yaml"

PASS_COUNT=0
FAIL_COUNT=0
pass() { PASS_COUNT=$((PASS_COUNT + 1)); echo "  PASS[$1]: $2"; }
fail() { FAIL_COUNT=$((FAIL_COUNT + 1)); echo "  FAIL[$1]: $2"; }
check() { local id="$1" msg="$2"; shift 2; if "$@"; then pass "$id" "$msg"; else fail "$id" "$msg"; fi; }
harness() { echo "HARNESS-ERROR: $*" >&2; exit 2; }

for f in "$FETCHER" "$DEFAULT_YAML" "$CONSENSUS_YAML" "${TOOLS}/workflow_worktree_deconflict.sh"; do
    [[ -f "$f" ]] || harness "required file not found: $f"
done
command -v python3 >/dev/null 2>&1 || harness "python3 is required"

# --- Hermetic scratch ---------------------------------------------------------
SCRATCH_PARENT="${TMPDIR:-/tmp}"
[[ "${SCRATCH_PARENT}" == /* && -d "${SCRATCH_PARENT}" ]] || harness "invalid scratch parent"
SCRATCH_PARENT="$(cd -- "${SCRATCH_PARENT:?}" && pwd -P)" || exit 2
[[ "${SCRATCH_PARENT}" != / ]] || harness "scratch parent is /"
SCRATCH="$(mktemp -d "${SCRATCH_PARENT:?}/i1957.XXXXXX")" || harness "mktemp failed"
: "${SCRATCH:?}"
case "${SCRATCH}" in "${SCRATCH_PARENT:?}/i1957."?*) ;; *) harness "invalid scratch allocation" ;; esac
[[ -d "${SCRATCH}" && ! -L "${SCRATCH}" && -O "${SCRATCH}" ]] || harness "scratch is not an owned directory"
cleanup() { case "${SCRATCH:-}" in "${SCRATCH_PARENT:?}"/i1957.?*) rm -rf -- "${SCRATCH}" ;; esac; }
trap cleanup EXIT
mkdir -p "${SCRATCH}/home" "${SCRATCH}/tmp" "${SCRATCH}/runtime"
export HOME="${SCRATCH}/home" XDG_CONFIG_HOME="${SCRATCH}/home/.config"
for _d in "${HOME}/.amplihack" "${HOME}/.copilot" "${HOME}/.claude"; do
    [[ ! -e "${_d}" ]] || harness "scratch HOME must not contain ${_d##*/}"
done
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 GIT_TERMINAL_PROMPT=0 GIT_ASKPASS=/bin/false
export GIT_ALLOW_PROTOCOL=file
export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@example.invalid
export GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@example.invalid
unset AMPLIHACK_WORK_ROOT AMPLIHACK_WORK_ROOT_EXPLICIT AMPLIHACK_RUN_DIR AMPLIHACK_RUN_ID \
    AMPLIHACK_CONTEXT_FILE GIT_TRACE GIT_TRACE_PACKET GIT_TRACE_CURL GIT_CURL_VERBOSE

# Everything a step or tool sees. `env -i` drops the rest of the caller's env.
BASE_ENV=(
    PATH="${PATH}" HOME="${HOME}" TMPDIR="${SCRATCH}/tmp" LC_ALL=C
    GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 GIT_TERMINAL_PROMPT=0
    GIT_ASKPASS=/bin/false GIT_ALLOW_PROTOCOL=file
    GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@example.invalid
    GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@example.invalid
)

# --- Fixtures -----------------------------------------------------------------
# commit_in <dir> <text>: one commit that writes <text> into its own file.
commit_in() {
    printf '%s\n' "$2" > "$1/f-$2.txt"
    git -C "$1" add "f-$2.txt"
    git -C "$1" commit -q -m "$2"
}

# make_remote <name> -> path of a bare origin with
#   main:               m1
#   dev:                m1 - d1 - d2         (annotated tag v1 on d1)
#   feature/unrelated:  m1 - u1
# Its working seed stays at $SCRATCH/<name>-seed for advance_remote.
make_remote() {
    local bare="${SCRATCH}/$1.git" seed="${SCRATCH}/$1-seed"
    git init -q --bare -b main "$bare"
    git init -q -b main "$seed"
    commit_in "$seed" "$1-m1"
    git -C "$seed" checkout -q -b dev
    commit_in "$seed" "$1-d1"
    git -C "$seed" tag -a v1 -m v1
    commit_in "$seed" "$1-d2"
    git -C "$seed" checkout -q -b feature/unrelated main
    commit_in "$seed" "$1-u1"
    git -C "$seed" checkout -q main
    git -C "$seed" push -q "$bare" main dev feature/unrelated v1 2>/dev/null
    printf '%s\n' "$bare"
}

# advance_remote <name> <branch> <text>: one more commit on <branch> of origin <name>.
advance_remote() {
    local seed="${SCRATCH}/$1-seed"
    git -C "$seed" checkout -q "$2"
    commit_in "$seed" "$3"
    git -C "$seed" push -q "${SCRATCH}/$1.git" "$2" 2>/dev/null
    git -C "$seed" checkout -q main
}

remote_sha() { git -C "$1" rev-parse "refs/heads/$2"; }

# assert_scratch_origin <repo>: refuse an origin outside the scratch dir.
assert_scratch_origin() {
    local url
    url="$(git -C "$1" remote get-url origin 2>/dev/null)" || return 0
    case "$url" in "${SCRATCH}"/*|"file://${SCRATCH}"/*) ;; *) harness "origin of $1 is outside scratch" ;; esac
}

# make_clone <bare> <dir>: a clone with origin/HEAD -> origin/main.
make_clone() {
    git clone -q "$1" "$2" 2>/dev/null || harness "clone of $1 failed"
    git -C "$2" remote set-head origin main >/dev/null 2>&1
    assert_scratch_origin "$2"
}

# snap <repo>: the worktree list and every local branch with its SHA.
snap() {
    git -C "$1" worktree list --porcelain 2>/dev/null | awk '$1=="worktree"'
    git -C "$1" for-each-ref --format='%(refname) %(objectname)' refs/heads 2>/dev/null
}

no_ctrl_bytes() { [ -z "$(printf '%s' "$1" | LC_ALL=C tr -d '\n\t -~')" ]; }
count_lines() { if [ -z "$2" ]; then echo 0; else printf '%s\n' "$2" | grep -c -- "$1"; fi; }
last_line() { printf '%s\n' "$1" | awk 'NF{l=$0} END{print l}'; }

# --- resolve wrapper ----------------------------------------------------------
# resolve <repo> [args...] -> RES_RC RES_OUT RES_ERR. Runs in <repo> under env -i.
resolve() {
    local repo="$1"; shift
    RES_RC=0
    (cd "$repo" && env -i "${BASE_ENV[@]}" bash "$SRC" resolve "$@") \
        >"${SCRATCH}/res.out" 2>"${SCRATCH}/res.err" </dev/null || RES_RC=$?
    RES_OUT="$(cat "${SCRATCH}/res.out")"
    RES_ERR="$(cat "${SCRATCH}/res.err")"
}
kv() { awk -v k="$1" -F= '$1==k { sub(/^[^=]*=/, ""); print; exit }' <<<"$RES_OUT"; }
rule_lines() { printf '%s\n' "$RES_ERR" | grep '^RULE: ' || true; }

# expect_ok <id> <verified> <rule>: exit 0, six keys in order, one RULE line.
expect_ok() {
    local id="$1" verified="$2" rule="$3" keys
    keys="$(cut -d= -f1 <<<"$RES_OUT" | paste -sd, -)"
    check "$id" "exit 0 (rc=$RES_RC)" test "$RES_RC" = 0
    check "$id" "six keys in order (got: $keys)" \
        test "$keys" = "base_ref,base_branch,base_sha,source_verified,source_rule,origin_fingerprint"
    check "$id" "source_verified=$verified" test "$(kv source_verified)" = "$verified"
    check "$id" "source_rule=$rule" test "$(kv source_rule)" = "$rule"
    check "$id" "exactly one RULE line, 'RULE: $rule'" test "$(rule_lines)" = "RULE: $rule"
}

# expect_refusal <id> <rule>: exit 1, stdout empty, one RULE line, one ERROR, last.
expect_refusal() {
    local id="$1" rule="$2"
    check "$id" "refused with exit 1 (rc=$RES_RC)" test "$RES_RC" = 1
    check "$id" "stdout is empty on refusal" test -z "$RES_OUT"
    check "$id" "exactly one RULE line, 'RULE: $rule' (got: $(rule_lines | paste -sd'|' -))" \
        test "$(rule_lines)" = "RULE: $rule"
    check "$id" "exactly one ERROR line" test "$(count_lines '^ERROR: ' "$RES_ERR")" = 1
    check "$id" "the ERROR is the last stderr line" \
        bash -c '[[ "$1" == "ERROR: "* ]]' _ "$(last_line "$RES_ERR")"
}

# =============================================================================
echo "Part A: static checks"
# =============================================================================
check A1 "workflow_worktree_source.sh exists" test -f "$SRC"
check A2 "workflow_worktree_source.md exists" test -f "${TOOLS}/workflow_worktree_source.md"
if [ -f "$SRC" ]; then
    check A3 "the tool never uses eval, declare from output, or set -x" \
        bash -c '! grep -vE "^[[:space:]]*#" "$1" | grep -qE "(^|[;&|[:space:]])(eval|set -x|set -o xtrace)([[:space:]]|$)"' _ "$SRC"
    check A3 "the tool uses set -u" grep -qE '^set -[a-z]*u' "$SRC"
fi
for y in "$DEFAULT_YAML" "$CONSENSUS_YAML"; do
    n="$(wc -l < "$y")"
    check A4 "$(basename "$y") stays under the 400-line brick limit ($n lines)" test "$n" -lt 400
    check A5 "$(basename "$y") creates the worktree from \${BASE_SHA:-\$BASE_REF}" \
        grep -qF '${BASE_SHA:-$BASE_REF}' "$y"
    check A6 "$(basename "$y") calls workflow_worktree_source.sh resolve" \
        grep -qE 'workflow_worktree_source\.sh"? resolve' "$y"
    check A7 "$(basename "$y") calls verify-worktree" grep -qF 'verify-worktree' "$y"
    check A8 "$(basename "$y") reads resolve output with a case loop, never eval" \
        bash -c 'grep -qF "while IFS='"'"'='"'"' read -r k v" "$1" && ! grep -vE "^[[:space:]]*#" "$1" | grep -qE "(^|[;&|[:space:]])eval[[:space:]]"' _ "$y"
done
check A9 "workflow-worktree.yaml has room left: at most 398 lines" test "$(wc -l < "$DEFAULT_YAML")" -le 398
check A10 "STEP_TOOLS lists workflow_worktree_source.sh" \
    bash -c 'grep -E "^ *STEP_TOOLS=" "$1" | grep -qF workflow_worktree_source.sh' _ "$DEFAULT_YAML"
for t in workflow_publish_pr.sh workflow_pr_ready.sh workflow_final_status.sh; do
    check A11 "$t asks workflow_worktree_source.sh verified-base" \
        grep -qE 'workflow_worktree_source\.sh"? verified-base' "${TOOLS}/$t"
done
TOOL_SRC_CMDS="$(grep -vE '^[[:space:]]*#' "$SRC" 2>/dev/null)"
check A14 "no offline source_verified value or rule is left in the tool" \
    bash -c '! printf "%s\n" "$1" | grep -qE "verified-offline|offline-cached-matches-base-sha|offline-base-sha-present"' _ "$TOOL_SRC_CMDS"
check A14 "verified-base's dispatch passes its status on ('cmd_verified_base \"\$@\"; exit \$?')" \
    grep -qF 'cmd_verified_base "$@"; exit $?' "$SRC"
check A14 "the dispatch knows requested-inputs" grep -qE '^[[:space:]]*requested-inputs\)' "$SRC"
check A14 "valid_branch refuses @ and HEAD next to -* and refs/*" \
    bash -c 'grep -A12 "^valid_branch()" "$1" | grep -E "case .*-\*" | grep -qE "@" && grep -A12 "^valid_branch()" "$1" | grep -E "case .*-\*" | grep -q "HEAD"' _ "$SRC"
for t in workflow_publish_pr.sh workflow_pr_ready.sh workflow_final_status.sh; do
    tf="${TOOLS}/$t"
    check A15 "$t never masks verified-base with 2>/dev/null or || true" \
        bash -c '! grep -E "verified-base" "$1" | grep -qE "2>/dev/null|\|\| *true"' _ "$tf"
    check A15 "$t captures verified-base's status with '|| vb_rc=\$?'" \
        bash -c 'grep -E "verified-base" "$1" | grep -qE "\|\| *vb_rc=\\\$\?"' _ "$tf"
    check A15 "$t asks requested-inputs when verified-base exits 3" \
        grep -qE 'workflow_worktree_source\.sh"? requested-inputs' "$tf"
    check A15 "$t refuses with the D3 ERROR text" \
        grep -qF 'base_branch/base_sha given but no verified base in worktree_setup; refusing to fall back' "$tf"
done
check A16 "pr_ready passes a resolver failure on (no 'resolve_expected_base_branch || true')" \
    bash -c '! grep -qE "resolve_expected_base_branch *(2>/dev/null)? *\|\| *true" "$1"' _ "${TOOLS}/workflow_pr_ready.sh"
check A16 "final_status passes a resolver failure on at both call sites (no '|| true', no '2>/dev/null')" \
    bash -c '! grep -E "resolve_expected_base_branch[^(]" "$1" | grep -v "^resolve_expected_base_branch()" | grep -qE "\|\| *true|2>/dev/null"' _ "${TOOLS}/workflow_final_status.sh"
check A16 "final_status captures the resolver status at both call sites" \
    test "$(grep -cE 'expected_base_rc=\$\?|base_rc=\$\?' "${TOOLS}/workflow_final_status.sh")" -ge 2
check A12 "ci.yml runs this test" \
    grep -qF 'test-issue-1957-explicit-worktree-source.sh' "${REPO_ROOT}/.github/workflows/ci.yml"

# =============================================================================
# Step extraction (same idioms as #1808 and #1626).
# =============================================================================
STEP04="${SCRATCH}/step-04.sh"
awk '/^  - id: "step-04-setup-worktree"/ { inblk = 1; next }
     inblk && /^  - id:/ { exit }
     inblk && /^    command:[[:space:]]*\|/ { grab = 1; next }
     grab && /^    [A-Za-z_-]+:/ { grab = 0 }
     grab { sub(/^      /, ""); print }' "$DEFAULT_YAML" > "$STEP04"
[[ -s "$STEP04" ]] || harness "could not extract step-04-setup-worktree"
STEP3="${SCRATCH}/step3.sh"
awk '/^[[:space:]]*- id: "step3-setup-worktree"[[:space:]]*$/ { found = 1; next }
     found && !inblk && /^[[:space:]]*- id: "/ { exit }
     found && !inblk && $0 == "      ```bash" { inblk = 1; next }
     inblk && /^      ```[[:space:]]*$/ { done = 1; exit }
     inblk { line = $0; sub(/^      /, "", line); print line }
     END { if (!done) exit 1 }' "$CONSENSUS_YAML" > "$STEP3" || harness "could not extract step3-setup-worktree"

# The inputs are captured before anything else in the body can assign
# BASE_BRANCH (the recipes' own local variable of that name).
first_code_lines() { awk '!/^[[:space:]]*(#|$)/ { if (++n <= 2) print }' "$1"; }
for s in "$STEP04" "$STEP3"; do
    check A13 "$(basename "$s") captures REQ_BASE_BRANCH and REQ_BASE_SHA on its first code lines" \
        test "$(first_code_lines "$s")" = "$(printf '%s\n%s' 'REQ_BASE_BRANCH="${BASE_BRANCH:-}"' 'REQ_BASE_SHA="${BASE_SHA:-}"')"
done

# =============================================================================
echo "Part F: workflow_fetch_origin.sh (C1)"
# =============================================================================
FREMOTE="$(make_remote fr)"
FCLONE="${SCRATCH}/f-clone"
make_clone "$FREMOTE" "$FCLONE"
FBROKEN="${SCRATCH}/f-broken"
make_clone "$FREMOTE" "$FBROKEN"
git -C "$FBROKEN" remote set-url origin "${SCRATCH}/no-such-origin.git"

fetch_in() {  # fetch_in <repo> [extra env...] -- [args...] -> F_RC F_ERR
    local repo="$1"; shift
    local -a extra=()
    while [ "$#" -gt 0 ] && [ "$1" != -- ]; do extra+=("$1"); shift; done
    [ "${1:-}" != -- ] || shift
    F_RC=0
    F_ERR="$(cd "$repo" && env -i "${BASE_ENV[@]}" "${extra[@]}" bash "$FETCHER" "$@" 2>&1 >/dev/null </dev/null)" || F_RC=$?
}

fetch_in "$FBROKEN" --
check F1 "with no arguments a fetch that keeps failing still exits 0" test "$F_RC" = 0
check F1 "the WARNING reports git's real exit number, not 0" \
    bash -c '[[ "$1" != *"(exit 0)"* && "$1" =~ "after 3 attempts (exit "[1-9] ]]' _ "$F_ERR"
fetch_in "$FBROKEN" -- --strict
check F2 "--strict exits non-zero when the fetch fails (rc=$F_RC)" test "$F_RC" != 0
check F2 "--strict prints 'WARNING: git fetch origin failed (exit N).' with git's status" \
    bash -c 'printf "%s\n" "$1" | grep -qxE "WARNING: git fetch origin failed \(exit $2\)\."' _ "$F_ERR" "$F_RC"
check F2 "--strict never prints the legacy retry or 'continuing' text" \
    bash -c '[[ "$1" != *"retrying"* && "$1" != *"after 3 attempts"* && "$1" != *"continuing with local refs"* ]]' _ "$F_ERR"

# F-strict (D2): one git call, no sleep. A PATH wrapper counts git calls and
# forwards to the real git by absolute path; a sleep stub fails the case.
REAL_GIT="$(command -v git)"
case "$REAL_GIT" in /*) ;; *) harness "git is not an absolute path: $REAL_GIT" ;; esac
STUBS="${SCRATCH}/stubs"
mkdir -p "$STUBS"
cat > "$STUBS/git" <<STUB
#!/bin/sh
printf '%s\n' "\$*" >> "${SCRATCH}/git-calls"
exec "$REAL_GIT" "\$@"
STUB
cat > "$STUBS/sleep" <<STUB
#!/bin/sh
printf 'sleep %s\n' "\$*" >> "${SCRATCH}/sleep-calls"
exit 97
STUB
chmod +x "$STUBS/git" "$STUBS/sleep"
STUB_PATH="${STUBS}:${PATH}"
: > "${SCRATCH}/git-calls"; rm -f "${SCRATCH}/sleep-calls"
fetch_in "$FBROKEN" PATH="$STUB_PATH" -- --strict '+refs/heads/dev:refs/remotes/origin/dev'
check F-strict "--strict against an unreachable origin exits non-zero (rc=$F_RC)" test "$F_RC" != 0 -a "$F_RC" != 97
check F-strict "--strict runs git exactly once ($(grep -c . "${SCRATCH}/git-calls") calls)" \
    test "$(grep -c . "${SCRATCH}/git-calls")" = 1
check F-strict "the one git call is 'fetch origin --no-tags -- <refspec>'" \
    test "$(cat "${SCRATCH}/git-calls")" = "fetch origin --no-tags -- +refs/heads/dev:refs/remotes/origin/dev"
check F-strict "--strict never sleeps" test ! -e "${SCRATCH}/sleep-calls"
: > "${SCRATCH}/git-calls"; rm -f "${SCRATCH}/sleep-calls"
fetch_in "$FCLONE" PATH="$STUB_PATH" -- --strict
check F-strict "--strict that works runs git once and exits 0 (rc=$F_RC)" \
    test "$F_RC" = 0 -a "$(grep -c . "${SCRATCH}/git-calls")" = 1 -a ! -e "${SCRATCH}/sleep-calls"
fetch_in "$FCLONE" -- --strict
check F3 "--strict exits 0 when the fetch works" test "$F_RC" = 0
for bad in "+refs/tags/v1:refs/remotes/origin/v1" "refs/heads/dev:refs/remotes/origin/dev" \
           "+refs/heads/dev:refs/heads/dev" "+refs/heads/dev" "--upload-pack=touch ${SCRATCH}/pwned-f" "-x"; do
    fetch_in "$FCLONE" -- --strict "$bad"
    check F4 "refspec/argument '$bad' is a usage error (exit 2, rc=$F_RC)" test "$F_RC" = 2
done
check F4 "no --upload-pack ever ran" test ! -e "${SCRATCH}/pwned-f"
advance_remote fr dev fr-d3
git -C "$FCLONE" config --replace-all remote.origin.fetch '+refs/heads/feature/unrelated:refs/remotes/origin/feature/unrelated'
fetch_in "$FCLONE" -- --strict '+refs/heads/dev:refs/remotes/origin/dev'
check F5 "an explicit refspec updates origin/dev under a narrowed remote.origin.fetch" \
    test "$F_RC" = 0 -a "$(git -C "$FCLONE" rev-parse refs/remotes/origin/dev)" = "$(remote_sha "$FREMOTE" dev)"
fetch_in "$FCLONE" GIT_TRACE=1 GIT_TRACE_PACKET=1 GIT_CURL_VERBOSE=1 -- --strict
check F6 "GIT_TRACE* and GIT_CURL_VERBOSE are unset before fetching" \
    bash -c '[[ "$1" = 0 && "$2" != *"trace: "* && "$2" != *"packet:"* ]]' _ "$F_RC" "$F_ERR"

# =============================================================================
echo "Part T: workflow_worktree_source.sh resolve (C2)"
# =============================================================================
TREMOTE="$(make_remote tr)"
T="${SCRATCH}/t-clone"
make_clone "$TREMOTE" "$T"
DEV_TIP="$(remote_sha "$TREMOTE" dev)"
DEV_OLD="$(git -C "$TREMOTE" rev-parse 'dev~1')"
UNREL="$(remote_sha "$TREMOTE" feature/unrelated)"
TAG_OBJ="$(git -C "$T" rev-parse v1)"
TREE_OBJ="$(git -C "$T" rev-parse 'origin/dev^{tree}')"
MISSING_SHA="deadbeefdeadbeefdeadbeefdeadbeefdeadbeef"
MISSING_SHA256="$(printf 'ab%.0s' $(seq 32))"

# --- usage (exit 2) ---
resolve "$T"
check T1 "no subcommand arguments is a usage error" bash -c '[[ "$1" = 2 ]]' _ "$RES_RC"
RES_RC=0; (cd "$T" && env -i "${BASE_ENV[@]}" bash "$SRC" frobnicate >/dev/null 2>&1) || RES_RC=$?
check T1 "an unknown subcommand is a usage error (rc=$RES_RC)" test "$RES_RC" = 2
RES_RC=0; (cd "$T" && env -i "${BASE_ENV[@]}" bash "$SRC" >/dev/null 2>&1) || RES_RC=$?
check T1 "no subcommand is a usage error (rc=$RES_RC)" test "$RES_RC" = 2
resolve "$T" --repo "$T" --base-branch dev
check T1 "a missing --recipe is a usage error (rc=$RES_RC)" test "$RES_RC" = 2
resolve "$T" --recipe other --repo "$T" --base-branch dev
check T1 "an unknown --recipe value is a usage error (rc=$RES_RC)" test "$RES_RC" = 2
resolve "$T" --recipe default --repo "$T" --base-branch dev --base-branch main
check T1 "a repeated flag is a usage error (rc=$RES_RC)" test "$RES_RC" = 2
resolve "$T" --recipe default --repo "$T" --frob x
check T1 "an unknown flag is a usage error (rc=$RES_RC)" test "$RES_RC" = 2
resolve "$T" --recipe default --repo "$T" --base-branch
check T1 "a flag without a value is a usage error (rc=$RES_RC)" test "$RES_RC" = 2
check T1 "usage errors print nothing on stdout" test -z "$RES_OUT"

# --- branch only (D2) ---
resolve "$T" --recipe default --repo "$T" --base-branch dev
expect_ok T2 verified fetched
check T2 "base_ref=origin/dev" test "$(kv base_ref)" = origin/dev
check T2 "base_branch=dev" test "$(kv base_branch)" = dev
check T2 "base_sha is the remote's dev tip" test "$(kv base_sha)" = "$DEV_TIP"
check T2 "origin_fingerprint is a hex object id" bash -c '[[ "$1" =~ ^([0-9a-f]{40}|[0-9a-f]{64})$ ]]' _ "$(kv origin_fingerprint)"
check T2 "the origin URL is never printed by the tool" \
    bash -c '[[ "$1" != *"$3"* ]] && ! printf "%s\n" "$2" | grep -E "^(INFO|WARNING|RULE|ERROR):" | grep -qF "$3"' \
    _ "$RES_OUT" "$RES_ERR" "$TREMOTE"
FP_T="$(kv origin_fingerprint)"
resolve "$T" --recipe consensus --repo "$T" --base-branch dev
check T2 "the consensus recipe gives the same explicit result" \
    test "$RES_RC" = 0 -a "$(kv base_sha)" = "$DEV_TIP" -a "$(kv origin_fingerprint)" = "$FP_T"

# --- sha only (D3) ---
resolve "$T" --recipe default --repo "$T" --base-sha "$DEV_OLD"
expect_ok T3 verified fetched
check T3 "base_ref is the SHA itself" test "$(kv base_ref)" = "$DEV_OLD"
check T3 "base_branch is empty" test -z "$(kv base_branch)"
check T3 "base_sha is the SHA" test "$(kv base_sha)" = "$DEV_OLD"
UPPER_OLD="$(printf '%s' "$DEV_OLD" | tr 'a-f' 'A-F')"
resolve "$T" --recipe default --repo "$T" --base-sha "$UPPER_OLD"
check T3 "an upper-case full SHA is accepted and lower-cased" test "$RES_RC" = 0 -a "$(kv base_sha)" = "$DEV_OLD"
resolve "$T" --recipe default --repo "$T" --base-sha "  $DEV_OLD  "
check T3 "surrounding whitespace is trimmed" test "$RES_RC" = 0 -a "$(kv base_sha)" = "$DEV_OLD"

# --- both (D1): contained, not contained ---
resolve "$T" --recipe default --repo "$T" --base-branch dev --base-sha "$DEV_OLD"
expect_ok T4 verified fetched
check T4 "both inputs: base_ref=origin/dev, base_branch=dev, base_sha is the pinned commit (not the tip)" \
    test "$(kv base_ref)" = origin/dev -a "$(kv base_branch)" = dev -a "$(kv base_sha)" = "$DEV_OLD"
resolve "$T" --recipe default --repo "$T" --base-branch dev --base-sha "$UNREL"
expect_refusal T5 refuse-not-contained
check T5 "the ERROR names the requested SHA and the actual tip" \
    bash -c '[[ "$1" == *"$2"* && "$1" == *"$3"* ]]' _ "$(last_line "$RES_ERR")" "$UNREL" "$DEV_TIP"

# --- result checks ---
resolve "$T" --recipe default --repo "$T" --base-branch no-such-branch
expect_refusal T6 refuse-missing-branch
check T6 "the ERROR names the branch" bash -c '[[ "$1" == *"no-such-branch"* ]]' _ "$(last_line "$RES_ERR")"
check T6 "no origin/HEAD fallback is used for a missing branch" test -z "$(kv base_ref)"
resolve "$T" --recipe default --repo "$T" --base-sha "$MISSING_SHA"
expect_refusal T7 refuse-missing-sha
check T7 "the ERROR names the requested SHA" bash -c '[[ "$1" == *"$2"* ]]' _ "$(last_line "$RES_ERR")" "$MISSING_SHA"
resolve "$T" --recipe default --repo "$T" --base-sha "$MISSING_SHA256"
expect_refusal T7 refuse-missing-sha
resolve "$T" --recipe default --repo "$T" --base-sha "$TAG_OBJ"
expect_refusal T8 refuse-not-commit
check T8 "a tag object is named with what it resolves to" \
    bash -c '[[ "$1" == *"$2"* ]]' _ "$(last_line "$RES_ERR")" "$TAG_OBJ"
resolve "$T" --recipe default --repo "$T" --base-sha "$TREE_OBJ"
expect_refusal T8 refuse-not-commit
resolve "$T" --recipe default --repo "$T" --base-branch dev --base-sha "$TAG_OBJ"
expect_refusal T8 refuse-not-commit

# --- input validation (D4, S-13) ---
resolve "$T" --recipe default --repo "$T" --base-sha "${DEV_OLD:0:12}"
expect_refusal T9 refuse-invalid-input
check T9 "an abbreviated SHA is refused by name" \
    bash -c '[[ "$1" == *"abbreviated SHAs are refused"* ]]' _ "$(last_line "$RES_ERR")"
for bad_sha in "HEAD" "dev" "${DEV_OLD}0" "g${DEV_OLD:1}" "${DEV_OLD:0:39}"; do
    resolve "$T" --recipe default --repo "$T" --base-sha "$bad_sha"
    check T9 "base_sha '$bad_sha' is refused as invalid input" \
        test "$RES_RC" = 1 -a "$(rule_lines)" = "RULE: refuse-invalid-input" -a -z "$RES_OUT"
done

BEFORE="$(snap "$T")"
LONG="$(printf 'a%.0s' $(seq 300))"
ESC=$'\e'
for bad in "@{-1}" "refs/heads/dev" "dev;id" '$(id)' $'dev\nRULE: fetched' "dev${ESC}[31m" \
           "$LONG" "dev..main" "dev branch" "dev~1" "dev^" "a:b" "dev.lock" "*"; do
    resolve "$T" --recipe default --repo "$T" --base-branch "$bad"
    label="$(printf '%s' "${bad:0:20}" | LC_ALL=C tr -c 'A-Za-z0-9._/+@{}$();-' '?')"
    check T10 "base_branch '$label' is refused (rc=$RES_RC)" \
        test "$RES_RC" = 1 -a -z "$RES_OUT" -a "$(rule_lines)" = "RULE: refuse-invalid-input"
    check T10 "base_branch '$label': stderr has no raw control bytes" no_ctrl_bytes "$RES_ERR"
    check T10 "base_branch '$label': exactly one ERROR line" test "$(count_lines '^ERROR: ' "$RES_ERR")" = 1
done
resolve "$T" --recipe default --repo "$T" --base-branch 'dev;id'
check T10 "a refused value is echoed with disallowed bytes as '?'" \
    bash -c '[[ "$1" == *"'"'"'dev?id'"'"'"* ]]' _ "$(last_line "$RES_ERR")"
resolve "$T" --recipe default --repo "$T" --base-branch "$LONG"
check T10 "a refused value is echoed truncated to 64 bytes" \
    bash -c '[[ "$1" == *"$2"* && "$1" != *"$3"* ]]' _ "$(last_line "$RES_ERR")" "${LONG:0:64}" "${LONG:0:65}"
# SEC-nl: a newline plus a forged ERROR line is refused with one ERROR line.
resolve "$T" --recipe default --repo "$T" --base-branch $'x\nERROR: fake'
check SEC-nl "base_branch 'x<LF>ERROR: fake' is refused as invalid input (rc=$RES_RC)" \
    test "$RES_RC" = 1 -a -z "$RES_OUT" -a "$(rule_lines)" = "RULE: refuse-invalid-input"
check SEC-nl "stderr has exactly one ERROR line" test "$(count_lines '^ERROR: ' "$RES_ERR")" = 1
check SEC-nl "no stderr line starts with the injected 'ERROR: fake'" \
    bash -c '! printf "%s\n" "$1" | grep -q "^ERROR: fake"' _ "$RES_ERR"
check SEC-nl "the refused value is shown on one line, the newline as '?'" \
    bash -c '[[ "$1" == *"x?ERROR??fake"* ]]' _ "$(last_line "$RES_ERR")"
# SEC-at (SR-3): `@` and `HEAD` are refused by valid_branch.
for bad in "@" "HEAD"; do
    resolve "$T" --recipe default --repo "$T" --base-branch "$bad"
    expect_refusal "SEC-at" refuse-invalid-input
    check SEC-at "base_branch '$bad' never reaches a fetch or a ref" test -z "$(kv base_ref)"
done
for bad in "--upload-pack=touch ${SCRATCH}/pwned-t" "-x" "--base-sha"; do
    resolve "$T" --recipe default --repo "$T" --base-branch "$bad"
    check T11 "a base_branch value starting with '-' (${bad:0:14}) is a usage error (rc=$RES_RC)" \
        test "$RES_RC" = 2 -a -z "$RES_OUT"
done
resolve "$T" --recipe default --repo "$T" --base-sha "-x"
check T11 "a base_sha value starting with '-' is a usage error (rc=$RES_RC)" test "$RES_RC" = 2
check T11 "no --upload-pack ever ran" test ! -e "${SCRATCH}/pwned-t"
check T12 "refused input changes no branch and no worktree" test "$(snap "$T")" = "$BEFORE"

# --- origin identity (D8 step 2) ---
NOORIGIN="${SCRATCH}/no-origin"
git init -q -b main "$NOORIGIN"; commit_in "$NOORIGIN" no-origin-1
resolve "$NOORIGIN" --recipe default --repo "$NOORIGIN" --base-branch dev
expect_refusal T13 refuse-origin-identity
mkdir -p "$T/sub"
resolve "$T" --recipe default --repo "$T/sub" --base-branch dev
expect_refusal T13 refuse-origin-identity
check T13 "the ERROR names --repo and the real top" \
    bash -c '[[ "$1" == *"$2/sub"* && "$1" == *"show-toplevel: $2"* ]]' _ "$(last_line "$RES_ERR")" "$T"
resolve "$T" --recipe default --repo "t-clone" --base-branch dev
check T13 "a relative --repo is refused (rc=$RES_RC)" bash -c '[[ "$1" = 1 || "$1" = 2 ]] && [[ -z "$2" ]]' _ "$RES_RC" "$RES_OUT"
ln -s "$T" "${SCRATCH}/t-link"
resolve "$T" --recipe default --repo "${SCRATCH}/t-link" --base-branch dev
check T13 "a --repo reached through a symlink is compared by its physical path" \
    test "$RES_RC" = 0 -a "$(kv base_sha)" = "$DEV_TIP"

# --- the explicit path never reads or rewrites origin/HEAD (case b at tool level) ---
git -C "$T" symbolic-ref refs/remotes/origin/HEAD refs/remotes/origin/feature/unrelated
resolve "$T" --recipe default --repo "$T" --base-branch dev
check T14 "origin/HEAD pointing elsewhere does not change the explicit result" \
    test "$RES_RC" = 0 -a "$(kv base_sha)" = "$DEV_TIP"
check T14 "the explicit path never runs set-head" \
    test "$(git -C "$T" symbolic-ref refs/remotes/origin/HEAD)" = refs/remotes/origin/feature/unrelated
git -C "$T" symbolic-ref refs/remotes/origin/HEAD refs/remotes/origin/main

# --- narrowed fetch config (case c at tool level) ---
CREMOTE="$(make_remote cr)"
C="${SCRATCH}/c-clone"
make_clone "$CREMOTE" "$C"
advance_remote cr dev cr-d3
git -C "$C" config --replace-all remote.origin.fetch '+refs/heads/feature/unrelated:refs/remotes/origin/feature/unrelated'
resolve "$C" --recipe default --repo "$C" --base-branch dev
check T15 "a narrowed remote.origin.fetch cannot leave origin/dev stale" \
    test "$RES_RC" = 0 -a "$(kv base_sha)" = "$(remote_sha "$CREMOTE" dev)" \
         -a "$(git -C "$C" rev-parse refs/remotes/origin/dev)" = "$(remote_sha "$CREMOTE" dev)"

# --- legacy path (D6, case g at tool level) ---
for inputs in "none" "blank"; do
    if [ "$inputs" = none ]; then resolve "$T" --recipe default --repo "$T"
    else resolve "$T" --recipe default --repo "$T" --base-branch "   " --base-sha " "; fi
    expect_ok T16 legacy legacy-unverified
    check T16 "($inputs) legacy base_ref is today's origin/main, base_branch main" \
        test "$(kv base_ref)" = origin/main -a "$(kv base_branch)" = main
    check T16 "($inputs) legacy base_sha and origin_fingerprint are empty" \
        test -z "$(kv base_sha)" -a -z "$(kv origin_fingerprint)"
    check T16 "($inputs) the legacy WARNING names the resolved ref" \
        bash -c 'printf "%s\n" "$1" | grep -qxF "WARNING: no base_branch/base_sha given; legacy base resolution (origin/main), source not verified"' _ "$RES_ERR"
done
# The consensus chain alone tries origin/main before origin/master (#858): an
# unreachable origin with no origin/HEAD tells the two chains apart.
LREMOTE="$(make_remote lr)"
git -C "${SCRATCH}/lr-seed" push -q "$LREMOTE" main:master 2>/dev/null
L="${SCRATCH}/l-clone"
make_clone "$LREMOTE" "$L"
git -C "$L" fetch -q origin 2>/dev/null
git -C "$L" symbolic-ref -d refs/remotes/origin/HEAD
git -C "$L" remote set-url origin "${SCRATCH}/lr-gone.git"
resolve "$L" --recipe default --repo "$L"
check T17 "legacy default chain: origin/HEAD, origin/master, origin/develop (rc=$RES_RC)" \
    test "$RES_RC" = 0 -a "$(kv base_ref)" = origin/master -a "$(kv source_verified)" = legacy
resolve "$L" --recipe consensus --repo "$L"
check T17 "legacy consensus chain tries origin/main first (rc=$RES_RC)" \
    test "$RES_RC" = 0 -a "$(kv base_ref)" = origin/main -a "$(kv source_verified)" = legacy

# --- case d: D1, any input plus a failed fetch refuses; no offline rule ---
D="${SCRATCH}/d-clone"
DREMOTE="$(make_remote dr)"
make_clone "$DREMOTE" "$D"
D_TIP="$(remote_sha "$DREMOTE" dev)"
D_OLD="$(git -C "$DREMOTE" rev-parse 'dev~1')"
git -C "$D" remote set-url origin "${SCRATCH}/dr-missing.git"
D_BEFORE="$(snap "$D")"
resolve "$D" --recipe default --repo "$D" --base-branch dev
expect_refusal d1 refuse-fetch-failed
check d1 "branch only, fetch failed: the ERROR names the fetch failure and the branch" \
    bash -c '[[ "$1" == *"git fetch origin failed"* && "$1" == *"base_branch '"'"'dev'"'"'"* && "$1" == *"cached refs cannot prove freshness"* ]]' _ "$(last_line "$RES_ERR")"
resolve "$D" --recipe default --repo "$D" --base-branch dev --base-sha "$D_TIP"
expect_refusal d2 refuse-fetch-failed
check d2 "branch + sha equal to the cached origin/dev is still refused: cached refs never stand in for a fetch" \
    bash -c '[[ "$1" == *"git fetch origin failed"* && "$1" == *"$2"* ]]' _ "$(last_line "$RES_ERR")" "$D_TIP"
resolve "$D" --recipe default --repo "$D" --base-branch dev --base-sha "$D_OLD"
expect_refusal d3 refuse-fetch-failed
resolve "$D" --recipe default --repo "$D" --base-branch not-cached --base-sha "$D_TIP"
expect_refusal d3 refuse-fetch-failed
resolve "$D" --recipe default --repo "$D" --base-sha "$D_OLD"
expect_refusal d4 refuse-fetch-failed
check d4 "sha only, present locally, fetch failed: refused, not trusted offline" \
    bash -c '[[ "$1" == *"base_sha '"'"'$2'"'"'"* ]]' _ "$(last_line "$RES_ERR")" "$D_OLD"
resolve "$D" --recipe default --repo "$D" --base-sha "$MISSING_SHA"
expect_refusal d5 refuse-fetch-failed
# A failed run makes exactly one git fetch call (D2) and never sleeps.
: > "${SCRATCH}/git-calls"; rm -f "${SCRATCH}/sleep-calls"
RES_RC=0
(cd "$D" && env -i "${BASE_ENV[@]}" PATH="$STUB_PATH" bash "$SRC" resolve --recipe default --repo "$D" --base-branch dev) \
    >/dev/null 2>"${SCRATCH}/res.err" </dev/null || RES_RC=$?
check d-all "a failed fetch is refused (rc=$RES_RC) after exactly one fetch call ($(grep -c '^fetch ' "${SCRATCH}/git-calls"))" \
    bash -c '[[ "$1" = 1 ]] && (( $(grep -c "^fetch " "$2") == 1 ))' _ "$RES_RC" "${SCRATCH}/git-calls"
check d-all "a failed fetch never sleeps" test ! -e "${SCRATCH}/sleep-calls"
git -C "$D" remote set-url origin "$DREMOTE"
resolve "$D" --recipe default --repo "$D" --base-branch dev
expect_ok d6 verified fetched
check d-all "no D1 refusal created a branch or worktree" test "$(snap "$D")" = "$D_BEFORE"

# --- T-partial: origin is reachable and the branch exists, but the branch
# fetch fails. That is a fetch failure, not a missing branch (17b #3). A stub
# fails every `git fetch` that carries a refspec and forwards everything else.
PSTUBS="${SCRATCH}/pstubs"
mkdir -p "$PSTUBS"
cat > "$PSTUBS/git" <<STUB
#!/bin/sh
case "\$*" in
    *fetch*+refs/heads/*) echo "fatal: simulated transfer failure" >&2; exit 128 ;;
esac
exec "$REAL_GIT" "\$@"
STUB
chmod +x "$PSTUBS/git"
RES_RC=0
(cd "$D" && env -i "${BASE_ENV[@]}" PATH="${PSTUBS}:${PATH}" bash "$SRC" resolve --recipe default --repo "$D" --base-branch dev) \
    >"${SCRATCH}/res.out" 2>"${SCRATCH}/res.err" </dev/null || RES_RC=$?
RES_OUT="$(cat "${SCRATCH}/res.out")"; RES_ERR="$(cat "${SCRATCH}/res.err")"
expect_refusal T-partial refuse-fetch-failed
check T-partial "the branch exists on origin, so the refusal is not refuse-missing-branch" \
    bash -c '! printf "%s\n" "$1" | grep -qxF "RULE: refuse-missing-branch"' _ "$RES_ERR"
RES_RC=0
(cd "$D" && env -i "${BASE_ENV[@]}" PATH="${PSTUBS}:${PATH}" bash "$SRC" resolve --recipe default --repo "$D" --base-branch no-such-branch) \
    >"${SCRATCH}/res.out" 2>"${SCRATCH}/res.err" </dev/null || RES_RC=$?
RES_OUT="$(cat "${SCRATCH}/res.out")"; RES_ERR="$(cat "${SCRATCH}/res.err")"
expect_refusal T-partial refuse-missing-branch
check T-partial "no D1 refusal created a branch or worktree" test "$(snap "$D")" = "$D_BEFORE"

# --- S2: C1 control bytes (0x80-0x9F) and DEL in a path are replaced ---
resolve "$T" --recipe default --repo "${SCRATCH}/c1"$'\x9b'"31m"$'\x85\x7f' --base-branch dev
expect_refusal T-c1 refuse-origin-identity
check T-c1 "stderr has no byte in 0x7F-0x9F" \
    bash -c '[ -z "$(printf "%s" "$1" | LC_ALL=C tr -dc "\177-\237")" ]' _ "$RES_ERR"
check T-c1 "the replaced bytes show as ?" \
    bash -c '[[ "$1" == *"c1?31m??"* ]]' _ "$(last_line "$RES_ERR")"

# --- SR-6: a path with a newline cannot forge a second ERROR line ---
resolve "$T" --recipe default --repo "${SCRATCH}/nl"$'\n'"ERROR: fake" --base-branch dev
expect_refusal T-nl refuse-origin-identity
check T-nl "the forged ERROR line never starts a stderr line" \
    bash -c '! printf "%s\n" "$1" | grep -q "^ERROR: fake"' _ "$RES_ERR"
check T-nl "stderr has no raw control bytes" no_ctrl_bytes "$RES_ERR"

# =============================================================================
echo "Part V: verify-worktree (C3)"
# vw <args...> -> VW_RC VW_ERR. Runs verify-worktree in $T under env -i.
vw() {
    VW_RC=0
    (cd "$T" && env -i "${BASE_ENV[@]}" bash "$SRC" verify-worktree "$@") \
        >"${SCRATCH}/vw.out" 2>"${SCRATCH}/vw.err" </dev/null || VW_RC=$?
    VW_ERR="$(cat "${SCRATCH}/vw.err")"
}
vw_one_error_last() {
    [[ "$(count_lines '^ERROR: ' "$VW_ERR")" = 1 && "$(last_line "$VW_ERR")" == "ERROR: "* ]]
}
T_FP="$(git -C "$T" remote get-url origin | git -C "$T" hash-object --stdin)"
wt_listed() { git -C "$T" worktree list --porcelain | grep -qxF "worktree $1"; }
branch_exists() { git -C "$T" rev-parse --verify --quiet "refs/heads/$1" >/dev/null; }

# V1 (SR-4): the path is not confirmed (its toplevel is the worktree above
# it), so --cleanup-created removes nothing.
V_WT="${SCRATCH}/v-wt"
git -C "$T" worktree add -q -b v-1957 "$V_WT" origin/dev 2>/dev/null || harness "worktree add v-1957 failed"
mkdir -p "$V_WT/sub"
vw "$V_WT/sub" "$DEV_TIP" created "$T" "$T_FP" --branch v-1957 --cleanup-created
check V1 "an unconfirmed path is refused (rc=$VW_RC)" test "$VW_RC" = 1
check V1 "exactly one ERROR line, and it is last" vw_one_error_last
check V1 "the WARNING says the path was not confirmed" \
    bash -c 'printf "%s\n" "$1" | grep -qF "WARNING: not removing $2: it was not confirmed as this run'"'"'s worktree (issue #1957)."' _ "$VW_ERR" "$V_WT/sub"
check V1 "no INFO removal line is printed" bash -c '! printf "%s\n" "$1" | grep -q "^INFO: "' _ "$VW_ERR"
check V1 "the worktree is still listed" wt_listed "$V_WT"
check V1 "the branch still exists" branch_exists v-1957
vw "$V_WT" "$DEV_TIP" created "${SCRATCH}/no-such-main" "$T_FP" --branch v-1957 --cleanup-created
check V1 "a main repo that does not share the common dir is refused (rc=$VW_RC)" test "$VW_RC" = 1
check V1 "common-dir mismatch: the WARNING says the path was not confirmed" \
    bash -c 'printf "%s\n" "$1" | grep -q "^WARNING: not removing .*not confirmed"' _ "$VW_ERR"
check V1 "common-dir mismatch: the worktree is still listed" wt_listed "$V_WT"
check V1 "common-dir mismatch: the branch still exists" branch_exists v-1957

# V2: a confirmed worktree whose HEAD is wrong is removed with its branch.
vw "$V_WT" "$DEV_OLD" created "$T" "$T_FP" --branch v-1957 --cleanup-created
check V2 "a confirmed worktree at the wrong commit is refused (rc=$VW_RC)" test "$VW_RC" = 1
check V2 "exactly one ERROR line, and it is last" vw_one_error_last
check V2 "no 'not confirmed' WARNING for a confirmed path" \
    bash -c '! printf "%s\n" "$1" | grep -q "not confirmed"' _ "$VW_ERR"
check V2 "the worktree created by this run was removed" bash -c '! git -C "$1" worktree list --porcelain | grep -qxF "worktree $2"' _ "$T" "$V_WT"
check V2 "the branch created by this run was deleted" bash -c '! git -C "$1" rev-parse --verify --quiet refs/heads/v-1957 >/dev/null' _ "$T"

# V3 (SR-6): a worktree path with a newline cannot forge an ERROR line.
vw "${SCRATCH}/nl-wt"$'\n'"ERROR: fake" "$DEV_TIP" created "$T" "$T_FP"
check V3 "a newline path is refused (rc=$VW_RC)" test "$VW_RC" = 1
check V3 "exactly one ERROR line, and it is last" vw_one_error_last
check V3 "the forged ERROR line never starts a stderr line" \
    bash -c '! printf "%s\n" "$1" | grep -q "^ERROR: fake"' _ "$VW_ERR"
check V3 "stderr has no raw control bytes" no_ctrl_bytes "$VW_ERR"

# =============================================================================
echo "Part B: verified-base and requested-inputs (C4)"
# =============================================================================
vb() {  # vb [VAR=value...] -> VB_RC VB_OUT VB_ERR
    VB_RC=0
    (cd "$T" && env -i "${BASE_ENV[@]}" "$@" bash "$SRC" verified-base) \
        >"${SCRATCH}/vb.out" 2>"${SCRATCH}/vb.err" </dev/null || VB_RC=$?
    VB_OUT="$(cat "${SCRATCH}/vb.out")"; VB_ERR="$(cat "${SCRATCH}/vb.err")"
}
ri() {  # ri [VAR=value...] -> RI_RC RI_OUT RI_ERR
    RI_RC=0
    (cd "$T" && env -i "${BASE_ENV[@]}" "$@" bash "$SRC" requested-inputs) \
        >"${SCRATCH}/ri.out" 2>"${SCRATCH}/ri.err" </dev/null || RI_RC=$?
    RI_OUT="$(cat "${SCRATCH}/ri.out")"; RI_ERR="$(cat "${SCRATCH}/ri.err")"
}
# vb_err <id> <msg>: exit 1, nothing on stdout, exactly one ERROR line, last.
vb_err() {
    check "$1" "$2: exit 1 (rc=$VB_RC)" test "$VB_RC" = 1
    check "$1" "$2: nothing on stdout" test -z "$VB_OUT"
    check "$1" "$2: exactly one ERROR line, and it is last" \
        bash -c '[[ "$(printf "%s\n" "$1" | grep -c "^ERROR: ")" = 1 && "$2" == "ERROR: "* ]]' _ "$VB_ERR" "$(last_line "$VB_ERR")"
    check "$1" "$2: stderr has no raw control bytes" no_ctrl_bytes "$VB_ERR"
}
# vb_none <id> <msg>: exit 3, nothing on stdout, no ERROR line.
vb_none() {
    check "$1" "$2: exit 3 (rc=$VB_RC)" test "$VB_RC" = 3
    check "$1" "$2: nothing on stdout and no ERROR line" \
        bash -c '[[ -z "$1" ]] && ! printf "%s\n" "$2" | grep -q "^ERROR: "' _ "$VB_OUT" "$VB_ERR"
}
if ! command -v jq >/dev/null 2>&1; then
    harness "jq is required (recipe_context_value.sh reads dotted keys with it)"
fi
ws() { printf 'RECIPE_VAR_worktree_setup=%s' "$1"; }
vb "$(ws '{"source_verified":"verified","base_branch":"dev"}')"
check B1 "verified + dev with origin/dev present prints dev, exit 0 (rc=$VB_RC)" test "$VB_RC" = 0 -a "$VB_OUT" = dev
check B1 "verified + dev writes nothing to stderr" test -z "$VB_ERR"
vb "$(ws '{"source_verified":"verified-offline","base_branch":"dev"}')"
vb_none B2 "verified-offline is not a trusted value"
for sv in legacy not-applicable VERIFIED Verified "verified " "" unknown; do
    vb "$(ws "{\"source_verified\":\"$sv\",\"base_branch\":\"dev\"}")"
    vb_none B3 "source_verified '$sv'"
done
vb "$(ws '{"base_branch":"dev"}')"
vb_none B3 "source_verified absent"
vb "$(ws '{"source_verified":"verified","base_branch":""}')"
vb_none B3 "a SHA-only run (verified, base_branch empty)"
# B-new1: verified, but the ref does not resolve -> exit 1, never a fallback.
vb "$(ws '{"source_verified":"verified","base_branch":"gone-1957"}')"
vb_err B-new1 "verified + a branch with no refs/remotes/origin ref"
check B-new1 "the ERROR names the branch and the missing ref" \
    bash -c '[[ "$1" == *"gone-1957"* && "$1" == *"refs/remotes/origin/gone-1957"* ]]' _ "$VB_ERR"
vb WORKTREE_SETUP_SOURCE_VERIFIED=verified WORKTREE_SETUP_BASE_BRANCH=dev
check B4 "with no context, the env pair is used (rc=$VB_RC)" test "$VB_RC" = 0 -a "$VB_OUT" = dev
vb WORKTREE_SETUP_SOURCE_VERIFIED=verified WORKTREE_SETUP_BASE_BRANCH=gone-1957
vb_err B4 "an env pair saying verified must still pass the ref check"
vb WORKTREE_SETUP_SOURCE_VERIFIED=verified 'WORKTREE_SETUP_BASE_BRANCH=dev;id'
vb_err B4 "an env pair saying verified must still pass valid_branch"
vb "$(ws '{"source_verified":"verified"}')" WORKTREE_SETUP_BASE_BRANCH=dev
vb_none B5 "context and env are never mixed (verified from context, branch from env)"
vb "$(ws '{"base_branch":"dev"}')" WORKTREE_SETUP_SOURCE_VERIFIED=verified
vb_none B5 "context and env are never mixed (branch from context, verified from env)"
vb "$(ws '{"source_verified":"legacy","base_branch":"main"}')" \
    WORKTREE_SETUP_SOURCE_VERIFIED=verified WORKTREE_SETUP_BASE_BRANCH=dev
vb_none B6 "a stale parent-run env pair never overrides this run's context"
vb WORKTREE_SETUP_SOURCE_VERIFIED=verified
vb_none B6 "half an env pair"
for bb in "refs/heads/dev" "-x" 'dev;id' '$(id)' "dev..main" "@{-1}" "@" "HEAD"; do
    vb "$(ws "{\"source_verified\":\"verified\",\"base_branch\":\"$bb\"}")"
    vb_err B7 "verified + base_branch '$bb' that fails the input rules"
done
vb "$(ws "$(jq -nc --arg b $'x\nERROR: fake' '{source_verified:"verified",base_branch:$b}')")"
vb_err B7 "verified + base_branch with a newline and a forged ERROR line"
check B7 "the forged ERROR line never starts a stderr line" \
    bash -c '! printf "%s\n" "$1" | grep -q "^ERROR: fake"' _ "$VB_ERR"
vb "RECIPE_VAR_worktree_setup=not json"
vb_none B8 "a worktree_setup that is not one JSON object (helper exit 3)"
vb
vb_none B8 "no context and no env"
CTXBAD="${SCRATCH}/ctx-bad.json"
printf 'not json{' > "$CTXBAD"
vb AMPLIHACK_CONTEXT_FILE="$CTXBAD"
vb_err B8 "an unparseable context file (helper exit 1)"
VB_RC=0; (cd "$T" && env -i "${BASE_ENV[@]}" bash "$SRC" verified-base extra >/dev/null 2>&1 </dev/null) || VB_RC=$?
check B9 "verified-base with an argument exits 2 (rc=$VB_RC)" test "$VB_RC" = 2

# requested-inputs: top-level base_branch / base_sha only.
ri
check B-ri "no inputs anywhere: exit 3 (rc=$RI_RC)" test "$RI_RC" = 3 -a -z "$RI_OUT"
ri RECIPE_VAR_base_branch= RECIPE_VAR_base_sha=
check B-ri "both empty: exit 3 (rc=$RI_RC)" test "$RI_RC" = 3 -a -z "$RI_OUT"
ri RECIPE_VAR_base_branch=dev
check B-ri "base_branch given: exit 0 and nothing on stdout (rc=$RI_RC)" test "$RI_RC" = 0 -a -z "$RI_OUT"
ri RECIPE_VAR_base_sha="$DEV_OLD"
check B-ri "base_sha given: exit 0 and nothing on stdout (rc=$RI_RC)" test "$RI_RC" = 0 -a -z "$RI_OUT"
check B-ri "the SHA value is never printed" bash -c '[[ "$1" != *"$2"* ]]' _ "$RI_OUT$RI_ERR" "$DEV_OLD"
# B-na: step 4 ignored the inputs on the existing-branch path and recorded
# not-applicable, so they count as not requested (crusty round-1, concern 2).
# Only the exact value counts; a lost worktree_setup still reports the inputs.
ri RECIPE_VAR_base_branch=dev RECIPE_VAR_base_sha="$DEV_OLD" \
    "$(ws '{"source_verified":"not-applicable","source_rule":"existing-branch","base_branch":""}')"
check B-na "not-applicable with base_branch and base_sha set: exit 3 (rc=$RI_RC)" test "$RI_RC" = 3 -a -z "$RI_OUT"
check B-na "not-applicable: one INFO line naming the ignored inputs, no ERROR" \
    bash -c '[[ "$1" == "INFO: "*"not-applicable"* ]] && ! printf "%s\n" "$1" | grep -q "^ERROR: "' _ "$RI_ERR"
for sv in legacy verified Not-Applicable "not-applicable " ""; do
    ri RECIPE_VAR_base_branch=dev "$(ws "{\"source_verified\":\"$sv\"}")"
    check B-na "source_verified '$sv' with base_branch set: exit 0 (rc=$RI_RC)" test "$RI_RC" = 0
done
ri RECIPE_VAR_base_branch=dev "$(ws '{"base_branch":"dev"}')"
check B-na "source_verified absent with base_branch set: exit 0 (rc=$RI_RC)" test "$RI_RC" = 0
CTXOK="${SCRATCH}/ctx-ok.json"
printf '{"base_branch":"dev"}\n' > "$CTXOK"
ri AMPLIHACK_CONTEXT_FILE="$CTXOK"
check B-ri "base_branch from the context file: exit 0 (rc=$RI_RC)" test "$RI_RC" = 0
# B-new4: an unparseable context is a read failure, never "not requested".
ri AMPLIHACK_CONTEXT_FILE="$CTXBAD"
check B-new4 "an unparseable context file: requested-inputs exits 1 (rc=$RI_RC)" test "$RI_RC" = 1 -a -z "$RI_OUT"
check B-new4 "exactly one ERROR line, and it is last" \
    bash -c '[[ "$(printf "%s\n" "$1" | grep -c "^ERROR: ")" = 1 && "$2" == "ERROR: "* ]]' _ "$RI_ERR" "$(last_line "$RI_ERR")"
RI_RC=0; (cd "$T" && env -i "${BASE_ENV[@]}" bash "$SRC" requested-inputs extra >/dev/null 2>&1 </dev/null) || RI_RC=$?
check B-ri "requested-inputs with an argument exits 2 (rc=$RI_RC)" test "$RI_RC" = 2

# B-nohelper (SR-1): a bundle without recipe_context_value.sh cannot read the
# context, which must never count as "not requested" or "no verified base".
NOHELPER="${SCRATCH}/nohelper-tools"
mkdir -p "$NOHELPER"
cp "$SRC" "$FETCHER" "$NOHELPER/"
[ ! -e "$NOHELPER/recipe_context_value.sh" ] || harness "nohelper bundle has the helper"
RI_RC=0
(cd "$T" && env -i "${BASE_ENV[@]}" RECIPE_VAR_base_branch=release/1.2 bash "$NOHELPER/workflow_worktree_source.sh" requested-inputs) \
    >"${SCRATCH}/ri.out" 2>"${SCRATCH}/ri.err" </dev/null || RI_RC=$?
RI_OUT="$(cat "${SCRATCH}/ri.out")"; RI_ERR="$(cat "${SCRATCH}/ri.err")"
check B-nohelper "requested-inputs without the helper exits 1, not 3 (rc=$RI_RC)" test "$RI_RC" = 1 -a -z "$RI_OUT"
check B-nohelper "requested-inputs without the helper: exactly one ERROR line, and it is last" \
    bash -c '[[ "$(printf "%s\n" "$1" | grep -c "^ERROR: ")" = 1 && "$2" == "ERROR: "* ]]' _ "$RI_ERR" "$(last_line "$RI_ERR")"
for nh_env in "RECIPE_VAR_worktree_setup={\"source_verified\":\"verified\",\"base_branch\":\"dev\"}" \
              "WORKTREE_SETUP_SOURCE_VERIFIED=verified"; do
    VB_RC=0
    (cd "$T" && env -i "${BASE_ENV[@]}" "$nh_env" WORKTREE_SETUP_BASE_BRANCH=dev bash "$NOHELPER/workflow_worktree_source.sh" verified-base) \
        >"${SCRATCH}/vb.out" 2>"${SCRATCH}/vb.err" </dev/null || VB_RC=$?
    VB_OUT="$(cat "${SCRATCH}/vb.out")"; VB_ERR="$(cat "${SCRATCH}/vb.err")"
    vb_err B-nohelper "verified-base without the helper (${nh_env%%=*}); never the env pair"
done

# =============================================================================
echo "Part P: the three real PR tools (D3, D4)"
# =============================================================================
# Each tool runs as itself (its own set -euo pipefail) in a clone whose origin
# URL looks like GitHub so the tools' repo-identity parse succeeds. An
# insteadOf rule maps that URL to the scratch bare repo, so nothing can reach
# the network. The gh stub answers `auth status` and records and fails every
# other call; a D3 refusal must stop each tool before any `gh pr`/`gh api`.
PREMOTE="$(make_remote pr)"
PR_REPO="${SCRATCH}/pr-clone"
make_clone "$PREMOTE" "$PR_REPO"
PR_GH_URL="https://github.com/example-1957/r1957.git"
git -C "$PR_REPO" config "url.file://${PREMOTE}.insteadOf" "$PR_GH_URL"
git -C "$PR_REPO" remote set-url origin "$PR_GH_URL"
git -C "$PR_REPO" checkout -q -b feat/issue-1957-pr-probe origin/dev
commit_in "$PR_REPO" pr-probe-1
PR_SHA="$(git -C "$PR_REPO" rev-parse 'origin/dev~1')"
GHSTUBS="${SCRATCH}/gh-stubs"
mkdir -p "$GHSTUBS"
cat > "$GHSTUBS/gh" <<STUB
#!/bin/sh
printf '%s\n' "\$*" >> "${SCRATCH}/gh-calls"
case "\$1 \${2:-}" in "auth status") exit 0 ;; esac
echo "gh stub: refusing '\$1 \${2:-}' (issue #1957 test)" >&2
exit 99
STUB
chmod +x "$GHSTUBS/gh"
D3_ERR='ERROR: base_branch/base_sha given but no verified base in worktree_setup; refusing to fall back (issue #1957).'
PR_URL_T="https://github.com/example-1957/r1957/pull/7"

# run_pr_tool <tool> [VAR=value...] -> PT_RC PT_OUT PT_ERR PT_GH (gh calls other than auth)
run_pr_tool() {
    local tool="$1"; shift
    : > "${SCRATCH}/gh-calls"
    PT_RC=0
    (cd "$PR_REPO" && env -i "${BASE_ENV[@]}" PATH="${GHSTUBS}:${PATH}" \
        REMOTE_HOST_TYPE=github ISSUE_NUMBER=1957 RECIPE_VAR_issue_number=1957 \
        TASK_DESCRIPTION="issue 1957 pr probe" REPO_PATH="$PR_REPO" \
        RECIPE_VAR_pr_url="$PR_URL_T" RECIPE_VAR_pr_number=7 \
        "$@" bash "${TOOLS}/$tool") >"${SCRATCH}/pt.out" 2>"${SCRATCH}/pt.err" </dev/null || PT_RC=$?
    PT_OUT="$(cat "${SCRATCH}/pt.out")"; PT_ERR="$(cat "${SCRATCH}/pt.err")"
    PT_GH="$(grep -v '^auth status' "${SCRATCH}/gh-calls" || true)"
}
# pt_refused <id> <tool> <case>: non-zero, the D3 ERROR line, gh never reached, not success.
pt_refused() {
    check "$1" "$2 ($3): exits non-zero (rc=$PT_RC)" test "$PT_RC" != 0
    check "$1" "$2 ($3): stderr has the D3 ERROR line" \
        bash -c 'printf "%s\n" "$1" | grep -qxF "$2"' _ "$PT_ERR" "$D3_ERR"
    check "$1" "$2 ($3): gh is never called past auth (got: ${PT_GH%%$'\n'*})" test -z "$PT_GH"
    check "$1" "$2 ($3): no success line" \
        bash -c '[[ "$1" != *"All 23 workflow steps completed successfully"* ]]' _ "$PT_OUT"
    check "$1" "$2 ($3): no legacy base was chosen (no origin/HEAD fallback message)" \
        bash -c '[[ "$1" != *"no supported remote base ref"* ]]' _ "$PT_ERR"
}
pub_field() { awk '/^\{/{l=$0} END{print l}' <<<"$PT_OUT" | jq -r --arg k "$1" '.[$k] // ""' 2>/dev/null; }
PR_EVIDENCE=(
    'RECIPE_VAR_implementation_terminal_evidence={"implementation_completed":true}'
    'RECIPE_VAR_verification_terminal_evidence={"verification_completed":true}'
)
for tool in workflow_publish_pr.sh workflow_pr_ready.sh workflow_final_status.sh; do
    # B-new2: inputs given, no worktree_setup at all.
    run_pr_tool "$tool" RECIPE_VAR_base_branch=dev "${PR_EVIDENCE[@]}"
    pt_refused B-new2 "$tool" "base_branch given, no worktree_setup"
    run_pr_tool "$tool" RECIPE_VAR_base_branch=dev "${PR_EVIDENCE[@]}" \
        'RECIPE_VAR_worktree_setup={"source_verified":"legacy","base_branch":"main"}'
    pt_refused B-new2 "$tool" "base_branch given, worktree_setup says legacy"
    # B-new3 (D4): a SHA-only run names no branch, so no PR can be published.
    run_pr_tool "$tool" RECIPE_VAR_base_sha="$PR_SHA" "${PR_EVIDENCE[@]}" \
        "RECIPE_VAR_worktree_setup={\"source_verified\":\"verified\",\"base_branch\":\"\",\"base_sha\":\"$PR_SHA\"}"
    pt_refused B-new3 "$tool" "SHA-only run"
    # An unreadable context fails closed: never a legacy base.
    run_pr_tool "$tool" AMPLIHACK_CONTEXT_FILE="$CTXBAD" "${PR_EVIDENCE[@]}"
    check B-new4 "$tool (unreadable context): exits non-zero (rc=$PT_RC)" test "$PT_RC" != 0
    check B-new4 "$tool (unreadable context): gh is never called past auth" test -z "$PT_GH"
    # Verified dev: the tool goes on past base resolution and reaches gh.
    run_pr_tool "$tool" RECIPE_VAR_base_branch=dev "${PR_EVIDENCE[@]}" \
        'RECIPE_VAR_worktree_setup={"source_verified":"verified","base_branch":"dev"}'
    check P-verified "$tool (verified dev): no D3 ERROR" \
        bash -c '! printf "%s\n" "$1" | grep -qxF "$2"' _ "$PT_ERR" "$D3_ERR"
    check P-verified "$tool (verified dev): resolution passed and the tool reached gh" test -n "$PT_GH"
    # No inputs and no verified base: verified-base 3, requested-inputs 3,
    # and the tool must carry on with the legacy origin/HEAD base under set -e.
    run_pr_tool "$tool" "${PR_EVIDENCE[@]}"
    check P-legacy "$tool (no inputs): no D3 ERROR" \
        bash -c '! printf "%s\n" "$1" | grep -qxF "$2"' _ "$PT_ERR" "$D3_ERR"
    check P-legacy "$tool (no inputs): no verified-base or requested-inputs ERROR" \
        bash -c '! printf "%s\n" "$1" | grep -qE "^ERROR: (cannot read|verified base_branch)"' _ "$PT_ERR"
    check P-legacy "$tool (no inputs): legacy resolution did not abort; the tool reached gh" test -n "$PT_GH"
    # P-na: existing_branch plus base_branch. Step 4 recorded not-applicable
    # and ignored the inputs, so the PR tools take the legacy chain as for a
    # run with no inputs, never the D3 refusal.
    run_pr_tool "$tool" RECIPE_VAR_base_branch=dev RECIPE_VAR_existing_branch=feat/issue-1957-pr-probe "${PR_EVIDENCE[@]}" \
        'RECIPE_VAR_worktree_setup={"source_verified":"not-applicable","source_rule":"existing-branch","base_ref":"","base_branch":"","base_sha":""}'
    check P-na "$tool (existing branch, base_branch set, not-applicable): no D3 ERROR" \
        bash -c '! printf "%s\n" "$1" | grep -qxF "$2"' _ "$PT_ERR" "$D3_ERR"
    check P-na "$tool (existing branch, base_branch set, not-applicable): no ERROR line from base resolution" \
        bash -c '! printf "%s\n" "$1" | grep -qE "^ERROR: (cannot read|verified base_branch|base_branch/base_sha given)"' _ "$PT_ERR"
    check P-na "$tool (existing branch, base_branch set, not-applicable): the legacy chain was used; the tool reached gh" test -n "$PT_GH"
    if [ "$tool" = workflow_publish_pr.sh ]; then
        check P-na "$tool (not-applicable): no FAILED_INVALID_INPUT publish result" \
            bash -c '[[ "$1" != FAILED_INVALID_INPUT ]]' _ "$(pub_field state)"
    fi
    if [ "$tool" = workflow_final_status.sh ]; then
        check P-na "$tool (not-applicable): not FAILED_BASE_NOT_VERIFIED" \
            bash -c '[[ "$1" != *FAILED_BASE_NOT_VERIFIED* ]]' _ "$PT_OUT$PT_ERR"
    fi
done

# P-publish-result: a D3 refusal in workflow_publish_pr.sh still writes its
# publish result through finish_publish (FAILED_INVALID_INPUT / invalid-input).
for pub_case in "base_branch given, no worktree_setup|RECIPE_VAR_base_branch=dev" \
                "unreadable context|AMPLIHACK_CONTEXT_FILE=${CTXBAD}"; do
    run_pr_tool workflow_publish_pr.sh "${pub_case#*|}" "${PR_EVIDENCE[@]}"
    check P-publish-result "${pub_case%%|*}: exits non-zero (rc=$PT_RC)" test "$PT_RC" != 0
    check P-publish-result "${pub_case%%|*}: state=FAILED_INVALID_INPUT legacy_state=invalid-input terminal_status=failure" \
        test "$(pub_field state)|$(pub_field legacy_state)|$(pub_field terminal_status)" = "FAILED_INVALID_INPUT|invalid-input|failure"
    check P-publish-result "${pub_case%%|*}: the message names the refused base" \
        bash -c '[[ "$1" == *"expected base branch was refused or could not be resolved (issue #1957)"* ]]' _ "$(pub_field message)"
    check P-publish-result "${pub_case%%|*}: gh is never called past auth" test -z "$PT_GH"
done

# =============================================================================
echo "Part R: the real step bodies (cases a-h)"
# =============================================================================
json_get() {  # json_get <json> <key> -> value ("" if absent); "<absent>" if the key is missing
    python3 -I -c 'import json,sys
try: d=json.loads(sys.argv[1])
except Exception: print("<unparseable>"); sys.exit(0)
v=d.get(sys.argv[2], "<absent>")
print(v if isinstance(v,str) else json.dumps(v))' "$1" "$2"
}

TASK="pin worktree source"
# run_recipe <default|consensus> <repo> <amplihack_home> [VAR=value...]
#   -> RUN_RC RUN_STDOUT RUN_STDERR RUN_JSON WT
run_recipe() {
    local which="$1" repo="$2" ahome="$3"; shift 3
    local script="$STEP04"
    [ "$which" = default ] || script="$STEP3"
    RUN_RC=0
    (
        cd "$repo" && env -i "${BASE_ENV[@]}" \
            AMPLIHACK_HOME="$ahome" \
            XDG_RUNTIME_DIR="${SCRATCH}/runtime" \
            AMPLIHACK_RUNTIME_ROOT="${SCRATCH}/runtime/root" \
            AMPLIHACK_WORKTREE_STALE_SECS=999999999 \
            REPO_PATH="$repo" WORKTREE_DIR="${repo}-wts" \
            TASK_DESCRIPTION="$TASK" ISSUE_NUMBER=1957 BRANCH_PREFIX=feat \
            BASE_BRANCH="" BASE_SHA="" EXISTING_BRANCH="" PR_NUMBER="" \
            "$@" bash "$script"
    ) >"${SCRATCH}/run.out" 2>"${SCRATCH}/run.err" </dev/null || RUN_RC=$?
    RUN_STDOUT="$(cat "${SCRATCH}/run.out")"
    RUN_STDERR="$(cat "${SCRATCH}/run.err")"
    RUN_JSON="$(awk '/^\{/{l=$0} END{print l}' <<<"$RUN_STDOUT")"
    WT="$(json_get "$RUN_JSON" worktree_path)"
}
wt_head() { git -C "$WT" rev-parse HEAD 2>/dev/null; }
run_tail() { awk '{ l[NR] = $0 } END { for (i = (NR > 4 ? NR - 3 : 1); i <= NR; i++) printf "%s|", l[i] }' <<<"$RUN_STDERR"; }

for R_ in default consensus; do
    echo " -- recipe: $R_"
    P="${R_:0:3}"

    # --- a: clone --local, origin moved to a second remote that is ahead ---
    AREMOTE="$(make_remote "${P}a")"
    A1="${SCRATCH}/${P}a-first"
    make_clone "$AREMOTE" "$A1"
    A2REMOTE="${SCRATCH}/${P}a2.git"
    git clone -q --bare "$AREMOTE" "$A2REMOTE" 2>/dev/null
    A="${SCRATCH}/${P}a-local"
    git clone -q --local "$A1" "$A" 2>/dev/null
    git -C "$A" fetch -q origin '+refs/remotes/origin/*:refs/remotes/origin/*' 2>/dev/null
    git -C "${SCRATCH}/${P}a-seed" checkout -q dev
    commit_in "${SCRATCH}/${P}a-seed" "${P}a-d3"
    git -C "${SCRATCH}/${P}a-seed" push -q "$A2REMOTE" dev 2>/dev/null
    git -C "${SCRATCH}/${P}a-seed" checkout -q main
    git -C "$A" remote set-url origin "$A2REMOTE"
    assert_scratch_origin "$A"
    A_DEV="$(remote_sha "$A2REMOTE" dev)"
    run_recipe "$R_" "$A" "$REPO_ROOT" BASE_BRANCH=dev
    check "a-$P" "base_branch=dev succeeds after origin moved ($(run_tail))" test "$RUN_RC" = 0
    check "a-$P" "worktree HEAD equals the new remote's dev, not the stale copy" test "$(wt_head)" = "$A_DEV"
    check "a-$P" "JSON: base_ref=origin/dev base_branch=dev base_sha=<tip> verified/fetched" \
        test "$(json_get "$RUN_JSON" base_ref)|$(json_get "$RUN_JSON" base_branch)|$(json_get "$RUN_JSON" base_sha)|$(json_get "$RUN_JSON" source_verified)|$(json_get "$RUN_JSON" source_rule)" \
             = "origin/dev|dev|$A_DEV|verified|fetched"
    check "a-$P" "the JSON carries no origin fingerprint and no origin URL" \
        bash -c '[[ "$1" != *origin_fingerprint* && "$1" != *"$2"* ]]' _ "$RUN_JSON" "$A2REMOTE"
    if [ "$R_" = default ]; then
        check "a-$P" "default JSON still has runtime_root and created" \
            bash -c '[[ "$1" != "<absent>" && "$2" = true ]]' _ "$(json_get "$RUN_JSON" runtime_root)" "$(json_get "$RUN_JSON" created)"
    else
        check "a-$P" "consensus JSON still has setup_complete=true" test "$(json_get "$RUN_JSON" setup_complete)" = true
    fi

    # D8 reruns on the same repo: unchanged, then the remote moves ahead.
    A_BRANCH="$(json_get "$RUN_JSON" branch_name)"; A_WT="$WT"
    run_recipe "$R_" "$A" "$REPO_ROOT" BASE_BRANCH=dev
    check "rr-$P" "a rerun with dev unchanged reuses the worktree ($(run_tail))" \
        test "$RUN_RC" = 0 -a "$WT" = "$A_WT" -a "$(wt_head)" = "$A_DEV"
    git -C "${SCRATCH}/${P}a-seed" checkout -q dev
    commit_in "${SCRATCH}/${P}a-seed" "${P}a-d4"
    git -C "${SCRATCH}/${P}a-seed" push -q "$A2REMOTE" dev 2>/dev/null
    git -C "${SCRATCH}/${P}a-seed" checkout -q main
    run_recipe "$R_" "$A" "$REPO_ROOT" BASE_BRANCH=dev
    check "rr-$P" "state 1, clean but behind the verified SHA: refused, not fast-forwarded (rc=$RUN_RC)" test "$RUN_RC" != 0
    check "rr-$P" "state 1 refusal names the fix" \
        bash -c '[[ "$1" == *"does not contain verified base"* && "$1" == *"existing_branch"* ]]' _ "$RUN_STDERR"
    check "rr-$P" "state 1 refusal touches nothing: worktree and branch kept" \
        bash -c 'git -C "$1" worktree list --porcelain | grep -qxF "worktree $2" && [[ "$(git -C "$1" rev-parse "refs/heads/$3")" = "$4" ]]' \
        _ "$A" "$A_WT" "$A_BRANCH" "$A_DEV"
    git -C "$A" worktree remove "$A_WT" 2>/dev/null
    run_recipe "$R_" "$A" "$REPO_ROOT" BASE_BRANCH=dev
    check "rr-$P" "state 2, attached but behind: refused (rc=$RUN_RC)" test "$RUN_RC" != 0
    check "rr-$P" "state 2 refusal removes only the attached worktree; the branch stays" \
        bash -c '! git -C "$1" worktree list --porcelain | grep -qxF "worktree $2" && [[ "$(git -C "$1" rev-parse "refs/heads/$3")" = "$4" ]]' \
        _ "$A" "$A_WT" "$A_BRANCH" "$A_DEV"

    # --- b: origin/HEAD points at an unrelated feature branch ---
    BREMOTE="$(make_remote "${P}b")"
    B="${SCRATCH}/${P}b-clone"
    make_clone "$BREMOTE" "$B"
    git -C "$B" symbolic-ref refs/remotes/origin/HEAD refs/remotes/origin/feature/unrelated
    run_recipe "$R_" "$B" "$REPO_ROOT" BASE_BRANCH=dev
    check "b-$P" "origin/HEAD elsewhere: worktree HEAD equals the remote's dev ($(run_tail))" \
        test "$RUN_RC" = 0 -a "$(wt_head)" = "$(remote_sha "$BREMOTE" dev)"

    # --- c: remote.origin.fetch narrowed to one unrelated branch ---
    CREMOTE2="$(make_remote "${P}c")"
    C2="${SCRATCH}/${P}c-clone"
    make_clone "$CREMOTE2" "$C2"
    advance_remote "${P}c" dev "${P}c-d3"
    git -C "$C2" config --replace-all remote.origin.fetch '+refs/heads/feature/unrelated:refs/remotes/origin/feature/unrelated'
    run_recipe "$R_" "$C2" "$REPO_ROOT" BASE_BRANCH=dev
    check "c-$P" "narrowed fetch: refs/remotes/origin/dev equals the remote's dev after setup ($(run_tail))" \
        test "$RUN_RC" = 0 -a "$(git -C "$C2" rev-parse refs/remotes/origin/dev)" = "$(remote_sha "$CREMOTE2" dev)" \
             -a "$(wt_head)" = "$(remote_sha "$CREMOTE2" dev)"

    # --- d: fetch fails (recipe level; every row is checked in Part T) ---
    DREMOTE2="$(make_remote "${P}d")"
    D2="${SCRATCH}/${P}d-clone"
    make_clone "$DREMOTE2" "$D2"
    D2_OLD="$(git -C "$DREMOTE2" rev-parse 'dev~1')"
    git -C "$D2" remote set-url origin "${SCRATCH}/${P}d-missing.git"
    D2_BEFORE="$(snap "$D2")"
    run_recipe "$R_" "$D2" "$REPO_ROOT" BASE_BRANCH=dev
    check "d-$P" "branch only with a failing fetch is refused (rc=$RUN_RC)" test "$RUN_RC" != 0
    check "d-$P" "the refusal is refuse-fetch-failed" \
        bash -c 'printf "%s\n" "$1" | grep -qxF "RULE: refuse-fetch-failed"' _ "$RUN_STDERR"
    check "d-$P" "nothing was created" test "$(snap "$D2")" = "$D2_BEFORE"
    run_recipe "$R_" "$D2" "$REPO_ROOT" BASE_SHA="$D2_OLD"
    check "d-$P" "sha only, present locally, failing fetch is refused (D1, rc=$RUN_RC)" test "$RUN_RC" != 0
    check "d-$P" "sha only: the refusal is refuse-fetch-failed" \
        bash -c 'printf "%s\n" "$1" | grep -qxF "RULE: refuse-fetch-failed"' _ "$RUN_STDERR"
    check "d-$P" "sha only: no worktree_setup JSON is written" test -z "$RUN_JSON"
    run_recipe "$R_" "$D2" "$REPO_ROOT" BASE_BRANCH=dev BASE_SHA="$(git -C "$D2" rev-parse refs/remotes/origin/dev)"
    check "d-$P" "branch + sha equal to the cached tip, failing fetch is refused (D1, rc=$RUN_RC)" \
        bash -c '[[ "$1" != 0 ]] && printf "%s\n" "$2" | grep -qxF "RULE: refuse-fetch-failed"' _ "$RUN_RC" "$RUN_STDERR"
    check "d-$P" "no D1 refusal created anything" test "$(snap "$D2")" = "$D2_BEFORE"

    # --- e: bad base_sha values leave everything unchanged ---
    EREMOTE="$(make_remote "${P}e")"
    E="${SCRATCH}/${P}e-clone"
    make_clone "$EREMOTE" "$E"
    E_TIP="$(remote_sha "$EREMOTE" dev)"
    E_UNREL="$(remote_sha "$EREMOTE" feature/unrelated)"
    E_BEFORE="$(snap "$E")"
    run_recipe "$R_" "$E" "$REPO_ROOT" BASE_BRANCH=dev BASE_SHA="$E_UNREL"
    check "e-$P" "a base_sha dev does not contain is refused, naming both SHAs (rc=$RUN_RC)" \
        bash -c '[[ "$1" != 0 && "$2" == *"ERROR: "*"$3"* && "$2" == *"$4"* ]]' _ "$RUN_RC" "$RUN_STDERR" "$E_UNREL" "$E_TIP"
    check "e-$P" "mismatched sha: worktree list and branch list unchanged" test "$(snap "$E")" = "$E_BEFORE"
    run_recipe "$R_" "$E" "$REPO_ROOT" BASE_SHA="$MISSING_SHA"
    check "e-$P" "a base_sha that does not exist is refused, naming it (rc=$RUN_RC)" \
        bash -c '[[ "$1" != 0 && "$2" == *"ERROR: "*"$3"* ]]' _ "$RUN_RC" "$RUN_STDERR" "$MISSING_SHA"
    check "e-$P" "missing sha: worktree list and branch list unchanged" test "$(snap "$E")" = "$E_BEFORE"
    run_recipe "$R_" "$E" "$REPO_ROOT" BASE_SHA="${E_TIP:0:12}"
    check "e-$P" "an abbreviated base_sha is refused (rc=$RUN_RC)" \
        bash -c '[[ "$1" != 0 && "$2" == *"ERROR: "*"${3}"* ]]' _ "$RUN_RC" "$RUN_STDERR" "${E_TIP:0:12}"
    check "e-$P" "abbreviated sha: worktree list and branch list unchanged" test "$(snap "$E")" = "$E_BEFORE"
    run_recipe "$R_" "$E" "$REPO_ROOT" BASE_BRANCH="--upload-pack=touch ${SCRATCH}/pwned-$P"
    check "e-$P" "a base_branch starting with '-' is refused and never reaches git (rc=$RUN_RC)" \
        bash -c '[[ "$1" != 0 && ! -e "$2" ]]' _ "$RUN_RC" "${SCRATCH}/pwned-$P"
    run_recipe "$R_" "$E" "$REPO_ROOT" BASE_BRANCH='$(touch pwned-sub)'
    check "e-$P" "a base_branch with \$( ) is refused and never expanded (rc=$RUN_RC)" \
        bash -c '[[ "$1" != 0 && ! -e "$2/pwned-sub" ]]' _ "$RUN_RC" "$E"
    check "e-$P" "hostile inputs: worktree list and branch list unchanged" test "$(snap "$E")" = "$E_BEFORE"

    # --- f: base_sha alone; both inputs, contained and not ---
    FREMOTE2="$(make_remote "${P}f")"
    F="${SCRATCH}/${P}f-clone"
    make_clone "$FREMOTE2" "$F"
    F_OLD="$(git -C "$FREMOTE2" rev-parse 'dev~1')"
    F_UNREL="$(remote_sha "$FREMOTE2" feature/unrelated)"
    run_recipe "$R_" "$F" "$REPO_ROOT" BASE_SHA="$F_OLD"
    check "f-$P" "base_sha alone: worktree HEAD is the SHA ($(run_tail))" test "$RUN_RC" = 0 -a "$(wt_head)" = "$F_OLD"
    check "f-$P" "base_sha alone: JSON base_ref=<sha>, base_branch empty" \
        test "$(json_get "$RUN_JSON" base_ref)|$(json_get "$RUN_JSON" base_branch)|$(json_get "$RUN_JSON" base_sha)" = "$F_OLD||$F_OLD"
    F2="${SCRATCH}/${P}f2-clone"
    make_clone "$FREMOTE2" "$F2"
    run_recipe "$R_" "$F2" "$REPO_ROOT" BASE_BRANCH=dev BASE_SHA="$F_OLD"
    check "f-$P" "both, contained: worktree HEAD is the pinned SHA, not the dev tip ($(run_tail))" \
        test "$RUN_RC" = 0 -a "$(wt_head)" = "$F_OLD"
    check "f-$P" "both, contained: JSON base_ref=origin/dev base_branch=dev base_sha=<sha>" \
        test "$(json_get "$RUN_JSON" base_ref)|$(json_get "$RUN_JSON" base_branch)|$(json_get "$RUN_JSON" base_sha)" = "origin/dev|dev|$F_OLD"
    F3="${SCRATCH}/${P}f3-clone"
    make_clone "$FREMOTE2" "$F3"
    F3_BEFORE="$(snap "$F3")"
    run_recipe "$R_" "$F3" "$REPO_ROOT" BASE_BRANCH=dev BASE_SHA="$F_UNREL"
    check "f-$P" "both, not contained: refused and nothing created (rc=$RUN_RC)" \
        test "$RUN_RC" != 0 -a "$(snap "$F3")" = "$F3_BEFORE"

    # --- g: no inputs, legacy ---
    GREMOTE="$(make_remote "${P}g")"
    G="${SCRATCH}/${P}g-clone"
    make_clone "$GREMOTE" "$G"
    run_recipe "$R_" "$G" "$REPO_ROOT"
    check "g-$P" "no inputs: legacy setup succeeds at origin/main ($(run_tail))" \
        test "$RUN_RC" = 0 -a "$(wt_head)" = "$(remote_sha "$GREMOTE" main)"
    check "g-$P" "the legacy WARNING is printed once" \
        test "$(count_lines '^WARNING: no base_branch/base_sha given; legacy base resolution (origin/main), source not verified$' "$RUN_STDERR")" = 1
    check "g-$P" "JSON: base_ref=origin/main base_branch=main base_sha='' legacy/legacy-unverified" \
        test "$(json_get "$RUN_JSON" base_ref)|$(json_get "$RUN_JSON" base_branch)|$(json_get "$RUN_JSON" base_sha)|$(json_get "$RUN_JSON" source_verified)|$(json_get "$RUN_JSON" source_rule)" \
             = "origin/main|main||legacy|legacy-unverified"

    # --- h: existing_branch with base inputs ---
    HREMOTE="$(make_remote "${P}h")"
    H="${SCRATCH}/${P}h-clone"
    make_clone "$HREMOTE" "$H"
    run_recipe "$R_" "$H" "$REPO_ROOT" EXISTING_BRANCH=feature/unrelated BASE_BRANCH=dev BASE_SHA="$(remote_sha "$HREMOTE" dev)"
    check "h-$P" "existing_branch wins; the worktree is on that branch ($(run_tail))" \
        test "$RUN_RC" = 0 -a "$(wt_head)" = "$(remote_sha "$HREMOTE" feature/unrelated)"
    check "h-$P" "JSON: base_sha='' not-applicable/existing-branch" \
        test "$(json_get "$RUN_JSON" base_sha)|$(json_get "$RUN_JSON" source_verified)|$(json_get "$RUN_JSON" source_rule)" = "|not-applicable|existing-branch"
    check "h-$P" "the step says the base inputs were ignored" \
        bash -c '[[ "$1" == *"INFO: base_branch/base_sha ignored on the existing-branch path"* ]]' _ "$RUN_STDERR"
    if [ "$R_" = consensus ]; then
        check "h-$P" "consensus existing-branch JSON has base_ref and base_branch keys" \
            bash -c '[[ "$1" != "<absent>" && "$2" != "<absent>" ]]' _ "$(json_get "$RUN_JSON" base_ref)" "$(json_get "$RUN_JSON" base_branch)"
    fi

    # --- old bundle: AMPLIHACK_HOME without workflow_worktree_source.sh ---
    STALE="${SCRATCH}/stale-${P}"
    mkdir -p "$STALE/amplifier-bundle/tools"
    cp "$TOOLS"/*.sh "$STALE/amplifier-bundle/tools/"
    rm -f "$STALE/amplifier-bundle/tools/workflow_worktree_source.sh"
    OREMOTE="$(make_remote "${P}o")"
    O="${SCRATCH}/${P}o-clone"
    make_clone "$OREMOTE" "$O"
    O_BEFORE="$(snap "$O")"
    run_recipe "$R_" "$O" "$STALE" BASE_BRANCH=dev
    check "o-$P" "an old bundle with base_branch set fails closed (rc=$RUN_RC)" test "$RUN_RC" = 1
    check "o-$P" "the old-bundle ERROR names the missing tool" \
        bash -c '[[ "$1" == *"ERROR: base_branch/base_sha given but workflow_worktree_source.sh is missing from"* ]]' _ "$RUN_STDERR"
    check "o-$P" "the old-bundle refusal created nothing" test "$(snap "$O")" = "$O_BEFORE"
    run_recipe "$R_" "$O" "$STALE" BASE_SHA="$(remote_sha "$OREMOTE" dev)"
    check "o-$P" "an old bundle with base_sha set fails closed (rc=$RUN_RC)" test "$RUN_RC" = 1 -a "$(snap "$O")" = "$O_BEFORE"
    # A fresh clone, so this cannot pass by reusing a worktree made above.
    O2="${SCRATCH}/${P}o2-clone"
    make_clone "$OREMOTE" "$O2"
    run_recipe "$R_" "$O2" "$STALE"
    check "o-$P" "an old bundle with no inputs still creates the legacy worktree ($(run_tail))" \
        test "$RUN_RC" = 0 -a "$(wt_head)" = "$(remote_sha "$OREMOTE" main)" \
             -a "$(git -C "$O2" for-each-ref --format=x refs/heads/feat | wc -l)" -ge 1
done

echo
echo "issue-1957 explicit worktree source: ${PASS_COUNT} passed, ${FAIL_COUNT} failed"
[[ "$FAIL_COUNT" -eq 0 ]]
