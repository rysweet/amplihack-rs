#!/usr/bin/env bash
# test-issue-3049-helper-survives-upgrade.sh: a run keeps working when an
# upgrade renames a shell helper under it (issue #3049, after #3019).
#
# Cases:
#   a   the old name workflow_pull_rebase is an alias of workflow_pull_merge:
#       a moved remote gives a two-parent merge, no rebase state, no
#       `pull --rebase` in the recorded git commands
#   b   a run pinned to release N (WORKFLOW_STAGE_CHANGES_HELPER, as the runner
#       sets it) keeps using N's helper after the link moves to N+1 (which has
#       no old name): the old-run step exits 0 and pushes
#   b2  control: without the pin the same step fails with exit 127
#   c   a run started on N+1 uses N+1's helper
#   d   a relative inherited value is ignored; a hostile file at that relative
#       path in the working directory is never sourced
#
# Real: git, a bare local origin, and the lookup lines of consensus-publish.yaml
# run as written. Stand-ins: `amplihack`, and the two releases (copies of the
# repository helper with a marker). HOME, git config and TMPDIR are private.
#
# This test SHOULD FAIL before #3049 is fixed and MUST PASS afterwards.
# Exit codes: 0 = pass, 1 = fail, 2 = harness error.

set -euo pipefail
unset GIT_ALTERNATE_OBJECT_DIRECTORIES GIT_CEILING_DIRECTORIES GIT_COMMON_DIR \
    GIT_DIR GIT_INDEX_FILE GIT_NAMESPACE GIT_OBJECT_DIRECTORY GIT_PREFIX \
    GIT_QUARANTINE_PATH GIT_WORK_TREE GIT_CONFIG_PARAMETERS GIT_CONFIG_COUNT \
    GH_REPO ISSUE_NUMBER REPO_PATH AMPLIHACK_HOME WORKFLOW_STAGE_CHANGES_HELPER \
    GITHUB_TOKEN GH_TOKEN AMPLIHACK_CONFIG

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
RECIPE="${REPO_ROOT}/amplifier-bundle/recipes/consensus-publish.yaml"
HELPER="${REPO_ROOT}/amplifier-bundle/tools/workflow_stage_changes.sh"
for tool in git python3; do
    command -v "${tool}" >/dev/null 2>&1 || { echo "HARNESS-ERROR: ${tool} is required" >&2; exit 2; }
done
python3 -I -c 'import yaml' 2>/dev/null || { echo "HARNESS-ERROR: python3 needs yaml" >&2; exit 2; }
[[ -f "${RECIPE}" && -f "${HELPER}" ]] || { echo "HARNESS-ERROR: recipe or helper missing" >&2; exit 2; }

PARENT="${TMPDIR:-}"
[[ "${PARENT}" == /* && -d "${PARENT}" ]] || { echo "HARNESS-ERROR: TMPDIR must be an absolute directory" >&2; exit 2; }
WORK="$(mktemp -d "${PARENT}/issue-3049-XXXXXX")" || exit 2
trap 'rm -rf -- "${WORK:?}"' EXIT
chmod 700 "${WORK}"
export HOME="${WORK}/home" GIT_CONFIG_GLOBAL="${WORK}/home/.gitconfig" GIT_CONFIG_NOSYSTEM=1
export GIT_TERMINAL_PROMPT=0 GIT_MERGE_AUTOEDIT=no
export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@example.com GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@example.com
mkdir -p "${HOME}" "${WORK}/bin"
git config --global pull.rebase true
git config --global pull.ff only
git config --global init.defaultBranch main
printf '#!/usr/bin/env bash\nexit 0\n' > "${WORK}/bin/amplihack"
chmod +x "${WORK}/bin/amplihack"
export PATH="${WORK}/bin:${PATH}"

FAILURES=0
fail() { echo "FAIL: $*" >&2; FAILURES=$((FAILURES + 1)); }
expect_eq() { [[ "$2" == "$3" ]] || fail "$1: expected '$3', got '$2'"; }
expect_has() { grep -qF -- "$3" "$2" || fail "$1: '$3' not found in ${2##*/}"; }
expect_lacks() { ! grep -qF -- "$3" "$2" || fail "$1: '$3' found in ${2##*/}"; }

# The lookup lines of the recipe, as written.
python3 -I - "${RECIPE}" "${WORK}/lookup.sh" <<'PY'
import sys, yaml
doc = yaml.safe_load(open(sys.argv[1]))
lines = []
for step in doc["steps"]:
    cmd = step.get("command") or ""
    if "workflow_stage_changes.sh" in cmd and 'S=""' in cmd:
        for l in cmd.splitlines():
            if l.lstrip().startswith('S=""') or ("not found" in l and "$S" in l) or l.strip() == '. "$S"' or l.lstrip().startswith('. "$S"'):
                lines.append(l.strip())
        break
if not lines:
    sys.exit("no lookup lines found")
open(sys.argv[2], "w").write("\n".join(lines) + "\n")
PY

# Fresh origin + worktree whose remote then moves, so a pull must merge.
new_case() {
    local name="$1"
    CASE="${WORK}/${name}"
    mkdir -p "${CASE}"
    git init -q --bare "${CASE}/origin.git"
    git clone -q "${CASE}/origin.git" "${CASE}/wt" 2>/dev/null
    git clone -q "${CASE}/origin.git" "${CASE}/other" 2>/dev/null
    ( cd "${CASE}/wt" && echo base > f && git add f && git commit -qm base && git push -q -u origin main )
    ( cd "${CASE}/other" && git pull -q origin main && echo remote > r && git add r && git commit -qm remote && git push -q origin main )
    ( cd "${CASE}/wt" && echo local > l && git add l && git commit -qm local )
    : > "${CASE}/hostile-ran"; rm -f "${CASE}/hostile-ran"
}
make_release() {   # name, keep_alias(yes|no)
    local dir="${WORK}/home-amp/releases/$1/amplifier-bundle/tools"
    mkdir -p "${dir}" "${WORK}/home-amp/releases/$1"
    if [[ "$2" == yes ]]; then cp "${HELPER}" "${dir}/workflow_stage_changes.sh"
    else sed '/^workflow_pull_rebase()/d' "${HELPER}" > "${dir}/workflow_stage_changes.sh"; fi
    echo "echo standin-release=$1 >&2" >> "${dir}/workflow_stage_changes.sh"
    : > "${WORK}/home-amp/releases/$1/.complete"
}
link_to() { ln -sfn "${WORK}/home-amp/releases/$1/amplifier-bundle" "${WORK}/home-amp/amplifier-bundle"; }
# The old run's push step: lookup as written, then the pre-#3019 helper name.
old_run_step() {   # case dir
    ( cd "$1/wt" && bash -c 'set -euo pipefail; . "$1"; workflow_pull_rebase; git push -q' _ "${WORK}/lookup.sh" )
}

# ---- (a) alias --------------------------------------------------------------
new_case a
export GIT_TRACE=0
( cd "${CASE}/wt" && bash -c 'set -euo pipefail; . "$1"; workflow_pull_rebase' _ "${HELPER}" ) \
    > "${CASE}/a.out" 2>&1 && a_rc=0 || a_rc=$?
expect_eq "a: exit" "${a_rc}" 0
expect_eq "a: parents" "$(git -C "${CASE}/wt" rev-list --parents -1 HEAD | wc -w)" 3
[[ ! -d "${CASE}/wt/.git/rebase-merge" && ! -d "${CASE}/wt/.git/rebase-apply" ]] || fail "a: rebase state left"
expect_lacks "a: output" "${CASE}/a.out" "pull --rebase"
git -C "${CASE}/wt" merge-base --is-ancestor "$(git -C "${CASE}/other" rev-parse HEAD)" HEAD || fail "a: remote commit not in HEAD"

# ---- (b) pinned run survives the swap --------------------------------------
mkdir -p "${WORK}/home-amp"
make_release N yes; make_release N1 no
link_to N
new_case b
pin="${WORK}/home-amp/releases/N/amplifier-bundle/tools/workflow_stage_changes.sh"
link_to N1   # the upgrade lands mid-run
AMPLIHACK_HOME="${WORK}/home-amp" WORKFLOW_STAGE_CHANGES_HELPER="${pin}" old_run_step "${CASE}" \
    > "${CASE}/b.out" 2>&1 && b_rc=0 || b_rc=$?
expect_eq "b: exit" "${b_rc}" 0
expect_lacks "b: output" "${CASE}/b.out" "command not found"
expect_has "b: marker" "${CASE}/b.out" "standin-release=N"
expect_eq "b: pushed" "$(git -C "${CASE}/origin.git" rev-parse main)" "$(git -C "${CASE}/wt" rev-parse HEAD)"

# ---- (b2) control: no pin ---------------------------------------------------
new_case b2
AMPLIHACK_HOME="${WORK}/home-amp" old_run_step "${CASE}" > "${CASE}/b2.out" 2>&1 && b2_rc=0 || b2_rc=$?
expect_eq "b2: exit" "${b2_rc}" 127
expect_has "b2: message" "${CASE}/b2.out" "workflow_pull_rebase: command not found"

# ---- (c) new run on N+1 -----------------------------------------------------
new_case c
( cd "${CASE}/wt" && AMPLIHACK_HOME="${WORK}/home-amp" bash -c 'set -euo pipefail; . "$1"; workflow_pull_merge; git push -q' _ "${WORK}/lookup.sh" ) \
    > "${CASE}/c.out" 2>&1 && c_rc=0 || c_rc=$?
expect_eq "c: exit" "${c_rc}" 0
expect_has "c: marker" "${CASE}/c.out" "standin-release=N1"

# ---- (d) relative inherited value is ignored --------------------------------
new_case d
mkdir -p "${CASE}/wt/tools"
printf 'touch "%s/hostile-ran"\n' "${CASE}" > "${CASE}/wt/tools/workflow_stage_changes.sh"
( cd "${CASE}/wt" && AMPLIHACK_HOME="${WORK}/home-amp" WORKFLOW_STAGE_CHANGES_HELPER="tools/workflow_stage_changes.sh" \
    bash -c 'set -euo pipefail; . "$1"; workflow_pull_merge' _ "${WORK}/lookup.sh" ) > "${CASE}/d.out" 2>&1 && d_rc=0 || d_rc=$?
expect_eq "d: exit" "${d_rc}" 0
[[ ! -e "${CASE}/hostile-ran" ]] || fail "d: a relative helper path was sourced"
expect_has "d: fell back to the loop" "${CASE}/d.out" "standin-release=N1"

if (( FAILURES > 0 )); then echo "FAILED: ${FAILURES} assertion(s)" >&2; exit 1; fi
echo "PASS: test-issue-3049-helper-survives-upgrade"
