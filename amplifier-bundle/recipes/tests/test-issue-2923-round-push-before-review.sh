#!/usr/bin/env bash
# test-issue-2923-round-push-before-review.sh: a crusty round's own fix commit
# reaches the PR's head branch before the next review, and step recovery pushes
# a commit the same run made instead of handing off (issue #2923).
#
# Contract: docs/reference/auto-drive-round-push.md
#
# Cases:
#   T0  harness proof: the recording pre-push hook fires on a known-good push
#       (an empty hook log is only evidence once this holds)
#   T1  the stand-in address step commits; before the next review begins the
#       remote branch resolves to the worktree HEAD
#   T2  a review returns a verdict against a commit present locally and absent
#       from the remote; recovery pushes it, no handoff is recorded, the drive
#       reaches a further round
#   T3a a pre-push hook refuses on a marker; git's message reaches the agent
#       step, the stand-in recovery fixes the cause, the push lands
#   T3b a second clone pushes first (non-fast-forward); the stand-in recovery
#       merges the remote and pushes, the other clone's commit survives
#
# What is real: git, the bare origin, a linked worktree per case, the
# repository's hooks, the bash block of step-04-address-concerns taken from
# autodrive-crusty-round.yaml and run as the step's agent would run it, and
# the "You must not:" list of step-failure-recovery.yaml.
#
# What is a stand-in: the agent. The address-step agent obeys an explicit
# `STOP:` in the block (the prompt says each check ends in `exit 1`) and
# otherwise recovers from a refused push. The recovery agent pushes a commit
# the run made only when the rendered prohibition list does not forbid every
# push. The review and the dispatch comparison (recorded head against the
# remote head) are scripted. These tests prove the plumbing permits and
# completes the behaviour; they do not prove that a live model chooses it.
# Assertions are on `git ls-remote`, hook logs, recorded git argv, recovery
# decision records and exit codes, never on recipe text.
#
# This test SHOULD FAIL before #2923 is implemented and MUST PASS afterwards.
#
# Usage: bash amplifier-bundle/recipes/tests/test-issue-2923-round-push-before-review.sh
# Exit codes: 0 = pass, 1 = fail, 2 = harness error.

set -euo pipefail

# An enclosing hook's repository selectors override even an absolute `git
# init <fixture>` target (#1937). Isolate before the first fixture Git call.
unset GIT_ALTERNATE_OBJECT_DIRECTORIES GIT_CEILING_DIRECTORIES GIT_COMMON_DIR \
    GIT_DIR GIT_DISCOVERY_ACROSS_FILESYSTEM GIT_INDEX_FILE GIT_INDEX_VERSION \
    GIT_NAMESPACE GIT_OBJECT_DIRECTORY GIT_PREFIX GIT_QUARANTINE_PATH GIT_WORK_TREE \
    GIT_CONFIG_PARAMETERS GIT_CONFIG_COUNT GH_REPO
while IFS= read -r git_variable; do
    case "${git_variable}" in GIT_CONFIG_KEY_*|GIT_CONFIG_VALUE_*) unset "${git_variable}" ;; esac
done < <(compgen -e)
unset git_variable

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
ROUND_RECIPE="${REPO_ROOT}/amplifier-bundle/recipes/autodrive-crusty-round.yaml"
RECOVERY_RECIPE="${REPO_ROOT}/amplifier-bundle/recipes/step-failure-recovery.yaml"
GIT_IDENTITY="${REPO_ROOT}/amplifier-bundle/tools/git-identity.sh"

for tool in git python3; do
    command -v "${tool}" >/dev/null 2>&1 || { echo "HARNESS-ERROR: ${tool} is required" >&2; exit 2; }
done
python3 -I -c 'import yaml' 2>/dev/null || { echo "HARNESS-ERROR: python3 needs the yaml module" >&2; exit 2; }
for path in "${ROUND_RECIPE}" "${RECOVERY_RECIPE}" "${GIT_IDENTITY}"; do
    [[ -f "${path}" ]] || { echo "HARNESS-ERROR: ${path} not found" >&2; exit 2; }
done
REAL_GIT="$(command -v git)"

SCRATCH_PARENT="${TMPDIR:-}"
[[ "${SCRATCH_PARENT}" == /* && -d "${SCRATCH_PARENT}" ]] || { echo "HARNESS-ERROR: TMPDIR must be an existing absolute directory" >&2; exit 2; }
SCRATCH_PARENT="$(cd -- "${SCRATCH_PARENT:?}" && pwd -P)"
WORK="$(mktemp -d "${SCRATCH_PARENT:?}/issue-2923-round-push-XXXXXX")" || exit 2
case "${WORK}" in "${SCRATCH_PARENT:?}/issue-2923-round-push-"*) ;; *) echo "HARNESS-ERROR: invalid scratch allocation" >&2; exit 2 ;; esac
[[ -n "${ISSUE_2923_KEEP_WORK:-}" ]] || trap 'rm -rf -- "${WORK:?}"' EXIT
export HOME="${WORK}/home" XDG_CONFIG_HOME="${WORK}/home/.config"
export GIT_CONFIG_GLOBAL="${WORK}/home/.gitconfig" GIT_CONFIG_NOSYSTEM=1 GIT_TERMINAL_PROMPT=0
export GIT_MERGE_AUTOEDIT=no
BIN="${WORK}/bin"; TOOLS_STUB="${WORK}/tools"
mkdir -p "${HOME}" "${BIN}" "${TOOLS_STUB}"
chmod 700 "${WORK}"

FAILURES=0
PASSES=0
fail() { echo "FAIL: $*" >&2; FAILURES=$((FAILURES + 1)); }
pass() { PASSES=$((PASSES + 1)); }
expect_eq() {
    if [[ "$2" == "$3" ]]; then pass; else fail "$1: expected '$3', got '$2'"; fi
}
expect_nonempty() {
    if [[ -s "$2" ]]; then pass; else fail "$1: ${2##*/} is empty or missing"; fi
}

# --- stand-ins on PATH -------------------------------------------------------
# git wrapper: record argv, then run the real git by absolute path.
cat > "${BIN}/git" <<WRAP
#!/usr/bin/env bash
{ printf 'git'; for a in "\$@"; do printf ' %q' "\$a"; done; printf '\n'; } >> "${WORK}/git-argv.log"
exec "${REAL_GIT}" "\$@"
WRAP
# amplihack: only the artifact guard is reached by the step-04 block.
cat > "${BIN}/amplihack" <<'STUB'
#!/usr/bin/env bash
case "$1 ${2:-}" in "hygiene artifact-guard") exit 0 ;; esac
echo "stub amplihack: unexpected call: $*" >&2
exit 64
STUB
# autodrive_worktree_guard.sh: the stand-in checkout is the PR worktree.
cat > "${TOOLS_STUB}/autodrive_worktree_guard.sh" <<'STUB'
#!/usr/bin/env bash
exit 0
STUB
chmod +x "${BIN}/git" "${BIN}/amplihack" "${TOOLS_STUB}/autodrive_worktree_guard.sh"
export PATH="${BIN}:${PATH}"
export AMPLIHACK_HOME="${REPO_ROOT}" AMPLIHACK_BIN="${BIN}/amplihack"
export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@example.invalid GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@example.invalid

git config --global init.defaultBranch main
git config --global user.name t
git config --global user.email t@example.invalid
git config --global push.default upstream
git config --global advice.pushUpdateRejected false

# --- the real prompts, read as the agents would read them -------------------
python3 -I - "${ROUND_RECIPE}" "${RECOVERY_RECIPE}" "${WORK}" <<'PY'
import re, sys, yaml
round_path, recovery_path, work = sys.argv[1:4]
steps = yaml.safe_load(open(round_path))["steps"]
step = next(s for s in steps if s["id"] == "step-04-address-concerns")
block = re.search(r"```bash\n(.*?)```", step["prompt"], re.S)
if not block:
    sys.exit("no bash block in step-04-address-concerns")
open(work + "/address-block.tmpl", "w").write(block.group(1))
rec = yaml.safe_load(open(recovery_path))["steps"][0]["prompt"]
after = rec.split("You must not:", 1)[1].split("\n\n", 1)[0]
bullets, cur = [], None
for line in after.splitlines():
    s = line.strip()
    if s.startswith("- "):
        cur = s[2:]; bullets.append(cur)
    elif cur is not None and s:
        bullets[-1] += " " + s
open(work + "/recovery-prohibitions.txt", "w").write("\n".join(bullets) + "\n")
PY

# --- fixture ---------------------------------------------------------------
ORIGIN="${WORK}/origin.git"
MAIN="${WORK}/main"
"${REAL_GIT}" init -q --bare "${ORIGIN}"
"${REAL_GIT}" clone -q "${ORIGIN}" "${MAIN}" 2>/dev/null
( cd "${MAIN}" && echo base > base.txt && git add base.txt && git commit -q -m base && git push -q origin main )

hooks_dir() { ( cd "$1" && git rev-parse --git-path hooks ) ; }
HOOK_DIR="$(cd "${MAIN}" && cd "$(hooks_dir .)" && pwd)"
HOOK_LOG="${WORK}/pre-push.log"
REFUSE_MARKER="${WORK}/refuse-push"
# The recording hook drains stdin (git writes the ref list to it) and refuses
# while the marker file exists.
cat > "${HOOK_DIR}/pre-push" <<HOOK
#!/usr/bin/env bash
cat >/dev/null
echo "pre-push fired \$(date +%s%N)" >> "${HOOK_LOG}"
if [ -e "${REFUSE_MARKER}" ]; then
  echo "HOOK-REFUSED: marker file present, remove it and push again" >&2
  exit 1
fi
exit 0
HOOK
chmod +x "${HOOK_DIR}/pre-push"

new_case() { # <name> -> sets CASE_WT CASE_BRANCH; worktree tracks origin/<branch>
    CASE_BRANCH="pr-$1"
    CASE_WT="${WORK}/wt-$1"
    ( cd "${MAIN}" && git worktree add -q -b "${CASE_BRANCH}" "${CASE_WT}" main )
    ( cd "${CASE_WT}" && git -c core.hooksPath="${HOOK_DIR}" push -q -u origin "${CASE_BRANCH}" 2>/dev/null )
    mkdir -p "${WORK}/rec-$1"
}
remote_head() { "${REAL_GIT}" -C "${ORIGIN}" rev-parse "refs/heads/$1"; }
wt_head() { "${REAL_GIT}" -C "$1" rev-parse HEAD; }

# The address-step agent: renders the real step-04 block, runs it, and reacts
# to what it printed. Output goes to $2 (the evidence git's message lands in).
run_address_step() { # <worktree> <evidence-file> -> rc 0 pushed, 1 stopped/failed
    local wt="$1" evidence="$2" rc=0
    local script="${evidence}.sh"
    {
        printf 'fix_paths=(fix.txt)\n'
        sed -e "s|{{repo_path}}|${wt}|g" -e "s|{{autodrive_launch_tools}}|${TOOLS_STUB}|g" \
            -e 's|{{pr_number}}|2923|g' -e 's|{{pre_review_evidence}}|none|g' "${WORK}/address-block.tmpl"
    } > "${script}"
    ( cd "${wt}" && echo "fix $RANDOM" > fix.txt && bash "${script}" ) > "${evidence}" 2>&1 || rc=$?
    [[ "${rc}" -eq 0 ]] && return 0
    # An explicit STOP in the block is an instruction the agent obeys.
    grep -q '^STOP:' "${evidence}" && return 1
    # Otherwise git's message is the evidence and the agent recovers.
    agent_recover_push "${wt}" "${evidence}"
}

# The recovery an agent applies to a refused push, read from git's message.
agent_recover_push() { # <worktree> <evidence-file>
    local wt="$1" evidence="$2"
    if grep -q 'HOOK-REFUSED' "${evidence}"; then
        rm -f "${REFUSE_MARKER}"                       # fix what the hook objected to
    elif grep -qE 'rejected|non-fast-forward|fetch first' "${evidence}"; then
        ( cd "${wt}" && git pull -q --no-rebase --no-edit ) >>"${evidence}" 2>&1 || return 1   # merge the remote
    else
        return 1
    fi
    ( cd "${wt}" && git push -q ) >>"${evidence}" 2>&1
}

# Step recovery: pushes a commit the run made unless the prompt forbids every push.
run_step_recovery() { # <worktree> <record-dir>
    local wt="$1" dir="$2" forbids=0 line n
    while IFS= read -r line; do
        case "${line}" in *force*) continue ;; esac
        case "${line}" in *push*) case "${line}" in *"this run"*) ;; *) forbids=1 ;; esac ;; esac
    done < "${WORK}/recovery-prohibitions.txt"
    n=$(( $(find "${dir}" -name 'record-*.json' | wc -l) + 1 ))
    if [[ "${forbids}" -eq 1 ]]; then
        printf '{"step_recovery":{"choice":"handoff","reason":"unpushed commit; push is forbidden here","owner":"operator","needs":"push the commit"}}\n' > "${dir}/record-${n}.json"
        return 0
    fi
    ( cd "${wt}" && git push -q ) >"${dir}/push-${n}.out" 2>&1
    printf '{"step_recovery":{"choice":"retry_after_repair","reason":"reviewed commit was unpushed","repair":"pushed the run commit"}}\n' > "${dir}/record-${n}.json"
}

# Dispatch comparison: the recorded reviewed head against the remote head.
dispatch_matches() { [[ "$1" == "$(remote_head "$2")" ]]; }
handoff_recorded() {
    local f
    for f in "$1"/record-*.json; do
        [[ -e "${f}" ]] || continue
        grep -q '"choice":"handoff"' "${f}" && return 0
    done
    return 1
}

# --- T0: the recording hook fires on a known-good push ----------------------
new_case t0
( cd "${CASE_WT}" && echo ok > ok.txt && git add ok.txt && git commit -q -m ok && git push -q )
expect_nonempty "T0 recording pre-push hook fired on a known-good push" "${HOOK_LOG}"
expect_eq "T0 remote carries the pushed commit" "$(remote_head "${CASE_BRANCH}")" "$(wt_head "${CASE_WT}")"
: > "${HOOK_LOG}"

# --- T1: the round pushes its fix before the next review --------------------
new_case t1
REVIEWED_1="$(wt_head "${CASE_WT}")"                       # round 1 reviews this head
rc=0; run_address_step "${CASE_WT}" "${WORK}/t1-evidence.txt" || rc=$?
expect_eq "T1 address step completed" "${rc}" "0"
REVIEWED_2="$(wt_head "${CASE_WT}")"                       # round 2 would review this head
expect_eq "T1 remote branch resolves to the worktree HEAD before the next review" \
    "$("${REAL_GIT}" ls-remote "${ORIGIN}" "refs/heads/${CASE_BRANCH}" | cut -f1)" "${REVIEWED_2}"
expect_eq "T1 the round made a new commit" "$([[ "${REVIEWED_1}" != "${REVIEWED_2}" ]] && echo yes || echo no)" "yes"
dispatch_matches "${REVIEWED_2}" "${CASE_BRANCH}" && pass || fail "T1 dispatch comparison disagrees after the push"
expect_nonempty "T1 the push went through the repository's hook" "${HOOK_LOG}"
: > "${HOOK_LOG}"

# --- T2: a verdict against an unpushed own commit leads to a push -----------
new_case t2
( cd "${CASE_WT}" && echo own > own.txt && git add own.txt && git commit -q -m "run's own fix, not pushed" )
REVIEWED="$(wt_head "${CASE_WT}")"
rounds=1; exit_code=0
if ! dispatch_matches "${REVIEWED}" "${CASE_BRANCH}"; then
    run_step_recovery "${CASE_WT}" "${WORK}/rec-t2"
fi
if handoff_recorded "${WORK}/rec-t2"; then exit_code=81; else
    rounds=2                                              # a further round runs
    dispatch_matches "$(wt_head "${CASE_WT}")" "${CASE_BRANCH}" || exit_code=1
fi
if handoff_recorded "${WORK}/rec-t2"; then fail "T2 step recovery handed off instead of pushing"; else pass; fi
expect_eq "T2 the run's commit reached the remote" "$(remote_head "${CASE_BRANCH}")" "${REVIEWED}"
expect_eq "T2 the drive reached a further round" "${rounds}" "2"
expect_eq "T2 the drive exit is not the handoff status" "${exit_code}" "0"
expect_nonempty "T2 the push went through the repository's hook" "${HOOK_LOG}"
: > "${HOOK_LOG}"

# --- T3a: a hook refuses the push -------------------------------------------
new_case t3a
: > "${REFUSE_MARKER}"
rc=0; run_address_step "${CASE_WT}" "${WORK}/t3a-evidence.txt" || rc=$?
if grep -q 'HOOK-REFUSED' "${WORK}/t3a-evidence.txt"; then pass; else fail "T3a git's own message did not reach the agent step"; fi
expect_eq "T3a the stand-in recovery completed the step" "${rc}" "0"
expect_eq "T3a the push landed after the hook's cause was fixed" "$(remote_head "${CASE_BRANCH}")" "$(wt_head "${CASE_WT}")"
rm -f "${REFUSE_MARKER}"
: > "${HOOK_LOG}"

# --- T3b: the remote moved (non-fast-forward) -------------------------------
new_case t3b
OTHER="${WORK}/other-clone"
"${REAL_GIT}" clone -q "${ORIGIN}" "${OTHER}" 2>/dev/null
( cd "${OTHER}" && git checkout -q "${CASE_BRANCH}" && echo theirs > theirs.txt && git add theirs.txt \
    && git commit -q -m "another actor's commit" && git push -q origin "${CASE_BRANCH}" )
THEIRS="$(wt_head "${OTHER}")"
rc=0; run_address_step "${CASE_WT}" "${WORK}/t3b-evidence.txt" || rc=$?
if grep -qE 'rejected|non-fast-forward|fetch first' "${WORK}/t3b-evidence.txt"; then pass; else fail "T3b git's own message did not reach the agent step"; fi
expect_eq "T3b the stand-in recovery completed the step" "${rc}" "0"
expect_eq "T3b the push landed" "$(remote_head "${CASE_BRANCH}")" "$(wt_head "${CASE_WT}")"
if "${REAL_GIT}" -C "${ORIGIN}" merge-base --is-ancestor "${THEIRS}" "refs/heads/${CASE_BRANCH}"; then pass; else fail "T3b the other actor's commit did not survive"; fi

# --- argv: nothing rewrote history or skipped the hooks ---------------------
expect_nonempty "recorded git argv (the wrapper was on PATH)" "${WORK}/git-argv.log"
if grep -qE -- ' (--force|-f|--force-with-lease(=[^ ]*)?|--no-verify|-n|--amend|rebase|--no-gpg-sign) | (reset --hard)|core\.hooksPath=' \
    <(grep -v 'core.hooksPath=' "${WORK}/git-argv.log" | sed 's/$/ /' ; grep 'core.hooksPath=' "${WORK}/git-argv.log" | grep -v "push -q -u origin pr-" | sed 's/$/ /'); then
    fail "a recorded git invocation carried a force, hook-skipping or history-rewriting argument"
    grep -nE -- ' (--force|-f|--force-with-lease|--no-verify|--amend|rebase)|reset --hard' "${WORK}/git-argv.log" >&2 || true
else pass; fi

echo "passes=${PASSES} failures=${FAILURES}"
[[ "${FAILURES}" -eq 0 ]] || exit 1
exit 0
